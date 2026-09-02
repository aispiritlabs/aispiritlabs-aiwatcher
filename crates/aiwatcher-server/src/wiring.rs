//! Where the traits meet their implementations.

use std::sync::Arc;

use anyhow::{Context, Result};
use tokio_util::sync::CancellationToken;

use aiwatcher_annotations::Registry as AnnotationRegistry;
use aiwatcher_annotations::SourceCatalog;
use aiwatcher_annotations::integrations::hubs::{HubConfig, Hubs};
use aiwatcher_api::state::{AppState, HealthState};
use aiwatcher_auth::{AuthMode, Authenticator};
use aiwatcher_bus::adapters::memory::InMemoryBus;
use aiwatcher_bus::adapters::wal::FileWal;
use aiwatcher_bus::{Checkpointer, MessageSink, MessageSource};
use aiwatcher_conversations::{
    ArchivePolicy, Keyring, PolicyMode, Registry as ConversationArchive,
};
use aiwatcher_core::ports::{
    CompletedSpan, DeadLetterSink, MetricSample, MetricSink, PortResult, TraceStore, WorkflowRunner,
};
use aiwatcher_datasets::Registry as DatasetRegistry;
use aiwatcher_pipeline::{FlyteConfig, FlyteEngine};
use aiwatcher_projector::pipeline::Outputs;
use aiwatcher_projector::{FileDeadLetters, LiveHub, Projector, ProjectorConfig, ReadModel};
use aiwatcher_prompts::{Registry, RegistryConfig};
use aiwatcher_runner::{HttpRunner, HttpRunnerConfig};
use aiwatcher_trace::AssemblerConfig;
use aiwatcher_trace::otlp::{OtlpConfig, OtlpMetricSink, OtlpTraceStore};
use aiwatcher_training::Registry as TrainingRegistry;

use crate::config::{
    BackendKind, Config, ConversationPolicyMode, EngineKind, PromptStoreKind, WorkflowRunnerKind,
};

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

/// Everything authored rather than observed, over one object store.
///
/// Three registries, three key prefixes, one store — and one setting deciding
/// whether any of them exist. They are grouped because the alternative is a
/// four-tuple that grows every time another authored artifact appears, and
/// because they genuinely share a lifetime: a deployment either has somewhere
/// durable to put authored data or it does not.
#[derive(Clone, Debug, Default)]
struct Registries {
    prompts: Option<Arc<Registry>>,
    datasets: Option<Arc<DatasetRegistry>>,
    annotations: Option<Arc<AnnotationRegistry>>,
    training: Option<Arc<TrainingRegistry>>,
    /// The fifth, and the one this struct's doc comment does not describe: it
    /// shares the store and nothing else. Its content is encrypted, its
    /// retention is its own, and it is absent unless a deployment asked for it
    /// — so it has a second switch above the store's.
    conversations: Option<Arc<ConversationArchive>>,
}

/// The authored-data registries, or empty when this deployment has no object store.
///
/// Built before the server starts listening, and allowed to fail the start-up:
/// an object store that is misconfigured answers 403 to everything, and
/// discovering that when somebody saves a prompt puts the failure in front of
/// the wrong person. `AIWATCHER_PROMPT_STORE=none` is how a deployment says it
/// does not want one.
async fn build_registries(
    config: &Config,
    images: Option<Arc<dyn aiwatcher_annotations::integrations::fetch::ImageSource>>,
) -> Result<Registries> {
    let registry_config = RegistryConfig {
        prefix: config.prompt_prefix.clone(),
        ..RegistryConfig::default()
    };

    let store: Arc<dyn aiwatcher_core::prompts::ObjectStore> = match config.prompt_store {
        PromptStoreKind::None => {
            tracing::info!(
                "AIWATCHER_PROMPT_STORE=none; the prompt, dataset, annotation and training registries are disabled"
            );
            return Ok(Registries::default());
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

    let prompts = Arc::new(Registry::new(Arc::clone(&store), registry_config));
    let datasets = Arc::new(DatasetRegistry::new(Arc::clone(&store), "datasets"));
    // The image source is handed to the registry rather than fetched by it:
    // the queued importer runs inside `aiwatcher-annotations`, which knows
    // nothing about hubs, and this is the one process that holds both halves.
    // `None` is a working state — a pipeline that stored its own bytes sends
    // rows carrying a content address — and it is never a silent one, because
    // a row with no `image_id` is then rejected saying exactly that.
    let annotations = {
        let registry = AnnotationRegistry::new(Arc::clone(&store), "annotations");
        Arc::new(match images {
            Some(images) => registry.with_image_source(images),
            None => registry,
        })
    };
    let training = Arc::new(TrainingRegistry::new(Arc::clone(&store), "training"));
    let conversations = build_conversation_archive(config, &store)?;
    Ok(Registries {
        prompts: Some(prompts),
        datasets: Some(datasets),
        annotations: Some(annotations),
        training: Some(training),
        conversations,
    })
}

/// The conversation archive, or `None` — which is the default.
///
/// The one authored store here that a deployment has to ask for. The others
/// exist because there is somewhere to put them; this one exists because
/// somebody decided to keep conversation content, and a system that started
/// holding it on an upgrade would be the failure ADR_0021 is about.
///
/// A missing key is a start-up failure rather than a plaintext archive. The
/// config layer already refuses that combination; this is the second check, and
/// it is here because the two failures have different fixes and only one of
/// them is "set a variable".
fn build_conversation_archive(
    config: &Config,
    store: &Arc<dyn aiwatcher_core::prompts::ObjectStore>,
) -> Result<Option<Arc<ConversationArchive>>> {
    if !config.conversation_archive {
        tracing::info!(
            "AIWATCHER_CONVERSATION_ARCHIVE is off; no conversation content is retained and \
             every /api/v1/conversation-* route answers 501"
        );
        return Ok(None);
    }
    let spec = config
        .conversation_keys
        .as_deref()
        .context("AIWATCHER_CONVERSATION_KEYS is required for AIWATCHER_CONVERSATION_ARCHIVE=on")?;
    let keyring = Keyring::parse("AIWATCHER_CONVERSATION_KEYS", spec)
        .context("reading the conversation archive's keyring")?;
    let policy = ArchivePolicy {
        mode: match config.conversation_policy {
            ConversationPolicyMode::Protected => PolicyMode::Protected,
            ConversationPolicyMode::Open => PolicyMode::Open,
        },
        max_ttl_days: config.conversation_max_ttl_days,
        reject_on_finding: config.conversation_reject_on_finding,
    };
    if policy.mode == PolicyMode::Open {
        tracing::warn!(
            "AIWATCHER_CONVERSATION_POLICY=open; turns with no consent record are accepted and \
             every export excludes them by name"
        );
    }
    tracing::info!(
        prefix = %config.conversation_prefix,
        policy = policy.mode.as_str(),
        max_ttl_days = policy.max_ttl_days,
        active_key = keyring.active(),
        keys = keyring.key_ids().len(),
        "the conversation archive is on"
    );
    Ok(Some(Arc::new(ConversationArchive::new(
        Arc::clone(store),
        config.conversation_prefix.clone(),
        keyring,
        policy,
    ))))
}

/// The workflow runner, or `None`.
///
/// No null-object branch, unlike [`NullExporter`] above, and the difference is
/// the point. A null exporter drops telemetry aiwatcher already has; a null
/// runner would answer `202 Accepted` for a rerun that no orchestrator was ever
/// asked to perform. Absence has to reach the caller, so it reaches them as a
/// 501 naming the variable that is unset.
fn build_workflow_runner(
    config: &Config,
    engine: Option<&Arc<FlyteEngine>>,
) -> Result<Option<Arc<dyn WorkflowRunner>>> {
    match config.workflow_runner {
        WorkflowRunnerKind::None => {
            tracing::info!(
                "AIWATCHER_WORKFLOW_RUNNER=none; reruns answer 501 and nothing is dispatched"
            );
            Ok(None)
        }
        WorkflowRunnerKind::Http => {
            let endpoint = config.workflow_runner_url.clone().context(
                "AIWATCHER_WORKFLOW_RUNNER_URL is required for \
                          AIWATCHER_WORKFLOW_RUNNER=http",
            )?;
            tracing::info!(
                %endpoint,
                authenticated = config.workflow_runner_token.is_some(),
                "reruns will be dispatched to the configured orchestrator"
            );
            let runner = HttpRunner::new(HttpRunnerConfig {
                endpoint,
                token: config.workflow_runner_token.clone(),
                timeout: config.workflow_runner_timeout,
            })
            .context("building the workflow runner's HTTP client")?;
            Ok(Some(Arc::new(runner)))
        }
        WorkflowRunnerKind::Engine => {
            // One adapter, both ports. The alternative is an HTTP runner
            // pointed at a shim that then talks to the same control plane —
            // a second thing to deploy for no new capability.
            let engine = engine.context(
                "AIWATCHER_WORKFLOW_RUNNER=engine needs AIWATCHER_ENGINE set to an engine",
            )?;
            tracing::info!("reruns will be dispatched to the configured pipeline engine");
            Ok(Some(Arc::clone(engine) as Arc<dyn WorkflowRunner>))
        }
    }
}

/// The pipeline engine, or `None`.
///
/// Absence is a 501 for the same reason the runner's is, and this one has a
/// second edge: a catalog that answered with an empty list would say "the
/// orchestrator has nothing to run" about a deployment that has no
/// orchestrator. Those are different problems with different fixes.
///
/// Built as the concrete type rather than as `Arc<dyn WorkflowEngine>` so the
/// same instance can serve `WorkflowRunner` as well — one connection pool, one
/// cached token.
/// The dataset hubs this instance may search, or `None`.
///
/// `None` is the default and makes `/api/v1/dataset-hubs` answer 501 naming
/// the variables. That matters more here than for the other optional
/// subsystems: an empty search result reads as "there is no such corpus",
/// which is a claim about the world rather than about this deployment.
///
/// Note what is *not* configurable: whether a hub's licence is believed. It
/// never is. See `aiwatcher_annotations::integrations::hubs`.
/// The corpora somebody read the licence of, or an empty table.
///
/// Empty is the shipped default and it is a *working* state, not a degraded
/// one: with no rows nothing matches a hub result, every one keeps
/// `SourceUsage::Unclear`, and an import of one records unknown rights — which
/// a commercial export excludes by name. The failure direction of configuring
/// nothing is a smaller export and a line saying why.
///
/// A malformed file fails the start-up rather than being skipped. A catalogue
/// that silently did not load would answer every licence question with
/// "unclear" while looking exactly like one that had loaded.
fn build_dataset_sources(config: &Config) -> Result<Arc<SourceCatalog>> {
    let Some(path) = config.dataset_sources.as_deref() else {
        tracing::info!(
            "AIWATCHER_DATASET_SOURCES is unset; no corpus is curated, so every dataset hub \
             result stays licence-unclear"
        );
        return Ok(Arc::new(SourceCatalog::default()));
    };
    let body = std::fs::read(path)
        .with_context(|| format!("reading the dataset source catalogue at {path}"))?;
    let catalog = SourceCatalog::parse(&body)
        .with_context(|| format!("parsing the dataset source catalogue at {path}"))?;
    tracing::info!(
        sources = catalog.sources.len(),
        directories = catalog.directories.len(),
        %path,
        "the dataset source catalogue is loaded"
    );
    Ok(Arc::new(catalog))
}

fn build_dataset_hubs(config: &Config, sources: &SourceCatalog) -> Result<Option<Arc<Hubs>>> {
    let hub_config = HubConfig {
        kaggle_username: config.kaggle_username.clone(),
        kaggle_key: config.kaggle_key.clone(),
        huggingface: config.huggingface_enabled,
        huggingface_token: config.huggingface_token.clone(),
    };
    if !hub_config.any() {
        tracing::info!(
            "no dataset hub is configured; /api/v1/dataset-hubs answers 501 \
             (AIWATCHER_HUGGINGFACE_ENABLED, AIWATCHER_KAGGLE_USERNAME/AIWATCHER_KAGGLE_KEY)"
        );
        return Ok(None);
    }
    tracing::info!(
        huggingface = hub_config.huggingface,
        kaggle = hub_config.kaggle().is_some(),
        "dataset hub search is on; every result is licence-unclear unless it matches the \
         curated table"
    );
    Ok(Some(Arc::new(
        Hubs::with_catalog(hub_config, sources.sources.clone())
            .context("the dataset hub HTTP client could not be built")?,
    )))
}

fn build_engine(config: &Config) -> Result<Option<Arc<FlyteEngine>>> {
    match config.engine {
        EngineKind::None => {
            tracing::info!(
                "AIWATCHER_ENGINE=none; the engine routes answer 501 and nothing can be launched"
            );
            Ok(None)
        }
        EngineKind::Flyte => {
            let endpoint = config
                .flyte_endpoint
                .clone()
                .context("AIWATCHER_FLYTE_ENDPOINT is required for AIWATCHER_ENGINE=flyte")?;
            tracing::info!(
                %endpoint,
                project = %config.flyte_project,
                domain = %config.flyte_domain,
                authenticated = config.flyte_token.is_some() || config.flyte_client_id.is_some(),
                "the Flyte catalog is readable and its launch plans can be started"
            );
            let engine = FlyteEngine::new(FlyteConfig {
                endpoint,
                project: config.flyte_project.clone(),
                domain: config.flyte_domain.clone(),
                token: config.flyte_token.clone(),
                client_id: config.flyte_client_id.clone(),
                client_secret: config.flyte_client_secret.clone(),
                token_url: config.flyte_token_url.clone(),
                scopes: config.flyte_scopes.clone(),
                console_url: config.flyte_console_url.clone(),
                timeout: config.flyte_timeout,
            })
            .context("building the pipeline engine's HTTP client")?;
            Ok(Some(Arc::new(engine)))
        }
    }
}

/// The authenticator, or `None` when this deployment has no identity provider.
///
/// The same shape as the registry and the runner, and for once absence is not
/// a 501: `None` here means every caller is anonymous and every role check
/// passes, which is what `AIWATCHER_AUTH_MODE=none` means. The 501 is reserved
/// for the sign-in routes, which cannot do anything useful without a provider.
///
/// Allowed to fail the start-up, and this is the one place in this file where
/// that matters most. An instance that could not reach its provider and
/// started anyway would be an instance serving without authentication, which
/// is the failure this whole crate exists to prevent — so `connect` retries
/// while the provider comes up, and then gives up rather than degrading.
async fn build_authenticator(config: &Config) -> Result<Option<Arc<Authenticator>>> {
    if config.auth.mode == AuthMode::None {
        tracing::warn!(
            "AIWATCHER_AUTH_MODE=none; every caller is anonymous and every role check passes"
        );
        return Ok(None);
    }

    let authenticator = Authenticator::connect(config.auth.clone())
        .await
        .context("connecting to the identity provider")?
        .context("the authentication mode is not none but produced no authenticator")?;

    match config.auth.mode {
        AuthMode::Proxy => tracing::warn!(
            username_header = %config.auth.proxy_headers.username,
            "AIWATCHER_AUTH_MODE=proxy; identity comes from request headers, which is only \
             sound while nothing but the authenticating proxy can reach this port"
        ),
        _ => tracing::info!(
            issuer = %config.auth.issuer,
            client_id = %config.auth.client_id,
            "single sign-on is on"
        ),
    }

    Ok(Some(Arc::new(authenticator)))
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
            workflows: aiwatcher_projector::WorkflowConfig {
                max_executions: config.max_workflow_executions,
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

    // Hubs before registries: the annotation registry's import job needs
    // somewhere to fetch bytes from, and a hub is the only thing in this
    // process that has one.
    let sources = build_dataset_sources(&config)?;
    let hubs = build_dataset_hubs(&config, &sources)?;
    let registries = build_registries(
        &config,
        hubs.clone()
            .map(|hubs| hubs as Arc<dyn aiwatcher_annotations::integrations::fetch::ImageSource>),
    )
    .await?;
    let engine = build_engine(&config)?;
    let state = AppState {
        read_model,
        live,
        source,
        sink: config.ingest_enabled.then_some(sink),
        prompts: registries.prompts,
        datasets: registries.datasets,
        annotations: registries.annotations,
        conversations: registries.conversations,
        export_worker: Some(Arc::new(tokio::sync::Notify::new())),
        import_worker: Some(Arc::new(tokio::sync::Notify::new())),
        hubs,
        sources,
        training: registries.training,
        runner: build_workflow_runner(&config, engine.as_ref())?,
        engine: engine.map(|engine| engine as Arc<dyn aiwatcher_core::engine::WorkflowEngine>),
        auth: build_authenticator(&config).await?,
        health,
    };

    Ok(Runtime {
        state,
        config,
        projector,
    })
}
