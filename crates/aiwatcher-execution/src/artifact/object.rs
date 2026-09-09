//! The catalog over an object store: the manifest beside the bytes, and the
//! cache index beside both.
//!
//! Section 17.1's layout, completed. `aiwatcher-server`'s reactor writes the
//! `data`; this writes the `manifest.json` that says who made it and what it
//! was made from, and the `cache/` entries that let an identical step be
//! answered without running it.
//!
//! ```text
//! artifacts/<kind>/<aa>/<sha256>/data           the bytes        (the reactor)
//! artifacts/<kind>/<aa>/<sha256>/manifest.json  what they are    (here)
//! artifacts/cache/<sha256 of the key>.json      what a key resolved to
//! artifacts/lineage/<execution>/<sha256>.json   what one run produced
//! ```
//!
//! ## Why the lineage entry is a second write
//!
//! An object store lists by prefix and nothing else. "Everything this execution
//! produced" is a question the manifest cannot answer without walking every
//! artifact in the bucket, so it gets a prefix of its own — a pointer, not a
//! copy, so the manifest stays the one description of an artifact.
//!
//! ## Why an unreadable entry is a miss
//!
//! Every read here answers `None` rather than an error when the stored JSON is
//! from a build this one cannot read. The cache is an *index* (section 18): a
//! rerun is the cost of dropping it, and a note about a previous run must never
//! be the thing that stops the next one.

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use time::OffsetDateTime;

use aiwatcher_core::prompts::ObjectStore;
use aiwatcher_core::{ArtifactKind, ArtifactRef};

use super::{ArtifactCatalog, CacheEntry, CatalogedArtifact};
use crate::error::StoreError;
use crate::state::ExecutionId;

/// The prefix everything here shares with the bytes it describes.
///
/// The same string `aiwatcher-server`'s reactor writes under, and deliberately
/// so: a manifest that lived somewhere else would be a second place to look
/// when an artifact is missing.
pub const PREFIX: &str = "artifacts";

/// Artifact metadata, lineage and the cache index, in an object store.
#[derive(Clone, Debug)]
pub struct ObjectArtifactCatalog {
    store: Arc<dyn ObjectStore>,
    prefix: String,
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
            prefix: PREFIX.to_owned(),
        }
    }

    fn manifest_key(&self, digest: &str, kind: ArtifactKind) -> String {
        format!(
            "{}/{}/{}/{digest}/manifest.json",
            self.prefix,
            kind.as_str(),
            digest.get(..2).unwrap_or("00")
        )
    }

    /// The key hashed, because a cache key is a digest of a digest and a
    /// caller's parameters, and only the first of those is guaranteed to be a
    /// path segment.
    fn cache_key_path(&self, cache_key: &str) -> String {
        format!(
            "{}/cache/{}.json",
            self.prefix,
            aiwatcher_jobs::digest(cache_key.as_bytes())
        )
    }

    /// One execution's own prefix. The id is hashed for the reason every other
    /// key here is: it is a caller's string, and a key is a path.
    fn lineage_prefix(&self, execution: &ExecutionId) -> String {
        format!(
            "{}/lineage/{}/",
            self.prefix,
            aiwatcher_jobs::digest(execution.as_str().as_bytes())
        )
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
}

#[async_trait]
impl ArtifactCatalog for ObjectArtifactCatalog {
    async fn record(&self, artifact: CatalogedArtifact) -> crate::Result<CatalogedArtifact> {
        let key = self.manifest_key(&artifact.artifact.digest, artifact.artifact.kind);
        // Idempotent by digest: two deterministic attempts over the same inputs
        // write byte-identical output, and a catalog that counted them twice
        // would report a rerun as a second artifact and a lineage that forked
        // where nothing forked. The *first* provenance wins, because it is the
        // one that was true.
        if let Some(held) = self.read::<CatalogedArtifact>(&key).await? {
            return Ok(held);
        }
        self.write(&key, &artifact).await?;
        if let Some(made) = &artifact.produced_by {
            self.write(
                &format!(
                    "{}{}.json",
                    self.lineage_prefix(&made.execution_id),
                    artifact.artifact.digest
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
        self.read(&self.manifest_key(digest, kind)).await
    }

    async fn produced_by(&self, execution: &ExecutionId) -> crate::Result<Vec<CatalogedArtifact>> {
        let entries = self
            .store
            .list(&self.lineage_prefix(execution))
            .await
            .map_err(|error| StoreError::Backend(error.to_string()))?;
        let mut found = Vec::new();
        for entry in entries {
            let Some(pointer) = self.read::<LineageEntry>(&entry.key).await? else {
                continue;
            };
            if let Some(artifact) = self.by_digest(&pointer.digest, pointer.kind).await? {
                found.push(artifact);
            }
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
            .read::<CacheEntry>(&self.cache_key_path(cache_key))
            .await?
            .filter(|entry| entry.is_usable(now)))
    }

    async fn remember(&self, entry: CacheEntry) -> crate::Result<()> {
        let key = self.cache_key_path(&entry.cache_key);
        self.write(&key, &entry).await
    }

    async fn invalidate(&self, cache_key: &str, at: OffsetDateTime) -> crate::Result<()> {
        let key = self.cache_key_path(cache_key);
        // Marked, never deleted, and the artifacts it names are left alone: an
        // old execution that used them is a record of what happened, and
        // rewriting it would be a different kind of lie.
        let Some(entry) = self.read::<CacheEntry>(&key).await? else {
            return Ok(());
        };
        self.write(
            &key,
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
    use aiwatcher_prompts::adapters::memory::MemoryObjectStore;

    use super::super::Provenance;
    use super::*;

    fn catalog() -> ObjectArtifactCatalog {
        ObjectArtifactCatalog::new(Arc::new(MemoryObjectStore::new()))
    }

    fn rows(digest: &str) -> ArtifactRef {
        ArtifactRef::new("rows", format!("object://artifacts/{digest}"), digest)
            .of_kind(ArtifactKind::Rows)
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
                &[rows(digest)],
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
                artifact: rows("aa"),
                produced_by: Some(made_by("exec-1", 1)),
                inputs: Vec::new(),
                created_at: OffsetDateTime::UNIX_EPOCH,
            })
            .await
            .expect("a record");
        let again = catalog
            .record(CatalogedArtifact {
                artifact: rows("aa"),
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
            &[rows("00")],
            &[rows("aa")],
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
        assert_eq!(hit.artifacts, vec![rows("aa")]);
        // And the lineage went down with the manifest, so the run stays
        // explainable from the artifact alone.
        let described = catalog
            .by_digest("aa", ArtifactKind::Rows)
            .await
            .expect("a read")
            .expect("a manifest");
        assert_eq!(described.inputs, vec!["00".to_owned()]);

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
                .by_digest("aa", ArtifactKind::Rows)
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
}
