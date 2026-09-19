//! Telling somebody a run died or a measurement got worse, once.
//!
//! Everything else here waits to be read. A run that failed at four in the
//! morning is a row in a list somebody opens at nine, and a candidate that
//! lost two points against its baseline is a gate a pipeline asked about and
//! nobody else did. This is the one part that speaks first.
//!
//! ```text
//!   a watcher sees          a rule matches        a delivery
//!   something happen  ────► its version    ────►  is created once  ────► one
//!   (AlertSignal)           (AlertTrigger)        (the dedup key)        POST
//! ```
//!
//! Four rules carry it, and each is a duplicate notification or a silent one
//! when a copy gets it wrong:
//!
//! * **the destination is configuration, never authored.** A rule says what is
//!   worth telling somebody about; where it goes is one deployment-wide
//!   channel, for the reason `aiwatcher-runner` gives about a rerun's endpoint
//!   — a URL that arrives in a request body is a request-forgery primitive
//!   posted by anything that can reach this API from inside the cluster;
//! * **a delivery is named by its dedup key**, `sha256(rule version ‖ what
//!   happened)`, and creating it is an atomic create-if-absent. The same
//!   occurrence reaching a second watcher lands on the object the first one
//!   wrote, so a repeat is not a second logical alert;
//! * **nothing here promises exactly-once.** A receiver that acknowledged into
//!   a broken socket is indistinguishable from one that never heard, and this
//!   retries — so the key travels *with* the payload and the receiver decides;
//! * **a failed attempt is [`aiwatcher_jobs::after_failure`]'s decision**, not
//!   a second copy of it. What belongs here is only how long a retry waits.

mod channel;
mod delivery;
mod registry;
mod rule;
mod signal;

pub use channel::{AlertChannel, ChannelDescription};
pub use delivery::{AlertPayload, Delivery, RETRY_BACKOFF_SECONDS, backoff_seconds};
pub use registry::{
    ActiveRule, DeliveryFilter, DeliveryPage, PublishRule, Raised, Registry, RegistryConfig,
    RuleFilter, RulePage, RulePublished, RuleSummary, now,
};
pub use rule::{AlertRule, AlertTrigger, RuleHead, RuleName, RuleVersion, RuleVersionSummary};
pub use signal::{AlertFact, AlertLink, AlertSignal, TriggerKind, dedup_key};

use serde::Serialize;

/// The largest page a list answers, and the default.
pub const ALERTS_PAGE_MAX: usize = 200;

#[derive(Debug, thiserror::Error)]
pub enum AlertError {
    #[error("{field}: {reason}")]
    Invalid { field: String, reason: String },
    #[error("no alert rule is published under `{0}`")]
    UnknownRule(RuleName),
    #[error("alert rule `{name}` has no version `{version}`")]
    UnknownVersion { name: RuleName, version: String },
    #[error("no delivery is stored under `{0}`")]
    UnknownDelivery(String),
    /// A delivery that never failed has nothing to retry, and one still queued
    /// is already going to be tried.
    #[error("delivery `{key}` is {state}, and only a failed one can be sent again")]
    NotRetryable { key: String, state: &'static str },
    #[error("the alert registry is not configured")]
    Disabled,
    #[error("no alert channel is configured")]
    NoChannel,
    #[error("stored alert record at {key} is not readable")]
    Corrupt {
        key: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("storage: {0}")]
    Store(#[from] aiwatcher_core::ports::PortError),
    #[error("cannot encode an alert record: {0}")]
    Encoding(#[from] serde_json::Error),
}

impl From<aiwatcher_evaluation::EvaluationError> for AlertError {
    /// A policy's refusal keeps its own sentence: it is about the tolerance or
    /// the case list the rule pins, and rewording it here would send a reader
    /// looking at the rule for a problem that is not in it.
    fn from(error: aiwatcher_evaluation::EvaluationError) -> Self {
        match error {
            aiwatcher_evaluation::EvaluationError::Invalid { field, reason } => {
                Self::Invalid { field, reason }
            }
            other => Self::Invalid {
                field: "trigger.policy".into(),
                reason: other.to_string(),
            },
        }
    }
}

impl AlertError {
    /// Whether coming back with the same request could answer differently.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Store(error) => error.is_retryable(),
            _ => false,
        }
    }
}

pub type Result<T> = std::result::Result<T, AlertError>;

pub(crate) fn require(ok: bool, field: &str, reason: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(AlertError::Invalid {
            field: field.into(),
            reason: reason.into(),
        })
    }
}

pub(crate) fn text(value: &str, field: &str) -> Result<()> {
    require(
        !value.is_empty()
            && value.trim() == value
            && value.len() <= 512
            && !value.chars().any(char::is_control),
        field,
        "must be trimmed nonblank text of at most 512 bytes without control characters",
    )
}

/// `sha256`, hex, lower case, over bytes this crate composed.
pub(crate) fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Sort every object explicitly, the way the other registries do: a version id
/// is a digest over these bytes, and a serde feature somebody else in the
/// workspace enables must not move it.
pub(crate) fn canonical<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let value: serde_json::Value = serde_json::to_value(value)?;
    let mut bytes = Vec::new();
    write_canonical(&value, &mut bytes)?;
    Ok(bytes)
}

fn write_canonical(value: &serde_json::Value, out: &mut Vec<u8>) -> Result<()> {
    match value {
        serde_json::Value::Object(map) => {
            out.push(b'{');
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                serde_json::to_writer(&mut *out, key)?;
                out.push(b':');
                write_canonical(&map[key], out)?;
            }
            out.push(b'}');
        }
        serde_json::Value::Array(items) => {
            out.push(b'[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                write_canonical(item, out)?;
            }
            out.push(b']');
        }
        other => serde_json::to_writer(&mut *out, other)?,
    }
    Ok(())
}
