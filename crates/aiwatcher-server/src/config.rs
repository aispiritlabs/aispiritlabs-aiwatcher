//! Configuration, from the environment.
//!
//! Environment variables rather than a config file: this runs in containers,
//! and a variable is the one thing every orchestrator can set. Every setting
//! has a default that works, so `cargo run --bin aiwatcher` starts a usable
//! instance with no setup at all.

use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;

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
