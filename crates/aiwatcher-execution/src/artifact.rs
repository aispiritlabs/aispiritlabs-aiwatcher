//! Where a step's output goes, and what it was made from.
//!
//! Sections 9.5, 17 and 18. Three questions with one owner, because they are
//! the same fact read three ways: *what is this artifact*, *what produced it*,
//! and *has this exact work already been done*.
//!
//! ## The catalog stores no bytes
//!
//! An [`ArtifactRef`] is a pointer with a digest. The bytes are the object
//! store's — `aiwatcher-core`'s `ObjectStore` port, the same one the prompt
//! registry, the annotations and the conversation archive already write
//! through. A catalog that also held content would be a second copy of
//! something addressed by its content, which is the one duplication that cannot
//! be detected afterwards.
//!
//! ## Deleting the index loses nothing authoritative
//!
//! The cache is an *index*, and section 18 is explicit that dropping it must
//! cost a rerun and never a result. So a hit is recorded in the workflow
//! history as [`WorkflowEvent::StepCacheHit`](crate::WorkflowEvent), with the
//! key and the artifact ids in it — the run stays explainable after the index
//! is gone. And invalidation *marks* an entry rather than deleting the
//! artifacts it names: an old execution that used them is a record of what
//! happened, and rewriting it would be a different kind of lie.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;

use aiwatcher_core::{ArtifactKind, ArtifactRef};

use crate::state::ExecutionId;

/// Who made this, so a reader can get from a byte range back to a decision.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
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

/// An in-memory catalog. For tests, and for a development run that keeps no
/// lineage across a restart.
#[derive(Debug, Default)]
pub struct MemoryArtifactCatalog {
    inner: tokio::sync::Mutex<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    artifacts: Vec<CatalogedArtifact>,
    cache: std::collections::BTreeMap<String, CacheEntry>,
}

impl MemoryArtifactCatalog {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl ArtifactCatalog for MemoryArtifactCatalog {
    async fn record(&self, artifact: CatalogedArtifact) -> crate::Result<CatalogedArtifact> {
        let mut inner = self.inner.lock().await;
        if let Some(held) = inner.artifacts.iter().find(|held| {
            held.artifact.digest == artifact.artifact.digest
                && held.artifact.kind == artifact.artifact.kind
        }) {
            return Ok(held.clone());
        }
        inner.artifacts.push(artifact.clone());
        Ok(artifact)
    }

    async fn by_digest(
        &self,
        digest: &str,
        kind: ArtifactKind,
    ) -> crate::Result<Option<CatalogedArtifact>> {
        Ok(self
            .inner
            .lock()
            .await
            .artifacts
            .iter()
            .find(|held| held.artifact.digest == digest && held.artifact.kind == kind)
            .cloned())
    }

    async fn produced_by(&self, execution: &ExecutionId) -> crate::Result<Vec<CatalogedArtifact>> {
        Ok(self
            .inner
            .lock()
            .await
            .artifacts
            .iter()
            .filter(|held| {
                held.produced_by
                    .as_ref()
                    .is_some_and(|made| &made.execution_id == execution)
            })
            .cloned()
            .collect())
    }

    async fn cached(
        &self,
        cache_key: &str,
        now: OffsetDateTime,
    ) -> crate::Result<Option<CacheEntry>> {
        Ok(self
            .inner
            .lock()
            .await
            .cache
            .get(cache_key)
            .filter(|entry| entry.is_usable(now))
            .cloned())
    }

    async fn remember(&self, entry: CacheEntry) -> crate::Result<()> {
        self.inner
            .lock()
            .await
            .cache
            .insert(entry.cache_key.clone(), entry);
        Ok(())
    }

    async fn invalidate(&self, cache_key: &str, at: OffsetDateTime) -> crate::Result<()> {
        if let Some(entry) = self.inner.lock().await.cache.get_mut(cache_key) {
            entry.invalidated_at = Some(at);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(digest: &str) -> ArtifactRef {
        ArtifactRef::new("rows", format!("s3://bucket/{digest}"), digest.to_owned())
            .of_kind(ArtifactKind::Rows)
    }

    fn cataloged(digest: &str) -> CatalogedArtifact {
        CatalogedArtifact {
            artifact: rows(digest),
            produced_by: Some(Provenance {
                execution_id: ExecutionId::new("exec-1"),
                step_id: "extract".to_owned(),
                attempt: 1,
            }),
            inputs: Vec::new(),
            created_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[tokio::test]
    async fn a_deterministic_rerun_is_one_artifact_and_not_two() {
        // Two attempts over the same inputs write byte-identical output. A
        // catalog that counted them twice would report a rerun as a second
        // artifact and a lineage that forked where nothing forked.
        let catalog = MemoryArtifactCatalog::new();
        let first = catalog.record(cataloged("ab")).await.expect("a record");
        let again = catalog
            .record(CatalogedArtifact {
                produced_by: Some(Provenance {
                    attempt: 2,
                    ..first.produced_by.clone().expect("provenance")
                }),
                ..cataloged("ab")
            })
            .await
            .expect("the same bytes again");
        assert_eq!(
            again.produced_by.expect("provenance").attempt,
            1,
            "the first attempt keeps the provenance; the second wrote the same bytes"
        );
        assert_eq!(
            catalog
                .produced_by(&ExecutionId::new("exec-1"))
                .await
                .expect("a lineage read")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn invalidating_an_entry_leaves_what_it_named_alone() {
        // Section 18: invalidation marks an entry, it does not mutate old
        // executions. An execution that used those artifacts is a record of
        // what happened.
        let catalog = MemoryArtifactCatalog::new();
        catalog.record(cataloged("ab")).await.expect("a record");
        catalog
            .remember(CacheEntry {
                cache_key: "key".to_owned(),
                artifacts: vec![rows("ab")],
                created_at: OffsetDateTime::UNIX_EPOCH,
                expires_at: None,
                invalidated_at: None,
            })
            .await
            .expect("a cache write");
        assert!(
            catalog
                .cached("key", OffsetDateTime::UNIX_EPOCH)
                .await
                .expect("a read")
                .is_some()
        );

        catalog
            .invalidate("key", OffsetDateTime::UNIX_EPOCH)
            .await
            .expect("an invalidation");
        assert!(
            catalog
                .cached("key", OffsetDateTime::UNIX_EPOCH)
                .await
                .expect("a read")
                .is_none(),
            "an invalidated entry does not answer"
        );
        assert!(
            catalog
                .by_digest("ab", ArtifactKind::Rows)
                .await
                .expect("a read")
                .is_some(),
            "and the artifact it named is still there"
        );
    }

    #[tokio::test]
    async fn an_expired_entry_and_a_missing_one_are_the_same_answer() {
        // The caller reruns either way, and separating them would put the
        // policy in every caller.
        let catalog = MemoryArtifactCatalog::new();
        catalog
            .remember(CacheEntry {
                cache_key: "key".to_owned(),
                artifacts: vec![rows("ab")],
                created_at: OffsetDateTime::UNIX_EPOCH,
                expires_at: Some(OffsetDateTime::UNIX_EPOCH + time::Duration::hours(1)),
                invalidated_at: None,
            })
            .await
            .expect("a cache write");

        let inside = OffsetDateTime::UNIX_EPOCH + time::Duration::minutes(30);
        let outside = OffsetDateTime::UNIX_EPOCH + time::Duration::hours(2);
        assert!(
            catalog
                .cached("key", inside)
                .await
                .expect("a read")
                .is_some()
        );
        assert!(
            catalog
                .cached("key", outside)
                .await
                .expect("a read")
                .is_none()
        );
        assert!(
            catalog
                .cached("absent", inside)
                .await
                .expect("a read")
                .is_none()
        );
    }
}
