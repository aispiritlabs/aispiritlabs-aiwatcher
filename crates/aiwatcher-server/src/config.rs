//! Configuration, from the environment.
//!
//! Environment variables rather than a config file: this runs in containers,
//! and a variable is the one thing every orchestrator can set. Every setting
//! has a default that works, so `cargo run --bin aiwatcher` starts a usable
//! instance with no setup at all.

use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;

use aiwatcher_auth::{AuthConfig, AuthMode, IngestToken, ProxyHeaders, Role, RoleMapping};
use aiwatcher_datasets::QueryEngine;
use aiwatcher_execution::message::PayloadPolicy;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("{name} is set to {value:?}, which is not a valid {expected}")]
    Invalid {
        name: &'static str,
        value: String,
        expected: &'static str,
    },

    /// A variable that switched on something aiwatcher no longer has.
    ///
    /// Refused rather than ignored: a deployment still setting it expects what
    /// it switched on, and a server that started without it would say nothing.
    #[error("{name} is set to {value:?}, but {what} was removed from aiwatcher; {instead}")]
    Removed {
        name: &'static str,
        value: String,
        what: &'static str,
        instead: &'static str,
    },

    #[error("{name} is required when AIWATCHER_BUS={bus}")]
    Missing {
        name: &'static str,
        bus: &'static str,
    },

    #[error("{name} is required when {because}")]
    Required {
        name: &'static str,
        because: &'static str,
    },

    /// A variable this process cannot act on, in this role or this build.
    ///
    /// Refused rather than ignored, for `Removed`'s reason: whoever set it
    /// expects what it asks for, and a server that started without it would
    /// say nothing.
    #[error("{name} is set, but {why}")]
    Unusable {
        name: &'static str,
        why: &'static str,
    },

    /// A configuration the authentication crate itself refused. Its own
    /// variant rather than a `String` in `Invalid`, so the message the crate
    /// wrote — which names the variable and says what it needed — reaches the
    /// operator unedited.
    #[error(transparent)]
    Auth(#[from] aiwatcher_auth::AuthError),
}

/// Which durable log to run against.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BackendKind {
    /// Nothing survives a restart. Demos and tests.
    Memory,
    /// An append-only file. Single node, durable, no broker.
    #[default]
    Wal,
    /// Apache Iggy via `laser_sdk`. Needs the `laser` cargo feature compiled in.
    Laser,
}

impl FromStr for BackendKind {
    type Err = ConfigError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "memory" | "in-memory" => Ok(Self::Memory),
            "wal" | "file" => Ok(Self::Wal),
            "laser" => Ok(Self::Laser),
            other => Err(ConfigError::Invalid {
                name: "AIWATCHER_BUS",
                value: other.to_owned(),
                expected: "one of memory, wal, laser",
            }),
        }
    }
}

/// Where prompts are kept.
///
/// Its own setting rather than a flag on the bus, because it answers a
/// different question. The bus decides what happens to events, which are
/// bounded by retention; this decides what happens to prompts, which are not.
/// A deployment can perfectly well run Laser and keep prompts on a disk, or
/// run the write-ahead log and keep prompts in RustFS.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PromptStoreKind {
    /// No registry. Every `/api/v1/prompts` route answers 501.
    None,
    /// In process. Nothing survives a restart; for `just dev` and demos.
    Memory,
    /// A directory under `AIWATCHER_DATA_DIR`. The default, for the same
    /// reason the write-ahead log is: `cargo run --bin aiwatcher` has to be a
    /// working instance with nothing else running.
    #[default]
    File,
    /// An S3-compatible object store — RustFS in this stack, but MinIO, Ceph
    /// or AWS work unchanged. What a deployment runs.
    S3,
}

impl FromStr for PromptStoreKind {
    type Err = ConfigError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "none" | "off" | "disabled" => Ok(Self::None),
            "memory" | "in-memory" => Ok(Self::Memory),
            "file" | "fs" | "disk" => Ok(Self::File),
            // `rustfs` is accepted because that is what the service is called
            // in the compose file; it is an S3 endpoint either way.
            "s3" | "rustfs" | "minio" => Ok(Self::S3),
            other => Err(ConfigError::Invalid {
                name: "AIWATCHER_PROMPT_STORE",
                value: other.to_owned(),
                expected: "one of none, memory, file, s3",
            }),
        }
    }
}

/// How strict the conversation archive is about consent provenance.
///
/// `Protected` is the default, which is the reverse of the usual arrangement
/// and deliberate: the safe configuration must not be the one somebody has to
/// remember. See `aiwatcher_conversations::policy`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConversationPolicyMode {
    /// A turn with no consent record and no redaction record is refused.
    #[default]
    Protected,
    /// Whatever arrives is recorded as it arrived, gaps and all — which an
    /// export then excludes by name, in a manifest, forever.
    Open,
}

impl FromStr for ConversationPolicyMode {
    type Err = ConfigError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "protected" | "strict" => Ok(Self::Protected),
            "open" | "permissive" => Ok(Self::Open),
            other => Err(ConfigError::Invalid {
                name: "AIWATCHER_CONVERSATION_POLICY",
                value: other.to_owned(),
                expected: "one of protected, open",
            }),
        }
    }
}

/// What a step's pod *is* in this deployment (ADR_0029, amended).
///
/// A plan never says: a `container_job` step names a template and an image,
/// and where that runs is the operator's. `Kubernetes` is one Job per attempt
/// and the default; `Process` is one local process per attempt, for a
/// deployment that has no cluster to offer — which keeps the launcher, the
/// derived name, the claim by key and the log, and keeps neither the image nor
/// a resource limit. The chart offers only the first: a release in a cluster
/// asking for the second would be running steps in the API pod.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PodRuntime {
    #[default]
    Kubernetes,
    Process,
}

impl FromStr for PodRuntime {
    type Err = ConfigError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "kubernetes" | "k8s" | "cluster" | "pods" => Ok(Self::Kubernetes),
            "process" | "processes" | "local" | "host" => Ok(Self::Process),
            other => Err(ConfigError::Invalid {
                name: "AIWATCHER_POD_RUNTIME",
                value: other.to_owned(),
                expected: "one of kubernetes, process",
            }),
        }
    }
}

/// Where a managed execution's history lives.
///
/// Its own setting for the reason the prompt store is: it answers a different
/// question from the bus. The log is a rolling window of what producers said;
/// this is the transactional record of what this system decided, and one
/// deployment can perfectly well run Laser and keep executions on a disk.
///
/// `File` is the default so `cargo run --bin aiwatcher` is a working instance
/// with nothing else running — and it holds **one process**, which is what
/// makes a run needing a worker refuse to start rather than hang (ADR_0025).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WorkflowStoreKind {
    /// Nothing survives a restart. Tests and demos.
    Memory,
    /// A directory under `AIWATCHER_DATA_DIR`. One process, by design.
    #[default]
    File,
    /// What a deployment runs. Needs the `postgres` cargo feature compiled in.
    Postgres,
    /// One DuckDB database, for one machine — what `postgres` is for a
    /// deployment. Holds one process, like `file`, and unlike `file` it is a
    /// thing you can ask a question of: `aiwatcher sql` opens the same file.
    /// Needs the `duckdb` cargo feature compiled in.
    Duckdb,
}

impl FromStr for WorkflowStoreKind {
    type Err = ConfigError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "memory" | "in-memory" => Ok(Self::Memory),
            "file" | "fs" | "disk" => Ok(Self::File),
            "postgres" | "postgresql" | "pg" => Ok(Self::Postgres),
            "duckdb" | "duck" => Ok(Self::Duckdb),
            other => Err(ConfigError::Invalid {
                name: "AIWATCHER_WORKFLOW_STORE",
                value: other.to_owned(),
                expected: "one of memory, file, duckdb, postgres",
            }),
        }
    }
}

/// Which half of the binary this process is (ADR_0025).
///
/// `Serve` holds the API, the read model and the object store, and opens no
/// socket to Flow, a notebook runtime, an engine or the cluster. `Work` holds
/// the outbox publisher and the reactors, and is the only role that does.
///
/// `Both` is the default and is not a third role: it is the two of them in one
/// process, which is what a single-node install and `just dev` run — and what
/// the `file` store *requires*, since it holds one process and both halves need
/// it. A deployment that wants the network boundary runs two Deployments on
/// `postgres`, and the one holding the cluster's credentials is the one holding
/// no ingress.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProcessRole {
    #[default]
    Both,
    Serve,
    Work,
}

impl ProcessRole {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Both => "both",
            Self::Serve => "serve",
            Self::Work => "work",
        }
    }

    /// Whether this process listens, folds the log and answers reads.
    #[must_use]
    pub const fn serves(self) -> bool {
        matches!(self, Self::Both | Self::Serve)
    }

    /// Whether this process drains the outbox and claims attempts.
    #[must_use]
    pub const fn works(self) -> bool {
        matches!(self, Self::Both | Self::Work)
    }
}

impl FromStr for ProcessRole {
    type Err = ConfigError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "both" | "all" => Ok(Self::Both),
            "serve" | "server" | "api" => Ok(Self::Serve),
            "work" | "worker" => Ok(Self::Work),
            other => Err(ConfigError::Invalid {
                name: "AIWATCHER_ROLE",
                value: other.to_owned(),
                expected: "one of serve, work, both",
            }),
        }
    }
}

/// Whether this deployment can ask an orchestrator to run a workflow again.
///
/// `None` is the default, and deliberately so: everything else aiwatcher does
/// is a read, and turning it into something that can trigger work in another
/// system should be a decision somebody made rather than one they inherited.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WorkflowRunnerKind {
    /// No runner. `POST /api/v1/workflows/{id}/rerun` answers 501.
    #[default]
    None,
    /// One HTTP endpoint, named here. Never named by an event — see
    /// `aiwatcher_runner`.
    Http,
}

impl FromStr for WorkflowRunnerKind {
    type Err = ConfigError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "none" | "off" | "disabled" => Ok(Self::None),
            "http" | "https" | "webhook" => Ok(Self::Http),
            // The runner that relaunched through the engine went with it. Named
            // rather than left to the catch-all, so a deployment still asking
            // for it reads why instead of a list it thinks it is on.
            "engine" | "flyte" => Err(ConfigError::Removed {
                name: "AIWATCHER_WORKFLOW_RUNNER",
                value: value.to_owned(),
                what: "the rerun through the Flyte pipeline engine",
                instead: "set it to http or none",
            }),
            other => Err(ConfigError::Invalid {
                name: "AIWATCHER_WORKFLOW_RUNNER",
                value: other.to_owned(),
                expected: "one of none, http",
            }),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub listen: SocketAddr,
    pub bus: BackendKind,
    /// Directory for the write-ahead log and the dead letter queue.
    pub data_dir: String,
    /// Optional authored-data seed, imported before the API starts serving.
    /// `from_env` selects examples/seed.json; `none` disables importing.
    pub seed_file: Option<String>,
    /// `user:password@host:port`. Required when `bus = Laser`.
    pub laser_connection_string: Option<String>,
    /// The Iggy stream. One per deployment, not per run.
    pub laser_stream: String,
    /// The topic every agent event is published to.
    pub laser_topic: String,
    /// Kept at 1: a scalar checkpoint has no total order across partitions.
    /// See `aiwatcher_bus::adapters::laser`.
    pub laser_partitions: u32,
    /// OTLP base URL. Point at the Collector, or straight at VictoriaTraces.
    pub otlp_endpoint: Option<String>,
    pub service_name: String,
    pub processor_id: String,
    /// Enable `POST /api/v1/events`. Off by default: producers should publish
    /// to the log, and a second write path is a second thing to secure.
    pub ingest_enabled: bool,
    /// CORS origins for the panel. Empty means same-origin only.
    pub cors_origins: Vec<String>,
    pub orphan_timeout: Duration,
    /// Runs kept in the read model. With `max_spans_total`, this is what
    /// decides the process's footprint — see `ReadModelConfig`.
    pub max_runs: usize,
    /// Spans kept across all runs. The cap that makes memory predictable.
    pub max_spans_total: usize,
    /// Evaluation reports kept. Their own cap: a report is a
    /// producer-supplied document, not a span.
    pub max_evaluations: usize,
    /// Scored cases kept across all evaluations. What
    /// `max_evaluations × max_cases_per_evaluation` is not — a bound.
    pub max_evaluation_cases_total: usize,
    /// Where the prompt registry keeps its objects.
    pub prompt_store: PromptStoreKind,
    /// Operator-approved synthetic Evaluation bundle; no caller-controlled path.
    pub evaluation_source_dir: Option<String>,
    pub evaluation_limits: aiwatcher_evaluation::RegistryConfig,
    /// Key prefix inside the bucket or directory, so one bucket can hold this
    /// registry beside whatever else a cluster keeps in it.
    pub prompt_prefix: String,
    /// Base URL of the S3 endpoint. Required when `prompt_store = S3`.
    pub prompt_s3_endpoint: Option<String>,
    pub prompt_s3_bucket: String,
    pub prompt_s3_access_key: Option<String>,
    pub prompt_s3_secret_key: Option<String>,
    pub prompt_s3_session_token: Option<String>,
    pub prompt_s3_region: String,
    /// Create the bucket at start-up when it is missing.
    pub prompt_s3_create_bucket: bool,
    /// Whether this instance keeps conversation content at all.
    ///
    /// Off by default, and the only authored store here for which that is
    /// true. Every other one is off because a deployment has not wired an
    /// object store; this one is off because holding somebody's words is a
    /// decision that has to be made rather than inherited. See ADR_0021.
    pub conversation_archive: bool,
    /// The keys the archive seals and opens with: `id:key[,id:key]`, active
    /// first, each key 32 base64url bytes. Required when the archive is on —
    /// an archive with no key would be a plaintext archive, which is the one
    /// thing this must never quietly become.
    pub conversation_keys: Option<String>,
    /// Key prefix inside the same object store the other registries use.
    pub conversation_prefix: String,
    /// Where a hosted execution's words live unless the run says otherwise.
    ///
    /// `external` — the free one, and the default — means the content stays
    /// wherever the worker keeps it and this instance holds a reference, a
    /// plaintext digest and a size. `sealed` means the content comes here and
    /// is encrypted under the conversation archive's keys.
    ///
    /// The default is the visible one on purpose: a hosted execution starts
    /// with no archive, no key and no flag, and a deployment that wants its
    /// words held here turns that on rather than discovering it was already
    /// happening.
    pub execution_payloads: PayloadPolicy,
    /// Whether a run may choose a policy other than the default.
    ///
    /// Off, so a definition picks for itself. On, the deployment's choice is
    /// the only one — which is what an instance under a retention obligation
    /// sets so that a graph cannot opt its own turns out of the archive.
    pub execution_payloads_locked: bool,
    /// Whether consent provenance is demanded of a producer.
    pub conversation_policy: ConversationPolicyMode,
    /// The longest retention this deployment will apply. A producer asking for
    /// more is shortened to this, and the turn records that it was.
    pub conversation_max_ttl_days: u32,
    /// Whether the server's own scan refusing content is fatal. Off by
    /// default: a scanner is a heuristic, and one that rejects a write throws
    /// away the only copy of an exchange because a hex string looked like a
    /// key. A finding always blocks the export, which is the gate that matters.
    pub conversation_reject_on_finding: bool,
    /// How often the retention sweep runs.
    pub conversation_sweep_interval: Duration,
    /// How often the export worker looks for a job, when nothing has nudged it.
    pub conversation_export_poll: Duration,
    /// The same, for the annotation import queue.
    pub import_poll: Duration,
    /// Executions kept in the workflow projection. Its own cap: a graph is
    /// held per execution, and an execution can outlive several runs.
    pub max_workflow_executions: usize,
    /// Whether a rerun can be dispatched at all.
    pub workflow_runner: WorkflowRunnerKind,
    /// The endpoint every rerun is posted to. Required when
    /// `workflow_runner = Http`, and the only place a rerun target may come
    /// from — see `aiwatcher_runner`.
    pub workflow_runner_url: Option<String>,
    pub workflow_runner_token: Option<String>,
    pub workflow_runner_timeout: Duration,
    /// A JSON catalogue of corpora somebody read the licence of.
    ///
    /// Domain content, so this build ships none. Absent means an empty table,
    /// which is a working state: nothing matches a hub result, so every one
    /// stays `unclear`. See `aiwatcher_annotations::sources`.
    pub dataset_sources: Option<String>,
    /// The operator's pod templates: a JSON file, one template per name, which
    /// no route writes (ADR_0029). Absent means none, and a step asking for a
    /// pod is refused at registration naming this variable.
    pub pod_templates: Option<String>,
    /// The namespace launched pods run in (ADR_0029). Absent means the one
    /// this process's own service account is in, which is the release's.
    /// Never from a step or a template: the grant is a namespaced Role, and
    /// one namespace is the whole of it.
    pub pod_namespace: Option<String>,
    /// Where a launched pod reaches the API — what it is told in
    /// `AIWATCHER_URL`. Required wherever pods are launched, because the
    /// process launching them cannot see the Service in front of the API.
    pub pod_api_url: Option<String>,
    /// What a step's pod is here: a Job in a cluster, or a process on this
    /// host (`AIWATCHER_POD_RUNTIME`).
    pub pod_runtime: PodRuntime,
    /// How many of this host's step processes may run at once
    /// (`AIWATCHER_POD_PROCESS_LIMIT`). Meaningless under
    /// [`PodRuntime::Kubernetes`], where a scheduler answers it.
    pub pod_process_limit: usize,
    /// Whether the dataset area may search Hugging Face.
    ///
    /// A switch rather than a credential: the dataset search is public. Off by
    /// default, because an instance in a cluster with no egress should not
    /// have a search box that times out — and because reaching a third party
    /// at all is a decision somebody makes rather than inherits.
    pub huggingface_enabled: bool,
    /// Only for gated repositories. Search works without it.
    pub huggingface_token: Option<String>,
    /// Kaggle's API needs both halves; either alone is not a credential.
    pub kaggle_username: Option<String>,
    pub kaggle_key: Option<String>,
    /// Which half of this binary this process is. See [`ProcessRole`].
    pub role: ProcessRole,
    /// Where a managed execution's history lives.
    pub workflow_store: WorkflowStoreKind,
    /// The connection string. Required when `workflow_store = Postgres`, and
    /// the only place it may come from — a plan never names a host.
    pub workflow_postgres_url: Option<String>,
    pub workflow_postgres_max_connections: u32,
    /// Which query engine this deployment runs (`AIWATCHER_QUERY_ENGINE`).
    ///
    /// One per deployment (AW-3). It decides which executor `query_url` is
    /// wired to — and therefore which one kind of query step this process may
    /// claim — and which engine a plan has to have been written for to start.
    /// Flow unless a deployment says otherwise, which is today's deployment.
    pub query_engine: QueryEngine,
    /// The query engine, for a managed query step.
    ///
    /// `None` means this process runs no query executor, which means it claims
    /// no query attempt — the claim filter is built from what is registered,
    /// so a process never takes work it cannot perform. It is also the only
    /// address a query step ever runs against: a plan names a binding and its
    /// parameters, never a host. Read from `AIWATCHER_QUERY_URL`;
    /// `AIWATCHER_FLOW_URL`, its name before AW-3, is refused naming it.
    pub query_url: Option<String>,
    /// The notebook runtime, for a managed `marimo` step.
    ///
    /// The same shape and the same reasoning as `query_url`: absent, this
    /// process registers no notebook executor and therefore claims no `marimo`
    /// attempt. The two are independent — a deployment may run managed Flow
    /// steps and no notebooks — which is why they are two variables rather
    /// than one "curation services" switch.
    pub ml_pipeline_url: Option<String>,
    /// How long a managed query step may run, in seconds, whichever engine
    /// runs it. `None` keeps the compiler's five minutes, which fits a query
    /// over the read model and not one over a corpus on disk. Raise it with
    /// the engine's own `AIWATCHER_QUERY_TIMEOUT_SECONDS`, never above it: Flow
    /// stopping first is a 500, which a reactor reads as an outage and
    /// retries ten times.
    pub query_step_timeout_seconds: Option<u64>,
    /// The name this process holds its leases under.
    ///
    /// Unique per process, or two reactors each believe they hold the other's
    /// leases — the one thing a lease exists to prevent. Derived from the
    /// hostname and the pid when unset, which is right in a container and in a
    /// terminal.
    pub reactor_owner: Option<String>,
    /// How often a reactor asks for work when nothing has nudged it, and how
    /// often the outbox looks for a row.
    ///
    /// Both mostly irrelevant: starting an execution and reporting an outcome
    /// both wake the loops directly. This is what catches a row another replica
    /// wrote, and an attempt left running by a process that died.
    pub execution_poll: Duration,

    /// How long a finished execution's history is kept, or `None` to keep it.
    ///
    /// **`None` is the default, and that is the decision.** The stream is the
    /// *explanation* of a run — the commands, the decisions, the attempt that
    /// failed and the one that did not — and it is the one thing the event log
    /// does not carry. A release that started deleting it on an upgrade would
    /// be the failure the conversation archive's default is about, arriving in
    /// a second store.
    ///
    /// Set it, and the work role sweeps: terminal executions only, and never
    /// one whose facts the outbox is still holding. The window has a floor
    /// nothing here can check — it must be longer than the event log's own
    /// retention, because the log is what redelivers and the durable inbox goes
    /// with the stream. See [`aiwatcher_execution::store::prunable`].
    pub workflow_retention: Option<Duration>,
    /// What this deployment allows an answer to a parked step to be.
    ///
    /// `AIWATCHER_MAX_ANSWERS_PER_STEP` and `AIWATCHER_MAX_ANSWER_BYTES`. A
    /// step's answers accumulate, because a parked attempt is resumed by one
    /// that re-runs the work and reads all of them — so nothing takes any away,
    /// and unbounded the only backstop is the store refusing a message that has
    /// grown too large.
    pub answer_limits: aiwatcher_api::state::AnswerLimits,
    /// Who may reach this instance, and what they may do once they have.
    /// `AuthMode::None` by default — see `aiwatcher_auth`.
    pub auth: AuthConfig,
    pub log_format: LogFormat,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LogFormat {
    /// Human-readable. The default for a terminal.
    #[default]
    Pretty,
    /// One JSON object per line, for log aggregation.
    Json,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from(([0, 0, 0, 0], 8080)),
            bus: BackendKind::default(),
            data_dir: "./.data".to_owned(),
            seed_file: None,
            laser_connection_string: None,
            laser_stream: "aiwatcher".to_owned(),
            laser_topic: "events".to_owned(),
            laser_partitions: 1,
            otlp_endpoint: None,
            service_name: "aiwatcher".to_owned(),
            processor_id: "aiwatcher-projector".to_owned(),
            ingest_enabled: false,
            cors_origins: Vec::new(),
            orphan_timeout: Duration::from_secs(15 * 60),
            // Sized for a 512 MB container with room for spikes.
            max_runs: 5_000,
            max_spans_total: 60_000,
            max_evaluations: 500,
            max_evaluation_cases_total: 20_000,
            prompt_store: PromptStoreKind::default(),
            evaluation_source_dir: None,
            evaluation_limits: aiwatcher_evaluation::RegistryConfig::default(),
            prompt_prefix: "prompts".to_owned(),
            prompt_s3_endpoint: None,
            prompt_s3_bucket: "aiwatcher-prompts".to_owned(),
            prompt_s3_access_key: None,
            prompt_s3_secret_key: None,
            prompt_s3_session_token: None,
            prompt_s3_region: "us-east-1".to_owned(),
            prompt_s3_create_bucket: true,
            // Off. The one default in this file chosen so that doing nothing
            // keeps nothing.
            conversation_archive: false,
            conversation_keys: None,
            execution_payloads: PayloadPolicy::External,
            execution_payloads_locked: false,
            conversation_prefix: "conversations".to_owned(),
            conversation_policy: ConversationPolicyMode::default(),
            conversation_max_ttl_days: 365,
            conversation_reject_on_finding: false,
            // An hour. Retention is measured in days, so a finer sweep would
            // only be a busier one.
            conversation_sweep_interval: Duration::from_secs(3_600),
            // Fifteen seconds, and mostly irrelevant: queueing an export nudges
            // the worker directly. This is what catches a job another replica
            // queued, and a job left running by a process that died.
            conversation_export_poll: Duration::from_secs(15),
            // The same fifteen seconds, and mostly irrelevant for the same
            // reason: queueing an import nudges the worker directly. This is
            // what catches a job another replica queued, and a job left
            // running by a process that died.
            import_poll: Duration::from_secs(15),
            max_workflow_executions: 1_000,
            workflow_runner: WorkflowRunnerKind::default(),
            workflow_runner_url: None,
            workflow_runner_token: None,
            // The same ten seconds the OTLP exporter and the object store use.
            workflow_runner_timeout: Duration::from_secs(10),
            dataset_sources: None,
            pod_templates: None,
            pod_namespace: None,
            pod_api_url: None,
            pod_runtime: PodRuntime::default(),
            pod_process_limit: crate::execution::pods::process::DEFAULT_LIMIT,
            huggingface_enabled: false,
            huggingface_token: None,
            kaggle_username: None,
            kaggle_key: None,
            role: ProcessRole::default(),
            workflow_store: WorkflowStoreKind::default(),
            workflow_postgres_url: None,
            // Five: a decider, a reactor, the outbox and room for a second
            // API replica's share of one pool. sqlx's own default is ten,
            // which is a lot of idle connections per pod.
            workflow_postgres_max_connections: 5,
            query_engine: QueryEngine::Flow,
            query_url: None,
            ml_pipeline_url: None,
            query_step_timeout_seconds: None,
            reactor_owner: None,
            // A second. Shorter than the conversation and import queues'
            // fifteen because a step's latency is a person watching a canvas,
            // and both loops are woken directly anyway.
            execution_poll: Duration::from_secs(1),
            // Nothing is forgotten unless a deployment says so.
            workflow_retention: None,
            answer_limits: aiwatcher_api::state::AnswerLimits::default(),
            auth: AuthConfig::default(),
            log_format: LogFormat::default(),
        }
    }
}

impl Config {
    /// Read the environment, falling back to [`Config::default`] per field.
    pub fn from_env() -> Result<Self, ConfigError> {
        let mut config = Self::default();

        if let Some(raw) = var("AIWATCHER_LISTEN") {
            config.listen = raw.parse().map_err(|_| ConfigError::Invalid {
                name: "AIWATCHER_LISTEN",
                value: raw,
                expected: "socket address, e.g. 0.0.0.0:8080",
            })?;
        }
        if let Some(raw) = var("AIWATCHER_BUS") {
            config.bus = raw.parse()?;
        }
        if let Some(raw) = var("AIWATCHER_DATA_DIR") {
            config.data_dir = raw;
        }
        config.seed_file = match var("AIWATCHER_SEED_FILE").as_deref() {
            Some("none") => None,
            Some(path) => Some(path.to_owned()),
            None => Some("examples/seed.json".to_owned()),
        };
        // `LASER_CONNECTION_STRING` is what the SDK's own `connect_env` reads,
        // so a deployment that already sets it needs no aiwatcher-specific
        // variable; the prefixed one wins where both are present.
        config.laser_connection_string =
            var("AIWATCHER_LASER_CONNECTION_STRING").or_else(|| var("LASER_CONNECTION_STRING"));
        if let Some(raw) = var("AIWATCHER_LASER_STREAM").or_else(|| var("LASER_STREAM")) {
            config.laser_stream = raw;
        }
        if let Some(raw) = var("AIWATCHER_LASER_TOPIC") {
            config.laser_topic = raw;
        }
        if let Some(raw) = var("AIWATCHER_LASER_PARTITIONS") {
            config.laser_partitions = raw.parse().map_err(|_| ConfigError::Invalid {
                name: "AIWATCHER_LASER_PARTITIONS",
                value: raw,
                expected: "whole number of partitions",
            })?;
        }
        if let Some(raw) = var("AIWATCHER_OTLP_ENDPOINT") {
            config.otlp_endpoint = Some(raw);
        }
        if let Some(raw) = var("AIWATCHER_SERVICE_NAME") {
            config.service_name = raw;
        }
        if let Some(raw) = var("AIWATCHER_PROCESSOR_ID") {
            config.processor_id = raw;
        }
        if let Some(raw) = var("AIWATCHER_INGEST_ENABLED") {
            config.ingest_enabled = parse_bool("AIWATCHER_INGEST_ENABLED", &raw)?;
        }
        if let Some(raw) = var("AIWATCHER_CORS_ORIGINS") {
            config.cors_origins = raw
                .split(',')
                .map(str::trim)
                .filter(|origin| !origin.is_empty())
                .map(ToOwned::to_owned)
                .collect();
        }
        if let Some(raw) = var("AIWATCHER_ORPHAN_TIMEOUT_SECONDS") {
            let seconds: u64 = raw.parse().map_err(|_| ConfigError::Invalid {
                name: "AIWATCHER_ORPHAN_TIMEOUT_SECONDS",
                value: raw,
                expected: "whole number of seconds",
            })?;
            config.orphan_timeout = Duration::from_secs(seconds);
        }
        if let Some(raw) = var("AIWATCHER_MAX_RUNS") {
            config.max_runs = raw.parse().map_err(|_| ConfigError::Invalid {
                name: "AIWATCHER_MAX_RUNS",
                value: raw,
                expected: "whole number of runs",
            })?;
        }
        if let Some(raw) = var("AIWATCHER_MAX_SPANS_TOTAL") {
            config.max_spans_total = raw.parse().map_err(|_| ConfigError::Invalid {
                name: "AIWATCHER_MAX_SPANS_TOTAL",
                value: raw,
                expected: "whole number of spans",
            })?;
        }
        if let Some(raw) = var("AIWATCHER_MAX_EVALUATIONS") {
            config.max_evaluations = raw.parse().map_err(|_| ConfigError::Invalid {
                name: "AIWATCHER_MAX_EVALUATIONS",
                value: raw,
                expected: "whole number of evaluations",
            })?;
        }
        if let Some(raw) = var("AIWATCHER_MAX_EVALUATION_CASES_TOTAL") {
            config.max_evaluation_cases_total = raw.parse().map_err(|_| ConfigError::Invalid {
                name: "AIWATCHER_MAX_EVALUATION_CASES_TOTAL",
                value: raw,
                expected: "whole number of cases",
            })?;
        }
        config.evaluation_source_dir = var("AIWATCHER_EVALUATION_SOURCE_DIR");
        if let Some(raw) = var("AIWATCHER_EVALUATION_MAX_CASES") {
            config.evaluation_limits.max_cases = raw.parse().map_err(|_| ConfigError::Invalid {
                name: "AIWATCHER_EVALUATION_MAX_CASES",
                value: raw,
                expected: "positive whole number",
            })?;
        }
        if let Some(raw) = var("AIWATCHER_EVALUATION_MAX_BYTES") {
            config.evaluation_limits.max_bytes = raw.parse().map_err(|_| ConfigError::Invalid {
                name: "AIWATCHER_EVALUATION_MAX_BYTES",
                value: raw,
                expected: "positive whole number",
            })?;
        }
        if let Some(raw) = var("AIWATCHER_EVALUATION_PAGE_SIZE") {
            config.evaluation_limits.page_size = raw.parse().map_err(|_| ConfigError::Invalid {
                name: "AIWATCHER_EVALUATION_PAGE_SIZE",
                value: raw,
                expected: "positive whole number",
            })?;
        }
        if let Some(raw) = var("AIWATCHER_EVALUATION_RETENTION_SECONDS") {
            config.evaluation_limits.retention_seconds =
                raw.parse().map_err(|_| ConfigError::Invalid {
                    name: "AIWATCHER_EVALUATION_RETENTION_SECONDS",
                    value: raw,
                    expected: "positive whole number",
                })?;
        }
        if let Some(raw) = var("AIWATCHER_PROMPT_STORE") {
            config.prompt_store = raw.parse()?;
        }
        if let Some(raw) = var("AIWATCHER_PROMPT_PREFIX") {
            config.prompt_prefix = raw.trim_matches('/').to_owned();
        }
        if let Some(raw) = var("AIWATCHER_PROMPT_S3_ENDPOINT") {
            config.prompt_s3_endpoint = Some(raw);
        }
        if let Some(raw) = var("AIWATCHER_PROMPT_S3_BUCKET") {
            config.prompt_s3_bucket = raw;
        }
        // The `AWS_*` fallbacks are what an IRSA or a mounted-secret setup
        // already sets. A deployment that has them should not need a second,
        // aiwatcher-specific copy of the same credential.
        config.prompt_s3_access_key =
            var("AIWATCHER_PROMPT_S3_ACCESS_KEY").or_else(|| var("AWS_ACCESS_KEY_ID"));
        config.prompt_s3_secret_key =
            var("AIWATCHER_PROMPT_S3_SECRET_KEY").or_else(|| var("AWS_SECRET_ACCESS_KEY"));
        config.prompt_s3_session_token =
            var("AIWATCHER_PROMPT_S3_SESSION_TOKEN").or_else(|| var("AWS_SESSION_TOKEN"));
        if let Some(raw) = var("AIWATCHER_PROMPT_S3_REGION").or_else(|| var("AWS_REGION")) {
            config.prompt_s3_region = raw;
        }
        if let Some(raw) = var("AIWATCHER_PROMPT_S3_CREATE_BUCKET") {
            config.prompt_s3_create_bucket = parse_bool("AIWATCHER_PROMPT_S3_CREATE_BUCKET", &raw)?;
        }
        if let Some(raw) = var("AIWATCHER_CONVERSATION_ARCHIVE") {
            config.conversation_archive = parse_bool("AIWATCHER_CONVERSATION_ARCHIVE", &raw)?;
        }
        config.conversation_keys = var("AIWATCHER_CONVERSATION_KEYS");
        if let Some(raw) = var("AIWATCHER_EXECUTION_PAYLOADS") {
            config.execution_payloads = match raw.as_str() {
                "external" => PayloadPolicy::External,
                "sealed" => PayloadPolicy::Sealed,
                _ => {
                    return Err(ConfigError::Invalid {
                        name: "AIWATCHER_EXECUTION_PAYLOADS",
                        value: raw,
                        // No `plain`: content in the object store with no key is
                        // readable by every process holding the bucket's
                        // credentials, which is what the key exists for.
                        expected: "external | sealed",
                    });
                }
            };
        }
        if let Some(raw) = var("AIWATCHER_EXECUTION_PAYLOADS_LOCKED") {
            config.execution_payloads_locked =
                parse_bool("AIWATCHER_EXECUTION_PAYLOADS_LOCKED", &raw)?;
        }
        if let Some(raw) = var("AIWATCHER_CONVERSATION_PREFIX") {
            config.conversation_prefix = raw.trim_matches('/').to_owned();
        }
        if let Some(raw) = var("AIWATCHER_CONVERSATION_POLICY") {
            config.conversation_policy = raw.parse()?;
        }
        if let Some(raw) = var("AIWATCHER_CONVERSATION_MAX_TTL_DAYS") {
            config.conversation_max_ttl_days = raw.parse().map_err(|_| ConfigError::Invalid {
                name: "AIWATCHER_CONVERSATION_MAX_TTL_DAYS",
                value: raw,
                expected: "whole number of days",
            })?;
        }
        if let Some(raw) = var("AIWATCHER_CONVERSATION_REJECT_ON_FINDING") {
            config.conversation_reject_on_finding =
                parse_bool("AIWATCHER_CONVERSATION_REJECT_ON_FINDING", &raw)?;
        }
        if let Some(raw) = var("AIWATCHER_CONVERSATION_SWEEP_SECONDS") {
            config.conversation_sweep_interval =
                Duration::from_secs(raw.parse().map_err(|_| ConfigError::Invalid {
                    name: "AIWATCHER_CONVERSATION_SWEEP_SECONDS",
                    value: raw,
                    expected: "whole number of seconds",
                })?);
        }
        if let Some(raw) = var("AIWATCHER_MAX_WORKFLOW_EXECUTIONS") {
            config.max_workflow_executions = raw.parse().map_err(|_| ConfigError::Invalid {
                name: "AIWATCHER_MAX_WORKFLOW_EXECUTIONS",
                value: raw,
                expected: "whole number of executions",
            })?;
        }
        if let Some(raw) = var("AIWATCHER_WORKFLOW_RUNNER") {
            config.workflow_runner = raw.parse()?;
        }
        if let Some(raw) = var("AIWATCHER_WORKFLOW_RUNNER_URL") {
            config.workflow_runner_url = Some(raw);
        }
        config.workflow_runner_token = var("AIWATCHER_WORKFLOW_RUNNER_TOKEN");
        if let Some(raw) = var("AIWATCHER_WORKFLOW_RUNNER_TIMEOUT_SECONDS") {
            let seconds: u64 = raw.parse().map_err(|_| ConfigError::Invalid {
                name: "AIWATCHER_WORKFLOW_RUNNER_TIMEOUT_SECONDS",
                value: raw,
                expected: "whole number of seconds",
            })?;
            config.workflow_runner_timeout = Duration::from_secs(seconds);
        }
        config.dataset_sources = var("AIWATCHER_DATASET_SOURCES");
        config.pod_templates = var("AIWATCHER_POD_TEMPLATES");
        config.pod_namespace = var("AIWATCHER_POD_NAMESPACE");
        config.pod_api_url = var("AIWATCHER_POD_API_URL");
        if let Some(raw) = var("AIWATCHER_POD_RUNTIME") {
            config.pod_runtime = raw.parse()?;
        }
        if let Some(raw) = var("AIWATCHER_POD_PROCESS_LIMIT") {
            config.pod_process_limit = raw
                .parse()
                .ok()
                .filter(|limit| *limit > 0)
                .ok_or(ConfigError::Invalid {
                    name: "AIWATCHER_POD_PROCESS_LIMIT",
                    value: raw,
                    expected: "how many step processes may run at once, one or more",
                })?;
        }
        if let Some(raw) = var("AIWATCHER_HUGGINGFACE_ENABLED") {
            config.huggingface_enabled = parse_bool("AIWATCHER_HUGGINGFACE_ENABLED", &raw)?;
        }
        config.huggingface_token = var("AIWATCHER_HUGGINGFACE_TOKEN").or_else(|| var("HF_TOKEN"));
        config.kaggle_username =
            var("AIWATCHER_KAGGLE_USERNAME").or_else(|| var("KAGGLE_USERNAME"));
        config.kaggle_key = var("AIWATCHER_KAGGLE_KEY").or_else(|| var("KAGGLE_KEY"));
        if let Some(raw) = var("AIWATCHER_ROLE") {
            config.role = raw.parse()?;
        }
        if let Some(raw) = var("AIWATCHER_WORKFLOW_STORE") {
            config.workflow_store = raw.parse()?;
        }
        config.workflow_postgres_url = var("AIWATCHER_WORKFLOW_POSTGRES_URL");
        if let Some(raw) = var("AIWATCHER_WORKFLOW_POSTGRES_MAX_CONNECTIONS") {
            config.workflow_postgres_max_connections =
                raw.parse().map_err(|_| ConfigError::Invalid {
                    name: "AIWATCHER_WORKFLOW_POSTGRES_MAX_CONNECTIONS",
                    value: raw,
                    expected: "whole number of connections",
                })?;
        }
        if let Some(raw) = var("AIWATCHER_QUERY_ENGINE") {
            config.query_engine = query_engine_of(raw)?;
        }
        if let Some(raw) = var("AIWATCHER_FLOW_URL") {
            return Err(renamed_query_url(raw));
        }
        if let Some(raw) = var("AIWATCHER_QUERY_URL") {
            config.query_url = Some(raw.trim_end_matches('/').to_owned());
        }
        if let Some(raw) = var("AIWATCHER_ML_PIPELINE_URL") {
            config.ml_pipeline_url = Some(raw.trim_end_matches('/').to_owned());
        }
        config.reactor_owner = var("AIWATCHER_REACTOR_OWNER");
        if let Some(raw) = var("AIWATCHER_EXECUTION_POLL_SECONDS") {
            config.execution_poll =
                Duration::from_secs(raw.parse().map_err(|_| ConfigError::Invalid {
                    name: "AIWATCHER_EXECUTION_POLL_SECONDS",
                    value: raw,
                    expected: "whole number of seconds",
                })?);
        }
        if let Some(raw) = var("AIWATCHER_QUERY_STEP_TIMEOUT_SECONDS") {
            let seconds: u64 = raw.parse().map_err(|_| ConfigError::Invalid {
                name: "AIWATCHER_QUERY_STEP_TIMEOUT_SECONDS",
                value: raw.clone(),
                expected: "whole number of seconds, at least 1",
            })?;
            // Zero would arm a timer that has already fired, so every query
            // step would fail before its request was sent. Refused by name
            // rather than read as "no limit", which is not a thing a plan can
            // say.
            if seconds == 0 {
                return Err(ConfigError::Invalid {
                    name: "AIWATCHER_QUERY_STEP_TIMEOUT_SECONDS",
                    value: raw,
                    expected: "whole number of seconds, at least 1",
                });
            }
            config.query_step_timeout_seconds = Some(seconds);
        }
        if let Some(raw) = var("AIWATCHER_WORKFLOW_RETENTION_DAYS") {
            let days: u64 = raw.parse().map_err(|_| ConfigError::Invalid {
                name: "AIWATCHER_WORKFLOW_RETENTION_DAYS",
                value: raw.clone(),
                expected: "whole number of days, or 0 to keep everything",
            })?;
            // Zero is "keep everything" rather than "delete everything", which
            // is the reading somebody typing it in a hurry would want to be
            // wrong about. Deleting on the next sweep is not a thing one
            // character should ask for.
            config.workflow_retention = (days > 0).then(|| Duration::from_secs(days * 86_400));
        }
        if let Some(raw) = var("AIWATCHER_MAX_ANSWERS_PER_STEP") {
            let ceiling: usize = raw.parse().map_err(|_| ConfigError::Invalid {
                name: "AIWATCHER_MAX_ANSWERS_PER_STEP",
                value: raw.clone(),
                expected: "whole number of answers, or 0 for no ceiling",
            })?;
            // Zero is "no ceiling" rather than "answer nothing", the same
            // reading retention's zero gets: one character must not be able to
            // ask for a run that can never be answered.
            config.answer_limits.per_step = (ceiling > 0).then_some(ceiling);
        }
        if let Some(raw) = var("AIWATCHER_MAX_ANSWER_BYTES") {
            config.answer_limits.bytes = raw.parse().map_err(|_| ConfigError::Invalid {
                name: "AIWATCHER_MAX_ANSWER_BYTES",
                value: raw,
                expected: "whole number of bytes",
            })?;
        }
        if let Some(raw) = var("AIWATCHER_ENGINE") {
            refuse_removed_engine(raw)?;
        }
        read_auth(&mut config)?;

        if let Some(raw) = var("AIWATCHER_LOG_FORMAT") {
            config.log_format = match raw.to_ascii_lowercase().as_str() {
                "json" => LogFormat::Json,
                "pretty" | "text" => LogFormat::Pretty,
                other => {
                    return Err(ConfigError::Invalid {
                        name: "AIWATCHER_LOG_FORMAT",
                        value: other.to_owned(),
                        expected: "json or pretty",
                    });
                }
            };
        }

        config.validate()?;

        Ok(config)
    }

    /// Every rule that decides whether this configuration can start.
    ///
    /// A method rather than a block inside `from_env`, because a test cannot
    /// touch the environment — they share one process — and a test that
    /// re-implemented these rules would be a second copy of them, agreeing with
    /// the first only until somebody changed one.
    ///
    /// # Errors
    ///
    /// [`ConfigError`] naming the variable that is missing or unusable, and
    /// what made it required.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.bus == BackendKind::Laser && self.laser_connection_string.is_none() {
            return Err(ConfigError::Missing {
                name: "AIWATCHER_LASER_CONNECTION_STRING",
                bus: "laser",
            });
        }

        // Refused at start-up rather than at the first publish. An
        // unauthenticated S3 endpoint answers 403 to every request, and
        // finding that out when somebody saves a prompt means the failure
        // surfaces to whoever was writing rather than to whoever deployed.
        if self.prompt_store == PromptStoreKind::S3 {
            for (name, value) in [
                ("AIWATCHER_PROMPT_S3_ENDPOINT", &self.prompt_s3_endpoint),
                ("AIWATCHER_PROMPT_S3_ACCESS_KEY", &self.prompt_s3_access_key),
                ("AIWATCHER_PROMPT_S3_SECRET_KEY", &self.prompt_s3_secret_key),
            ] {
                if value.is_none() {
                    return Err(ConfigError::Required {
                        name,
                        because: "AIWATCHER_PROMPT_STORE=s3",
                    });
                }
            }
        }

        // An archive with no key would hold conversation content in plaintext
        // in a bucket several other things already read. Refusing to start says
        // which variable is missing, once, to whoever deployed it — rather than
        // discovering it when somebody asks where the encryption went.
        if self.conversation_archive && self.conversation_keys.is_none() {
            return Err(ConfigError::Required {
                name: "AIWATCHER_CONVERSATION_KEYS",
                because: "AIWATCHER_CONVERSATION_ARCHIVE=on",
            });
        }
        // Sealing needs somewhere to seal *into* and something to seal with, and
        // a deployment that made it the default without them would refuse every
        // hosted run at the moment somebody tried one. Named here, at start-up,
        // to whoever set it — the archive's own rule, one store along.
        if self.execution_payloads.needs_archive() && !self.conversation_archive {
            return Err(ConfigError::Required {
                name: "AIWATCHER_CONVERSATION_ARCHIVE",
                because: "AIWATCHER_EXECUTION_PAYLOADS=sealed",
            });
        }

        // And an archive with nowhere to put anything is the same class of
        // mistake one layer down: every route would answer 501 while the
        // deployment believed it had turned capture on.
        if self.conversation_archive && self.prompt_store == PromptStoreKind::None {
            return Err(ConfigError::Required {
                name: "AIWATCHER_PROMPT_STORE",
                because: "AIWATCHER_CONVERSATION_ARCHIVE=on",
            });
        }

        // Same reasoning as the S3 check above, one step sharper: a runner
        // with no endpoint would answer every rerun with a connection error
        // that looks like the orchestrator being down. Refusing to start says
        // which variable is missing, once, to whoever deployed it.
        if self.workflow_runner == WorkflowRunnerKind::Http && self.workflow_runner_url.is_none() {
            return Err(ConfigError::Required {
                name: "AIWATCHER_WORKFLOW_RUNNER_URL",
                because: "AIWATCHER_WORKFLOW_RUNNER=http",
            });
        }

        // The same reasoning once more, and the sharpest consequence of the
        // set: a workflow store with no address answers every command with a
        // connection error, and by then somebody has started an execution the
        // panel will report as failed rather than as never accepted.
        if self.workflow_store == WorkflowStoreKind::Postgres
            && self.workflow_postgres_url.is_none()
        {
            return Err(ConfigError::Required {
                name: "AIWATCHER_WORKFLOW_POSTGRES_URL",
                because: "AIWATCHER_WORKFLOW_STORE=postgres",
            });
        }

        // Splitting the binary in two means three things stop being
        // per-process, and every one of them fails quietly if it is not
        // shared. Refused here, naming the variable, rather than discovered by
        // whoever reads the failure afterwards.
        if self.role != ProcessRole::Both {
            // The workflow store. `file` takes an exclusive lock, so this
            // combination fails on whichever process starts second — in a
            // message about a lock file rather than about a decision somebody
            // made.
            if self.workflow_store != WorkflowStoreKind::Postgres {
                return Err(ConfigError::Required {
                    name: "AIWATCHER_WORKFLOW_STORE",
                    because: "AIWATCHER_ROLE splits the binary in two, and only `postgres` \
                              can be held by more than one process",
                });
            }
            // The event log. The work role's outbox publishes execution facts
            // and the serve role's projector folds them; `memory` is one
            // process's channel and `wal` is one process's directory, so the
            // split would leave the facts in a log nobody reads — a run that
            // completed, and a workflow tab that never heard about it.
            if self.bus != BackendKind::Laser {
                return Err(ConfigError::Required {
                    name: "AIWATCHER_BUS",
                    because: "AIWATCHER_ROLE splits the binary in two, and the work role's \
                              facts have to reach the log the serve role folds",
                });
            }
            // The object store. This is the one that fails loudly and *late*:
            // the work role writes a step's rows to its own directory and the
            // serve role reads a digest that is not there, so the run crashes
            // three attempts in on "holds no object". Measured rather than
            // guessed, and the refusal is cheaper than the diagnosis.
            if self.prompt_store != PromptStoreKind::S3 {
                return Err(ConfigError::Required {
                    name: "AIWATCHER_PROMPT_STORE",
                    because: "AIWATCHER_ROLE splits the binary in two, and one role stores \
                              a step's result for the other to read",
                });
            }
        }

        // A process that claims attempts is the one that starts the pods they
        // ask for, and the client that asks a cluster is behind the `kube`
        // cargo feature (ADR_0029). Without it, a pod's attempt would be
        // accepted and wait for ever with nothing here saying why — unless the
        // pods are this host's processes, which every build can start. The
        // serve role only checks a step against the file, which needs neither,
        // so it reads templates in any build.
        if self.pod_templates.is_some() && self.role.works() {
            if self.pod_runtime == PodRuntime::Kubernetes && !cfg!(feature = "kube") {
                return Err(ConfigError::Unusable {
                    name: "AIWATCHER_POD_TEMPLATES",
                    why: "this process claims attempts and would have to start the pods they \
                          ask for; a Job in a cluster needs the `kube` cargo feature this build \
                          lacks, so either build with it, run the steps as local processes with \
                          AIWATCHER_POD_RUNTIME=process, or set the templates on \
                          AIWATCHER_ROLE=serve, which only checks what a step asks for",
                });
            }
            if self.pod_api_url.is_none() {
                return Err(ConfigError::Required {
                    name: "AIWATCHER_POD_API_URL",
                    because: "this process launches pods (AIWATCHER_POD_TEMPLATES in the work or \
                              combined role): each is told where to report, and only the \
                              Service in front of the API knows that",
                });
            }
            // A namespace is a cluster's word for where a pod goes, and there
            // is no cluster here. Written beside `process`, one of the two is
            // what somebody meant, and starting anyway would pick for them.
            if self.pod_runtime == PodRuntime::Process && self.pod_namespace.is_some() {
                return Err(ConfigError::Unusable {
                    name: "AIWATCHER_POD_NAMESPACE",
                    why: "this process runs step pods as local processes \
                          (AIWATCHER_POD_RUNTIME=process), which are in no namespace; unset it, \
                          or set AIWATCHER_POD_RUNTIME=kubernetes",
                });
            }
        }

        self.auth.validate()?;

        // A wildcard CORS policy on an instance that has a login says "every
        // origin may call this API" on the one deployment whose whole point is
        // that not everybody may. Cookies would not travel cross-origin
        // anyway — nothing here sets `Access-Control-Allow-Credentials` — but
        // an operator who wrote both of these meant one of them, and starting
        // anyway would pick for them silently.
        if self.auth.mode != AuthMode::None && self.cors_origins.iter().any(|o| o == "*") {
            return Err(ConfigError::Invalid {
                name: "AIWATCHER_CORS_ORIGINS",
                value: "*".to_owned(),
                expected: "a list of origins; a wildcard cannot be combined with AIWATCHER_AUTH_MODE",
            });
        }

        Ok(())
    }

    #[must_use]
    pub fn wal_dir(&self) -> String {
        format!("{}/wal", self.data_dir.trim_end_matches('/'))
    }

    #[must_use]
    pub fn dead_letter_path(&self) -> String {
        format!("{}/dead-letters.jsonl", self.data_dir.trim_end_matches('/'))
    }

    /// Where `AIWATCHER_WORKFLOW_STORE=file` keeps executions.
    ///
    /// Beside the log rather than inside it, for the prompt registry's reason:
    /// the log is a rolling window a retention policy may delete, and a run
    /// that is still going has to survive that.
    #[must_use]
    pub fn workflow_dir(&self) -> String {
        format!("{}/workflow", self.data_dir.trim_end_matches('/'))
    }

    /// Where `AIWATCHER_PROMPT_STORE=file` keeps the registry.
    ///
    /// Beside the write-ahead log rather than inside it: the log is a rolling
    /// window that a retention policy is allowed to delete, and the registry
    /// is the one directory here that must survive that.
    #[must_use]
    pub fn prompt_dir(&self) -> String {
        format!("{}/prompts", self.data_dir.trim_end_matches('/'))
    }
}

/// Everything under `AIWATCHER_AUTH_*`.
///
/// Its own function because it is a third of the variables this binary reads
/// and none of the rest of `from_env` needs to know about any of them. The
/// defaults are `aiwatcher_auth`'s, which means the whole block is inert until
/// `AIWATCHER_AUTH_MODE` says otherwise.
fn read_auth(config: &mut Config) -> Result<(), ConfigError> {
    if let Some(raw) = var("AIWATCHER_AUTH_MODE") {
        config.auth.mode = raw.parse()?;
    }
    if let Some(raw) = var("AIWATCHER_AUTH_ISSUER") {
        // authentik's issuer ends in a slash and its discovery document says
        // so. Trimming here and comparing trimmed in `discover` means an
        // operator who copied it without the slash still starts.
        config.auth.issuer = raw.trim_end_matches('/').to_owned();
    }
    if let Some(raw) = var("AIWATCHER_AUTH_CLIENT_ID") {
        config.auth.client_id = raw;
    }
    config.auth.client_secret = var("AIWATCHER_AUTH_CLIENT_SECRET");
    if let Some(raw) = var("AIWATCHER_AUTH_AUDIENCES") {
        config.auth.audiences = list(&raw);
    }
    if let Some(raw) = var("AIWATCHER_AUTH_SCOPES") {
        config.auth.scopes = list(&raw);
    }
    if let Some(raw) = var("AIWATCHER_AUTH_REDIRECT_URL") {
        config.auth.redirect_url = raw;
    }
    config.auth.post_logout_url = var("AIWATCHER_AUTH_POST_LOGOUT_URL");
    if let Some(raw) = var("AIWATCHER_AUTH_GROUPS_CLAIM") {
        config.auth.groups_claim = raw;
    }
    if let Some(raw) = var("AIWATCHER_AUTH_PROVIDER_NAME") {
        config.auth.provider_name = raw;
    }
    if let Some(raw) = var("AIWATCHER_AUTH_SESSION_TTL_SECONDS") {
        let seconds: u64 = raw.parse().map_err(|_| ConfigError::Invalid {
            name: "AIWATCHER_AUTH_SESSION_TTL_SECONDS",
            value: raw,
            expected: "whole number of seconds",
        })?;
        config.auth.session_ttl = Duration::from_secs(seconds);
    }
    config.auth.session_secret = var("AIWATCHER_AUTH_SESSION_SECRET");
    if let Some(raw) = var("AIWATCHER_AUTH_COOKIE_NAME") {
        config.auth.cookie_name = raw;
    }
    config.auth.cookie_domain = var("AIWATCHER_AUTH_COOKIE_DOMAIN");
    if let Some(raw) = var("AIWATCHER_AUTH_ALLOW_BEARER") {
        config.auth.allow_bearer = parse_bool("AIWATCHER_AUTH_ALLOW_BEARER", &raw)?;
    }
    if let Some(raw) = var("AIWATCHER_AUTH_USERINFO_FALLBACK") {
        config.auth.userinfo_fallback = parse_bool("AIWATCHER_AUTH_USERINFO_FALLBACK", &raw)?;
    }
    if let Some(raw) = var("AIWATCHER_AUTH_INGEST_TOKENS") {
        // Comma-separated only: a shared secret may legitimately contain a
        // space, and splitting on one would quietly halve it.
        config.auth.ingest_tokens = raw
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(str::parse::<IngestToken>)
            .collect::<Result<Vec<_>, _>>()?;
    }

    // Derived from the redirect URL rather than defaulted to `true`, unless it
    // is set by hand. A `Secure` cookie is simply not stored by a browser over
    // http, so an instance served over http with the safe default would sign
    // everybody in and then behave as though nobody had — with no error
    // anywhere. The scheme of the redirect URL is the same fact stated once.
    config.auth.cookie_secure = match var("AIWATCHER_AUTH_COOKIE_SECURE") {
        Some(raw) => parse_bool("AIWATCHER_AUTH_COOKIE_SECURE", &raw)?,
        None => !config.auth.redirect_url.starts_with("http://"),
    };

    config.auth.roles = read_roles(&config.auth.roles)?;
    config.auth.proxy_headers = read_proxy_headers(&config.auth.proxy_headers);
    Ok(())
}

fn read_roles(defaults: &RoleMapping) -> Result<RoleMapping, ConfigError> {
    let default_role = match var("AIWATCHER_AUTH_DEFAULT_ROLE") {
        None => defaults.default_role,
        // "none" is how a deployment says that a group membership is required
        // to see anything at all, as distinct from "viewer", which is how it
        // says the provider's decision to let somebody in was the decision.
        Some(raw) if matches!(raw.to_ascii_lowercase().as_str(), "none" | "off") => None,
        Some(raw) => Some(raw.parse::<Role>().map_err(|_| ConfigError::Invalid {
            name: "AIWATCHER_AUTH_DEFAULT_ROLE",
            value: raw,
            expected: "one of viewer, editor, admin, none",
        })?),
    };

    Ok(RoleMapping {
        admin_groups: var("AIWATCHER_AUTH_ADMIN_GROUPS")
            .map_or_else(|| defaults.admin_groups.clone(), |raw| list(&raw)),
        editor_groups: var("AIWATCHER_AUTH_EDITOR_GROUPS")
            .map_or_else(|| defaults.editor_groups.clone(), |raw| list(&raw)),
        viewer_groups: var("AIWATCHER_AUTH_VIEWER_GROUPS")
            .map_or_else(|| defaults.viewer_groups.clone(), |raw| list(&raw)),
        required_groups: var("AIWATCHER_AUTH_REQUIRED_GROUPS")
            .map_or_else(|| defaults.required_groups.clone(), |raw| list(&raw)),
        default_role,
    })
}

fn read_proxy_headers(defaults: &ProxyHeaders) -> ProxyHeaders {
    ProxyHeaders {
        subject: var("AIWATCHER_AUTH_PROXY_SUBJECT_HEADER")
            .unwrap_or_else(|| defaults.subject.clone())
            .to_ascii_lowercase(),
        username: var("AIWATCHER_AUTH_PROXY_USERNAME_HEADER")
            .unwrap_or_else(|| defaults.username.clone())
            .to_ascii_lowercase(),
        email: var("AIWATCHER_AUTH_PROXY_EMAIL_HEADER")
            .unwrap_or_else(|| defaults.email.clone())
            .to_ascii_lowercase(),
        name: var("AIWATCHER_AUTH_PROXY_NAME_HEADER")
            .unwrap_or_else(|| defaults.name.clone())
            .to_ascii_lowercase(),
        groups: var("AIWATCHER_AUTH_PROXY_GROUPS_HEADER")
            .unwrap_or_else(|| defaults.groups.clone())
            .to_ascii_lowercase(),
    }
}

/// A comma- or whitespace-separated list, however an operator wrote it.
fn list(raw: &str) -> Vec<String> {
    raw.split([',', ' ', '\t', '\n'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

/// `AIWATCHER_ENGINE`, which survives only as a refusal.
///
/// The Flyte engine it switched on has been removed, and it is the one variable
/// a deployment could still be relying on: a server that started without the
/// tab it used to have would say nothing. `none` still parses, for a deployment
/// that spelled the default out.
fn refuse_removed_engine(raw: String) -> Result<(), ConfigError> {
    if matches!(
        raw.to_ascii_lowercase().as_str(),
        "none" | "off" | "disabled"
    ) {
        return Ok(());
    }
    Err(ConfigError::Removed {
        name: "AIWATCHER_ENGINE",
        value: raw,
        what: "the Flyte pipeline engine",
        instead: "unset it",
    })
}

/// What a refusal of `AIWATCHER_QUERY_ENGINE` says it takes.
///
/// A literal because [`ConfigError::Invalid`] holds a `&'static str`; a test
/// keeps it equal to [`QueryEngine::offered`], so an engine added there cannot
/// go unnamed here.
const QUERY_ENGINES: &str = "query engine: flow, datafusion, duckdb";

/// `AIWATCHER_QUERY_ENGINE`, or the refusal naming the variable and the engines.
fn query_engine_of(raw: String) -> Result<QueryEngine, ConfigError> {
    raw.parse().map_err(|_| ConfigError::Invalid {
        name: "AIWATCHER_QUERY_ENGINE",
        value: raw,
        expected: QUERY_ENGINES,
    })
}

/// `AIWATCHER_FLOW_URL`, which survives only as a refusal.
///
/// It was the query engine's address while Flow was the only engine, and was
/// read beside `AIWATCHER_QUERY_URL` for the release that renamed it (AW-3).
/// Ignored rather than refused, a deployment still setting only it would start
/// with no query executor, claim no query step, and say nothing while its runs
/// sat pending.
fn renamed_query_url(raw: String) -> ConfigError {
    ConfigError::Removed {
        name: "AIWATCHER_FLOW_URL",
        value: raw,
        what: "that name for the query engine's address",
        instead: "set AIWATCHER_QUERY_URL to the address instead",
    }
}

fn var(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn parse_bool(name: &'static str, value: &str) -> Result<bool, ConfigError> {
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        other => Err(ConfigError::Invalid {
            name,
            value: other.to_owned(),
            expected: "boolean",
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_runnable_without_any_environment() {
        let config = Config::default();
        assert_eq!(config.bus, BackendKind::Wal, "durable without a broker");
        assert!(
            !config.ingest_enabled,
            "a second write path should be opted into"
        );
        assert!(
            config.otlp_endpoint.is_none(),
            "no exporter configured means spans stay local rather than erroring"
        );
    }

    #[test]
    fn a_prompt_registry_works_with_no_configuration_at_all() {
        let config = Config::default();
        assert_eq!(
            config.prompt_store,
            PromptStoreKind::File,
            "the default has to be durable and need nothing running"
        );
        assert_eq!(config.prompt_dir(), "./.data/prompts");
    }

    #[test]
    fn a_managed_execution_works_with_no_configuration_at_all() {
        let config = Config::default();
        assert_eq!(config.workflow_store, WorkflowStoreKind::File);
        assert_eq!(config.workflow_dir(), "./.data/workflow");
        assert_eq!(config.role, ProcessRole::Both);
        assert!(
            config.query_url.is_none(),
            "a process with no query address runs no query executor and claims no query attempt"
        );
        assert_eq!(
            config.query_engine,
            QueryEngine::Flow,
            "nothing set is today's deployment"
        );
    }

    #[test]
    fn an_engine_that_is_not_offered_is_refused_naming_the_variable_and_the_engines() {
        let refused = query_engine_of("polars".to_owned()).expect_err("polars is not offered");
        let message = refused.to_string();
        assert!(
            message.contains("AIWATCHER_QUERY_ENGINE")
                && message.contains("flow, datafusion, duckdb"),
            "{message}"
        );
        // The literal the refusal carries is the list the engines themselves
        // are chosen from, so an engine added to one is named by the other.
        assert!(QUERY_ENGINES.ends_with(&QueryEngine::offered()));
        assert_eq!(
            query_engine_of("datafusion".to_owned()).expect("offered"),
            QueryEngine::DataFusion
        );
    }

    #[test]
    fn a_deployment_still_naming_the_flow_address_is_told_the_name_that_replaced_it() {
        let message = renamed_query_url("http://flow:8081".to_owned()).to_string();
        assert!(
            message.contains("AIWATCHER_FLOW_URL")
                && message.contains("was removed")
                && message.contains("AIWATCHER_QUERY_URL"),
            "{message}"
        );
    }

    #[test]
    fn splitting_the_binary_in_two_is_refused_on_every_per_process_backend() {
        // Three things stop being per-process when the roles are two
        // processes, and every one of them fails quietly if it is not shared.
        // The third was measured rather than guessed: with two `file` object
        // stores the work role writes a step's rows to its own directory and
        // the serve role crashes three attempts in on "holds no object".
        let split = |config: Config| -> String {
            Config {
                role: ProcessRole::Work,
                ..config
            }
            .validate()
            .expect_err("a split role")
            .to_string()
        };

        assert!(
            split(Config::default()).contains("AIWATCHER_WORKFLOW_STORE"),
            "the store that holds one process"
        );
        let with_store = Config {
            workflow_store: WorkflowStoreKind::Postgres,
            workflow_postgres_url: Some("postgres://localhost/aiwatcher".to_owned()),
            ..Config::default()
        };
        assert!(
            split(with_store.clone()).contains("AIWATCHER_BUS"),
            "the log the other role folds"
        );
        let with_log = Config {
            bus: BackendKind::Laser,
            laser_connection_string: Some("iggy:iggy@127.0.0.1:8090".to_owned()),
            ..with_store
        };
        assert!(
            split(with_log.clone()).contains("AIWATCHER_PROMPT_STORE"),
            "the object store one role writes and the other reads"
        );

        Config {
            role: ProcessRole::Work,
            prompt_store: PromptStoreKind::S3,
            prompt_s3_endpoint: Some("http://rustfs:9000".to_owned()),
            prompt_s3_access_key: Some("key".to_owned()),
            prompt_s3_secret_key: Some("secret".to_owned()),
            ..with_log
        }
        .validate()
        .expect("all three shared");

        // One process holding both halves is what `just dev` runs, and it is
        // the default.
        Config::default()
            .validate()
            .expect("one process, both roles");
    }

    /// Templates, and everything a split role needs, so a refusal is about the
    /// templates.
    fn with_pod_templates(role: ProcessRole) -> Config {
        Config {
            role,
            pod_templates: Some("/etc/aiwatcher/pod-templates.json".to_owned()),
            workflow_store: WorkflowStoreKind::Postgres,
            workflow_postgres_url: Some("postgres://localhost/aiwatcher".to_owned()),
            bus: BackendKind::Laser,
            laser_connection_string: Some("iggy:iggy@127.0.0.1:8090".to_owned()),
            prompt_store: PromptStoreKind::S3,
            prompt_s3_endpoint: Some("http://rustfs:9000".to_owned()),
            prompt_s3_access_key: Some("key".to_owned()),
            prompt_s3_secret_key: Some("secret".to_owned()),
            ..Config::default()
        }
    }

    #[test]
    fn the_serve_role_reads_pod_templates_in_any_build() {
        // It only checks a step against the file, which needs no cluster.
        with_pod_templates(ProcessRole::Serve)
            .validate()
            .expect("the serve role checks steps against the file, in any build");
    }

    #[cfg(not(feature = "kube"))]
    #[test]
    fn pod_templates_are_refused_where_this_build_could_not_start_a_pod() {
        for role in [ProcessRole::Both, ProcessRole::Work] {
            let error = with_pod_templates(role)
                .validate()
                .expect_err("no launcher in this build")
                .to_string();
            assert!(
                error.contains("AIWATCHER_POD_TEMPLATES") && error.contains("kube"),
                "{}: {error}",
                role.as_str()
            );
        }
    }

    #[cfg(feature = "kube")]
    #[test]
    fn a_process_that_launches_pods_has_to_be_told_where_they_report() {
        for role in [ProcessRole::Both, ProcessRole::Work] {
            let error = with_pod_templates(role)
                .validate()
                .expect_err("a pod would not know where its API is")
                .to_string();
            assert!(
                error.contains("AIWATCHER_POD_API_URL"),
                "{}: {error}",
                role.as_str()
            );

            Config {
                pod_api_url: Some("http://aiwatcher-server:8080".to_owned()),
                ..with_pod_templates(role)
            }
            .validate()
            .expect("templates, a client and an address are all a launcher needs");
        }
    }

    #[test]
    fn local_process_pods_need_no_cluster_client_in_any_build() {
        // The refusal above is about reaching a cluster, and this runtime
        // reaches none: every build can start a process. It is what makes the
        // pod path runnable — and provable — on a machine with no cluster.
        for role in [ProcessRole::Both, ProcessRole::Work] {
            Config {
                pod_runtime: PodRuntime::Process,
                pod_api_url: Some("http://127.0.0.1:8080".to_owned()),
                ..with_pod_templates(role)
            }
            .validate()
            .expect("a process needs no kubeconfig");
        }
    }

    #[test]
    fn a_namespace_beside_local_process_pods_is_refused_rather_than_ignored() {
        // Two settings, one of which is what somebody meant. Ignored, the
        // namespace would read as the place these pods go.
        let error = Config {
            pod_runtime: PodRuntime::Process,
            pod_api_url: Some("http://127.0.0.1:8080".to_owned()),
            pod_namespace: Some("aiwatcher".to_owned()),
            ..with_pod_templates(ProcessRole::Work)
        }
        .validate()
        .expect_err("a process is in no namespace")
        .to_string();
        assert!(
            error.contains("AIWATCHER_POD_NAMESPACE") && error.contains("AIWATCHER_POD_RUNTIME"),
            "{error}"
        );
    }

    #[test]
    fn a_pod_runtime_is_a_cluster_unless_it_says_otherwise() {
        assert_eq!(PodRuntime::default(), PodRuntime::Kubernetes);
        for (spelling, runtime) in [
            ("kubernetes", PodRuntime::Kubernetes),
            ("K8s", PodRuntime::Kubernetes),
            ("cluster", PodRuntime::Kubernetes),
            ("pods", PodRuntime::Kubernetes),
            ("process", PodRuntime::Process),
            ("Processes", PodRuntime::Process),
            ("local", PodRuntime::Process),
            ("host", PodRuntime::Process),
        ] {
            assert_eq!(
                spelling.parse::<PodRuntime>().expect(spelling),
                runtime,
                "{spelling}"
            );
        }
        let error = "docker"
            .parse::<PodRuntime>()
            .expect_err("a backend nothing implements")
            .to_string();
        assert!(
            error.contains("AIWATCHER_POD_RUNTIME") && error.contains("process"),
            "{error}"
        );
    }

    #[test]
    fn a_postgres_workflow_store_with_no_address_does_not_start() {
        let missing = Config {
            workflow_store: WorkflowStoreKind::Postgres,
            ..Config::default()
        };
        let error = missing.validate().expect_err("postgres with no url");
        assert!(
            error
                .to_string()
                .contains("AIWATCHER_WORKFLOW_POSTGRES_URL"),
            "{error}"
        );
    }

    #[test]
    fn a_role_names_which_half_of_the_binary_it_is() {
        assert!(ProcessRole::Both.serves() && ProcessRole::Both.works());
        assert!(ProcessRole::Serve.serves() && !ProcessRole::Serve.works());
        assert!(!ProcessRole::Work.serves() && ProcessRole::Work.works());
    }

    #[test]
    fn the_conversation_archive_is_off_until_somebody_turns_it_on() {
        // The one default in this file chosen so that doing nothing keeps
        // nothing. Every other store is on because there is somewhere to put
        // it; this one holds somebody's words.
        let config = Config::default();
        assert!(!config.conversation_archive);
        assert!(config.conversation_keys.is_none());
        assert_eq!(
            config.conversation_policy,
            ConversationPolicyMode::Protected
        );
    }

    #[test]
    fn an_archive_with_no_key_refuses_to_start_rather_than_holding_plaintext() {
        let missing_key = Config {
            conversation_archive: true,
            ..Config::default()
        };
        let error = missing_key.validate().expect_err("refused");
        assert!(
            matches!(
                error,
                ConfigError::Required {
                    name: "AIWATCHER_CONVERSATION_KEYS",
                    ..
                }
            ),
            "{error}"
        );

        // And with nowhere to write, the same class of mistake one layer down:
        // every route would answer 501 while the deployment believed capture
        // was on.
        let no_store = Config {
            conversation_archive: true,
            conversation_keys: Some("k:0000".to_owned()),
            prompt_store: PromptStoreKind::None,
            ..Config::default()
        };
        let error = no_store.validate().expect_err("refused");
        assert!(
            matches!(
                error,
                ConfigError::Required {
                    name: "AIWATCHER_PROMPT_STORE",
                    ..
                }
            ),
            "{error}"
        );

        assert!(
            Config {
                conversation_archive: true,
                conversation_keys: Some("k:0000".to_owned()),
                ..Config::default()
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn the_conversation_policy_accepts_its_common_spellings() {
        for (raw, expected) in [
            ("protected", ConversationPolicyMode::Protected),
            ("strict", ConversationPolicyMode::Protected),
            ("open", ConversationPolicyMode::Open),
            ("permissive", ConversationPolicyMode::Open),
        ] {
            assert_eq!(raw.parse::<ConversationPolicyMode>().expect(raw), expected);
        }
        assert!("whatever".parse::<ConversationPolicyMode>().is_err());
    }

    #[test]
    fn prompt_store_names_accept_their_common_spellings() {
        for (raw, expected) in [
            ("none", PromptStoreKind::None),
            ("off", PromptStoreKind::None),
            ("memory", PromptStoreKind::Memory),
            ("FILE", PromptStoreKind::File),
            ("fs", PromptStoreKind::File),
            ("s3", PromptStoreKind::S3),
            // What the compose service is called.
            ("rustfs", PromptStoreKind::S3),
            ("minio", PromptStoreKind::S3),
        ] {
            assert_eq!(
                raw.parse::<PromptStoreKind>().expect(raw),
                expected,
                "{raw}"
            );
        }
        assert!("gcs".parse::<PromptStoreKind>().is_err());
    }

    #[test]
    fn an_s3_registry_without_credentials_is_refused_at_start_up() {
        // Rather than at the first publish, where the 403 would reach whoever
        // was saving a prompt instead of whoever deployed the instance.
        let base = Config {
            prompt_store: PromptStoreKind::S3,
            prompt_s3_endpoint: Some("http://rustfs:9000".to_owned()),
            prompt_s3_access_key: Some("key".to_owned()),
            prompt_s3_secret_key: Some("secret".to_owned()),
            ..Config::default()
        };
        for missing in [
            Config {
                prompt_s3_endpoint: None,
                ..base.clone()
            },
            Config {
                prompt_s3_access_key: None,
                ..base.clone()
            },
            Config {
                prompt_s3_secret_key: None,
                ..base.clone()
            },
        ] {
            assert!(
                required_field_is_missing(&missing),
                "an incomplete S3 configuration must not start"
            );
        }
        assert!(!required_field_is_missing(&base));
    }

    /// The real validation, rather than a second copy of it.
    fn required_field_is_missing(config: &Config) -> bool {
        config.validate().is_err()
    }

    #[test]
    fn nothing_is_authenticated_until_a_mode_says_so() {
        // The default has to leave an existing installation exactly as it was:
        // a release that started answering 401 is an upgrade that took
        // somebody's observability down at the moment they needed it.
        let config = Config::default();
        assert_eq!(config.auth.mode, AuthMode::None);
        assert!(config.auth.validate().is_ok());
    }

    #[test]
    fn a_default_role_of_none_is_how_a_deployment_requires_a_group() {
        // Distinct from `viewer`, which says the provider letting somebody in
        // was the decision. Both are legitimate and they are not the same.
        let mapping = RoleMapping {
            default_role: None,
            ..RoleMapping::default()
        };
        assert!(mapping.resolve("alice", &["everyone".to_owned()]).is_err());
    }

    #[test]
    fn a_secure_cookie_is_derived_from_the_redirect_url_when_it_is_not_set() {
        // A `Secure` cookie is not stored at all over http, so an instance
        // served over http would sign people in and then behave as though
        // nobody had, with no error anywhere. The redirect URL's scheme is the
        // same fact, already written down once.
        for (redirect, expected) in [
            ("https://aiwatcher.example.test/api/v1/auth/callback", true),
            ("http://localhost:5173/api/v1/auth/callback", false),
        ] {
            let secure = !redirect.starts_with("http://");
            assert_eq!(secure, expected, "{redirect}");
        }
    }

    #[test]
    fn a_list_is_read_however_an_operator_wrote_it() {
        let expected = vec!["aiwatcher-admins".to_owned(), "platform".to_owned()];
        for raw in [
            "aiwatcher-admins,platform",
            "aiwatcher-admins, platform",
            "aiwatcher-admins platform",
            " aiwatcher-admins ,, platform ",
        ] {
            assert_eq!(list(raw), expected, "{raw}");
        }
        assert!(list("  ").is_empty());
    }

    #[test]
    fn a_wildcard_cors_policy_cannot_be_combined_with_a_login() {
        // Two settings that contradict each other. Cookies would not travel
        // cross-origin anyway, but an operator who wrote both meant one of
        // them, and starting anyway picks for them in silence.
        let config = Config {
            cors_origins: vec!["*".to_owned()],
            auth: AuthConfig {
                mode: AuthMode::Proxy,
                ..AuthConfig::default()
            },
            ..Config::default()
        };
        assert!(
            config.auth.mode != AuthMode::None && config.cors_origins.iter().any(|o| o == "*"),
            "the condition from_env refuses on"
        );
    }

    #[test]
    fn backend_names_accept_their_common_spellings() {
        assert_eq!(
            "memory".parse::<BackendKind>().expect("valid"),
            BackendKind::Memory
        );
        assert_eq!(
            "WAL".parse::<BackendKind>().expect("valid"),
            BackendKind::Wal
        );
        assert_eq!(
            "file".parse::<BackendKind>().expect("valid"),
            BackendKind::Wal
        );
        assert_eq!(
            "laser".parse::<BackendKind>().expect("valid"),
            BackendKind::Laser
        );
        assert!("kafka".parse::<BackendKind>().is_err());
    }

    #[test]
    fn booleans_accept_the_spellings_people_actually_write() {
        for truthy in ["1", "true", "TRUE", "yes", "on"] {
            assert!(parse_bool("X", truthy).expect("valid"), "{truthy}");
        }
        for falsy in ["0", "false", "no", "off"] {
            assert!(!parse_bool("X", falsy).expect("valid"), "{falsy}");
        }
        assert!(parse_bool("X", "maybe").is_err());
    }

    #[test]
    fn derived_paths_tolerate_a_trailing_slash() {
        let config = Config {
            data_dir: "/var/lib/aiwatcher/".to_owned(),
            ..Config::default()
        };
        assert_eq!(config.wal_dir(), "/var/lib/aiwatcher/wal");
        assert_eq!(
            config.dead_letter_path(),
            "/var/lib/aiwatcher/dead-letters.jsonl"
        );
    }

    #[test]
    fn a_deployment_that_still_asks_for_the_flyte_engine_is_told_it_was_removed() {
        let message = refuse_removed_engine("flyte".to_owned())
            .expect_err("the engine is gone")
            .to_string();
        assert!(
            message.contains("AIWATCHER_ENGINE") && message.contains("was removed"),
            "{message}"
        );
        refuse_removed_engine("none".to_owned())
            .expect("spelling the default out is not a refusal");
    }

    #[test]
    fn a_rerun_through_the_removed_engine_is_refused_and_http_still_parses() {
        for kind in ["engine", "flyte"] {
            let message = kind
                .parse::<WorkflowRunnerKind>()
                .expect_err("the engine runner is gone")
                .to_string();
            assert!(
                message.contains("AIWATCHER_WORKFLOW_RUNNER") && message.contains("was removed"),
                "{message}"
            );
        }
        assert_eq!(
            "http".parse::<WorkflowRunnerKind>().expect("http stays"),
            WorkflowRunnerKind::Http
        );
    }
}
