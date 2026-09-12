//! Where a step's output goes, and what it was made from: what an artifact is,
//! what produced it, and whether this exact work has already been done.
//!
//! **The catalog stores no bytes.** An [`ArtifactRef`] is a pointer with a
//! digest; the bytes belong to the `ObjectStore` port the registries already
//! write through. A catalog holding content would be a second copy of something
//! addressed by its content.
//!
//! Two adapters, one port: [`memory`] for tests and a development run that
//! keeps no lineage across a restart, [`object`] for everything else, under the
//! `artifacts/` prefix beside the bytes it describes.
//!
//! **Deleting the index loses nothing authoritative.** Dropping the cache must
//! cost a rerun and never a result, so a hit is recorded in the workflow
//! history as [`WorkflowEvent::StepCacheHit`](crate::WorkflowEvent) with its
//! key and artifact ids — the run stays explainable without the index. And
//! invalidation *marks* an entry rather than deleting the artifacts it names:
//! an old execution that used them is a record of what happened.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;

use aiwatcher_core::{ArtifactKind, ArtifactRef};

use crate::state::ExecutionId;

/// Who made this, so a reader can get from a byte range back to a decision.
///
/// Named `ArtifactProvenance` in the contract, because an OpenAPI components
/// block is one global namespace and a conversation turn already has a
/// `Provenance` in it. Two crates are free to call their own noun the same
/// thing; the document is not.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[schema(as = ArtifactProvenance)]
pub struct Provenance {
    pub execution_id: ExecutionId,
    pub step_id: String,
    pub attempt: u32,
}

/// One artifact as the catalog holds it: the pointer, who made it, and what it
/// was made from.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, ToSchema)]
pub struct CatalogedArtifact {
    pub artifact: ArtifactRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub produced_by: Option<Provenance>,
    /// The digests this was made from, in the order the step read them.
    ///
    /// Digests rather than ids: lineage has to survive a catalog that was
    /// rebuilt, and a digest is the same fact in every copy of it.
    #[serde(default)]
    pub inputs: Vec<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// A cache entry: a key, what it resolved to, and whether it still counts.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, ToSchema)]
pub struct CacheEntry {
    pub cache_key: String,
    pub artifacts: Vec<ArtifactRef>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
    /// Set rather than deleted. An old execution that used this entry is a
    /// record of what happened; removing the row would not change that and
    /// removing the artifacts would break it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "time::serde::rfc3339::option")]
    pub invalidated_at: Option<OffsetDateTime>,
}

impl CacheEntry {
    /// Whether this entry may still answer for a step.
    #[must_use]
    pub fn is_usable(&self, now: OffsetDateTime) -> bool {
        self.invalidated_at.is_none() && self.expires_at.is_none_or(|at| now < at)
    }
}

/// Artifact metadata, lineage and the cache index.
#[async_trait]
pub trait ArtifactCatalog: Send + Sync + std::fmt::Debug {
    /// Record an artifact and what it was made from.
    ///
    /// Idempotent by digest: writing the same bytes twice is one row, because
    /// two deterministic attempts over the same inputs produce byte-identical
    /// output and a catalog that counted them twice would report a rerun as a
    /// second artifact.
    ///
    /// # Errors
    ///
    /// Whatever the backend could not do.
    async fn record(&self, artifact: CatalogedArtifact) -> crate::Result<CatalogedArtifact>;

    /// One artifact by its content address.
    ///
    /// # Errors
    ///
    /// Whatever the backend could not do.
    async fn by_digest(
        &self,
        digest: &str,
        kind: ArtifactKind,
    ) -> crate::Result<Option<CatalogedArtifact>>;

    /// Everything one execution produced, in the order it was recorded.
    ///
    /// # Errors
    ///
    /// Whatever the backend could not do.
    async fn produced_by(&self, execution: &ExecutionId) -> crate::Result<Vec<CatalogedArtifact>>;

    /// A usable cache entry, or `None`.
    ///
    /// `None` for an entry that is expired or invalidated as well as for one
    /// that was never written — the caller reruns either way, and separating
    /// them would put the policy in every caller.
    ///
    /// # Errors
    ///
    /// Whatever the backend could not do.
    async fn cached(
        &self,
        cache_key: &str,
        now: OffsetDateTime,
    ) -> crate::Result<Option<CacheEntry>>;

    /// Remember what this key resolved to.
    ///
    /// # Errors
    ///
    /// Whatever the backend could not do.
    async fn remember(&self, entry: CacheEntry) -> crate::Result<()>;

    /// Mark an entry unusable without touching what it names.
    ///
    /// # Errors
    ///
    /// Whatever the backend could not do.
    async fn invalidate(&self, cache_key: &str, at: OffsetDateTime) -> crate::Result<()>;
}

pub mod memory;
pub mod object;

pub use memory::MemoryArtifactCatalog;
pub use object::ObjectArtifactCatalog;
