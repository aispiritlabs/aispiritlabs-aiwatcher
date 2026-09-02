//! The key layout, and the orderings that are contracts.
//!
//! Every slice reads and writes through here. `pub(crate)` on purpose, exactly
//! as `aiwatcher_annotations::store` is: callers get [`Registry`](crate::Registry),
//! which is a vocabulary of archive operations, and this is a vocabulary of
//! keys.
//!
//! ```text
//! conversations/<conversation>/head.json      counts, first and last seen
//! conversations/<conversation>/index/000000.json   the order an export reads
//! conversations/<conversation>/turns/<turn>.json   the plaintext head
//! content/<turn>.json                              the sealed content
//! digests/<content>.json                           which turn said this first
//! subjects/<subject>/<turn>.json                   what an erasure request reads
//! exports/jobs/<job>.json                          the resumable job record
//! exports/shards/<job>/000000.json                 sealed JSONL, the unit of resume
//! exports/manifests/<name>/<version>.json          the immutable manifest
//! exports/index/<name>.json                        which versions exist
//! ```
//!
//! Three orderings are contracts rather than choices, and the first two are the
//! ones the prompt and annotation registries already keep.
//!
//! * **The content before the head that names it.** A head pointing at content
//!   that was never sealed is a turn whose review page 404s.
//! * **The head before the index entry.** An index naming a head that is not
//!   there is a list whose rows fail, and an unindexed head is merely invisible
//!   — which the next write of the same turn repairs, because the head records
//!   whether it was ever indexed.
//! * **The shard before the cursor that passes it.** This is the export's
//!   resumability: a crash between the two rewrites one shard identically, and
//!   a crash the other way round would skip its rows forever.

use std::sync::Arc;

use aiwatcher_core::prompts::ObjectStore;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::archive::crypt::{Keyring, SealedObject};
use crate::{Error, Result, digest};

/// One namespace in the configured authored object store, plus the keys that
/// seal what goes into it.
#[derive(Clone, Debug)]
pub(crate) struct Backend {
    store: Arc<dyn ObjectStore>,
    prefix: String,
    keyring: Keyring,
}

impl Backend {
    pub(crate) fn new(
        store: Arc<dyn ObjectStore>,
        prefix: impl Into<String>,
        keyring: Keyring,
    ) -> Self {
        Self {
            store,
            prefix: prefix.into().trim_matches('/').to_owned(),
            keyring,
        }
    }

    pub(crate) fn keyring(&self) -> &Keyring {
        &self.keyring
    }

    /// A caller-supplied name, as a path segment.
    ///
    /// Hashed rather than escaped. A `conversation_id` comes from a producer
    /// that never heard of this crate and an export name holds slashes by
    /// design, and a key that embedded either would put one conversation's
    /// turns inside another's prefix.
    fn id(value: &str) -> String {
        digest(value.as_bytes())
    }

    // ── Conversations ────────────────────────────────────────────────────

    pub(crate) fn conversation_key(&self, conversation_id: &str) -> String {
        format!(
            "{}/conversations/{}/head.json",
            self.prefix,
            Self::id(conversation_id)
        )
    }

    pub(crate) fn conversations_prefix(&self) -> String {
        format!("{}/conversations/", self.prefix)
    }

    pub(crate) fn index_key(&self, conversation_id: &str, shard: usize) -> String {
        format!(
            "{}/conversations/{}/index/{shard:06}.json",
            self.prefix,
            Self::id(conversation_id)
        )
    }

    pub(crate) fn turn_key(&self, conversation_id: &str, turn_id: &str) -> String {
        format!(
            "{}/conversations/{}/turns/{turn_id}.json",
            self.prefix,
            Self::id(conversation_id)
        )
    }

    pub(crate) fn content_key(&self, turn_id: &str) -> String {
        format!("{}/content/{turn_id}.json", self.prefix)
    }

    /// Where the one turn holding this exact content is remembered, so the
    /// second copy is a finding rather than a second row in a corpus.
    pub(crate) fn digest_key(&self, content_digest: &str) -> String {
        format!("{}/digests/{content_digest}.json", self.prefix)
    }

    pub(crate) fn subject_key(&self, subject: &str, turn_id: &str) -> String {
        format!(
            "{}/subjects/{}/{turn_id}.json",
            self.prefix,
            Self::id(subject)
        )
    }

    pub(crate) fn subject_prefix(&self, subject: &str) -> String {
        format!("{}/subjects/{}/", self.prefix, Self::id(subject))
    }

    // ── Exports ──────────────────────────────────────────────────────────

    pub(crate) fn job_key(&self, job_id: &str) -> String {
        format!("{}/exports/jobs/{job_id}.json", self.prefix)
    }

    pub(crate) fn jobs_prefix(&self) -> String {
        format!("{}/exports/jobs/", self.prefix)
    }

    pub(crate) fn shard_key(&self, job_id: &str, shard: usize) -> String {
        format!("{}/exports/shards/{job_id}/{shard:06}.json", self.prefix)
    }

    pub(crate) fn manifest_key(&self, name: &str, version: &str) -> String {
        format!(
            "{}/exports/manifests/{}/{version}.json",
            self.prefix,
            Self::id(name)
        )
    }

    pub(crate) fn export_index_key(&self, name: &str) -> String {
        format!("{}/exports/index/{}.json", self.prefix, Self::id(name))
    }

    pub(crate) fn export_indexes_prefix(&self) -> String {
        format!("{}/exports/index/", self.prefix)
    }

    // ── Plaintext documents ──────────────────────────────────────────────

    pub(crate) async fn read<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let Some(bytes) = self.store.get(key).await? else {
            return Ok(None);
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| Error::Corrupt {
                key: key.to_owned(),
                message: error.to_string(),
            })
    }

    pub(crate) async fn write<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        let bytes = serde_json::to_vec(value).map_err(|error| Error::Corrupt {
            key: key.to_owned(),
            message: error.to_string(),
        })?;
        self.store.put(key, bytes).await?;
        Ok(())
    }

    pub(crate) async fn delete(&self, key: &str) -> Result<()> {
        self.store.delete(key).await?;
        Ok(())
    }

    pub(crate) async fn list(&self, prefix: &str) -> Result<Vec<aiwatcher_core::ObjectEntry>> {
        Ok(self.store.list(prefix).await?)
    }

    // ── Sealed documents ─────────────────────────────────────────────────

    /// Seal `value` and store it. The key is authenticated, so this object
    /// cannot later be read from another key.
    pub(crate) async fn seal<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        let plaintext = serde_json::to_vec(value).map_err(|error| Error::Corrupt {
            key: key.to_owned(),
            message: error.to_string(),
        })?;
        let sealed = self.keyring.seal(key, &plaintext)?;
        self.write(key, &sealed).await
    }

    /// Seal raw bytes — what an export shard is, since a shard is JSONL rather
    /// than one document.
    pub(crate) async fn seal_bytes(&self, key: &str, plaintext: &[u8]) -> Result<()> {
        let sealed = self.keyring.seal(key, plaintext)?;
        self.write(key, &sealed).await
    }

    pub(crate) async fn open<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let Some(bytes) = self.open_bytes(key).await? else {
            return Ok(None);
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| Error::Corrupt {
                key: key.to_owned(),
                message: error.to_string(),
            })
    }

    pub(crate) async fn open_bytes(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let Some(sealed) = self.read::<SealedObject>(key).await? else {
            return Ok(None);
        };
        Ok(Some(self.keyring.open(key, &sealed)?))
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::archive::crypt::KEY_BYTES;

    fn backend() -> Backend {
        Backend::new(
            Arc::new(aiwatcher_prompts::adapters::memory::MemoryObjectStore::new()),
            "conversations",
            Keyring::single("k", [5u8; KEY_BYTES]),
        )
    }

    #[test]
    fn a_conversation_id_holding_a_slash_cannot_reach_another_prefix() {
        let backend = backend();
        let hostile = backend.turn_key("../../prompts", "a".repeat(64).as_str());
        assert!(!hostile.contains(".."), "{hostile}");
        assert!(
            hostile.starts_with("conversations/conversations/"),
            "{hostile}"
        );
    }

    #[tokio::test]
    async fn sealed_content_is_unreadable_without_going_through_the_keyring() {
        let backend = backend();
        let key = backend.content_key(&"a".repeat(64));
        backend
            .seal(&key, &"the customer said hello")
            .await
            .expect("seals");

        // What the bucket holds.
        let raw: SealedObject = backend.read(&key).await.expect("reads").expect("present");
        assert!(!raw.ciphertext.contains("customer"));

        let opened: String = backend.open(&key).await.expect("opens").expect("present");
        assert_eq!(opened, "the customer said hello");
    }

    #[tokio::test]
    async fn a_missing_object_is_none_rather_than_an_error() {
        let backend = backend();
        let absent: Option<String> = backend
            .open(&backend.content_key(&"b".repeat(64)))
            .await
            .expect("reads");
        assert!(absent.is_none());
    }
}
