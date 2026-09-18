//! The catalog over an object store: the manifest beside the bytes, and the
//! cache index beside both.
//!
//! `aiwatcher-server`'s reactor writes the `data`; this writes the
//! `manifest.json` that says who made it and what it was made from, and the
//! `cache/` entries that let an identical step be answered without running it.
//! Both spell their keys with [`layout`], which is the only place that says
//! what one looks like.
//!
//! ```text
//! artifacts/<kind>/<aa>/<sha256>/data           the bytes        (the reactor)
//! artifacts/<kind>/<aa>/<sha256>/manifest.json  what they are    (here)
//! artifacts/cache/<sha256 of the key>.json      what a key resolved to
//! artifacts/lineage/<execution>/<sha256>.json   what one run produced
//! ```
//!
//! **The lineage entry is a second write** because an object store lists by
//! prefix and nothing else: "everything this execution produced" is a question
//! the manifest cannot answer without walking the bucket. A pointer, so the
//! manifest stays the one description of an artifact.
//!
//! **An unreadable entry is a miss, and a *readable* one describing something
//! else is not.** The cache is an index and a rerun is the cost of dropping it;
//! but a record that names another key, execution or namespace says something
//! false, and a hit would hand back somebody else's. Refused by name — the
//! receipt's own rule, one family over.
//!
//! [`ObjectArtifactCatalog::for_project`] binds every family below
//! `artifacts/scopes/<organization>/<project>/registry/`, beside that project's
//! bytes (ADR_0033). Identical bytes, cache key and execution id in two projects
//! share no manifest, provenance, cache answer or invalidation. A bound catalog
//! accepts only its own namespace's references; the deployment-wide one accepts
//! everything it historically did **except** one reaching into a project.
//!
//! **Storage isolation, not authorization.** A caller binds a scope taken from
//! trusted durable execution ownership and checks the grant *before* it asks —
//! the cache lookup included, since a hit is an answer about somebody's data
//! whether or not work follows.

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use time::OffsetDateTime;

use aiwatcher_core::prompts::ObjectStore;
use aiwatcher_core::{ArtifactKind, ArtifactRef};
use aiwatcher_iam::ProjectScope;

use super::layout;
use super::{ArtifactCatalog, CacheEntry, CatalogedArtifact};
use crate::error::StoreError;
use crate::state::ExecutionId;

/// The prefix everything here shares with the bytes it describes.
///
/// The same string `aiwatcher-server`'s reactor writes under, and deliberately
/// so: a manifest that lived somewhere else would be a second place to look
/// when an artifact is missing.
pub const PREFIX: &str = layout::PREFIX;

/// Artifact metadata, lineage and the cache index, in an object store.
#[derive(Clone, Debug)]
pub struct ObjectArtifactCatalog {
    store: Arc<dyn ObjectStore>,
    prefix: String,
    scope: Option<ProjectScope>,
}

/// A lineage pointer: which artifact, so the manifest can be read for the rest.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct LineageEntry {
    digest: String,
    kind: ArtifactKind,
}

impl ObjectArtifactCatalog {
    #[must_use]
    pub fn new(store: Arc<dyn ObjectStore>) -> Self {
        Self {
            store,
            prefix: layout::prefix(None),
            scope: None,
        }
    }

    /// Bind the metadata, the lineage and the cache index to one project,
    /// beside that project's bytes and without changing a content address.
    ///
    /// Binding to the scope already held is the same catalog, so a caller that
    /// resolves a scope twice gets one answer; binding a bound catalog to a
    /// *second* project is refused, because a catalog that could be re-aimed is
    /// one a later call can aim somewhere its first caller never authorized.
    ///
    /// This grants nothing. See the module docs: the scope comes from trusted
    /// durable execution ownership, and the grant and the lease are checked by
    /// whoever asks.
    ///
    /// # Errors
    ///
    /// [`StoreError::OutOfScope`] when this catalog is already bound elsewhere.
    pub fn for_project(&self, scope: ProjectScope) -> crate::Result<Self> {
        if let Some(current) = self.scope {
            return if current == scope {
                Ok(self.clone())
            } else {
                Err(StoreError::OutOfScope(
                    "the artifact catalog is already bound to another project".to_owned(),
                ))
            };
        }
        Ok(Self {
            store: Arc::clone(&self.store),
            prefix: layout::prefix(Some(scope)),
            scope: Some(scope),
        })
    }

    /// The project this catalog is bound to, or `None` for the deployment-wide
    /// one.
    #[must_use]
    pub const fn project_scope(&self) -> Option<ProjectScope> {
        self.scope
    }

    /// The prefix every family here is written under.
    ///
    /// Read by the caller that pairs this catalog with the store writing the
    /// bytes, so the pairing can be checked rather than assumed.
    #[must_use]
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// Whether this reference is one this namespace may write down or hand
    /// back.
    ///
    /// A bound catalog takes its own canonical pointers and nothing else. The
    /// deployment-wide one is deliberately looser — it has always recorded
    /// references it did not mint, and narrowing that here would drop rows
    /// rather than isolate anything — but it refuses a pointer into the scoped
    /// area, which is the crossing this exists to close. Both require a real
    /// content address, because the digest is a path segment and a digest that
    /// is not one is a key this catalog may not build.
    fn admits(&self, artifact: &ArtifactRef) -> crate::Result<()> {
        let uri = &artifact.uri;
        if !artifact.has_digest() {
            return Err(StoreError::OutOfScope(format!(
                "{uri} is not named by a sha256 content address"
            )));
        }
        match self.scope {
            Some(scope) if !layout::addresses(&self.prefix, artifact) => {
                Err(StoreError::OutOfScope(format!(
                    "{uri} is not an artifact of project {} of organization {}",
                    scope.project.0, scope.organization.0
                )))
            }
            None if layout::reaches_a_project(uri) => Err(StoreError::OutOfScope(format!(
                "{uri} belongs to a project rather than to this deployment"
            ))),
            _ => Ok(()),
        }
    }

    fn manifest_key(&self, digest: &str, kind: ArtifactKind) -> String {
        layout::manifest_key(&self.prefix, kind, digest)
    }

    fn cache_key_path(&self, cache_key: &str) -> String {
        layout::cache_entry_key(&self.prefix, cache_key)
    }

    fn lineage_prefix(&self, execution: &ExecutionId) -> String {
        layout::lineage_prefix(&self.prefix, execution.as_str())
    }

    async fn read<T: DeserializeOwned>(&self, key: &str) -> crate::Result<Option<T>> {
        let Some(bytes) = self
            .store
            .get(key)
            .await
            .map_err(|error| StoreError::Backend(error.to_string()))?
        else {
            return Ok(None);
        };
        // A record a newer build wrote is a miss, not a failure. See the module
        // docs: the index is droppable by design, and refusing to run because
        // of a note about a previous run would be the note taking the run down.
        Ok(serde_json::from_slice(&bytes).ok())
    }

    async fn write<T: Serialize>(&self, key: &str, value: &T) -> crate::Result<()> {
        let body = serde_json::to_vec(value)?;
        self.store
            .put(key, body)
            .await
            .map_err(|error| StoreError::Backend(error.to_string()))
    }

    /// A manifest read back, checked against what was asked for.
    ///
    /// Three things have to hold and each is a different swap: the record
    /// describes the digest and kind the key names, and its pointer is one this
    /// namespace may hand back. Checked on the way out as well as on the way in
    /// because the store is the one part of this nobody here writes.
    async fn manifest(
        &self,
        digest: &str,
        kind: ArtifactKind,
    ) -> crate::Result<Option<CatalogedArtifact>> {
        let key = self.manifest_key(digest, kind);
        let Some(held) = self.read::<CatalogedArtifact>(&key).await? else {
            return Ok(None);
        };
        if held.artifact.digest != digest || held.artifact.kind != kind {
            return Err(StoreError::OutOfScope(format!(
                "{key} holds a manifest for a different artifact"
            )));
        }
        self.admits(&held.artifact)?;
        Ok(Some(held))
    }

    /// A cache entry read back, checked the same way.
    ///
    /// The stored key as well as the artifacts: the path is a hash of the key,
    /// so an entry naming another one is an object somebody put there, and
    /// serving it would answer this step with a different question's rows.
    async fn entry(&self, cache_key: &str) -> crate::Result<Option<CacheEntry>> {
        let key = self.cache_key_path(cache_key);
        let Some(entry) = self.read::<CacheEntry>(&key).await? else {
            return Ok(None);
        };
        if entry.cache_key != cache_key {
            return Err(StoreError::OutOfScope(format!(
                "{key} holds the answer to a different cache key"
            )));
        }
        for artifact in &entry.artifacts {
            self.admits(artifact)?;
        }
        Ok(Some(entry))
    }
}

#[async_trait]
impl ArtifactCatalog for ObjectArtifactCatalog {
    async fn record(&self, artifact: CatalogedArtifact) -> crate::Result<CatalogedArtifact> {
        self.admits(&artifact.artifact)?;
        // Idempotent by digest: two deterministic attempts over the same inputs
        // write byte-identical output, and a catalog that counted them twice
        // would report a rerun as a second artifact and a lineage that forked
        // where nothing forked. The *first* provenance wins, because it is the
        // one that was true.
        if let Some(held) = self
            .manifest(&artifact.artifact.digest, artifact.artifact.kind)
            .await?
        {
            return Ok(held);
        }
        let key = self.manifest_key(&artifact.artifact.digest, artifact.artifact.kind);
        self.write(&key, &artifact).await?;
        if let Some(made) = &artifact.produced_by {
            self.write(
                &layout::lineage_key(
                    &self.prefix,
                    made.execution_id.as_str(),
                    &artifact.artifact.digest,
                ),
                &LineageEntry {
                    digest: artifact.artifact.digest.clone(),
                    kind: artifact.artifact.kind,
                },
            )
            .await?;
        }
        Ok(artifact)
    }

    async fn by_digest(
        &self,
        digest: &str,
        kind: ArtifactKind,
    ) -> crate::Result<Option<CatalogedArtifact>> {
        // A lookup for something that cannot have been written. No key is
        // built from it, so no traversal is either.
        if !layout::is_content_address(digest) {
            return Ok(None);
        }
        self.manifest(digest, kind).await
    }

    async fn produced_by(&self, execution: &ExecutionId) -> crate::Result<Vec<CatalogedArtifact>> {
        let prefix = self.lineage_prefix(execution);
        let entries = self
            .store
            .list(&prefix)
            .await
            .map_err(|error| StoreError::Backend(error.to_string()))?;
        let mut found = Vec::new();
        for entry in entries {
            // Checked *before* it is opened. A listing is not trusted to have
            // answered inside the prefix it was asked about, and reading a key
            // to find out whether it was allowed to be read is the refusal
            // doing the very I/O it exists to prevent.
            let named = entry
                .key
                .strip_prefix(&prefix)
                .and_then(|tail| tail.strip_suffix(".json"))
                .filter(|digest| layout::is_content_address(digest))
                .ok_or_else(|| {
                    StoreError::OutOfScope(format!(
                        "{} is not this execution's lineage pointer",
                        entry.key
                    ))
                })?;
            let Some(pointer) = self.read::<LineageEntry>(&entry.key).await? else {
                continue;
            };
            // And the pointer is not trusted to describe the key it is filed
            // under: that would let one object claim a run's output for a
            // digest that is not its own.
            if pointer.digest != named {
                return Err(StoreError::OutOfScope(format!(
                    "{} points at an artifact it is not named by",
                    entry.key
                )));
            }
            // A pointer with no manifest is the one gap a crash can leave: the
            // manifest is written first, so a missing one means somebody
            // removed it. Skipped, as an absent index entry is.
            let Some(artifact) = self.manifest(&pointer.digest, pointer.kind).await? else {
                continue;
            };
            if artifact.produced_by.as_ref().map(|made| &made.execution_id) != Some(execution) {
                return Err(StoreError::OutOfScope(format!(
                    "{} names an artifact another execution produced",
                    entry.key
                )));
            }
            found.push(artifact);
        }
        found.sort_by_key(|artifact| artifact.created_at);
        Ok(found)
    }

    async fn cached(
        &self,
        cache_key: &str,
        now: OffsetDateTime,
    ) -> crate::Result<Option<CacheEntry>> {
        Ok(self
            .entry(cache_key)
            .await?
            .filter(|entry| entry.is_usable(now)))
    }

    async fn remember(&self, entry: CacheEntry) -> crate::Result<()> {
        for artifact in &entry.artifacts {
            self.admits(artifact)?;
        }
        let key = self.cache_key_path(&entry.cache_key);
        self.write(&key, &entry).await
    }

    async fn invalidate(&self, cache_key: &str, at: OffsetDateTime) -> crate::Result<()> {
        // Read through the same checks as a hit: an entry this namespace may
        // not answer with is not one it may rewrite either, and marking it
        // would leave its foreign pointers in place under a fresh timestamp.
        let Some(entry) = self.entry(cache_key).await? else {
            return Ok(());
        };
        // Marked, never deleted, and the artifacts it names are left alone: an
        // old execution that used them is a record of what happened, and
        // rewriting it would be a different kind of lie.
        self.write(
            &self.cache_key_path(cache_key),
            &CacheEntry {
                invalidated_at: Some(at),
                ..entry
            },
        )
        .await
    }
}

/// What a reactor records after an attempt it actually ran.
///
/// A free function rather than a method on the port, because the two writes are
/// one decision — the manifest is the description and the cache entry is the
/// index, and a caller that did the first and forgot the second would build a
/// catalog nothing ever hits.
///
/// # Errors
///
/// Whatever the backend could not do.
pub async fn record_outputs(
    catalog: &dyn ArtifactCatalog,
    produced_by: super::Provenance,
    inputs: &[ArtifactRef],
    outputs: &[ArtifactRef],
    cache_key: Option<&str>,
    now: OffsetDateTime,
) -> crate::Result<()> {
    let lineage: Vec<String> = inputs
        .iter()
        .map(|artifact| artifact.digest.clone())
        .collect();
    for artifact in outputs {
        catalog
            .record(CatalogedArtifact {
                artifact: artifact.clone(),
                produced_by: Some(produced_by.clone()),
                inputs: lineage.clone(),
                created_at: now,
            })
            .await?;
    }
    // Only after the manifests. An index entry pointing at an artifact nobody
    // described would be a hit that resolves to nothing — `aiwatcher_jobs`'
    // ordering, in the sixth place it applies.
    if let Some(cache_key) = cache_key {
        catalog
            .remember(CacheEntry {
                cache_key: cache_key.to_owned(),
                artifacts: outputs.to_vec(),
                created_at: now,
                expires_at: None,
                invalidated_at: None,
            })
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use aiwatcher_iam::{OrganizationId, ProjectId};
    use aiwatcher_prompts::adapters::memory::MemoryObjectStore;

    use super::super::Provenance;
    use super::*;

    fn catalog() -> ObjectArtifactCatalog {
        ObjectArtifactCatalog::new(Arc::new(MemoryObjectStore::new()))
    }

    fn scope() -> ProjectScope {
        ProjectScope {
            organization: OrganizationId::new(),
            project: ProjectId::new(),
        }
    }

    /// A real content address, because the digest is a path segment: the
    /// catalog refuses to build a key out of anything else.
    fn address(seed: &str) -> String {
        aiwatcher_jobs::digest(seed.as_bytes())
    }

    fn rows_in(prefix: &str, digest: &str) -> ArtifactRef {
        ArtifactRef::new(
            "rows",
            layout::data_uri(prefix, ArtifactKind::Rows, digest),
            digest,
        )
        .of_kind(ArtifactKind::Rows)
    }

    fn rows(digest: &str) -> ArtifactRef {
        rows_in(layout::PREFIX, digest)
    }

    fn made_by(execution: &str, attempt: u32) -> Provenance {
        Provenance {
            execution_id: ExecutionId::new(execution),
            step_id: "read".to_owned(),
            attempt,
        }
    }

    #[tokio::test]
    async fn what_one_run_produced_is_readable_without_walking_the_bucket() {
        // An object store lists by prefix and nothing else, which is why the
        // lineage entry exists at all.
        let catalog = catalog();
        for (execution, digest) in [("exec-1", "aa"), ("exec-1", "bb"), ("exec-2", "cc")] {
            crate::artifact::object::record_outputs(
                &catalog,
                made_by(execution, 1),
                &[],
                &[rows(&address(digest))],
                None,
                OffsetDateTime::UNIX_EPOCH,
            )
            .await
            .expect("a record");
        }

        let mine = catalog
            .produced_by(&ExecutionId::new("exec-1"))
            .await
            .expect("a lineage read");
        assert_eq!(mine.len(), 2);
        assert!(
            mine.iter()
                .all(|a| a.produced_by.as_ref().expect("provenance").execution_id
                    == ExecutionId::new("exec-1"))
        );
    }

    #[tokio::test]
    async fn a_deterministic_rerun_keeps_the_provenance_it_already_had() {
        let catalog = catalog();
        catalog
            .record(CatalogedArtifact {
                artifact: rows(&address("aa")),
                produced_by: Some(made_by("exec-1", 1)),
                inputs: Vec::new(),
                created_at: OffsetDateTime::UNIX_EPOCH,
            })
            .await
            .expect("a record");
        let again = catalog
            .record(CatalogedArtifact {
                artifact: rows(&address("aa")),
                produced_by: Some(made_by("exec-1", 2)),
                inputs: Vec::new(),
                created_at: OffsetDateTime::UNIX_EPOCH,
            })
            .await
            .expect("the same bytes again");
        assert_eq!(
            again.produced_by.expect("provenance").attempt,
            1,
            "the attempt that wrote the bytes keeps them"
        );
    }

    #[tokio::test]
    async fn a_key_resolves_to_what_it_named_and_stops_when_it_is_invalidated() {
        let catalog = catalog();
        crate::artifact::object::record_outputs(
            &catalog,
            made_by("exec-1", 1),
            &[rows(&address("00"))],
            &[rows(&address("aa"))],
            Some("key-1"),
            OffsetDateTime::UNIX_EPOCH,
        )
        .await
        .expect("a record");

        let hit = catalog
            .cached("key-1", OffsetDateTime::UNIX_EPOCH)
            .await
            .expect("a read")
            .expect("a hit");
        assert_eq!(hit.artifacts, vec![rows(&address("aa"))]);
        // And the lineage went down with the manifest, so the run stays
        // explainable from the artifact alone.
        let described = catalog
            .by_digest(&address("aa"), ArtifactKind::Rows)
            .await
            .expect("a read")
            .expect("a manifest");
        assert_eq!(described.inputs, vec![address("00")]);

        catalog
            .invalidate("key-1", OffsetDateTime::UNIX_EPOCH)
            .await
            .expect("an invalidation");
        assert!(
            catalog
                .cached("key-1", OffsetDateTime::UNIX_EPOCH)
                .await
                .expect("a read")
                .is_none()
        );
        assert!(
            catalog
                .by_digest(&address("aa"), ArtifactKind::Rows)
                .await
                .expect("a read")
                .is_some(),
            "invalidation marks an entry and leaves what it named alone"
        );
    }

    #[tokio::test]
    async fn a_record_this_build_cannot_read_is_a_miss_rather_than_a_failure() {
        // The index is droppable by design. A note about a previous run must
        // never be the thing that stops the next one.
        let store = Arc::new(MemoryObjectStore::new());
        let catalog = ObjectArtifactCatalog::new(Arc::clone(&store) as Arc<dyn ObjectStore>);
        store
            .put(
                &catalog.cache_key_path("key-1"),
                br#"{"from":"a newer build"}"#.to_vec(),
            )
            .await
            .expect("a write");

        assert!(
            catalog
                .cached("key-1", OffsetDateTime::UNIX_EPOCH)
                .await
                .expect("a read")
                .is_none()
        );
    }

    #[tokio::test]
    async fn the_deployment_wide_catalog_will_not_describe_a_projects_artifact() {
        // The other half of the byte store's rule: knowing a project's URI is
        // not access to it through the reader that came before projects.
        let catalog = catalog();
        let scoped = rows_in(
            &layout::prefix(Some(scope())),
            &aiwatcher_jobs::digest(b"rows"),
        );
        let refused = catalog
            .record(CatalogedArtifact {
                artifact: scoped.clone(),
                produced_by: Some(made_by("exec-1", 1)),
                inputs: Vec::new(),
                created_at: OffsetDateTime::UNIX_EPOCH,
            })
            .await
            .expect_err("a project's pointer");
        assert!(matches!(refused, StoreError::OutOfScope(_)), "{refused}");
        assert!(refused.says_the_same_next_time());

        let refused = catalog
            .remember(CacheEntry {
                cache_key: "key-1".to_owned(),
                artifacts: vec![scoped],
                created_at: OffsetDateTime::UNIX_EPOCH,
                expires_at: None,
                invalidated_at: None,
            })
            .await
            .expect_err("a project's pointer");
        assert!(matches!(refused, StoreError::OutOfScope(_)), "{refused}");
    }

    #[tokio::test]
    async fn a_digest_that_is_not_a_content_address_never_becomes_a_key() {
        // The digest is a path segment. `../` in one is a read outside the
        // prefix, and a lookup for it is an answer rather than an attempt.
        let catalog = catalog();
        for digest in ["", "aa", "../secret", &"AB".repeat(32)] {
            assert!(
                catalog
                    .by_digest(digest, ArtifactKind::Rows)
                    .await
                    .expect("a read")
                    .is_none(),
                "{digest}"
            );
            let refused = catalog
                .record(CatalogedArtifact {
                    artifact: ArtifactRef::new("rows", format!("object://x/{digest}"), digest)
                        .of_kind(ArtifactKind::Rows),
                    produced_by: None,
                    inputs: Vec::new(),
                    created_at: OffsetDateTime::UNIX_EPOCH,
                })
                .await
                .expect_err("not a content address");
            assert!(matches!(refused, StoreError::OutOfScope(_)), "{refused}");
        }
    }
}
