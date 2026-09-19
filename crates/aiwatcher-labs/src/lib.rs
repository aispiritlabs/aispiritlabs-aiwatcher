//! Workshop labs: the authored brief somebody reads, and the pinned
//! measurement their work is held to (ADR_0034).
//!
//! **Almost nothing a lab needs is here.** Its tests are an
//! `aiwatcher-evaluation` scorecard at a version and a cohort derived from a
//! dataset version; the work handed in is a staged recording, or answers a
//! worker generated; the mark is a published evaluation result, and the class's
//! view of one is every result sharing the lab's `context_id`. All four already
//! existed under names that came from measuring a model rather than from
//! teaching anybody, and this crate adds no second copy of any of them.
//!
//! What it adds is the one thing nothing here held: an authored document saying
//! *this is the exercise, this is what it is measured by, and this is where it
//! sits in the workshop*. A lab is a version of that document, addressed by its
//! content like a prompt version (ADR_0011), stored in the same object store
//! outside the log's retention, and project-scoped from birth (ADR_0033) —
//! because a workshop **is** a project and who may see a lab is the grant on it.

mod lab;
mod measurement;
mod registry;
mod scope;

pub use lab::{
    Lab, LabHead, LabName, LabNotebook, LabTests, LabVersion, LabVersionSummary, MAX_BRIEF_BYTES,
    MAX_VERSIONS_INDEXED, PUBLISHED_LABEL,
};
pub use measurement::LabMeasurement;
pub use registry::{LabFilter, LabPage, LabPublished, LabSummary, PublishLab, Registry};

use serde::Serialize;

/// The largest page a list answers, and the default.
pub const LABS_PAGE_MAX: usize = 200;

#[derive(Debug, thiserror::Error)]
pub enum LabError {
    #[error("{field}: {reason}")]
    Invalid { field: String, reason: String },
    #[error("no lab is published under `{0}`")]
    UnknownLab(LabName),
    #[error("lab `{name}` has no version `{version}`")]
    UnknownVersion { name: LabName, version: String },
    /// A lab bound to one project cannot answer for another, and a key that
    /// would leave its prefix is not a key this registry writes.
    #[error("scope: {0}")]
    InvalidScope(&'static str),
    #[error("the lab registry is not configured")]
    Disabled,
    #[error("stored lab at {key} is not readable")]
    Corrupt {
        key: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("storage: {0}")]
    Store(#[from] aiwatcher_core::ports::PortError),
    #[error("cannot encode a lab: {0}")]
    Encoding(#[from] serde_json::Error),
    /// The card this lab pins measures something a lab does not pin.
    #[error("the scorecard's `{metric}` {reason}")]
    Unmeasurable { metric: String, reason: String },
    /// A lab's tests name a card or a cohort this project does not hold.
    /// Refused where the lab is written, rather than when somebody submits
    /// to a slot that measures nothing.
    #[error("tests: {0}")]
    Unpinned(String),
}

impl From<aiwatcher_evaluation::EvaluationError> for LabError {
    /// An evaluation refusal keeps its own sentence: it is about the card or
    /// the cohort a lab points at, and rewording it here would send a reader
    /// looking at the lab for a problem that is not in it.
    fn from(error: aiwatcher_evaluation::EvaluationError) -> Self {
        use aiwatcher_evaluation::EvaluationError;
        match error {
            EvaluationError::Invalid { field, reason } => Self::Invalid { field, reason },
            other => Self::Invalid {
                field: "tests".into(),
                reason: other.to_string(),
            },
        }
    }
}

impl LabError {
    /// Whether coming back with the same request could answer differently.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Store(error) => error.is_retryable(),
            _ => false,
        }
    }
}

pub type Result<T> = std::result::Result<T, LabError>;

pub(crate) fn require(ok: bool, field: &str, reason: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(LabError::Invalid {
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

/// Sort every object explicitly, the way `aiwatcher-evaluation` does: a
/// version id is a digest over these bytes, and a serde feature somebody else
/// in the workspace enables must not move it.
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
