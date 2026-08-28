//! Where the traits meet their implementations.

use std::sync::Arc;

use anyhow::{Context, Result};
use tokio_util::sync::CancellationToken;

use aiwatcher_api::state::{AppState, HealthState};
use aiwatcher_bus::adapters::memory::InMemoryBus;
use aiwatcher_bus::adapters::wal::FileWal;
use aiwatcher_bus::{Checkpointer, MessageSink, MessageSource};
use aiwatcher_core::ports::{
    CompletedSpan, DeadLetterSink, MetricSample, MetricSink, PortResult, TraceStore,
};
use aiwatcher_projector::pipeline::Outputs;
use aiwatcher_projector::{FileDeadLetters, LiveHub, Projector, ProjectorConfig, ReadModel};
use aiwatcher_prompts::{Registry, RegistryConfig};
use aiwatcher_trace::AssemblerConfig;
use aiwatcher_trace::otlp::{OtlpConfig, OtlpMetricSink, OtlpTraceStore};

use crate::config::{BackendKind, Config, PromptStoreKind};

/// Discards what it is given, loudly enough to notice at startup and quietly
/// enough not to fill a log.
///
/// Used when no OTLP endpoint is configured. The alternative — refusing to
/// start — would make it impossible to run the panel against the read model
/// alone, which is a legitimate way to use this.
#[derive(Debug)]
struct NullExporter;

#[async_trait::async_trait]
impl TraceStore for NullExporter {
    async fn write_spans(&self, spans: Vec<CompletedSpan>) -> PortResult<()> {
        tracing::debug!(
            spans = spans.len(),
            "no OTLP endpoint configured; spans are served from the read model only"
        );
        Ok(())
    }
}

#[async_trait::async_trait]
impl MetricSink for NullExporter {
    async fn record(&self, samples: Vec<MetricSample>) -> PortResult<()> {
        tracing::trace!(samples = samples.len(), "no OTLP endpoint configured");
        Ok(())
    }
}

/// The prompt registry, or `None` when this deployment has none.
///
/// Built before the server starts listening, and allowed to fail the start-up:
/// an object store that is misconfigured answers 403 to everything, and
/// discovering that when somebody saves a prompt puts the failure in front of
/// the wrong person. `AIWATCHER_PROMPT_STORE=none` is how a deployment says it
/// does not want one.
async fn build_prompt_registry(config: &Config) -> Result<Option<Arc<Registry>>> {
    let registry_config = RegistryConfig {
        prefix: config.prompt_prefix.clone(),
        ..RegistryConfig::default()
    };

    let store: Arc<dyn aiwatcher_core::prompts::ObjectStore> = match config.prompt_store {
        PromptStoreKind::None => {
            tracing::info!("AIWATCHER_PROMPT_STORE=none; the prompt registry is disabled");
            return Ok(None);
        }
        PromptStoreKind::Memory => {
            tracing::warn!("the prompt registry is in memory; prompts will not survive a restart");
            Arc::new(aiwatcher_prompts::adapters::memory::MemoryObjectStore::new())
        }
        PromptStoreKind::File => {
            let directory = config.prompt_dir();
            tracing::info!(%directory, "the prompt registry is on disk");
            Arc::new(
                aiwatcher_prompts::adapters::fs::FileObjectStore::open(directory)
                    .await
                    .context("opening the prompt directory")?,
            )
        }
        PromptStoreKind::S3 => {
            use aiwatcher_prompts::adapters::s3::{S3Config, S3ObjectStore};
            use aiwatcher_prompts::sigv4::Credentials;

            let endpoint = config.prompt_s3_endpoint.clone().context(
                "AIWATCHER_PROMPT_S3_ENDPOINT is required for AIWATCHER_PROMPT_STORE=s3",
            )?;
            tracing::info!(%endpoint, bucket = %config.prompt_s3_bucket, "the prompt registry is in an object store");
            Arc::new(
                S3ObjectStore::connect(S3Config {
                    endpoint,
                    bucket: config.prompt_s3_bucket.clone(),
                    credentials: Credentials {
                        access_key_id: config.prompt_s3_access_key.clone().unwrap_or_default(),
                        secret_access_key: config.prompt_s3_secret_key.clone().unwrap_or_default(),
                        session_token: config.prompt_s3_session_token.clone(),
                        region: config.prompt_s3_region.clone(),
                    },
                    timeout: std::time::Duration::from_secs(10),
                    create_bucket: config.prompt_s3_create_bucket,
                })
                .await
                .context("connecting to the prompt object store")?,
            )
        }
    };

    Ok(Some(Arc::new(Registry::new(store, registry_config))))
}

/// A fully wired instance, ready to serve and to consume.
pub struct Runtime {
    pub state: AppState,
    pub config: Config,
    projector: Box<dyn ProjectorTask>,
}

impl std::fmt::Debug for Runtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runtime")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl Runtime {
    /// Consume the log until `shutdown` fires.
    pub async fn run_projector(self: Box<Self>, shutdown: CancellationToken) -> Result<()> {
        self.projector.run(shutdown).await
    }

    pub fn split(self) -> (AppState, Config, Box<dyn ProjectorTask>) {
        (self.state, self.config, self.projector)
    }
}

/// Erases the projector's two generic parameters so `Runtime` does not have to
/// carry them.
#[async_trait::async_trait]
pub trait ProjectorTask: Send + Sync {
    async fn run(self: Box<Self>, shutdown: CancellationToken) -> Result<()>;
}

struct TypedProjector<S, C> {
    inner: Arc<Projector<S, C>>,
}

#[async_trait::async_trait]
impl<S, C> ProjectorTask for TypedProjector<S, C>
where
    S: MessageSource + 'static,
    C: Checkpointer + 'static,
{
    async fn run(self: Box<Self>, shutdown: CancellationToken) -> Result<()> {
        Arc::clone(&self.inner)
            .run(shutdown)
            .await
            .context("projector stopped with an error")
    }
}

/// Build everything from configuration.
pub async fn build(config: Config) -> Result<Runtime> {
    let read_model = Arc::new(ReadModel::new(
        aiwatcher_projector::readmodel::ReadModelConfig {
            max_runs: config.max_runs,
            max_spans_total: config.max_spans_total,
            evaluations: aiwatcher_projector::evaluations::EvaluationConfig {
                max_evaluations: config.max_evaluations,
                max_cases_total: config.max_evaluation_cases_total,
                ..Default::default()
            },
            ..Default::default()
        },
    ));
    let live = Arc::new(LiveHub::default());
    let health = HealthState::new();

    let (traces, metrics): (Arc<dyn TraceStore>, Arc<dyn MetricSink>) = match config
        .otlp_endpoint
        .as_deref()
    {
        Some(endpoint) => {
            let otlp = OtlpConfig::new(endpoint, config.service_name.clone());
            (
                Arc::new(
                    OtlpTraceStore::new(otlp.clone())
                        .context("building the OTLP trace exporter")?,
                ),
                Arc::new(OtlpMetricSink::new(otlp).context("building the OTLP metric exporter")?),
            )
        }
        None => {
            tracing::warn!(
                "AIWATCHER_OTLP_ENDPOINT is unset; traces and metrics will not be exported"
            );
            (Arc::new(NullExporter), Arc::new(NullExporter))
        }
    };

    let dead_letters: Arc<dyn DeadLetterSink> = Arc::new(
        FileDeadLetters::open(config.dead_letter_path())
            .await
            .context("opening the dead letter queue")?,
    );

    let projector_config = ProjectorConfig {
        processor_id: config.processor_id.clone(),
        assembler: AssemblerConfig {
            orphan_timeout: time::Duration::try_from(config.orphan_timeout)
                .unwrap_or_else(|_| time::Duration::minutes(15)),
            ..AssemblerConfig::default()
        },
        ..ProjectorConfig::default()
    };

    let outputs = Outputs {
        live: Arc::clone(&live) as _,
        traces,
        metrics,
        dead_letters,
        read_model: Arc::clone(&read_model),
    };

    // Each arm produces the same three things; only the concrete types differ.
    let (source, sink, projector): (
        Arc<dyn MessageSource>,
        Arc<dyn MessageSink>,
        Box<dyn ProjectorTask>,
    ) = match config.bus {
        BackendKind::Memory => {
            let bus = Arc::new(InMemoryBus::new());
            let projector = Arc::new(Projector::new(
                Arc::clone(&bus),
                Arc::clone(&bus),
                outputs,
                projector_config,
            ));
            (
                Arc::clone(&bus) as _,
                Arc::clone(&bus) as _,
                Box::new(TypedProjector { inner: projector }),
            )
        }
        BackendKind::Wal => {
            let wal = Arc::new(
                FileWal::open(config.wal_dir())
                    .await
                    .context("opening the write-ahead log")?,
            );
            let projector = Arc::new(Projector::new(
                Arc::clone(&wal),
                Arc::clone(&wal),
                outputs,
                projector_config,
            ));
            (
                Arc::clone(&wal) as _,
                Arc::clone(&wal) as _,
                Box::new(TypedProjector { inner: projector }),
            )
        }
        #[cfg(feature = "laser")]
        BackendKind::Laser => {
            use aiwatcher_bus::adapters::laser::{LaserBus, LaserConfig};

            let connection_string = config
                .laser_connection_string
                .clone()
                .context("AIWATCHER_LASER_CONNECTION_STRING is required for AIWATCHER_BUS=laser")?;
            let bus = Arc::new(
                LaserBus::connect(LaserConfig {
                    connection_string,
                    stream: config.laser_stream.clone(),
                    topic: config.laser_topic.clone(),
                    partitions: config.laser_partitions,
                    batch_length: 256,
                    ..LaserConfig::default()
                })
                .await
                .context("connecting to Laser")?,
            );
            // The broker owns the group's resume position, so a cold start
            // means "after whatever this group last committed" rather than a
            // full replay. `Beginning` here would re-read the whole topic on
            // every restart.
            let projector_config = ProjectorConfig {
                cold_start: aiwatcher_bus::StartFrom::Now,
                // The broker owns the group's resume position and the topic
                // outlives any one process, so replaying it on every restart
                // would re-read history that could be arbitrarily long.
                rebuild_on_start: false,
                ..projector_config
            };
            let projector = Arc::new(Projector::new(
                Arc::clone(&bus),
                Arc::clone(&bus),
                outputs,
                projector_config,
            ));
            (
                Arc::clone(&bus) as _,
                Arc::clone(&bus) as _,
                Box::new(TypedProjector { inner: projector }),
            )
        }

        #[cfg(not(feature = "laser"))]
        BackendKind::Laser => {
            // A silent fallback to a different log would be worse than not
            // starting: the events would go somewhere nobody is looking.
            anyhow::bail!(
                "AIWATCHER_BUS=laser needs this binary built with the `laser` cargo feature \
                 (`cargo build --features laser`, or `just build-laser`)"
            );
        }
    };

    let state = AppState {
        read_model,
        live,
        source,
        sink: config.ingest_enabled.then_some(sink),
        prompts: build_prompt_registry(&config).await?,
        health,
    };

    Ok(Runtime {
        state,
        config,
        projector,
    })
}
