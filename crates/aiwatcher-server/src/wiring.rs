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
use aiwatcher_execution::{ExecutionHandler, WorkflowStore};
use aiwatcher_projector::pipeline::Outputs;
use aiwatcher_projector::{FileDeadLetters, LiveHub, Projector, ProjectorConfig, ReadModel};
use aiwatcher_prompts::{Registry, RegistryConfig};
use aiwatcher_runner::{HttpRunner, HttpRunnerConfig};
use aiwatcher_trace::AssemblerConfig;
use aiwatcher_trace::otlp::{OtlpConfig, OtlpMetricSink, OtlpTraceStore};
use aiwatcher_training::Registry as TrainingRegistry;

use crate::config::{
    BackendKind, Config, ConversationPolicyMode, PromptStoreKind, WorkflowRunnerKind,
    WorkflowStoreKind,
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
    /// A workshop's labs (ADR_0034): the authored brief and the measurement it
    /// pins. The same store, its own prefix, and no second switch — a lab is
    /// authored like a prompt and needs nothing a prompt does not.
    labs: Option<Arc<aiwatcher_labs::Registry>>,
    /// Alert rules and the history of what was sent. The same store again, its
    /// own prefix, and no second switch either — what a deployment does need
    /// to say is where a notification goes, and that is a variable rather than
    /// anything stored here.
    alerts: Option<Arc<aiwatcher_alerts::Registry>>,
    evaluations: Option<Arc<aiwatcher_evaluation::Registry>>,
    /// The same adapter, seen from the other side: what an operator stages is
    /// what it later admits by, and the prefix those bytes land in is its own.
    evaluation_bundles: Option<Arc<dyn aiwatcher_evaluation::ApprovalBundles>>,
    /// The fifth, and the one this struct's doc comment does not describe: it
    /// shares the store and nothing else. Its content is encrypted, its
    /// retention is its own, and it is absent unless a deployment asked for it
    /// — so it has a second switch above the store's.
    conversations: Option<Arc<ConversationArchive>>,
    /// The store underneath all five, for the one writer that is not a
    /// registry: a reactor putting a step's result somewhere before it hands
    /// the digest on. A sixth prefix rather than a sixth registry, because an
    /// artifact has no head, no labels and no list — it is bytes named by
    /// their own hash.
    objects: Option<Arc<dyn aiwatcher_core::prompts::ObjectStore>>,
}

/// The object store every registry and the journal write, or `None` when this
/// deployment has none (`AIWATCHER_PROMPT_STORE=none`).
async fn build_object_store(
    config: &Config,
) -> Result<Option<Arc<dyn aiwatcher_core::prompts::ObjectStore>>> {
    let store: Arc<dyn aiwatcher_core::prompts::ObjectStore> = match config.prompt_store {
        PromptStoreKind::None => {
            tracing::info!(
                "AIWATCHER_PROMPT_STORE=none; the prompt, dataset, annotation and training registries are disabled"
            );
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
    Ok(Some(store))
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

    let Some(store) = build_object_store(config).await? else {
        return Ok(Registries::default());
    };

    let prompts = Arc::new(Registry::new(Arc::clone(&store), registry_config));
    let datasets = Arc::new(DatasetRegistry::new(Arc::clone(&store), "datasets"));
    if config.role.serves()
        && let Some(path) = &config.seed_file
    {
        let imported = crate::seed::import_file(&datasets, std::path::Path::new(path)).await?;
        tracing::info!(path, ?imported, "curation seed imported");
    }
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
    let labs = Arc::new(aiwatcher_labs::Registry::new(
        Arc::clone(&store),
        Default::default(),
    ));
    let alerts = Arc::new(aiwatcher_alerts::Registry::new(
        Arc::clone(&store),
        Default::default(),
    ));
    let conversations = build_conversation_archive(config, &store)?;
    let mut source = crate::evaluation::LocalSource::new(config.evaluation_source_dir.clone())
        .with_bundles(store.clone())
        .with_curation(datasets.clone())
        .with_prompts(prompts.clone())
        .with_training(training.clone())
        .with_annotations(annotations.clone());
    if let Some(owner) = &conversations {
        source = source.with_conversations(owner.clone());
    }
    // One adapter, two ports: what it admits a pair by, and where an operator
    // puts those bytes when the host this runs on has no disk to put them on.
    let source = Arc::new(source);
    let mut evaluations = aiwatcher_evaluation::Registry::new(
        store.clone(),
        source.clone(),
        config.evaluation_limits.clone(),
    )?;
    if conversations.is_some() {
        let keys = Keyring::parse(
            "AIWATCHER_CONVERSATION_KEYS",
            config
                .conversation_keys
                .as_deref()
                .context("conversation keys")?,
        )?;
        evaluations =
            evaluations.with_cipher(Arc::new(crate::evaluation::ConversationCipher(keys)));
    }
    Ok(Registries {
        prompts: Some(Arc::clone(&prompts)),
        datasets: Some(Arc::clone(&datasets)),
        annotations: Some(Arc::clone(&annotations)),
        training: Some(Arc::clone(&training)),
        labs: Some(labs),
        alerts: Some(alerts),
        evaluations: Some(Arc::new(evaluations)),
        evaluation_bundles: Some(source),
        conversations,
        objects: Some(store),
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
fn build_workflow_runner(config: &Config) -> Result<Option<Arc<dyn WorkflowRunner>>> {
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
    }
}

/// The corpora somebody read the licence of, or an empty table.
///
/// Empty is the shipped default and a *working* state: nothing matches a hub
/// result, every one keeps `SourceUsage::Unclear`, and an import records
/// unknown rights — which a commercial export excludes by name. Configuring
/// nothing costs a smaller export and a line saying why.
///
/// A malformed file fails the start-up rather than being skipped: a catalogue
/// that silently did not load answers every licence question with "unclear"
/// while looking exactly like one that had.
/// The deployment's price table, or `None` — nothing is priced without one.
///
/// Refused whole when any entry is not a price somebody could check: one
/// without the page it was read from, or the day.
fn build_model_prices(config: &Config) -> Result<Option<Arc<aiwatcher_core::prices::ModelPrices>>> {
    let Some(path) = config.model_prices.as_deref() else {
        tracing::info!(
            "AIWATCHER_MODEL_PRICES is unset; nothing a variant was observed doing is priced"
        );
        return Ok(None);
    };
    let body =
        std::fs::read(path).with_context(|| format!("reading the model price table at {path}"))?;
    let table: aiwatcher_core::prices::ModelPrices = serde_json::from_slice(&body)
        .with_context(|| format!("parsing the model price table at {path}"))?;
    table.validate().map_err(|problems| {
        anyhow::anyhow!(
            "the model price table at {path} is refused: {}",
            problems.join("; ")
        )
    })?;
    tracing::info!(prices = table.prices.len(), currency = %table.currency, %path, "the model price table is loaded");
    Ok(Some(Arc::new(table)))
}

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

/// The operator's pod templates, or `None` when none are configured (ADR_0029).
///
/// A malformed file fails the start-up with every template and field that is
/// wrong, for the source catalogue's reason: a template that silently did not
/// load refuses every step naming it while looking exactly like one that had.
fn build_pod_templates(
    config: &Config,
) -> Result<Option<Arc<aiwatcher_execution::pods::PodTemplates>>> {
    let Some(path) = config.pod_templates.as_deref() else {
        return Ok(None);
    };
    let body =
        std::fs::read(path).with_context(|| format!("reading the pod templates at {path}"))?;
    let templates = aiwatcher_execution::pods::PodTemplates::parse(&body)
        .with_context(|| format!("checking the pod templates at {path}"))?;
    tracing::info!(templates = templates.len(), %path, "the pod templates are loaded");
    Ok(Some(Arc::new(templates)))
}

/// The dataset hubs this instance may search, or `None`.
///
/// `None` is the default and makes `/api/v1/dataset-hubs` answer 501 naming the
/// variables. An empty search result would read as "there is no such corpus",
/// which is a claim about the world rather than about this deployment.
///
/// Not configurable: whether a hub's licence is believed. It never is. See
/// `aiwatcher_annotations::integrations::hubs`.
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

/// Where a managed execution's history lives.
///
/// Not an `Option`, unlike every other store above it. There is no "this
/// deployment configured none": the default needs a directory and nothing
/// else, and an execution store that was absent would make `POST /executions`
/// a 501 on the one route whose whole point is that this system does the work
/// itself.
///
/// The four adapters are the pattern this repository has set twice —
/// `memory | wal | laser` and `none | memory | file | s3`. `file` and `duckdb`
/// each take an exclusive lock, which is what stops a second process from
/// interleaving appends that each look fine alone; the refusal names the
/// variable (ADR_0025). Between those two, `duckdb` is the one a local install
/// wants: same single-process contract, no whole-file rewrites, and a database
/// `aiwatcher sql` can open.
async fn build_workflow_store(config: &Config) -> Result<Arc<dyn WorkflowStore>> {
    match config.workflow_store {
        WorkflowStoreKind::Memory => {
            tracing::warn!(
                "the workflow store is in memory; a managed execution will not survive a restart"
            );
            Ok(Arc::new(
                aiwatcher_execution::store::memory::MemoryWorkflowStore::new(),
            ))
        }
        WorkflowStoreKind::File => {
            let directory = config.workflow_dir();
            tracing::info!(
                %directory,
                "the workflow store is on disk and holds this process only"
            );
            Ok(Arc::new(
                aiwatcher_execution::store::file::FileWorkflowStore::open(&directory)
                    .await
                    .context("opening the workflow store")?,
            ))
        }
        #[cfg(feature = "duckdb")]
        WorkflowStoreKind::Duckdb => {
            let path = std::path::Path::new(&config.data_dir).join("aiwatcher.duckdb");
            tracing::info!(
                path = %path.display(),
                "the workflow store is DuckDB and holds this process only"
            );
            Ok(Arc::new(
                aiwatcher_execution::store::duckdb::DuckdbWorkflowStore::open(&path)
                    .context("opening the workflow store")?,
            ))
        }
        #[cfg(not(feature = "duckdb"))]
        WorkflowStoreKind::Duckdb => {
            // Named rather than fallen back from, for the reason below: a
            // silent `file` would be a different store with different
            // durability than the one somebody asked for.
            anyhow::bail!(
                "AIWATCHER_WORKFLOW_STORE=duckdb needs this binary built with the `duckdb` \
                 cargo feature (`cargo build --features duckdb`, or `just run-duckdb`)"
            );
        }
        #[cfg(feature = "postgres")]
        WorkflowStoreKind::Postgres => {
            let url = config.workflow_postgres_url.clone().context(
                "AIWATCHER_WORKFLOW_POSTGRES_URL is required for AIWATCHER_WORKFLOW_STORE=postgres",
            )?;
            tracing::info!(
                max_connections = config.workflow_postgres_max_connections,
                "the workflow store is PostgreSQL; the schema is applied on connect"
            );
            Ok(Arc::new(
                aiwatcher_execution::store::postgres::connect(
                    &url,
                    config.workflow_postgres_max_connections,
                )
                .await
                .context("connecting to the workflow store")?,
            ))
        }
        #[cfg(not(feature = "postgres"))]
        WorkflowStoreKind::Postgres => {
            // Silently falling back to `file` would be worse than not
            // starting: a deployment that asked for the multi-process store
            // would get the one that refuses a worker, and find out when a
            // step was dispatched to nobody.
            anyhow::bail!(
                "AIWATCHER_WORKFLOW_STORE=postgres needs this binary built with the `postgres` \
                 cargo feature (`cargo build --features postgres`, or `just run-postgres`)"
            );
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
    /// What the work role's reactors and outbox publisher run against. The
    /// same store the API accepts commands into — one adapter, three readers.
    pub workflow_store: Arc<dyn WorkflowStore>,
    /// Where a reactor puts a step's result, when this deployment has one.
    /// `None` disables every executor that produces an artifact, which is
    /// every one of them.
    pub artifacts: Option<Arc<dyn aiwatcher_core::prompts::ObjectStore>>,
    /// The event log this process publishes execution facts onto, after
    /// commit. Held whether or not HTTP ingest is enabled: the outbox is not a
    /// second write path for producers, it is this system's own.
    pub sink: Arc<dyn MessageSink>,
    /// Where this process reports what it is doing, as numbers. The same sink
    /// the projector flushes run metrics through — one exporter, because a
    /// second would be a second answer to "is this deployment configured to
    /// export metrics".
    pub metrics: Arc<dyn aiwatcher_core::ports::MetricSink>,
    pub projector: Box<dyn ProjectorTask>,
    /// The journal of what the period fold reads, when the deployment keeps
    /// one: a consumer of the log of its own, run by whichever role drains work.
    pub journal: Option<Box<dyn JournalTask>>,
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
}

/// A journal told how long the log keeps an event and where to say how far
/// behind it is, where the deployment has either.
fn observed_journal<S, C>(
    journal: aiwatcher_projector::Journal<S, C>,
    config: &Config,
    metrics: Option<Arc<dyn MetricSink>>,
) -> aiwatcher_projector::Journal<S, C>
where
    S: MessageSource + 'static,
    C: Checkpointer + 'static,
{
    let journal = match metrics {
        Some(metrics) => journal.measuring(metrics),
        None => journal,
    };
    match config.log_retention {
        Some(retention) => journal.with_log_retention(retention),
        None => journal,
    }
}

/// The observation journal on its own (`AIWATCHER_ROLE=journal`): the log, the
/// object store its pages go to, and nothing else — no listener, no read model,
/// no workflow store and no identity provider, so it stays up where they
/// cannot and keeps reading while every process that accepts or drains work is
/// down. It reads under the name the other roles' journals read under, so a
/// broker gives the log to whichever of them is up.
///
/// # Errors
///
/// A log other than Laser, a deployment with no object store or no days to
/// keep, and a broker or store that could not be reached.
pub async fn build_journal(config: &Config) -> Result<Box<dyn JournalTask>> {
    match config.bus {
        #[cfg(feature = "laser")]
        BackendKind::Laser => {
            use aiwatcher_bus::adapters::laser::{LaserBus, LaserConfig};

            let days = config.observation_journal_days.context(
                "AIWATCHER_OBSERVATION_JOURNAL_DAYS is required for AIWATCHER_ROLE=journal",
            )?;
            let store = build_object_store(config)
                .await?
                .context("AIWATCHER_PROMPT_STORE is required for AIWATCHER_ROLE=journal")?;

            let laser = Arc::new(
                LaserBus::connect(LaserConfig {
                    connection_string: config.laser_connection_string.clone().context(
                        "AIWATCHER_LASER_CONNECTION_STRING is required for AIWATCHER_BUS=laser",
                    )?,
                    stream: config.laser_stream.clone(),
                    topic: config.laser_topic.clone(),
                    partitions: config.laser_partitions,
                    batch_length: 256,
                    ..LaserConfig::default()
                })
                .await
                .context("connecting the observation journal to Laser")?,
            );
            Ok(Box::new(TypedJournal {
                inner: Arc::new(observed_journal(
                    aiwatcher_projector::Journal::new(
                        Arc::clone(&laser),
                        laser,
                        aiwatcher_projector::PeriodStore::new(store),
                        format!("{}-journal", config.processor_id),
                        days,
                        // The broker resumes the journal's group from its own
                        // committed offset.
                        aiwatcher_bus::StartFrom::Now,
                    ),
                    config,
                    // Where every other role's metrics go, so its margin to a
                    // gap is on the same dashboard as theirs.
                    match config.otlp_endpoint.as_deref() {
                        Some(endpoint) => Some(Arc::new(
                            OtlpMetricSink::new(OtlpConfig::new(
                                endpoint,
                                config.service_name.clone(),
                            ))
                            .context("building the OTLP metric exporter")?,
                        ) as Arc<dyn MetricSink>),
                        None => None,
                    },
                )),
            }))
        }
        other => anyhow::bail!(
            "AIWATCHER_ROLE=journal reads the log other processes write through Laser, and this \
             process has AIWATCHER_BUS={other:?}{}",
            if cfg!(feature = "laser") {
                ""
            } else {
                " in a build without the `laser` feature"
            }
        ),
    }
}

/// Erases the projector's two generic parameters so `Runtime` does not have to
/// carry them./// Erases the projector's two generic parameters so `Runtime` does not have to
/// carry them.
#[async_trait::async_trait]
pub trait ProjectorTask: Send + Sync {
    async fn run(self: Box<Self>, shutdown: CancellationToken) -> Result<()>;
}

/// The observation journal, erased over its log the way the projector is.
#[async_trait::async_trait]
pub trait JournalTask: Send + Sync {
    async fn run(self: Box<Self>, shutdown: CancellationToken) -> Result<()>;
}

struct TypedJournal<S, C> {
    inner: Arc<aiwatcher_projector::Journal<S, C>>,
}

#[async_trait::async_trait]
impl<S, C> JournalTask for TypedJournal<S, C>
where
    S: MessageSource + 'static,
    C: Checkpointer + 'static,
{
    async fn run(self: Box<Self>, shutdown: CancellationToken) -> Result<()> {
        Arc::clone(&self.inner)
            .run(shutdown)
            .await
            .context("the observation journal stopped with an error")
    }
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
    let observations = registries.objects.clone().map(|store| {
        Arc::new(
            aiwatcher_projector::PeriodOutput::new(
                aiwatcher_projector::PeriodStore::new(store),
                config.processor_id.clone(),
                i64::try_from(config.observation_period.as_secs()).unwrap_or(300),
            )
            .reading_journal(config.observation_journal_days.is_some()),
        )
    });
    // What witnesses saw asked, call by call, wherever there is an object
    // store to keep it in — and read back by every traces step.
    let asked = registries.objects.clone().map(|store| {
        let index = aiwatcher_projector::AskedIndex::new(store, config.processor_id.clone());
        Arc::new(match config.asked_index_days {
            0 => index,
            days => index.keeping(std::time::Duration::from_secs(days.saturating_mul(86_400))),
        })
    });
    // The journal reads the log under its own name and commits its own
    // position, and keeps its pages beside the periods it refills.
    let journal_of = |days: u64| {
        registries.objects.clone().map(|store| {
            (
                aiwatcher_projector::PeriodStore::new(store),
                format!("{}-journal", config.processor_id),
                days,
            )
        })
    };
    let outputs = Outputs {
        live: Arc::clone(&live) as _,
        traces,
        metrics: Arc::clone(&metrics),
        dead_letters,
        read_model: Arc::clone(&read_model),
        // What variants were observed doing, written as the log passes each
        // period, wherever there is an object store to write it to — and read
        // back by every window over it.
        periods: observations.clone(),
        asked: asked.clone(),
    };

    // Each arm produces the same three things, and the journal where one is
    // kept; only the concrete types differ.
    let journal: Option<Box<dyn JournalTask>>;
    let (source, sink, projector): (
        Arc<dyn MessageSource>,
        Arc<dyn MessageSink>,
        Box<dyn ProjectorTask>,
    ) =
        match config.bus {
            BackendKind::Memory => {
                let bus = Arc::new(InMemoryBus::new());
                let projector = Arc::new(Projector::new(
                    Arc::clone(&bus),
                    Arc::clone(&bus),
                    outputs,
                    projector_config,
                ));
                journal = config.observation_journal_days.and_then(journal_of).map(
                    |(store, name, days)| {
                        Box::new(TypedJournal {
                            inner: Arc::new(observed_journal(
                                aiwatcher_projector::Journal::new(
                                    Arc::clone(&bus),
                                    Arc::clone(&bus),
                                    store,
                                    name,
                                    days,
                                    aiwatcher_bus::StartFrom::Beginning,
                                ),
                                &config,
                                Some(Arc::clone(&metrics)),
                            )),
                        }) as Box<dyn JournalTask>
                    },
                );
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
                journal = config.observation_journal_days.and_then(journal_of).map(
                    |(store, name, days)| {
                        Box::new(TypedJournal {
                            inner: Arc::new(observed_journal(
                                aiwatcher_projector::Journal::new(
                                    Arc::clone(&wal),
                                    Arc::clone(&wal),
                                    store,
                                    name,
                                    days,
                                    aiwatcher_bus::StartFrom::Beginning,
                                ),
                                &config,
                                Some(Arc::clone(&metrics)),
                            )),
                        }) as Box<dyn JournalTask>
                    },
                );
                (
                    Arc::clone(&wal) as _,
                    Arc::clone(&wal) as _,
                    Box::new(TypedProjector { inner: projector }),
                )
            }
            #[cfg(feature = "laser")]
            BackendKind::Laser => {
                use aiwatcher_bus::adapters::laser::{LaserBus, LaserConfig};

                let connection_string = config.laser_connection_string.clone().context(
                    "AIWATCHER_LASER_CONNECTION_STRING is required for AIWATCHER_BUS=laser",
                )?;
                let laser = LaserConfig {
                    connection_string,
                    stream: config.laser_stream.clone(),
                    topic: config.laser_topic.clone(),
                    partitions: config.laser_partitions,
                    batch_length: 256,
                    ..LaserConfig::default()
                };
                // A connection of its own: one holds one subscription's commits,
                // and the journal's must never move the projector's offset.
                journal = match config.observation_journal_days.and_then(journal_of) {
                    Some((store, name, days)) => {
                        let own = Arc::new(
                            LaserBus::connect(laser.clone())
                                .await
                                .context("connecting the observation journal to Laser")?,
                        );
                        Some(Box::new(TypedJournal {
                            inner: Arc::new(observed_journal(
                                aiwatcher_projector::Journal::new(
                                    Arc::clone(&own),
                                    own,
                                    store,
                                    name,
                                    days,
                                    // The broker resumes the journal's group from its
                                    // own committed offset.
                                    aiwatcher_bus::StartFrom::Now,
                                ),
                                &config,
                                Some(Arc::clone(&metrics)),
                            )),
                        }) as Box<dyn JournalTask>)
                    }
                    None => None,
                };
                let bus = Arc::new(
                    LaserBus::connect(laser)
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

    let workflow_store = build_workflow_store(&config).await?;
    let state = AppState {
        read_model,
        live,
        source,
        sink: config.ingest_enabled.then(|| Arc::clone(&sink)),
        prompts: registries.prompts,
        labs: registries.labs,
        alerts: registries.alerts,
        alert_channel: crate::alerts::build_channel(&config)?,
        datasets: registries.datasets,
        query_engine: config.query_engine,
        query_step_timeout_seconds: config.query_step_timeout_seconds,
        judge_provider: config.judge_url.as_ref().and(config.judge_provider.clone()),
        judge_concurrency: config.judge_concurrency,
        scorer_concurrency: config
            .scorer_url
            .as_ref()
            .map(|_| config.scorer_concurrency),
        // The same object store the definitions live in, under its own prefix.
        workflow_definitions: registries.objects.as_ref().map(|store| {
            Arc::new(aiwatcher_execution::definition::DefinitionRegistry::new(
                Arc::clone(store),
            ))
        }),
        pod_templates: build_pod_templates(&config)?,
        pod_runtime: config.pod_runtime,
        schedules: registries
            .objects
            .as_ref()
            .map(|store| Arc::new(aiwatcher_execution::ScheduleStore::new(Arc::clone(store)))),
        annotations: registries.annotations,
        conversations: registries.conversations,
        executions: Some(Arc::new(ExecutionHandler::new(Arc::clone(&workflow_store)))),
        export_worker: Some(Arc::new(tokio::sync::Notify::new())),
        import_worker: Some(Arc::new(tokio::sync::Notify::new())),
        // Held whatever this process's role is: the API accepts a command
        // either way, and a nudge nobody is waiting for costs nothing. What
        // decides whether anything drains it is `AIWATCHER_ROLE`.
        execution_worker: Some(Arc::new(tokio::sync::Notify::new())),
        hubs,
        sources,
        training: registries.training,
        evaluations: registries.evaluations,
        evaluation_bundles: registries.evaluation_bundles,
        observations,
        asked,
        model_prices: build_model_prices(&config)?,
        // A witness's digests of a call's words are keyed by the credential it
        // published with, which this deployment issued — or by the credential
        // the deployment says it digests under: every ingest token's key, and
        // only the named ones admitted where any are named.
        witnesses: aiwatcher_evaluation::Witnesses::named(config.witnesses.clone()).keyed(
            config.auth.ingest_tokens.iter().filter_map(|token| {
                let owner = config
                    .witness_digests
                    .get(&token.label)
                    .map_or(token.label.as_str(), String::as_str);
                let secret = config
                    .auth
                    .ingest_tokens
                    .iter()
                    .find(|issued| issued.label == owner)?;
                Some((
                    token.label.clone(),
                    aiwatcher_core::witness::key_for(&secret.secret),
                ))
            }),
        ),
        runner: build_workflow_runner(&config)?,
        // Built in the `serve` role too, unlike an executor: opening a block's
        // editor is a person waiting on a request, not an attempt somebody
        // claimed. It still registers nothing without an address and an object
        // store, so a deployment that runs no notebook runtime answers 501.
        editor: registries
            .objects
            .clone()
            .map(crate::execution::artifacts::Artifacts::new)
            .and_then(|artifacts| crate::execution::editor::host(&config, Some(&artifacts))),
        // The worker's half of the same store, in the `serve` role for the
        // same reason the editor is: a worker is a client waiting on a
        // request, not a reactor this process runs. Absent with no object
        // store, which makes the artifact routes answer 501 while claiming and
        // settling still work — a task that takes its parameters and returns a
        // bounded value needs no artifact at all.
        artifacts: registries.objects.clone().map(|store| {
            Arc::new(crate::execution::artifacts::Artifacts::new(store))
                as Arc<dyn aiwatcher_core::ports::AttemptArtifacts>
        }),
        // The same catalog the work role's reactors hold. A worker's step is
        // cached and its lineage recorded like any other, and building a
        // second one here would be a second index over one store.
        catalog: registries.objects.clone().map(|store| {
            Arc::new(aiwatcher_execution::ObjectArtifactCatalog::new(store))
                as Arc<dyn aiwatcher_execution::ArtifactCatalog>
        }),
        execution_payloads: aiwatcher_execution::PayloadDefault {
            policy: config.execution_payloads,
            locked: config.execution_payloads_locked,
        },
        answer_limits: config.answer_limits,
        auth: build_authenticator(&config).await?,
        iam: build_iam_store(&config).await?,
        // Built here rather than inside the authenticator: it is an outbound
        // credential for the provider's own API, used by one route, and a
        // deployment that has single sign-on and has not configured this is
        // the ordinary case.
        provisioning: config
            .provisioning
            .clone()
            .map(aiwatcher_auth::AccountProvisioning::new)
            .transpose()
            .context("the identity provider's enrolment API")?
            .map(Arc::new),
        // Two halves: a trail to read and somewhere to put the copy. Absent
        // when either is, which answers 501 rather than queueing a job that
        // could never write a shard.
        iam_audit_exports: registries.objects.clone().map(|store| {
            Arc::new(aiwatcher_iam::AuditExports::new(
                store,
                config.iam_audit_prefix.clone(),
            ))
        }),
        iam_audit_worker: Some(Arc::new(tokio::sync::Notify::new())),
        health,
    };

    Ok(Runtime {
        state,
        config,
        workflow_store,
        artifacts: registries.objects,
        sink,
        metrics,
        projector,
        journal,
    })
}

/// Explicit opt-in: never silently use a process-local store for IAM in a server.
async fn build_iam_store(config: &Config) -> Result<Option<Arc<dyn aiwatcher_iam::IamStore>>> {
    let Some(url) = config.iam_postgres_url.as_deref() else {
        return Ok(None);
    };
    anyhow::ensure!(
        config.auth.mode == AuthMode::Oidc,
        "IAM requires AIWATCHER_AUTH_MODE=oidc"
    );
    #[cfg(feature = "postgres")]
    {
        // Do not log the URL: it may contain a password.
        let store = aiwatcher_iam::postgres::PostgresIamStore::connect(url, 5)
            .await
            .context("connecting the IAM control plane to PostgreSQL")?;
        Ok(Some(Arc::new(store)))
    }
    #[cfg(not(feature = "postgres"))]
    {
        let _ = url;
        anyhow::bail!(
            "AIWATCHER_IAM_POSTGRES_URL needs this binary built with the `postgres` cargo feature"
        )
    }
}

#[cfg(test)]
mod iam_tests {
    use super::*;

    #[tokio::test]
    async fn absent_iam_configuration_opens_no_store() {
        assert!(
            build_iam_store(&Config::default())
                .await
                .expect("disabled")
                .is_none()
        );
    }

    #[cfg(not(feature = "postgres"))]
    #[tokio::test]
    async fn configured_iam_never_falls_back_without_postgres_feature() {
        let config = Config {
            iam_postgres_url: Some("postgres://localhost/test".into()),
            auth: aiwatcher_auth::AuthConfig {
                mode: AuthMode::Oidc,
                ..Default::default()
            },
            ..Config::default()
        };
        let error = build_iam_store(&config).await.expect_err("missing feature");
        assert!(error.to_string().contains("postgres"));
    }
}
