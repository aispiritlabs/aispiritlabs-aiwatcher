//! The staged artifact: rows written to the object store in pages, before
//! anything looks at them.
//!
//! The synchronous import (`images::import`) takes every row in one request
//! body, and the cap that makes that safe — five thousand rows — is not a cap
//! a corpus fits inside. Making the body bigger only moves the number: the
//! request still has to be held open, retried whole, and kept in one process's
//! memory, and a network blip at row 900 000 loses the lot.
//!
//! So a batch is staged first. A page of rows is appended, hashed and stored;
//! the batch manifest is updated *after* the page it names — the same ordering
//! as every other staged write in this workspace ([`aiwatcher_jobs::ORDERING`])
//! — and a crash between the two costs one re-sent page rather than a corpus.
//!
//! Two properties are worth stating because they are what the import job then
//! relies on.
//!
//! **A page is idempotent when the caller numbers it.** An `append` that
//! carries a `page` already written is compared by digest: identical bytes are
//! an acknowledged retry, different bytes are a refusal naming the page. A
//! client streaming a million rows over a flaky link needs to be able to
//! re-send without wondering whether it duplicated, and a client that changed
//! its mind about page 12 needs to be told rather than silently agreed with.
//!
//! **Sealing is what makes a batch a thing.** A sealed batch has a digest over
//! its page digests, in order, and takes no more rows. That digest is what the
//! import job's version is built from, which is what makes "re-run the same
//! pinned source and pipeline" a reference somebody can compare rather than a
//! promise.

use aiwatcher_jobs::{ShardRef, version_of};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;

use crate::images::import::{ImportRow, ImportSource};
use crate::license::{RightsEvidence, UsageRights};
use crate::store::Backend;
use crate::{Error, Result, digest, validate_digest, validate_name};

/// Rows one page may carry, and therefore one request body.
///
/// The same number the synchronous import allows, deliberately: a client that
/// already builds a 5 000-row body keeps building it, and the only thing that
/// changes is that there may now be two hundred of them.
pub const MAX_PAGE_ROWS: usize = 5_000;

/// Pages one batch may hold. Five million rows, which is a corpus rather than
/// a catalogue, and still a bounded number of objects to list.
pub const MAX_BATCH_PAGES: usize = 1_000;

/// What a batch is for, decided once and pinned.
///
/// Rights, evidence and source sit here rather than on each page because they
/// are properties of the *corpus*: a per-page field would invite a pipeline to
/// derive them from a column, which is the mirror's word laundered through a
/// `withEntry`. This is also the "pinned source" half of "re-running the same
/// pinned source and pipeline yields the same version".
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StageBatchRequest {
    pub project: String,
    #[serde(default)]
    pub description: String,
    /// What the caller asserts may be done with every image in this batch.
    #[serde(default = "unknown_rights")]
    pub rights: UsageRights,
    /// Who checked that, where and when. Recorded, never enforced.
    #[serde(default)]
    pub evidence: RightsEvidence,
    #[serde(default)]
    pub source: ImportSource,
}

const fn unknown_rights() -> UsageRights {
    UsageRights::Unknown
}

impl StageBatchRequest {
    /// # Errors
    /// [`Error::Invalid`] for an unusable project name.
    pub fn validate(&self) -> Result<()> {
        validate_name(&self.project, "a project")
    }

    /// The identity of "what this batch is for", independent of its rows.
    #[must_use]
    pub fn digest(&self) -> String {
        digest(&serde_json::to_vec(self).unwrap_or_default())
    }
}

/// A batch as it sits in the store.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct StagedBatch {
    pub batch_id: String,
    pub project: String,
    pub description: String,
    pub rights: UsageRights,
    pub evidence: RightsEvidence,
    pub source: ImportSource,
    /// What was asked for, hashed. Half of the batch's content address.
    pub request_digest: String,
    /// One entry per stored page, in order. The same [`ShardRef`] the
    /// conversation export writes, because it is the same thing: an ordered,
    /// digested piece of a staged artifact.
    pub pages: Vec<ShardRef>,
    pub rows: usize,
    /// Pages on which no two rows shared a `group_id`.
    ///
    /// The family split is the one mistake an import cannot detect afterwards:
    /// a `group_id` mapped from the file name gives every image its own
    /// family, the test score then measures memorisation, and nothing in the
    /// numbers says so. Counted per page rather than over the whole batch
    /// because the alternative is a set of every group id a million-row import
    /// has seen, held in a manifest — so what this supports is the exact
    /// statement "every page of this batch gave each row its own family",
    /// which is what a filename mapping produces and what a real one does not.
    pub singleton_pages: usize,
    /// True once the batch takes no more rows. Set by queueing a job.
    pub sealed: bool,
    /// `sha256(request ‖ every page digest, in order)`, once sealed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub created_by: String,
    #[serde(with = "time::serde::rfc3339")]
    #[schema(value_type = String)]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    #[schema(value_type = String)]
    pub updated_at: OffsetDateTime,
}

impl StagedBatch {
    /// Whether every page of this batch gave each row its own family.
    ///
    /// See [`singleton_pages`](Self::singleton_pages) for why the statement is
    /// about pages rather than about the batch.
    #[must_use]
    pub const fn every_page_is_singletons(&self) -> bool {
        self.rows > 1 && !self.pages.is_empty() && self.singleton_pages == self.pages.len()
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct BatchPage {
    pub batches: Vec<StagedBatch>,
}

/// A page of rows for an already-staged batch.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AppendRowsRequest {
    pub batch: String,
    /// Which page this is, when the caller is numbering them.
    ///
    /// Supplying it makes the append idempotent, which is the difference
    /// between a client that can retry and one that has to reconcile. Omitting
    /// it appends at the end, which is what a single-threaded uploader wants.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<usize>,
    pub rows: Vec<ImportRow>,
}

/// What one append did.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct AppendReport {
    pub batch_id: String,
    pub page: usize,
    pub rows: usize,
    /// Rows in the whole batch so far.
    pub total_rows: usize,
    pub digest: String,
    /// False when this exact page was already stored — an acknowledged retry
    /// rather than a second copy of the rows.
    pub created: bool,
}

// ── Operations ───────────────────────────────────────────────────────────────

/// Open a batch. Nothing is read and nothing is registered yet.
pub(crate) async fn stage(
    backend: &Backend,
    request: StageBatchRequest,
    created_by: &str,
) -> Result<StagedBatch> {
    request.validate()?;
    let now = OffsetDateTime::now_utc();
    let request_digest = request.digest();
    // Not derived from the request alone: two imports of the same corpus into
    // the same project on the same terms are two batches, and joining them
    // would interleave one uploader's pages with another's. The *job* is where
    // determinism belongs, and it gets it from the sealed content.
    let batch_id = digest(
        format!(
            "{request_digest}\0{}\0{}",
            now.unix_timestamp_nanos(),
            std::process::id()
        )
        .as_bytes(),
    );
    let batch = StagedBatch {
        batch_id,
        project: request.project,
        description: request.description,
        rights: request.rights,
        evidence: request.evidence,
        source: request.source,
        request_digest,
        pages: Vec::new(),
        rows: 0,
        singleton_pages: 0,
        sealed: false,
        digest: None,
        created_by: created_by.to_owned(),
        created_at: now,
        updated_at: now,
    };
    backend
        .write_json(&backend.batch_key(&batch.batch_id), &batch)
        .await?;
    Ok(batch)
}

pub(crate) async fn batch(backend: &Backend, batch_id: &str) -> Result<StagedBatch> {
    validate_digest(batch_id, "a batch id")?;
    backend
        .read_json(&backend.batch_key(batch_id))
        .await?
        .ok_or_else(|| Error::NotFound(format!("the import batch {batch_id}")))
}

pub(crate) async fn batches(backend: &Backend) -> Result<BatchPage> {
    let mut batches = Vec::new();
    for key in backend.keys(&backend.batches_prefix()).await? {
        if !key.ends_with("/manifest.json") {
            continue;
        }
        if let Some(found) = backend.read_json::<StagedBatch>(&key).await? {
            batches.push(found);
        }
    }
    batches.sort_by_key(|batch| std::cmp::Reverse(batch.created_at));
    Ok(BatchPage { batches })
}

/// Store one page, then the manifest that names it. Never the other way round.
pub(crate) async fn append(
    backend: &Backend,
    request: AppendRowsRequest,
) -> Result<(StagedBatch, AppendReport)> {
    if request.rows.len() > MAX_PAGE_ROWS {
        return Err(Error::TooLarge {
            what: "an import page",
            size: request.rows.len(),
            limit: MAX_PAGE_ROWS,
        });
    }
    if request.rows.is_empty() {
        return Err(Error::Invalid(
            "an empty page adds nothing and would still take a page number".to_owned(),
        ));
    }
    let mut found = batch(backend, &request.batch).await?;
    if found.sealed {
        return Err(Error::Invalid(format!(
            "the import batch {} is sealed; its rows are pinned and a job is reading them",
            found.batch_id
        )));
    }
    if found.pages.len() >= MAX_BATCH_PAGES {
        return Err(Error::TooLarge {
            what: "an import batch",
            size: found.pages.len() + 1,
            limit: MAX_BATCH_PAGES,
        });
    }

    let bytes = encode(&request.rows, &found.batch_id)?;
    let page_digest = digest(&bytes);
    let index = request.page.unwrap_or(found.pages.len());

    // An idempotent retry, an out-of-order page and a changed mind are three
    // different answers. Returning "fine" to the third would let a client
    // rewrite a page a job has already read.
    if let Some(existing) = found.pages.get(index) {
        if existing.digest == page_digest {
            let report = AppendReport {
                batch_id: found.batch_id.clone(),
                page: index,
                rows: request.rows.len(),
                total_rows: found.rows,
                digest: page_digest,
                created: false,
            };
            return Ok((found, report));
        }
        return Err(Error::Invalid(format!(
            "page {index} of this batch was already stored with different rows. A page is \
             immutable once written, because a job may already have read it"
        )));
    }
    if index != found.pages.len() {
        return Err(Error::Invalid(format!(
            "this batch has {} pages, so the next one is {}; page {index} would leave a gap and \
             the page order is the row order",
            found.pages.len(),
            found.pages.len()
        )));
    }

    backend
        .put_bytes(&backend.batch_page_key(&found.batch_id, index), bytes)
        .await?;

    let families = crate::images::import::families(&request.rows);
    found.pages.push(ShardRef {
        index,
        rows: request.rows.len(),
        digest: page_digest.clone(),
    });
    found.rows += request.rows.len();
    if families == request.rows.len() {
        found.singleton_pages += 1;
    }
    found.updated_at = OffsetDateTime::now_utc();
    backend
        .write_json(&backend.batch_key(&found.batch_id), &found)
        .await?;

    let report = AppendReport {
        batch_id: found.batch_id.clone(),
        page: index,
        rows: request.rows.len(),
        total_rows: found.rows,
        digest: page_digest,
        created: true,
    };
    Ok((found, report))
}

/// Close a batch and give it its content address.
///
/// Idempotent: sealing a sealed batch returns it, so a retried queue request
/// joins the job it already started rather than being refused.
pub(crate) async fn seal(backend: &Backend, batch_id: &str) -> Result<StagedBatch> {
    let mut found = batch(backend, batch_id).await?;
    if found.sealed {
        return Ok(found);
    }
    if found.pages.is_empty() {
        return Err(Error::Invalid(
            "this batch has no rows; an import of nothing is not an import".to_owned(),
        ));
    }
    found.sealed = true;
    found.digest = Some(version_of(&found.request_digest, &found.pages));
    found.updated_at = OffsetDateTime::now_utc();
    backend
        .write_json(&backend.batch_key(&found.batch_id), &found)
        .await?;
    Ok(found)
}

/// One page's rows, as they were staged.
pub(crate) async fn page(
    backend: &Backend,
    batch: &StagedBatch,
    index: usize,
) -> Result<Vec<ImportRow>> {
    let key = backend.batch_page_key(&batch.batch_id, index);
    let bytes = backend
        .get_bytes(&key)
        .await?
        .ok_or_else(|| Error::Corrupt {
            key: key.clone(),
            message: "the batch names a page that is not stored".to_owned(),
        })?;
    // Verified rather than trusted, and this is the one read where that is
    // worth the hash: the page digest is what the import version is built
    // from, so a page that changed under the job would produce a reference
    // describing rows nobody imported.
    let found = digest(&bytes);
    let expected = batch.pages.get(index).map(|shard| shard.digest.as_str());
    if expected.is_some_and(|expected| expected != found) {
        return Err(Error::Corrupt {
            key,
            message: format!(
                "page {index} hashes to {found} and the batch says {}",
                expected.unwrap_or_default()
            ),
        });
    }
    decode(&bytes, &key)
}

fn encode(rows: &[ImportRow], batch_id: &str) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    for row in rows {
        bytes.extend_from_slice(&serde_json::to_vec(row).map_err(|error| Error::Corrupt {
            key: batch_id.to_owned(),
            message: error.to_string(),
        })?);
        bytes.push(b'\n');
    }
    Ok(bytes)
}

fn decode(bytes: &[u8], key: &str) -> Result<Vec<ImportRow>> {
    let text = std::str::from_utf8(bytes).map_err(|error| Error::Corrupt {
        key: key.to_owned(),
        message: error.to_string(),
    })?;
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).map_err(|error| Error::Corrupt {
                key: key.to_owned(),
                message: error.to_string(),
            })
        })
        .collect()
}
