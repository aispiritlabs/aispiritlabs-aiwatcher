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
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("{name} is set to {value:?}, which is not a valid {expected}")]
    Invalid {
        name: &'static str,
        value: String,
        expected: &'static str,
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
            other => Err(ConfigError::Invalid {
                name: "AIWATCHER_WORKFLOW_RUNNER",
                value: other.to_owned(),
                expected: "none or http",
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
            prompt_prefix: "prompts".to_owned(),
            prompt_s3_endpoint: None,
            prompt_s3_bucket: "aiwatcher-prompts".to_owned(),
            prompt_s3_access_key: None,
            prompt_s3_secret_key: None,
            prompt_s3_session_token: None,
            prompt_s3_region: "us-east-1".to_owned(),
            prompt_s3_create_bucket: true,
            max_workflow_executions: 1_000,
            workflow_runner: WorkflowRunnerKind::default(),
            workflow_runner_url: None,
            workflow_runner_token: None,
            // The same ten seconds the OTLP exporter and the object store use.
            workflow_runner_timeout: Duration::from_secs(10),
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

        if config.bus == BackendKind::Laser && config.laser_connection_string.is_none() {
            return Err(ConfigError::Missing {
                name: "AIWATCHER_LASER_CONNECTION_STRING",
                bus: "laser",
            });
        }

        // Refused at start-up rather than at the first publish. An
        // unauthenticated S3 endpoint answers 403 to every request, and
        // finding that out when somebody saves a prompt means the failure
        // surfaces to whoever was writing rather than to whoever deployed.
        if config.prompt_store == PromptStoreKind::S3 {
            for (name, value) in [
                ("AIWATCHER_PROMPT_S3_ENDPOINT", &config.prompt_s3_endpoint),
                (
                    "AIWATCHER_PROMPT_S3_ACCESS_KEY",
                    &config.prompt_s3_access_key,
                ),
                (
                    "AIWATCHER_PROMPT_S3_SECRET_KEY",
                    &config.prompt_s3_secret_key,
                ),
            ] {
                if value.is_none() {
                    return Err(ConfigError::Required {
                        name,
                        because: "AIWATCHER_PROMPT_STORE=s3",
                    });
                }
            }
        }

        // Same reasoning as the S3 check above, one step sharper: a runner
        // with no endpoint would answer every rerun with a connection error
        // that looks like the orchestrator being down. Refusing to start says
        // which variable is missing, once, to whoever deployed it.
        if config.workflow_runner == WorkflowRunnerKind::Http
            && config.workflow_runner_url.is_none()
        {
            return Err(ConfigError::Required {
                name: "AIWATCHER_WORKFLOW_RUNNER_URL",
                because: "AIWATCHER_WORKFLOW_RUNNER=http",
            });
        }

        config.auth.validate()?;

        // A wildcard CORS policy on an instance that has a login says "every
        // origin may call this API" on the one deployment whose whole point is
        // that not everybody may. Cookies would not travel cross-origin
        // anyway — nothing here sets `Access-Control-Allow-Credentials` — but
        // an operator who wrote both of these meant one of them, and starting
        // anyway would pick for them silently.
        if config.auth.mode != AuthMode::None && config.cors_origins.iter().any(|o| o == "*") {
            return Err(ConfigError::Invalid {
                name: "AIWATCHER_CORS_ORIGINS",
                value: "*".to_owned(),
                expected: "a list of origins; a wildcard cannot be combined with AIWATCHER_AUTH_MODE",
            });
        }

        Ok(config)
    }

    #[must_use]
    pub fn wal_dir(&self) -> String {
        format!("{}/wal", self.data_dir.trim_end_matches('/'))
    }

    #[must_use]
    pub fn dead_letter_path(&self) -> String {
        format!("{}/dead-letters.jsonl", self.data_dir.trim_end_matches('/'))
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

    /// The validation `from_env` applies, without touching the environment —
    /// which no test may do, because they share one process.
    fn required_field_is_missing(config: &Config) -> bool {
        config.prompt_store == PromptStoreKind::S3
            && (config.prompt_s3_endpoint.is_none()
                || config.prompt_s3_access_key.is_none()
                || config.prompt_s3_secret_key.is_none())
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
}
