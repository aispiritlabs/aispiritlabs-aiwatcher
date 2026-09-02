//! The key layout, and the two orderings that are contracts.
//!
//! Every slice in this crate reads and writes through here, which is the point:
//! the shape of the store is one decision in one place, and a slice that
//! invented its own key would be a slice whose objects `Registry::rebuild`
//! cannot find.
//!
//! Two orderings are contracts rather than choices, and both are the ones the
//! prompt registry keeps: **the revision object is written before the head
//! that indexes it**, and **the export manifest before the index entry that
//! lists it**. An index naming an object that was never stored is a list whose
//! rows 404; an unindexed object is merely waiting to be found again.

use std::sync::Arc;

use aiwatcher_core::prompts::ObjectStore;
use serde::{Deserialize, Serialize};

use crate::{Error, Result, digest};

/// One namespace in the configured authored object store.
///
/// `pub(crate)` on purpose. Callers get [`Registry`](crate::Registry), which is
/// a vocabulary of annotation operations; this is a vocabulary of keys, and
/// exposing it would let a consumer write objects the registry's own reads
/// never look for.
#[derive(Clone, Debug)]
pub(crate) struct Backend {
    store: Arc<dyn ObjectStore>,
    prefix: String,
}

impl Backend {
    pub(crate) fn new(store: Arc<dyn ObjectStore>, prefix: impl Into<String>) -> Self {
        Self {
            store,
            prefix: prefix.into().trim_matches('/').to_owned(),
        }
    }

    /// A project's name, as a path segment.
    ///
    /// Hashed rather than escaped, because a project name may hold slashes
    /// (`floor-plans/dom-projekt`) and a key that embedded them would put one
    /// project's images inside another's prefix.
    fn id(name: &str) -> String {
        digest(name.as_bytes())
    }

    pub(crate) fn project_key(&self, name: &str) -> String {
        format!("{}/projects/{}/head.json", self.prefix, Self::id(name))
    }

    pub(crate) fn projects_prefix(&self) -> String {
        format!("{}/projects/", self.prefix)
    }

    pub(crate) fn image_key(&self, project: &str, image_id: &str) -> String {
        format!(
            "{}/projects/{}/images/{image_id}.json",
            self.prefix,
            Self::id(project)
        )
    }

    pub(crate) fn images_prefix(&self, project: &str) -> String {
        format!("{}/projects/{}/images/", self.prefix, Self::id(project))
    }

    pub(crate) fn revision_key(&self, project: &str, image_id: &str, revision: &str) -> String {
        format!(
            "{}/projects/{}/revisions/{image_id}/{revision}.json",
            self.prefix,
            Self::id(project)
        )
    }

    pub(crate) fn export_key(&self, project: &str, export: &str) -> String {
        format!(
            "{}/projects/{}/exports/{export}.json",
            self.prefix,
            Self::id(project)
        )
    }

    pub(crate) fn export_index_key(&self, project: &str) -> String {
        format!(
            "{}/projects/{}/exports/index.json",
            self.prefix,
            Self::id(project)
        )
    }

    // ── Staged imports ───────────────────────────────────────────────────
    //
    // Outside the per-project prefix on purpose. A batch and its job are
    // *about* a project and are not part of it: they hold rows that may never
    // become images, they outlive the import as a receipt, and a project
    // listing that had to skip them would be a listing with a filter in it.

    pub(crate) fn batch_key(&self, batch_id: &str) -> String {
        format!("{}/imports/batches/{batch_id}/manifest.json", self.prefix)
    }

    pub(crate) fn batches_prefix(&self) -> String {
        format!("{}/imports/batches/", self.prefix)
    }

    /// Zero-padded, because these are listed and a store lists them as
    /// strings: `page-10` sorts before `page-9` and the shard order *is* the
    /// row order.
    pub(crate) fn batch_page_key(&self, batch_id: &str, page: usize) -> String {
        format!(
            "{}/imports/batches/{batch_id}/pages/{page:06}.jsonl",
            self.prefix
        )
    }

    pub(crate) fn import_job_key(&self, job_id: &str) -> String {
        format!("{}/imports/jobs/{job_id}/job.json", self.prefix)
    }

    pub(crate) fn import_jobs_prefix(&self) -> String {
        format!("{}/imports/jobs/", self.prefix)
    }

    pub(crate) fn import_result_key(&self, job_id: &str, page: usize) -> String {
        format!(
            "{}/imports/jobs/{job_id}/results/{page:06}.jsonl",
            self.prefix
        )
    }

    pub(crate) fn import_reject_key(&self, job_id: &str, page: usize) -> String {
        format!(
            "{}/imports/jobs/{job_id}/rejects/{page:06}.jsonl",
            self.prefix
        )
    }

    pub(crate) fn import_manifest_key(&self, version: &str) -> String {
        format!("{}/imports/manifests/{version}.json", self.prefix)
    }

    pub(crate) fn import_index_key(&self) -> String {
        format!("{}/imports/manifests/index.json", self.prefix)
    }

    /// Blobs are keyed by content and shared across projects: the same plan
    /// registered into two projects is one copy of the bytes.
    pub(crate) fn blob_key(&self, image_id: &str) -> String {
        format!("{}/blobs/{image_id}", self.prefix)
    }

    pub(crate) fn blob_meta_key(&self, image_id: &str) -> String {
        format!("{}/blobs/{image_id}.meta.json", self.prefix)
    }

    pub(crate) async fn read_json<T: for<'de> Deserialize<'de>>(
        &self,
        key: &str,
    ) -> Result<Option<T>> {
        let Some(body) = self.store.get(key).await? else {
            return Ok(None);
        };
        serde_json::from_slice(&body)
            .map(Some)
            .map_err(|error| Error::Corrupt {
                key: key.to_owned(),
                message: error.to_string(),
            })
    }

    pub(crate) async fn write_json<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        let body = serde_json::to_vec(value).map_err(|error| Error::Corrupt {
            key: key.to_owned(),
            message: error.to_string(),
        })?;
        self.store.put(key, body).await?;
        Ok(())
    }

    pub(crate) async fn get_bytes(&self, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.store.get(key).await?)
    }

    pub(crate) async fn put_bytes(&self, key: &str, body: Vec<u8>) -> Result<()> {
        self.store.put(key, body).await?;
        Ok(())
    }

    pub(crate) async fn keys(&self, prefix: &str) -> Result<Vec<String>> {
        Ok(self
            .store
            .list(prefix)
            .await?
            .into_iter()
            .map(|entry| entry.key)
            .collect())
    }
}

/// Enough of a content type to serve the bytes back correctly.
///
/// Deliberately tiny: the browser sent a type, this is the fallback for an
/// import that did not, and guessing wrong costs a broken `<img>` rather than
/// anything else.
pub(crate) fn sniff(body: &[u8]) -> &'static str {
    match body {
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [0xFF, 0xD8, 0xFF, ..] => "image/jpeg",
        [b'G', b'I', b'F', b'8', ..] => "image/gif",
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => "image/webp",
        body if body.starts_with(b"<?xml") || body.starts_with(b"<svg") => "image/svg+xml",
        _ => "application/octet-stream",
    }
}
