//! Evaluation owns pinned variants and evidence, independently of telemetry.
//! The facade validates and freezes contracts. The registry owns durable
//! evidence, source authorization, immutable publication and erasure (ADR_0030).

mod context;
mod manifest;
mod reference;
mod registry;
mod result;
mod store;

pub use registry::{Registry, RegistryConfig, SourceAuthority, SourceEvidence};
pub use result::*;

pub use context::{
    Aggregation, EvaluationContext, JudgeConfiguration, MetricDefinition, MetricDirection,
};
pub use manifest::{EvaluationManifest, EvaluationOrigin, PreparedEvaluation, VariantManifest};
pub use reference::{DatasetKind, DatasetReference, VersionReference};

use serde::Serialize;
use sha2::{Digest, Sha256};

pub const SCHEMA_VERSION: u32 = 1;
/// Contract metadata, not result bytes. Large inputs belong in artifacts.
pub const MAX_MANIFEST_BYTES: usize = 256 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum EvaluationError {
    #[error("{field}: {reason}")]
    Invalid { field: String, reason: String },
    #[error("evaluation ID already belongs to a different result")]
    Conflict,
    #[error("evaluation evidence is {0:?}")]
    Unavailable(EvidenceState),
    #[error("storage: {0}")]
    Storage(#[from] aiwatcher_core::ports::PortError),
    #[error("cannot encode evaluation metadata: {0}")]
    Encoding(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, EvaluationError>;

/// The public entry point. A prepared value can only be built after validation.
#[derive(Debug)]
pub struct Evaluation;

impl Evaluation {
    pub fn prepare(manifest: EvaluationManifest) -> Result<PreparedEvaluation> {
        manifest.validate()?;
        let bytes = canonical(&manifest)?;
        require(
            bytes.len() <= MAX_MANIFEST_BYTES,
            "manifest",
            "exceeds 256 KiB",
        )?;
        let variant_id = digest(&manifest.variant)?;
        let context_id = digest(&(SCHEMA_VERSION, &manifest.context))?;
        Ok(PreparedEvaluation::new(manifest, variant_id, context_id))
    }
}

pub(crate) fn require(ok: bool, field: &str, reason: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(EvaluationError::Invalid {
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

pub(crate) fn digest<T: Serialize>(value: &T) -> Result<String> {
    Ok(hex::encode(Sha256::digest(canonical(value)?)))
}

// Sort every object explicitly: another workspace consumer may enable
// serde_json's preserve_order feature. SDKs use the server's returned IDs.
fn canonical<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    fn sort(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => {
                map.sort_keys();
                for child in map.values_mut() {
                    sort(child);
                }
            }
            serde_json::Value::Array(items) => {
                for child in items {
                    sort(child);
                }
            }
            _ => {}
        }
    }
    let mut value = serde_json::to_value(value)?;
    sort(&mut value);
    Ok(serde_json::to_vec(&value)?)
}
