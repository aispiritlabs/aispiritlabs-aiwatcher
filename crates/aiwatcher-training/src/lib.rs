//! Training runs, and the model versions they produce.
//!
//! **This is not on the tracing path, and that is the decision.** An earlier
//! design put `train.*` events on the event log beside `llm.*` and `tool.*`;
//! following it through, an epoch turned out not to be a span, a step turned
//! out not to belong on the log at all, and a profiler session turned out not
//! to be a trace — which left one span with no children and a special case in
//! the read model to make its status work. A training run has a different
//! grain (minutes, not milliseconds), a different lifetime (it must outlive
//! retention) and a different reader (a curve, not a waterfall). So it has its
//! own module, its own store and its own API. See ADR_0018.
//!
//! ```text
//! annotation export ──► training run ──► checkpoint ──► model version
//!  project@sha256         the curve       a pointer      name@sha256
//!                                                             │
//!                                            an agent span's `model` ◄┘
//! ```
//!
//! That last arrow is the whole reason this lives here rather than in Weights
//! & Biases: from a floor plan coming back with bad geometry, to the model
//! version that produced it, to the labelled images behind it, without leaving
//! one system.

use aiwatcher_core::ports::{PortError, PortResult};
use aiwatcher_core::prompts::ObjectStore;
use sha2::{Digest, Sha256};

pub mod model;
pub mod package;
pub mod registry;
pub mod run;

pub use model::{
    ModelDetail, ModelHead, ModelLabelRequest, ModelMetrics, ModelPage, ModelVersion,
    ModelVersionSummary, PRODUCTION, RegisterModelRequest, RegisteredModel,
};
pub use package::{ArtifactRef, MAX_ARTIFACTS, ModelPackage, ResourceRequest, Runtime, TensorSpec};
pub use registry::Registry;
pub use run::{
    BestMetric, CheckpointInput, CheckpointRecord, EpochInput, EpochRecord, FinishRunRequest,
    ProfileInput, ProfileRecord, ProgressRequest, RunFilter, SampleInput, SampleRecord,
    StartRunRequest, TrainingRun, TrainingRunPage, TrainingRunSummary, TrainingStatus,
    is_reproducible,
};

/// Names and ids.
const MAX_NAME_BYTES: usize = 160;
/// The epoch index a run may reach. Beyond this something is looping.
pub const MAX_EPOCHS: usize = 10_000;
/// Points on the fine-grained series before it is halved. See `decimate`.
pub const MAX_SAMPLES: usize = 2_000;
const MAX_CHECKPOINTS: usize = 500;
const MAX_PROFILES: usize = 50;
/// One run's whole record. A 10 000-epoch run with six metrics is well inside
/// this; a run that is not is a run putting something in `params` that belongs
/// in an object store.
const MAX_RUN_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),

    /// A decision this registry made, which it will make identically again.
    /// Separate from [`Self::Invalid`] because the request was well formed —
    /// promoting a model with no held-out score is not a typo.
    #[error("{0}")]
    Refused(String),

    #[error("{0} was not found")]
    NotFound(String),

    #[error("{what} is {size}; the limit is {limit}")]
    TooLarge {
        what: &'static str,
        size: usize,
        limit: usize,
    },

    #[error("the training registry could not use its object store: {0}")]
    Store(#[from] PortError),

    #[error("stored object {key} is not a training registry document: {message}")]
    Corrupt { key: String, message: String },
}

pub type Result<T> = std::result::Result<T, Error>;

/// A single identifier: a model name, a run id, a label.
///
/// No slashes, unlike a prompt or an annotation project, and for a concrete
/// reason: a model name is a *path segment* in `/api/v1/models/{name}`, and a
/// name with a slash in it would match no route at all. Dots separate instead,
/// exactly as they do for a prompt name — `floor-plan.segmenter`.
///
/// Checked before it is interpolated into an object key, for the same reason
/// every part of an `EngineRef` is: a `..` in an identifier is a path traversal
/// into somebody else's data.
pub fn validate_slug(value: &str, what: &str) -> Result<()> {
    if value.is_empty() || value.len() > MAX_NAME_BYTES {
        return Err(Error::Invalid(format!(
            "{what} must be between 1 and {MAX_NAME_BYTES} bytes"
        )));
    }
    let mut characters = value.chars();
    let starts_well = characters
        .next()
        .is_some_and(|value| value.is_ascii_alphanumeric());
    let continues_well =
        characters.all(|value| value.is_ascii_alphanumeric() || matches!(value, '-' | '_' | '.'));
    if !starts_well || !continues_well || matches!(value, "." | "..") {
        return Err(Error::Invalid(format!(
            "{what} must start with a letter or number and hold only letters, numbers, '.', '_' or '-'"
        )));
    }
    Ok(())
}

fn digest(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}

impl From<Error> for PortError {
    fn from(error: Error) -> Self {
        match error {
            Error::Store(error) => error,
            other => Self::Rejected {
                target: "training-registry",
                message: other.to_string(),
            },
        }
    }
}

/// A small probe useful to wiring and health checks.
pub async fn probe(store: &dyn ObjectStore, prefix: &str) -> PortResult<()> {
    store.list(prefix).await.map(|_| ())
}
