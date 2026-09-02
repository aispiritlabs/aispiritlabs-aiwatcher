//! Registering a staged batch, a page at a time, in a job that survives the
//! process that started it.
//!
//! The second caller of [`aiwatcher_jobs`], and the reason that crate exists:
//! the conversation archive's export was the first, and
//! [plan.md](../../../../plan.md) said to decide whether the machinery becomes
//! shared *before* writing this one rather than after. What is shared is the
//! rules — lease, retry budget, shard-before-cursor, the content address a
//! finished job is named by. What is not is the record, because an export
//! counts exclusions by policy reason and an import counts rejected rows by
//! what was wrong with them, and those are different questions.
//!
//! ```text
//!  stage ──► append page ──► append page ──► … ──► queue
//!             │                │                    │  seals the batch
//!             ▼                ▼                    ▼
//!          pages/000000.jsonl  pages/000001.jsonl   job
//!                                                    │  one page at a time
//!                                          ┌─────────┴─────────┐
//!                                          ▼                   ▼
//!                                 results/000000.jsonl   rejects/000000.jsonl
//!                                 what was registered     what was not, and why
//!                                          └─────────┬─────────┘
//!                                                    ▼
//!                                          manifest  project@sha256
//! ```
//!
//! Four things are worth reading rather than skipping.
//!
//! **A page is the unit of resume**, so a killed process re-does at most one
//! page. That is only safe because registering an image is idempotent: an
//! image id is the content address of its bytes, and re-registering keeps the
//! revisions and the review state. An import job could not resume at all if
//! that were not true.
//!
//! **The bytes go through [`fetch`](crate::integrations::fetch).** Every gate —
//! host allowlist, public-address check, no redirects, a streamed byte
//! ceiling, a header-only pixel ceiling, a verified content address — is
//! applied to every row, in the job, rather than at the edge. A fetcher wired
//! into one route and not the other is the fetcher somebody routes around.
//!
//! **A rejected row is written down, not returned.** Six hundred thousand rows
//! do not fit in a response, so the counts go on the job and the rows go into
//! their own shards, paged. "Progress and rejected-row reasons are observable
//! without reading the whole artifact" is an acceptance criterion, and this is
//! what satisfies it.
//!
//! **An interrupted job has no manifest**, and therefore no version. The
//! images it registered stay registered — they are content-addressed, and
//! re-running writes the same ones — but nothing indexes a corpus somebody
//! stopped halfway.

pub mod staging;

use std::collections::BTreeMap;

use aiwatcher_jobs::{JobState, ShardRef, after_failure, version_of};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;

use crate::images::import::{ImportRow, ImportSource, to_request, warnings};
use crate::images::{RegisterImageRequest, import::ImportRequest};
use crate::integrations::fetch::ImageSource;
use crate::license::{RightsEvidence, UsageRights, check_rights};
use crate::project::AnnotationProject;
use crate::store::Backend;
use crate::{Error, Result, digest, validate_digest};

use staging::StagedBatch;

/// Times one batch may be imported before the ids run out.
///
/// A batch is immutable once sealed, so a second job over it is a *retry* of
/// something that failed rather than a re-read of changed data — a hundred is
/// far more than anybody needs and still a bound.
const MAX_GENERATIONS: u32 = 100;

/// Rejected rows kept as rows, per job.
///
/// The counts are complete. This is the shard sample: an import that rejected
/// four hundred thousand rows for one reason has said everything useful in the
/// first thousand, and writing the rest is a dead-letter file nobody opens and
/// everybody pays to store.
pub const MAX_REJECT_ROWS: usize = 10_000;

/// Rejected rows one read returns.
pub const MAX_REJECT_PAGE: usize = 200;

/// Why a row did not become an image.
///
/// Coarser than the message, on purpose: a reason with a row count is what
/// somebody reads first, and "38 000 rows had no readable picture at the
/// address they named" points at a mapping mistake in a way that thirty-eight
/// thousand distinct sentences do not.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    /// The address was refused before anything was downloaded: not https, not
    /// an allowlisted host, or resolving inside the network.
    AddressRefused,
    /// The download failed, timed out, or answered something that was not a
    /// success.
    Unreachable,
    /// What came back was not a picture, was too large, or did not hash to
    /// what the row claimed.
    NotAnImage,
    /// The registry refused the row: a missing content address, a bad name, a
    /// zero dimension.
    Invalid,
    /// The object store could not be written. Unlike the others, this one is
    /// about *here* rather than about the row.
    StoreFailed,
}

impl RejectReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AddressRefused => "address_refused",
            Self::Unreachable => "unreachable",
            Self::NotAnImage => "not_an_image",
            Self::Invalid => "invalid",
            Self::StoreFailed => "store_failed",
        }
    }

    /// Which sentence a fetcher produced.
    ///
    /// String matching, because [`ImageSource`] is a port returning a
    /// human-readable refusal rather than a typed error — deliberately, since
    /// its whole job is to say precisely what was wrong with one address. The
    /// classification is a convenience for the counts; the sentence is the
    /// record, and it is kept verbatim on the row.
    fn of_fetch(message: &str) -> Self {
        if message.contains("is not a host this instance may fetch from")
            || message.contains("only https is fetched")
            || message.contains("credentials in its authority")
            || message.contains("not a public address")
            || message.contains("is not an address this instance fetches")
        {
            Self::AddressRefused
        } else if message.contains("not a picture")
            || message.contains("the limit is")
            || message.contains("are not the bytes that row is about")
            || message.contains("zero dimension")
        {
            Self::NotAnImage
        } else {
            Self::Unreachable
        }
    }
}

/// One row that did not make it, kept whole.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct RejectedRow {
    pub page: usize,
    pub uri: String,
    pub group_id: String,
    pub reason: RejectReason,
    /// The sentence, verbatim. A count says how many; this says what to fix.
    pub detail: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct RejectPage {
    pub rows: Vec<RejectedRow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<usize>,
    /// Complete, even when [`rows`](Self::rows) is a page of a sample.
    pub total: usize,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct ImportCounts {
    pub rows_considered: usize,
    pub accepted: usize,
    pub rejected: usize,
    /// Rows whose bytes this job downloaded and stored.
    pub fetched: usize,
    /// Bytes stored. The number that answers "why is the bucket bigger".
    pub bytes_stored: u64,
}

/// What a job was asked to do.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportJobRequest {
    /// The staged batch to read. Sealed by queueing, so its rows are pinned.
    pub batch: String,
    /// Check every row and register nothing.
    ///
    /// It still downloads: a row with no content address is refused by the
    /// registry, so a dry run that skipped the fetch would reject every row
    /// and teach the reader nothing about the batch. Blobs are addressed by
    /// their content, so a dry run followed by a real import stores each
    /// picture once.
    #[serde(default)]
    pub dry_run: bool,
}

/// A resumable import, as it sits in the store.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct ImportJob {
    pub job_id: String,
    pub batch_id: String,
    pub project: String,
    pub request: ImportJobRequest,
    pub request_digest: String,
    pub state: JobState,
    /// Pages this job will read, pinned when it was created.
    pub pages: usize,
    /// Rows across those pages. The denominator a progress bar may use,
    /// because it was counted rather than guessed.
    pub rows: usize,
    /// Pages already registered *and* whose result shard is stored. The resume
    /// point.
    pub cursor: usize,
    pub attempts: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub claimed_by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "time::serde::rfc3339::option")]
    #[schema(value_type = Option<String>)]
    pub claimed_at: Option<OffsetDateTime>,
    pub counts: ImportCounts,
    /// Complete, keyed by [`RejectReason::as_str`].
    pub rejects: BTreeMap<String, usize>,
    /// Pages that wrote a reject shard, so reading them is a seek rather than
    /// a scan of every page of a million-row import.
    pub reject_pages: Vec<usize>,
    /// One per finished page: what was registered, hashed. The version
    /// material.
    pub shards: Vec<ShardRef>,
    /// `project@version`, once there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Things that are not errors and that somebody has to read anyway.
    ///
    /// Always present, even when empty: a client that has to distinguish
    /// "no warnings" from "the field is missing" is a client with a bug
    /// waiting in it.
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub created_by: String,
    #[serde(with = "time::serde::rfc3339")]
    #[schema(value_type = String)]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    #[schema(value_type = String)]
    pub updated_at: OffsetDateTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "time::serde::rfc3339::option")]
    #[schema(value_type = Option<String>)]
    pub finished_at: Option<OffsetDateTime>,
}

impl ImportJob {
    #[must_use]
    pub fn lease_expired(&self, now: OffsetDateTime) -> bool {
        aiwatcher_jobs::lease_expired(self.claimed_at, now)
    }

    /// How much of the batch has been read, as a fraction.
    ///
    /// Never `None` in practice — a job is refused without pages — and never a
    /// guess: the denominator was pinned when the batch was sealed.
    #[must_use]
    pub fn progress(&self) -> Option<f64> {
        aiwatcher_jobs::progress(self.cursor, self.pages)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct ImportJobPage {
    pub jobs: Vec<ImportJob>,
}

/// What a finished import is, forever.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct ImportManifest {
    pub project: String,
    pub version: String,
    pub job_id: String,
    pub batch_id: String,
    /// The batch's own content address. Two manifests naming the same one read
    /// the same rows.
    pub batch_digest: String,
    pub rights: UsageRights,
    #[serde(default)]
    pub evidence: RightsEvidence,
    #[serde(default)]
    pub source: ImportSource,
    pub counts: ImportCounts,
    pub rejects: BTreeMap<String, usize>,
    pub shards: Vec<ShardRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    #[serde(with = "time::serde::rfc3339")]
    #[schema(value_type = String)]
    pub built_at: OffsetDateTime,
}

impl ImportManifest {
    /// `project@version`, the reference an image's provenance can be traced to.
    #[must_use]
    pub fn reference(&self) -> String {
        format!("{}@{}", self.project, self.version)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct ImportIndex {
    #[serde(default)]
    pub imports: Vec<ImportSummary>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct ImportSummary {
    pub project: String,
    pub version: String,
    pub job_id: String,
    pub accepted: usize,
    pub rejected: usize,
    #[serde(with = "time::serde::rfc3339")]
    #[schema(value_type = String)]
    pub built_at: OffsetDateTime,
}

// ── Operations ───────────────────────────────────────────────────────────────

/// Seal the batch and queue a job over it.
///
/// The rights check runs *here* rather than per row, and before anything is
/// registered: a claim contradicting what a human recorded about the corpus is
/// a decision to reverse, not a row to skip.
pub(crate) async fn create(
    backend: &Backend,
    project: &AnnotationProject,
    request: ImportJobRequest,
    created_by: &str,
) -> Result<ImportJob> {
    validate_digest(&request.batch, "a batch id")?;
    let batch = staging::batch(backend, &request.batch).await?;
    if batch.project != project.name {
        return Err(Error::Invalid(format!(
            "batch {} was staged for the project {}, not {}",
            batch.batch_id, batch.project, project.name
        )));
    }
    check_rights(
        &batch.rights,
        batch.source.curated_usage,
        batch.source.curated_source.as_deref(),
    )
    .map_err(Error::Invalid)?;

    let batch = staging::seal(backend, &request.batch).await?;
    let batch_digest = batch.digest.clone().unwrap_or_default();
    let now = OffsetDateTime::now_utc();
    // The batch's *content* address, never its id. Two people who staged the
    // same rows on the same terms staged the same corpus, and an import
    // version that changed because somebody clicked twice would not be a
    // content address of anything — which is the whole property a training
    // run relies on when it names a dataset.
    let request_digest = digest(format!("{batch_digest}\0{}", request.dry_run).as_bytes());

    // The job id is per *batch*, because two batches are two things to run
    // even when they hold identical rows. Derived and generational exactly as
    // an export's is: a retried POST joins the job it already started, and
    // asking again after one finished starts a new one rather than handing
    // back a stale receipt.
    let mut job_id = String::new();
    for generation in 0..MAX_GENERATIONS {
        job_id = digest(format!("{request_digest}\0{}\0{generation}", batch.batch_id).as_bytes());
        match backend
            .read_json::<ImportJob>(&backend.import_job_key(&job_id))
            .await?
        {
            Some(existing) if !existing.state.is_finished() => return Ok(existing),
            Some(_) => continue,
            None => break,
        }
    }

    let job = ImportJob {
        job_id,
        batch_id: batch.batch_id.clone(),
        project: project.name.clone(),
        request,
        request_digest,
        state: JobState::Queued,
        pages: batch.pages.len(),
        rows: batch.rows,
        cursor: 0,
        attempts: 0,
        claimed_by: String::new(),
        claimed_at: None,
        counts: ImportCounts::default(),
        rejects: BTreeMap::new(),
        reject_pages: Vec::new(),
        shards: Vec::new(),
        version: None,
        error: None,
        warnings: batch_warnings(&batch),
        created_by: created_by.to_owned(),
        created_at: now,
        updated_at: now,
        finished_at: None,
    };
    backend
        .write_json(&backend.import_job_key(&job.job_id), &job)
        .await?;
    Ok(job)
}

/// Everything worth saying about a batch that is not a refusal.
///
/// The synchronous import's warnings, over a staged batch: the same sentences,
/// because they describe the same mistakes and a reader should not have to
/// notice which route produced them.
fn batch_warnings(batch: &StagedBatch) -> Vec<String> {
    let stand_in = ImportRequest {
        project: batch.project.clone(),
        rights: batch.rights.clone(),
        evidence: batch.evidence.clone(),
        source: batch.source.clone(),
        rows: Vec::new(),
        dry_run: false,
    };
    let mut found = warnings(&stand_in, 0);
    if batch.every_page_is_singletons() {
        found.insert(
            0,
            format!(
                "every page of this batch gave each of its {} rows its own family. The split key \
                 is the building, so a mirrored plan and its original have to share a group_id — \
                 otherwise the test score measures memorisation and nothing in the numbers says \
                 so. Check what the pipeline mapped group_id from.",
                batch.rows
            ),
        );
    }
    found
}

pub(crate) async fn job(backend: &Backend, job_id: &str) -> Result<ImportJob> {
    validate_digest(job_id, "a job id")?;
    backend
        .read_json(&backend.import_job_key(job_id))
        .await?
        .ok_or_else(|| Error::NotFound(format!("the import job {job_id}")))
}

pub(crate) async fn jobs(backend: &Backend) -> Result<ImportJobPage> {
    let mut jobs = Vec::new();
    for key in backend.keys(&backend.import_jobs_prefix()).await? {
        if !key.ends_with("/job.json") {
            continue;
        }
        if let Some(found) = backend.read_json::<ImportJob>(&key).await? {
            jobs.push(found);
        }
    }
    jobs.sort_by_key(|job| std::cmp::Reverse(job.created_at));
    Ok(ImportJobPage { jobs })
}

pub(crate) async fn cancel(backend: &Backend, job_id: &str) -> Result<ImportJob> {
    let mut found = job(backend, job_id).await?;
    if found.state.is_finished() {
        return Err(Error::Invalid(format!(
            "the import job {job_id} already finished as {}",
            found.state.as_str()
        )));
    }
    found.state = JobState::Cancelled;
    found.finished_at = Some(OffsetDateTime::now_utc());
    found.updated_at = found.finished_at.unwrap_or(found.updated_at);
    backend
        .write_json(&backend.import_job_key(job_id), &found)
        .await?;
    Ok(found)
}

/// Job ids nobody is working on, oldest first.
pub(crate) async fn claimable(backend: &Backend, now: OffsetDateTime) -> Result<Vec<String>> {
    let mut waiting: Vec<(OffsetDateTime, String)> = Vec::new();
    for job in jobs(backend).await?.jobs {
        match job.state {
            JobState::Queued => waiting.push((job.created_at, job.job_id)),
            // The restart case: a process that died mid-import left its job
            // saying `running`, and nothing else would ever move it.
            JobState::Running if job.lease_expired(now) => {
                waiting.push((job.created_at, job.job_id));
            }
            _ => {}
        }
    }
    waiting.sort();
    Ok(waiting.into_iter().map(|(_, job_id)| job_id).collect())
}

/// Rejected rows, paged.
pub(crate) async fn rejects(
    backend: &Backend,
    job_id: &str,
    offset: usize,
    limit: usize,
) -> Result<RejectPage> {
    let found = job(backend, job_id).await?;
    let total: usize = found.rejects.values().sum();
    let limit = limit.clamp(1, MAX_REJECT_PAGE);

    let mut rows = Vec::new();
    let mut seen = 0usize;
    for page in &found.reject_pages {
        let key = backend.import_reject_key(job_id, *page);
        let Some(bytes) = backend.get_bytes(&key).await? else {
            continue;
        };
        for line in String::from_utf8_lossy(&bytes).lines() {
            if line.trim().is_empty() {
                continue;
            }
            if seen >= offset && rows.len() < limit {
                rows.push(serde_json::from_str(line).map_err(|error| Error::Corrupt {
                    key: key.clone(),
                    message: error.to_string(),
                })?);
            }
            seen += 1;
        }
        if rows.len() >= limit {
            break;
        }
    }
    let next_offset = (rows.len() == limit).then_some(offset + rows.len());
    Ok(RejectPage {
        rows,
        next_offset,
        total,
    })
}

pub(crate) async fn manifest(backend: &Backend, version: &str) -> Result<ImportManifest> {
    validate_digest(version, "an import version")?;
    backend
        .read_json(&backend.import_manifest_key(version))
        .await?
        .ok_or_else(|| Error::NotFound(format!("the import {version}")))
}

pub(crate) async fn manifests(backend: &Backend) -> Result<ImportIndex> {
    Ok(backend
        .read_json(&backend.import_index_key())
        .await?
        .unwrap_or_default())
}

/// Run one job to completion, or until it is cancelled, fails, or is taken
/// over.
///
/// `worker` is this process's identity — a pod name in a cluster. Two things
/// depend on it, and they are the export's two: a job whose lease is live and
/// held by somebody *else* is left alone, and the lease is re-checked at every
/// page boundary so a worker whose lease expired under it stops instead of
/// registering beside its replacement.
///
/// Concurrent workers are less dangerous here than in an export — registering
/// the same image twice is one image, because the id is the content address —
/// but the *job record* is not idempotent, and two workers writing it would
/// produce counts that describe neither run.
pub(crate) async fn run(
    backend: &Backend,
    project: &AnnotationProject,
    job_id: &str,
    worker: &str,
    images: Option<&dyn ImageSource>,
) -> Result<ImportJob> {
    let mut found = job(backend, job_id).await?;
    if found.state.is_finished() {
        return Ok(found);
    }
    let now = OffsetDateTime::now_utc();
    // Taking over a live lease held by another worker is what the lease
    // prevents. Taking over *our own* is not: a pod that restarted has the
    // same name and the process that held it is gone.
    if found.state == JobState::Running && !found.lease_expired(now) && found.claimed_by != worker {
        return Ok(found);
    }
    let batch = staging::batch(backend, &found.batch_id).await?;
    found.state = JobState::Running;
    found.claimed_by = worker.to_owned();
    found.claimed_at = Some(now);
    found.updated_at = now;
    backend
        .write_json(&backend.import_job_key(job_id), &found)
        .await?;

    while found.cursor < found.pages {
        if let Some(stopped) = interrupted(backend, job_id, worker).await? {
            return Ok(stopped);
        }
        let index = found.cursor;
        let rows = match staging::page(backend, &batch, index).await {
            Ok(rows) => rows,
            Err(error) => {
                let retryable = is_retryable(&error);
                return park(backend, found, &error, retryable).await;
            }
        };
        match page(backend, project, &batch, &mut found, index, rows, images).await {
            Ok(()) => {}
            Err(error) => {
                let retryable = is_retryable(&error);
                return park(backend, found, &error, retryable).await;
            }
        }
    }
    complete(backend, found, &batch).await
}

/// The stored job, when this worker should stop writing to it.
async fn interrupted(backend: &Backend, job_id: &str, worker: &str) -> Result<Option<ImportJob>> {
    let current = job(backend, job_id).await?;
    if current.state == JobState::Cancelled {
        return Ok(Some(current));
    }
    if current.claimed_by != worker {
        return Ok(Some(current));
    }
    Ok(None)
}

fn is_retryable(error: &Error) -> bool {
    match error {
        Error::Store(port) => port.is_retryable(),
        _ => false,
    }
}

/// Put a failing job back in the queue, or fail it for good.
///
/// The cursor is untouched either way: a job that comes back picks up at the
/// last page it committed, and one that does not still says how far it got.
async fn park(
    backend: &Backend,
    mut job: ImportJob,
    error: &Error,
    retryable: bool,
) -> Result<ImportJob> {
    job.attempts += 1;
    job.error = Some(error.to_string());
    job.updated_at = OffsetDateTime::now_utc();
    // Released rather than left to expire: a requeued job should be picked up
    // on the next tick, not five minutes later.
    job.claimed_by = String::new();
    job.claimed_at = None;
    job.state = after_failure(job.attempts, retryable);
    if job.state == JobState::Failed {
        job.finished_at = Some(job.updated_at);
    }
    backend
        .write_json(&backend.import_job_key(&job.job_id), &job)
        .await?;
    Ok(job)
}

/// What one page did, before it is allowed to count.
///
/// Kept separate from the job and merged only when the page's shard is stored.
/// The reason is the resume: a page that was registered and whose shard was
/// never written *will be done again*, and counts already folded into the job
/// record would then describe that page twice. Registering an image twice is
/// harmless — the id is the content address — but a receipt saying 40 rows
/// were accepted out of 30 is a receipt nobody can use.
#[derive(Default)]
struct PageTally {
    counts: ImportCounts,
    rejects: BTreeMap<String, usize>,
    rejected: Vec<RejectedRow>,
    results: Vec<serde_json::Value>,
}

/// One page: fetch what needs fetching, register what registers, then write
/// the shards and only then the cursor that passes them.
async fn page(
    backend: &Backend,
    project: &AnnotationProject,
    batch: &StagedBatch,
    job: &mut ImportJob,
    index: usize,
    rows: Vec<ImportRow>,
    images: Option<&dyn ImageSource>,
) -> Result<()> {
    let mut tally = PageTally::default();
    let dry_run = job.request.dry_run;
    let kept_already: usize = job.rejects.values().sum();

    for row in rows {
        tally.counts.rows_considered += 1;
        let uri = row.uri.clone();
        let group_id = row.group_id.clone();

        let row = match hydrate(backend, &mut tally, row, images).await {
            Ok(row) => row,
            Err((reason, detail)) => {
                reject(
                    &mut tally,
                    kept_already,
                    index,
                    uri,
                    group_id,
                    reason,
                    detail,
                );
                continue;
            }
        };

        let single: RegisterImageRequest =
            to_request(row, &project.name, &batch.rights, &batch.source);
        let image_id = single.image_id.clone();
        let outcome = if dry_run {
            crate::images::check(&single).map(|()| image_id)
        } else {
            crate::images::register(backend, project, single)
                .await
                .map(|head| head.image.image_id)
        };

        match outcome {
            Ok(image_id) => {
                tally.counts.accepted += 1;
                tally
                    .results
                    .push(serde_json::json!({ "image_id": image_id }));
            }
            // A store that will not answer is about *here*, not about the row.
            // Failing the page requeues it; recording it as a rejected row
            // would bake an outage into an immutable receipt.
            Err(error @ Error::Store(_)) => return Err(error),
            Err(error) => reject(
                &mut tally,
                kept_already,
                index,
                uri,
                group_id,
                RejectReason::Invalid,
                error.to_string(),
            ),
        }
    }

    // The result shard first — it is what the version is built from — then the
    // rejects beside it, then the cursor that passes both.
    let bytes = encode(
        &tally.results,
        &backend.import_result_key(&job.job_id, index),
    )?;
    let shard_digest = digest(&bytes);
    backend
        .put_bytes(&backend.import_result_key(&job.job_id, index), bytes)
        .await?;

    let has_rejects = !tally.rejected.is_empty();
    if has_rejects {
        let key = backend.import_reject_key(&job.job_id, index);
        let bytes = encode(&tally.rejected, &key)?;
        backend.put_bytes(&key, bytes).await?;
    }

    job.counts.rows_considered += tally.counts.rows_considered;
    job.counts.accepted += tally.counts.accepted;
    job.counts.rejected += tally.counts.rejected;
    job.counts.fetched += tally.counts.fetched;
    job.counts.bytes_stored += tally.counts.bytes_stored;
    for (reason, count) in tally.rejects {
        *job.rejects.entry(reason).or_default() += count;
    }
    if has_rejects && !job.reject_pages.contains(&index) {
        job.reject_pages.push(index);
    }
    job.shards.retain(|shard| shard.index != index);
    job.shards.push(ShardRef {
        index,
        rows: tally.results.len(),
        digest: shard_digest,
    });
    job.shards.sort_by_key(|shard| shard.index);
    job.cursor = index + 1;
    job.updated_at = OffsetDateTime::now_utc();
    // The cursor and the lease are renewed in one write, which is why the
    // lease bounds a page rather than an import: as long as pages keep
    // landing, the claim keeps holding.
    job.claimed_at = Some(job.updated_at);
    backend
        .write_json(&backend.import_job_key(&job.job_id), &*job)
        .await?;
    Ok(())
}

fn encode<T: Serialize>(values: &[T], key: &str) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    for value in values {
        bytes.extend_from_slice(&serde_json::to_vec(value).map_err(|error| Error::Corrupt {
            key: key.to_owned(),
            message: error.to_string(),
        })?);
        bytes.push(b'\n');
    }
    Ok(bytes)
}

/// Download the bytes for a row that names an address and carries no content
/// address, and store them here.
///
/// A row that already has an `image_id` is left alone, which is every batch
/// whose pipeline did its own fetching. Everything else goes through
/// [`ImageSource`], which is the only door onto the network in this crate and
/// carries every gate.
async fn hydrate(
    backend: &Backend,
    tally: &mut PageTally,
    mut row: ImportRow,
    images: Option<&dyn ImageSource>,
) -> std::result::Result<ImportRow, (RejectReason, String)> {
    if row.image_id.is_some() {
        return Ok(row);
    }
    let Some(images) = images else {
        return Err((
            RejectReason::AddressRefused,
            "this row carries no image_id and this instance has no configured image source, so \
             there is nothing to fetch its bytes with"
                .to_owned(),
        ));
    };

    let found = images
        .fetch(&row.uri, row.image_id.as_deref())
        .await
        .map_err(|message| (RejectReason::of_fetch(&message), message))?;

    // A hub says nothing about a binary column, so the row may have arrived
    // with zeroes the registry would refuse. The bytes are here and their
    // header knows.
    if row.width == 0 || row.height == 0 {
        row.width = found.width;
        row.height = found.height;
    }
    let bytes = found.bytes.len() as u64;
    let stored = crate::images::put_blob(backend, found.bytes, &found.content_type)
        .await
        .map_err(|error| (RejectReason::StoreFailed, error.to_string()))?;

    tally.counts.fetched += 1;
    tally.counts.bytes_stored += bytes;
    row.metadata
        .insert("import.hub_uri".to_owned(), row.uri.clone().into());
    row.uri = stored.uri;
    row.image_id = Some(stored.image_id);
    Ok(row)
}

fn reject(
    tally: &mut PageTally,
    kept_already: usize,
    page: usize,
    uri: String,
    group_id: String,
    reason: RejectReason,
    detail: String,
) {
    tally.counts.rejected += 1;
    *tally.rejects.entry(reason.as_str().to_owned()).or_default() += 1;
    if kept_already + tally.rejected.len() < MAX_REJECT_ROWS {
        tally.rejected.push(RejectedRow {
            page,
            uri,
            group_id,
            reason,
            detail,
        });
    }
}

/// Seal the version, write the manifest, then index it.
async fn complete(backend: &Backend, mut job: ImportJob, batch: &StagedBatch) -> Result<ImportJob> {
    let version = version_of(&job.request_digest, &job.shards);
    let now = OffsetDateTime::now_utc();

    // A dry run proves what would happen and is not a thing that happened. It
    // gets counts, rejects and a state, and no manifest — the alternative is a
    // published reference naming images nobody registered.
    if !job.request.dry_run {
        let manifest = ImportManifest {
            project: job.project.clone(),
            version: version.clone(),
            job_id: job.job_id.clone(),
            batch_id: job.batch_id.clone(),
            batch_digest: batch.digest.clone().unwrap_or_default(),
            rights: batch.rights.clone(),
            evidence: batch.evidence.clone(),
            source: batch.source.clone(),
            counts: job.counts,
            rejects: job.rejects.clone(),
            shards: job.shards.clone(),
            warnings: job.warnings.clone(),
            built_at: now,
        };
        // The manifest before the index entry that lists it — the ordering
        // every registry in this workspace keeps.
        backend
            .write_json(&backend.import_manifest_key(&version), &manifest)
            .await?;

        let index_key = backend.import_index_key();
        let mut index: ImportIndex = backend.read_json(&index_key).await?.unwrap_or_default();
        if !index
            .imports
            .iter()
            .any(|summary| summary.version == version)
        {
            index.imports.insert(
                0,
                ImportSummary {
                    project: manifest.project.clone(),
                    version: version.clone(),
                    job_id: manifest.job_id.clone(),
                    accepted: manifest.counts.accepted,
                    rejected: manifest.counts.rejected,
                    built_at: now,
                },
            );
            backend.write_json(&index_key, &index).await?;
        }
    }

    job.state = JobState::Completed;
    job.version = Some(version);
    job.finished_at = Some(now);
    job.updated_at = now;
    job.claimed_by = String::new();
    job.claimed_at = None;
    backend
        .write_json(&backend.import_job_key(&job.job_id), &job)
        .await?;
    Ok(job)
}
