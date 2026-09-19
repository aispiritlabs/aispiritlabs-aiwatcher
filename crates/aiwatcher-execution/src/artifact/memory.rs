//! The catalog in process. For tests, and for a run that keeps no lineage.

use std::sync::Arc;

use async_trait::async_trait;
use time::OffsetDateTime;

use aiwatcher_core::ArtifactKind;
use aiwatcher_iam::ProjectScope;

use super::{ArtifactCatalog, CacheEntry, CatalogedArtifact};
use crate::state::ExecutionId;

/// An in-memory catalog. For tests, and for a development run that keeps no
/// lineage across a restart.
///
/// One process, so one map — and the scope is part of its key rather than a
/// second map, for the reason the object adapter puts it in the prefix: two
/// projects may record the same digest, and a catalog that kept one row for
/// both would hand the second project the first one's lineage.
#[derive(Debug, Default)]
pub struct MemoryArtifactCatalog {
    inner: Arc<tokio::sync::Mutex<Inner>>,
    scope: Option<ProjectScope>,
}

#[derive(Debug, Default)]
struct Inner {
    artifacts: Vec<(Option<ProjectScope>, CatalogedArtifact)>,
    cache: std::collections::BTreeMap<(Option<ProjectScope>, String), CacheEntry>,
}

impl MemoryArtifactCatalog {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl ArtifactCatalog for MemoryArtifactCatalog {
    fn for_project(&self, scope: ProjectScope) -> crate::Result<Arc<dyn ArtifactCatalog>> {
        if let Some(current) = self.scope
            && current != scope
        {
            return Err(crate::StoreError::OutOfScope(
                "the artifact catalog is already bound to another project".to_owned(),
            ));
        }
        Ok(Arc::new(Self {
            inner: Arc::clone(&self.inner),
            scope: Some(scope),
        }))
    }

    async fn record(&self, artifact: CatalogedArtifact) -> crate::Result<CatalogedArtifact> {
        let mut inner = self.inner.lock().await;
        if let Some((_, held)) = inner.artifacts.iter().find(|(side, held)| {
            *side == self.scope
                && held.artifact.digest == artifact.artifact.digest
                && held.artifact.kind == artifact.artifact.kind
        }) {
            return Ok(held.clone());
        }
        inner.artifacts.push((self.scope, artifact.clone()));
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
            .find(|(side, held)| {
                *side == self.scope && held.artifact.digest == digest && held.artifact.kind == kind
            })
            .map(|(_, held)| held.clone()))
    }

    async fn produced_by(&self, execution: &ExecutionId) -> crate::Result<Vec<CatalogedArtifact>> {
        Ok(self
            .inner
            .lock()
            .await
            .artifacts
            .iter()
            .filter(|(side, held)| {
                *side == self.scope
                    && held
                        .produced_by
                        .as_ref()
                        .is_some_and(|made| &made.execution_id == execution)
            })
            .map(|(_, held)| held.clone())
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
            .get(&(self.scope, cache_key.to_owned()))
            .filter(|entry| entry.is_usable(now))
            .cloned())
    }

    async fn remember(&self, entry: CacheEntry) -> crate::Result<()> {
        self.inner
            .lock()
            .await
            .cache
            .insert((self.scope, entry.cache_key.clone()), entry);
        Ok(())
    }

    async fn invalidate(&self, cache_key: &str, at: OffsetDateTime) -> crate::Result<()> {
        if let Some(entry) = self
            .inner
            .lock()
            .await
            .cache
            .get_mut(&(self.scope, cache_key.to_owned()))
        {
            entry.invalidated_at = Some(at);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use aiwatcher_core::ArtifactRef;

    use super::super::Provenance;
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
        // Invalidation marks an entry, it does not mutate old executions. An execution that used those artifacts is a record of
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
    async fn one_digest_in_two_projects_is_two_rows_with_two_lineages() {
        // The same bytes in two projects are one content address and two
        // objects (ADR_0033 pt. 4). A catalog keyed by digest alone would hand
        // the second project the first one's provenance, which is a lineage
        // naming an execution its reader may not open.
        let catalog = MemoryArtifactCatalog::new();
        let mine = catalog.for_project(scope(1)).expect("bound");
        let yours = catalog.for_project(scope(2)).expect("bound");
        mine.record(cataloged("ab")).await.expect("a record");

        assert_eq!(
            mine.produced_by(&ExecutionId::new("exec-1"))
                .await
                .expect("a lineage read")
                .len(),
            1
        );
        assert!(
            yours
                .produced_by(&ExecutionId::new("exec-1"))
                .await
                .expect("a lineage read")
                .is_empty(),
            "the other project's rows are not this project's"
        );
        assert!(
            catalog
                .produced_by(&ExecutionId::new("exec-1"))
                .await
                .expect("a lineage read")
                .is_empty(),
            "and neither are they the deployment's"
        );
        assert!(
            mine.for_project(scope(2)).is_err(),
            "a catalog that could be re-aimed is one a later call aims somewhere \
             its first caller never authorized"
        );
    }

    fn scope(project: u128) -> ProjectScope {
        ProjectScope {
            organization: aiwatcher_iam::OrganizationId(uuid::Uuid::from_u128(1)),
            project: aiwatcher_iam::ProjectId(uuid::Uuid::from_u128(project)),
        }
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
