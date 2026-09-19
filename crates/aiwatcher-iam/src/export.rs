//! Freezing an organization's audit trail into immutable, resumable shards.
//!
//! ```text
//!   POST  ──►  queued ──► running ──► completed  sha256
//!                 ▲          │  │
//!                 └──────────┘  └──► failed      the retry budget ran out
//!                  retryable         cancelled   somebody asked
//! ```
//!
//! This is [`aiwatcher_jobs`]' machine, and its third caller. The rules are the
//! ones ADR_0022 wrote down once so that three copies cannot disagree:
//!
//! * **The range is pinned when the job is created.** `through_sequence` is the
//!   highest sequence the trail held at that moment, so a grant issued while an
//!   export runs belongs to the next one rather than appearing halfway through
//!   this one.
//! * **A shard is written before the cursor that passes it.** A crash re-reads
//!   one shard's worth of entries and writes byte-identical bytes; the other
//!   order leaves an export missing entries that nothing can tell you about —
//!   which, for an audit trail, is the whole of the problem it exists to solve.
//! * **The lease is renewed per shard and re-checked at every boundary**, so a
//!   replaced worker stops rather than writing beside its replacement.
//! * **The version is `sha256(request ‖ every shard digest, in order)`**, so the
//!   same range over an unchanged trail is one reference.
//! * **Every entry that is not there is counted.** An export whose range covers
//!   four hundred entries and whose shards hold three hundred says so in
//!   `missing`: the retention sweep reached them first, and a corpus that
//!   quietly came up short would be worse than one that says it did.
//!
//! **Authorization is re-asked for every page.** The job stores the principal
//! who requested it — the pair, not a decision — and each page is read through
//! [`IamStore::audit`] as them. An administrator demoted mid-export therefore
//! stops the export, which is the README's rule about `ProjectAccess` applied
//! to the one long-running thing this crate now has: a snapshot is not a bearer
//! capability, and a job is a sequence of operations rather than one.
//!
//! ADR_0022.

use std::sync::Arc;

use aiwatcher_core::storage::ObjectStore;
use aiwatcher_jobs::{JobState, ShardRef, digest, version_of};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::{AuditEntry, Error, IamStore, OrganizationId, Principal, Result};

/// Entries per shard. Ten pages of the audit read's maximum, which makes a
/// shard a round number of round trips and keeps a resumed job from re-reading
/// more than ten.
pub const SHARD_ENTRIES: usize = 1_000;

/// What one read of the trail asks for. The audit route's own ceiling, because
/// this reads through exactly that route's store method and nothing here may
/// widen a limit somebody else published.
const PAGE: usize = 100;

/// Times one range may be exported before the ids run out.
const MAX_GENERATIONS: u32 = 1_000;

/// Rows one read of a finished export may return.
pub const MAX_ROW_PAGE: usize = 1_000;

/// What was asked for. With the pinned range below, the identity of an export.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = IamAuditExportRequest))]
pub struct AuditExportRequest {
    /// Why this was taken. Recorded on the job, never interpreted — an export
    /// of an audit trail is usually somebody answering a question in writing,
    /// and the question is worth keeping beside the answer.
    #[serde(default)]
    pub reason: String,
    /// Entries after this sequence, exclusive. Zero is "everything the trail
    /// still holds", which is the common case.
    #[serde(default)]
    pub after_sequence: i64,
}

impl AuditExportRequest {
    fn validate(&self) -> Result<()> {
        if self.after_sequence < 0 {
            return Err(Error::Invalid("after_sequence must not be negative".into()));
        }
        if self.reason.len() > 1_000 || self.reason.chars().any(char::is_control) {
            return Err(Error::Invalid(
                "a reason must be at most 1000 bytes without control characters".into(),
            ));
        }
        Ok(())
    }

    /// The identity of "what was asked", independent of what the trail held.
    #[must_use]
    fn digest(&self) -> String {
        digest(&serde_json::to_vec(self).unwrap_or_default())
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = IamAuditExportCounts))]
pub struct AuditExportCounts {
    /// Entries written into a shard.
    pub entries: usize,
    /// Sequences in the pinned range that the trail no longer held.
    ///
    /// Set once, when the job completes, from the range and what was written —
    /// sequences are contiguous within an organization, so the arithmetic is
    /// exact rather than a tally somebody has to keep in step.
    pub missing: usize,
}

/// A resumable export, as it sits in the object store.
///
/// This *is* the manifest. A conversation corpus earns a `name@version` and an
/// index because a training run names one; nothing names an audit export, so a
/// second record listing the first would be a second thing to keep in step for
/// no reader.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = IamAuditExport))]
pub struct AuditExportJob {
    pub job_id: String,
    pub organization: OrganizationId,
    pub request: AuditExportRequest,
    pub request_digest: String,
    pub state: JobState,
    /// The exclusive lower bound, from the request.
    pub after_sequence: i64,
    /// The inclusive upper bound, pinned when the job was created.
    pub through_sequence: i64,
    /// The highest sequence already accounted for by a written shard. The
    /// resume point, and the only field the ordering rule is about.
    #[serde(default)]
    pub cursor: i64,
    #[serde(default)]
    pub attempts: u32,
    /// The worker holding this job: a pod name in a cluster. Not an owner — a
    /// lease.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub claimed_by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "time::serde::rfc3339::option")]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub claimed_at: Option<OffsetDateTime>,
    #[serde(default)]
    pub counts: AuditExportCounts,
    #[serde(default)]
    pub shards: Vec<ShardRef>,
    /// The immutable reference, once there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Who asked, and who every page is read as. Stored because it is re-asked,
    /// not because it was once allowed.
    pub requested_by: Principal,
    #[serde(with = "time::serde::rfc3339")]
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    #[cfg_attr(feature = "openapi", schema(value_type = String))]
    pub updated_at: OffsetDateTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "time::serde::rfc3339::option")]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub finished_at: Option<OffsetDateTime>,
}

impl AuditExportJob {
    /// Whether nobody is demonstrably working on this right now.
    #[must_use]
    pub fn lease_expired(&self, now: OffsetDateTime) -> bool {
        aiwatcher_jobs::lease_expired(self.claimed_at, now)
    }

    /// How much of the pinned range is behind the cursor.
    ///
    /// A fact rather than a guess: the denominator was fixed when the job was
    /// created.
    #[must_use]
    pub fn progress(&self) -> Option<f64> {
        let total = usize::try_from(self.through_sequence - self.after_sequence).unwrap_or(0);
        let done = usize::try_from(self.cursor.max(self.after_sequence) - self.after_sequence)
            .unwrap_or(0);
        aiwatcher_jobs::progress(done, total)
    }

    /// Sequences in the pinned range, whether or not the trail still holds them.
    fn range(&self) -> usize {
        usize::try_from(self.through_sequence - self.after_sequence).unwrap_or(0)
    }
}

/// One page of a finished export's entries.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(as = IamAuditExportRowsPage))]
pub struct AuditExportRowsPage {
    pub entries: Vec<AuditEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<usize>,
    pub total: usize,
}

/// The object store namespace an instance's audit exports live in.
///
/// Separate from the IAM store on purpose. The trail is a transactional table
/// whose whole value is that a mutation and its record commit together; an
/// export is bytes, written once, read rarely and kept after the rows it froze
/// are gone. Putting the frozen copy in the same table that retention empties
/// would be a backup inside the thing being deleted.
#[derive(Clone, Debug)]
pub struct AuditExports {
    store: Arc<dyn ObjectStore>,
    prefix: String,
}

impl AuditExports {
    #[must_use]
    pub fn new(store: Arc<dyn ObjectStore>, prefix: impl Into<String>) -> Self {
        Self {
            store,
            prefix: prefix.into().trim_matches('/').to_owned(),
        }
    }

    fn jobs_prefix(&self) -> String {
        format!("{}/jobs/", self.prefix)
    }

    fn job_key(&self, organization: OrganizationId, job_id: &str) -> String {
        format!("{}{}/{job_id}.json", self.jobs_prefix(), organization.0)
    }

    fn shard_key(&self, organization: OrganizationId, job_id: &str, index: usize) -> String {
        format!(
            "{}/shards/{}/{job_id}/{index:06}.jsonl",
            self.prefix, organization.0
        )
    }

    async fn read<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let Some(bytes) = self.store.get(key).await? else {
            return Ok(None);
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| Error::Incompatible(format!("{key}: {error}")))
    }

    async fn write<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        let bytes = serde_json::to_vec(value).map_err(|error| Error::Backend(error.to_string()))?;
        self.store.put(key, bytes).await?;
        Ok(())
    }

    /// Queue an export, pinning the range it will read.
    ///
    /// # Errors
    ///
    /// [`Error::Forbidden`] unless the caller administers the organization now,
    /// [`Error::Invalid`] for a request that cannot be run, and
    /// [`Error::Conflict`] when the trail holds nothing to export.
    pub async fn create(
        &self,
        iam: &dyn IamStore,
        organization: OrganizationId,
        actor: &Principal,
        request: AuditExportRequest,
    ) -> Result<AuditExportJob> {
        request.validate()?;
        let bounds = iam.audit_bounds(organization, actor).await?;
        let Some(through_sequence) = bounds.last_sequence else {
            return Err(Error::Conflict(
                "this organization's audit trail holds no entry to export".into(),
            ));
        };
        if request.after_sequence >= through_sequence {
            return Err(Error::Conflict(format!(
                "the trail ends at sequence {through_sequence}; an export after \
                 {} would hold nothing",
                request.after_sequence
            )));
        }

        let request_digest = request.digest();
        let now = OffsetDateTime::now_utc();
        // Derived rather than random, so a retried POST joins the job it already
        // started instead of freezing the same range twice — and generational,
        // so asking again after one finished starts a new one rather than
        // handing back a copy that predates whatever prompted the second ask.
        let mut job_id = String::new();
        for generation in 0..MAX_GENERATIONS {
            job_id = digest(
                format!(
                    "{}\0{request_digest}\0{through_sequence}\0{generation}",
                    organization.0
                )
                .as_bytes(),
            );
            match self
                .read::<AuditExportJob>(&self.job_key(organization, &job_id))
                .await?
            {
                Some(existing) if !existing.state.is_finished() => return Ok(existing),
                Some(_) => continue,
                None => break,
            }
        }

        let job = AuditExportJob {
            job_id,
            organization,
            request_digest,
            after_sequence: request.after_sequence,
            // The cursor starts where the range does, so a job that writes no
            // shard at all has still accounted for nothing rather than for
            // everything before its lower bound.
            cursor: request.after_sequence,
            request,
            state: JobState::Queued,
            through_sequence,
            attempts: 0,
            claimed_by: String::new(),
            claimed_at: None,
            counts: AuditExportCounts::default(),
            shards: Vec::new(),
            version: None,
            error: None,
            requested_by: actor.clone(),
            created_at: now,
            updated_at: now,
            finished_at: None,
        };
        self.write(&self.job_key(organization, &job.job_id), &job)
            .await?;
        Ok(job)
    }

    /// One job, for somebody who administers the organization it belongs to.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when no such job exists, and whatever
    /// [`IamStore::authorize_audit`] answers.
    pub async fn job(
        &self,
        iam: &dyn IamStore,
        organization: OrganizationId,
        actor: &Principal,
        job_id: &str,
    ) -> Result<AuditExportJob> {
        iam.authorize_audit(organization, actor).await?;
        self.stored(organization, job_id).await
    }

    async fn stored(&self, organization: OrganizationId, job_id: &str) -> Result<AuditExportJob> {
        validate_job_id(job_id)?;
        self.read(&self.job_key(organization, job_id))
            .await?
            .ok_or(Error::NotFound)
    }

    /// Every export of one organization, newest first.
    ///
    /// # Errors
    ///
    /// Whatever [`IamStore::authorize_audit`] and the object store answer.
    pub async fn jobs(
        &self,
        iam: &dyn IamStore,
        organization: OrganizationId,
        actor: &Principal,
    ) -> Result<Vec<AuditExportJob>> {
        iam.authorize_audit(organization, actor).await?;
        let prefix = format!("{}{}/", self.jobs_prefix(), organization.0);
        let mut jobs = Vec::new();
        for entry in self.store.list(&prefix).await? {
            if let Some(job) = self.read::<AuditExportJob>(&entry.key).await? {
                jobs.push(job);
            }
        }
        jobs.sort_by_key(|job| std::cmp::Reverse(job.created_at));
        Ok(jobs)
    }

    /// Stop a job that has not finished.
    ///
    /// # Errors
    ///
    /// [`Error::Conflict`] for one that already has: a finished export is a
    /// record, and there is nothing left for a cancellation to prevent.
    pub async fn cancel(
        &self,
        iam: &dyn IamStore,
        organization: OrganizationId,
        actor: &Principal,
        job_id: &str,
    ) -> Result<AuditExportJob> {
        iam.authorize_audit(organization, actor).await?;
        let mut job = self.stored(organization, job_id).await?;
        if job.state.is_finished() {
            return Err(Error::Conflict(format!(
                "the audit export {job_id} already finished as {}",
                job.state.as_str()
            )));
        }
        job.state = JobState::Cancelled;
        job.updated_at = OffsetDateTime::now_utc();
        job.finished_at = Some(job.updated_at);
        self.write(&self.job_key(organization, job_id), &job)
            .await?;
        Ok(job)
    }

    /// A finished export's entries, a page at a time.
    ///
    /// # Errors
    ///
    /// [`Error::Conflict`] for a job that has not completed — a running export's
    /// shards are real but its range is not yet covered, and reading them as a
    /// result would be reading a partial answer as a whole one.
    pub async fn rows(
        &self,
        iam: &dyn IamStore,
        organization: OrganizationId,
        actor: &Principal,
        job_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<AuditExportRowsPage> {
        iam.authorize_audit(organization, actor).await?;
        if limit == 0 || limit > MAX_ROW_PAGE {
            return Err(Error::Invalid(format!(
                "limit must be in 1..={MAX_ROW_PAGE}"
            )));
        }
        let job = self.stored(organization, job_id).await?;
        if job.state != JobState::Completed {
            return Err(Error::Conflict(format!(
                "the audit export {job_id} is {} and has no result to read",
                job.state.as_str()
            )));
        }
        let total = job.counts.entries;
        let mut entries = Vec::new();
        let mut seen = 0usize;
        for shard in &job.shards {
            if seen + shard.rows <= offset {
                seen += shard.rows;
                continue;
            }
            let key = self.shard_key(organization, job_id, shard.index);
            let bytes = self.store.get(&key).await?.ok_or_else(|| {
                Error::Incompatible(format!("{key}: a completed export is missing a shard"))
            })?;
            for line in bytes.split(|byte| *byte == b'\n') {
                if line.is_empty() {
                    continue;
                }
                if seen >= offset && entries.len() < limit {
                    entries.push(
                        serde_json::from_slice(line)
                            .map_err(|error| Error::Incompatible(format!("{key}: {error}")))?,
                    );
                }
                seen += 1;
            }
            if entries.len() >= limit {
                break;
            }
        }
        let next = offset + entries.len();
        Ok(AuditExportRowsPage {
            entries,
            next_offset: (next < total).then_some(next),
            total,
        })
    }

    /// Jobs nobody is working on, oldest first, across every organization.
    ///
    /// A worker's question, so it asks IAM nothing: what may be *exported* was
    /// decided when the job was created and is re-asked for every page it
    /// reads. What is left here is scheduling.
    ///
    /// # Errors
    ///
    /// Whatever the object store answers.
    pub async fn claimable(&self, now: OffsetDateTime) -> Result<Vec<(OrganizationId, String)>> {
        let mut waiting: Vec<(OffsetDateTime, OrganizationId, String)> = Vec::new();
        for entry in self.store.list(&self.jobs_prefix()).await? {
            let Some(job) = self.read::<AuditExportJob>(&entry.key).await? else {
                continue;
            };
            match job.state {
                JobState::Queued => {
                    waiting.push((job.created_at, job.organization, job.job_id));
                }
                // `Running` becomes claimable when its lease runs out, which is
                // the restart case: a process that died mid-export left a job
                // saying `running` and nothing else would ever move it.
                JobState::Running if job.lease_expired(now) => {
                    waiting.push((job.created_at, job.organization, job.job_id));
                }
                _ => {}
            }
        }
        waiting.sort();
        Ok(waiting
            .into_iter()
            .map(|(_, organization, job_id)| (organization, job_id))
            .collect())
    }

    /// Run one job to completion, or until it is cancelled, fails, or is taken
    /// over.
    ///
    /// `worker` is this process's identity — a pod name in a cluster. A job
    /// whose lease is live and held by somebody else is left alone; the lease is
    /// re-checked at every shard, so a worker whose claim expired under it stops
    /// rather than writing beside its replacement.
    ///
    /// # Errors
    ///
    /// Whatever the object store answers for the job record itself. A failure
    /// *of the export* is recorded on the job and returned as a job, because a
    /// caller draining a queue needs to keep going.
    pub async fn run(
        &self,
        iam: &dyn IamStore,
        organization: OrganizationId,
        job_id: &str,
        worker: &str,
    ) -> Result<AuditExportJob> {
        let mut job = self.stored(organization, job_id).await?;
        if job.state.is_finished() {
            return Ok(job);
        }
        let now = OffsetDateTime::now_utc();
        // Taking over a live lease held by another worker is what the lease
        // exists to prevent. Taking over our own is not: a pod that restarted
        // has the same name and the process that held it is gone.
        if job.state == JobState::Running && !job.lease_expired(now) && job.claimed_by != worker {
            tracing::debug!(
                job_id,
                held_by = %job.claimed_by,
                "another worker holds this audit export's lease"
            );
            return Ok(job);
        }
        job.state = JobState::Running;
        job.claimed_by = worker.to_owned();
        job.claimed_at = Some(now);
        job.updated_at = now;
        self.write(&self.job_key(organization, job_id), &job)
            .await?;

        let mut staged: Vec<AuditEntry> = Vec::new();
        let mut read_from = job.cursor;
        while read_from < job.through_sequence {
            let page = match iam
                .audit(organization, &job.requested_by, read_from, PAGE)
                .await
            {
                Ok(page) => page,
                // A store that is down is worth coming back for; a principal who
                // no longer administers this organization is not, and spending
                // the retry budget on them only delays the message that says so.
                Err(error) => {
                    let retryable = matches!(error, Error::Backend(_));
                    return self.park(job, &error, retryable).await;
                }
            };
            let in_range: Vec<AuditEntry> = page
                .into_iter()
                .filter(|entry| entry.sequence <= job.through_sequence)
                .collect();
            // Either the trail is exhausted or everything left in the range has
            // been swept. Both mean the same thing here, and `missing` is what
            // tells them apart afterwards.
            let Some(last) = in_range.last().map(|entry| entry.sequence) else {
                break;
            };
            read_from = last;
            staged.extend(in_range);

            if staged.len() >= SHARD_ENTRIES {
                // A cancellation and a lost lease both land between shards.
                if let Some(stopped) = self.interrupted(organization, job_id, worker).await? {
                    return Ok(stopped);
                }
                self.flush(&mut job, &mut staged, worker).await?;
            }
        }
        if !staged.is_empty() {
            if let Some(stopped) = self.interrupted(organization, job_id, worker).await? {
                return Ok(stopped);
            }
            self.flush(&mut job, &mut staged, worker).await?;
        }
        // Once more before the job is called finished. Without it a
        // cancellation that arrived after the last shard — or during a read
        // phase that produced none — would be overwritten by `completed`, and
        // a record saying an export finished when somebody stopped it is the
        // one kind of wrong answer this feature exists to avoid.
        if let Some(stopped) = self.interrupted(organization, job_id, worker).await? {
            return Ok(stopped);
        }
        self.complete(job, worker).await
    }

    /// The stored job, when this worker should stop writing to it.
    async fn interrupted(
        &self,
        organization: OrganizationId,
        job_id: &str,
        worker: &str,
    ) -> Result<Option<AuditExportJob>> {
        let current = self.stored(organization, job_id).await?;
        if current.state == JobState::Cancelled {
            return Ok(Some(current));
        }
        if current.claimed_by != worker {
            tracing::warn!(
                job_id,
                taken_by = %current.claimed_by,
                "this audit export's lease was taken over; stopping rather than writing beside it"
            );
            return Ok(Some(current));
        }
        Ok(None)
    }

    /// The shard, then the cursor that passes it. [`aiwatcher_jobs::ORDERING`].
    async fn flush(
        &self,
        job: &mut AuditExportJob,
        staged: &mut Vec<AuditEntry>,
        worker: &str,
    ) -> Result<()> {
        let index = job.shards.len();
        let mut bytes = Vec::new();
        for entry in staged.iter() {
            bytes.extend_from_slice(
                &serde_json::to_vec(entry).map_err(|error| Error::Backend(error.to_string()))?,
            );
            bytes.push(b'\n');
        }
        let shard_digest = digest(&bytes);
        self.store
            .put(&self.shard_key(job.organization, &job.job_id, index), bytes)
            .await?;

        job.shards.push(ShardRef {
            index,
            rows: staged.len(),
            digest: shard_digest,
        });
        // Only now do the entries behind that shard count. Everything from here
        // to the write below moves together: cursor, counts and lease are one
        // record.
        job.counts.entries += staged.len();
        job.cursor = staged
            .last()
            .map_or(job.cursor, |entry| entry.sequence.max(job.cursor));
        job.updated_at = OffsetDateTime::now_utc();
        // The cursor and the lease are renewed in one write, which is why the
        // lease bounds a shard rather than an export: as long as shards keep
        // landing, the claim keeps holding.
        job.claimed_by = worker.to_owned();
        job.claimed_at = Some(job.updated_at);
        self.write(&self.job_key(job.organization, &job.job_id), &*job)
            .await?;
        staged.clear();
        Ok(())
    }

    async fn complete(&self, mut job: AuditExportJob, worker: &str) -> Result<AuditExportJob> {
        let now = OffsetDateTime::now_utc();
        job.counts.missing = job.range().saturating_sub(job.counts.entries);
        job.cursor = job.through_sequence;
        job.version = Some(version_of(&job.request_digest, &job.shards));
        job.state = JobState::Completed;
        job.error = None;
        job.updated_at = now;
        job.finished_at = Some(now);
        // Kept rather than cleared: "which worker built this" is worth having on
        // a finished job, and nothing claims a finished one.
        job.claimed_by = worker.to_owned();
        self.write(&self.job_key(job.organization, &job.job_id), &job)
            .await?;
        Ok(job)
    }

    /// Put a failing job back in the queue, or fail it for good.
    ///
    /// The cursor is untouched either way, which is the point: a job that comes
    /// back picks up at the last shard it committed, and one that does not still
    /// says how far it got.
    async fn park(
        &self,
        mut job: AuditExportJob,
        error: &Error,
        retryable: bool,
    ) -> Result<AuditExportJob> {
        job.attempts += 1;
        job.error = Some(error.to_string());
        job.updated_at = OffsetDateTime::now_utc();
        // Released rather than left to expire: a requeued job should be picked
        // up on the next tick, not five minutes later.
        job.claimed_by = String::new();
        job.claimed_at = None;
        job.state = aiwatcher_jobs::after_failure(job.attempts, retryable);
        if job.state == JobState::Failed {
            job.finished_at = Some(job.updated_at);
        }
        self.write(&self.job_key(job.organization, &job.job_id), &job)
            .await?;
        Ok(job)
    }
}

fn validate_job_id(job_id: &str) -> Result<()> {
    if job_id.len() != 64 || !job_id.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(Error::Invalid(
            "a job_id is a 64-character hexadecimal digest".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicBool, Ordering};

    use aiwatcher_core::ports::{PortError, PortResult};
    use aiwatcher_core::storage::ObjectEntry;
    use async_trait::async_trait;
    use tokio::sync::RwLock;

    use super::*;
    use crate::{
        AuditAction, AuditBounds, AuditRetention, Change, Command, Grant, Invitation, InvitationId,
        InvitationOffer, IssuedInvitation, Offered, Organization, ProjectAccess, ProjectScope,
        PruneReport, Redeemed, Roster,
    };

    /// Bytes in a map, and a switch that makes every write fail.
    ///
    /// The failure switch is what the retry rules need: a shard that could not
    /// be stored must leave the cursor exactly where it was.
    #[derive(Debug, Default)]
    struct Bytes {
        objects: RwLock<BTreeMap<String, Vec<u8>>>,
        refusing: AtomicBool,
    }

    #[async_trait]
    impl ObjectStore for Bytes {
        async fn put(&self, key: &str, body: Vec<u8>) -> PortResult<()> {
            if self.refusing.load(Ordering::SeqCst) {
                return Err(PortError::Unavailable {
                    target: "object-store",
                    message: "a test switched it off".into(),
                });
            }
            self.objects.write().await.insert(key.to_owned(), body);
            Ok(())
        }
        async fn get(&self, key: &str) -> PortResult<Option<Vec<u8>>> {
            Ok(self.objects.read().await.get(key).cloned())
        }
        async fn list(&self, prefix: &str) -> PortResult<Vec<ObjectEntry>> {
            Ok(self
                .objects
                .read()
                .await
                .iter()
                .filter(|(key, _)| key.starts_with(prefix))
                .map(|(key, body)| ObjectEntry {
                    key: key.clone(),
                    size: body.len() as u64,
                    last_modified: None,
                })
                .collect())
        }
        async fn delete(&self, key: &str) -> PortResult<()> {
            self.objects.write().await.remove(key);
            Ok(())
        }
    }

    /// A trail and a switch for whether the requester still administers it.
    ///
    /// Only the four audit-shaped methods do anything: the export reads through
    /// `audit`, authorizes through `authorize_audit` and pins its range from
    /// `audit_bounds`, and nothing here touches membership.
    #[derive(Debug)]
    struct Trail {
        entries: Vec<AuditEntry>,
        admits: AtomicBool,
    }

    impl Trail {
        /// `count` entries, one per second from the epoch, sequences 1..=count.
        fn of(count: i64) -> Self {
            let organization = OrganizationId::new();
            Self {
                entries: (1..=count)
                    .map(|sequence| AuditEntry {
                        sequence,
                        organization,
                        actor: principal(),
                        occurred_at: sequence,
                        action: AuditAction::CommandApplied {
                            command: Box::new(Command::CreateTeam {
                                name: format!("team-{sequence}"),
                            }),
                            change: Change::Applied,
                        },
                    })
                    .collect(),
                admits: AtomicBool::new(true),
            }
        }

        fn organization(&self) -> OrganizationId {
            self.entries[0].organization
        }

        /// Everything at or below `through` is gone, as a retention sweep would
        /// leave it.
        fn swept_through(&mut self, through: i64) {
            self.entries.retain(|entry| entry.sequence > through);
        }
    }

    fn principal() -> Principal {
        Principal::new("https://identity.test/oidc", "subject").expect("a valid principal")
    }

    #[async_trait]
    impl IamStore for Trail {
        async fn audit(
            &self,
            _organization: OrganizationId,
            _actor: &Principal,
            after: i64,
            limit: usize,
        ) -> Result<Vec<AuditEntry>> {
            if !self.admits.load(Ordering::SeqCst) {
                return Err(Error::Forbidden);
            }
            Ok(self
                .entries
                .iter()
                .filter(|entry| entry.sequence > after)
                .take(limit)
                .cloned()
                .collect())
        }
        async fn authorize_audit(
            &self,
            _organization: OrganizationId,
            _actor: &Principal,
        ) -> Result<()> {
            if self.admits.load(Ordering::SeqCst) {
                Ok(())
            } else {
                Err(Error::Forbidden)
            }
        }
        async fn audit_bounds(
            &self,
            organization: OrganizationId,
            _actor: &Principal,
        ) -> Result<AuditBounds> {
            Ok(AuditBounds {
                organization,
                first_sequence: self.entries.first().map(|entry| entry.sequence),
                last_sequence: self.entries.last().map(|entry| entry.sequence),
                entries: self.entries.len() as i64,
                watermark: None,
            })
        }
        async fn prune_audit(&self, _retention: &AuditRetention, _now: i64) -> Result<PruneReport> {
            unimplemented!("the export never prunes")
        }
        async fn create_organization(&self, _: &Principal, _: &str) -> Result<Organization> {
            unimplemented!()
        }
        async fn organizations(&self, _: &Principal) -> Result<Vec<Organization>> {
            unimplemented!()
        }
        async fn apply(&self, _: OrganizationId, _: &Principal, _: Command) -> Result<Change> {
            unimplemented!()
        }
        async fn projects(&self, _: OrganizationId, _: &Principal) -> Result<Vec<ProjectAccess>> {
            unimplemented!()
        }
        async fn roster(&self, _: OrganizationId, _: &Principal) -> Result<Roster> {
            unimplemented!()
        }
        async fn project_grants(&self, _: ProjectScope, _: &Principal) -> Result<Vec<Grant>> {
            unimplemented!()
        }
        async fn access(&self, _: ProjectScope, _: &Principal) -> Result<ProjectAccess> {
            unimplemented!()
        }
        async fn invite(
            &self,
            _: ProjectScope,
            _: &Principal,
            _: InvitationOffer,
        ) -> Result<IssuedInvitation> {
            unimplemented!()
        }
        async fn invitations(&self, _: OrganizationId, _: &Principal) -> Result<Vec<Invitation>> {
            unimplemented!()
        }
        async fn revoke_invitation(
            &self,
            _: OrganizationId,
            _: &Principal,
            _: InvitationId,
        ) -> Result<()> {
            unimplemented!()
        }
        async fn offered(&self, _: &str) -> Result<Offered> {
            unimplemented!()
        }
        async fn redeem(&self, _: &str, _: &Principal) -> Result<Redeemed> {
            unimplemented!()
        }
    }

    fn exports(bytes: &Arc<Bytes>) -> AuditExports {
        AuditExports::new(Arc::clone(bytes) as Arc<dyn ObjectStore>, "iam-audit")
    }

    async fn queued(trail: &Trail, exports: &AuditExports) -> AuditExportJob {
        exports
            .create(
                trail,
                trail.organization(),
                &principal(),
                AuditExportRequest::default(),
            )
            .await
            .expect("a queued export")
    }

    #[tokio::test]
    async fn an_export_freezes_its_pinned_range_and_is_named_by_its_content() {
        let bytes = Arc::new(Bytes::default());
        let exports = exports(&bytes);
        let trail = Trail::of(2_500);
        let organization = trail.organization();

        let job = queued(&trail, &exports).await;
        assert_eq!(job.through_sequence, 2_500);
        assert_eq!(job.cursor, 0);

        let done = exports
            .run(&trail, organization, &job.job_id, "worker-a")
            .await
            .expect("a finished export");
        assert_eq!(done.state, JobState::Completed);
        assert_eq!(done.counts.entries, 2_500);
        assert_eq!(done.counts.missing, 0);
        assert_eq!(done.cursor, 2_500);
        // 1000 + 1000 + 500: the last shard is short because the range ended,
        // never because a page did.
        assert_eq!(
            done.shards
                .iter()
                .map(|shard| shard.rows)
                .collect::<Vec<_>>(),
            vec![1_000, 1_000, 500]
        );
        assert_eq!(
            done.version.as_deref(),
            Some(version_of(&done.request_digest, &done.shards).as_str())
        );

        let page = exports
            .rows(&trail, organization, &principal(), &job.job_id, 0, 10)
            .await
            .expect("the first rows");
        assert_eq!(page.total, 2_500);
        assert_eq!(page.entries.first().map(|entry| entry.sequence), Some(1));
        assert_eq!(page.next_offset, Some(10));

        let late = exports
            .rows(&trail, organization, &principal(), &job.job_id, 2_495, 10)
            .await
            .expect("the last rows");
        assert_eq!(late.entries.len(), 5);
        assert_eq!(late.next_offset, None);
    }

    #[tokio::test]
    async fn a_shard_that_could_not_be_stored_leaves_the_cursor_where_it_was() {
        let bytes = Arc::new(Bytes::default());
        let exports = exports(&bytes);
        let trail = Trail::of(2_500);
        let organization = trail.organization();
        let job = queued(&trail, &exports).await;

        bytes.refusing.store(true, Ordering::SeqCst);
        let error = exports
            .run(&trail, organization, &job.job_id, "worker-a")
            .await
            .expect_err("the store refused");
        assert!(matches!(error, Error::Storage(_)), "{error:?}");

        // The claim write failed too, so nothing at all moved — which is the
        // point: a cursor never runs ahead of a shard.
        bytes.refusing.store(false, Ordering::SeqCst);
        let stored = exports
            .job(&trail, organization, &principal(), &job.job_id)
            .await
            .expect("the job is still there");
        assert_eq!(stored.cursor, 0);
        assert!(stored.shards.is_empty());

        let done = exports
            .run(&trail, organization, &job.job_id, "worker-a")
            .await
            .expect("a finished export");
        assert_eq!(done.counts.entries, 2_500);
    }

    #[tokio::test]
    async fn a_worker_whose_lease_was_taken_stops_rather_than_writing_beside_it() {
        let bytes = Arc::new(Bytes::default());
        let exports = exports(&bytes);
        let trail = Trail::of(2_500);
        let organization = trail.organization();
        let job = queued(&trail, &exports).await;

        // The first worker claims it and gets partway.
        let claimed = AuditExportJob {
            state: JobState::Running,
            claimed_by: "worker-b".to_owned(),
            claimed_at: Some(OffsetDateTime::now_utc()),
            ..job.clone()
        };
        exports
            .write(&exports.job_key(organization, &job.job_id), &claimed)
            .await
            .expect("a claimed job");

        let seen = exports
            .run(&trail, organization, &job.job_id, "worker-a")
            .await
            .expect("a job somebody else holds");
        assert_eq!(seen.claimed_by, "worker-b");
        assert!(seen.shards.is_empty(), "no shard was written beside it");
    }

    #[tokio::test]
    async fn an_export_stops_when_its_requester_stops_administering() {
        let bytes = Arc::new(Bytes::default());
        let exports = exports(&bytes);
        let trail = Trail::of(2_500);
        let organization = trail.organization();
        let job = queued(&trail, &exports).await;

        trail.admits.store(false, Ordering::SeqCst);
        let parked = exports
            .run(&trail, organization, &job.job_id, "worker-a")
            .await
            .expect("a parked job");
        // Failed rather than requeued: a demotion will be just as true on the
        // third attempt, and the retry budget spent on it only delays the
        // message that says so.
        assert_eq!(parked.state, JobState::Failed);
        assert_eq!(parked.attempts, 1);
        assert!(parked.shards.is_empty());
    }

    #[tokio::test]
    async fn entries_a_sweep_already_took_are_counted_rather_than_missed_quietly() {
        let bytes = Arc::new(Bytes::default());
        let exports = exports(&bytes);
        let mut trail = Trail::of(1_500);
        let organization = trail.organization();
        let job = queued(&trail, &exports).await;
        assert_eq!(job.through_sequence, 1_500);

        // The retention sweep runs between the job being queued and the worker
        // picking it up.
        trail.swept_through(400);

        let done = exports
            .run(&trail, organization, &job.job_id, "worker-a")
            .await
            .expect("a finished export");
        assert_eq!(done.state, JobState::Completed);
        assert_eq!(done.counts.entries, 1_100);
        assert_eq!(done.counts.missing, 400);
        assert_eq!(done.cursor, 1_500);
    }

    #[tokio::test]
    async fn the_same_range_over_an_unchanged_trail_reaches_the_same_reference() {
        let bytes = Arc::new(Bytes::default());
        let exports = exports(&bytes);
        let trail = Trail::of(1_200);
        let organization = trail.organization();

        let first = queued(&trail, &exports).await;
        let first = exports
            .run(&trail, organization, &first.job_id, "worker-a")
            .await
            .expect("a finished export");
        let second = queued(&trail, &exports).await;
        assert_ne!(second.job_id, first.job_id, "a second ask is a second job");
        let second = exports
            .run(&trail, organization, &second.job_id, "worker-a")
            .await
            .expect("a second finished export");

        assert_eq!(first.version, second.version);
    }

    #[tokio::test]
    async fn a_retried_request_joins_the_job_it_already_started() {
        let bytes = Arc::new(Bytes::default());
        let exports = exports(&bytes);
        let trail = Trail::of(10);

        let first = queued(&trail, &exports).await;
        let again = queued(&trail, &exports).await;
        assert_eq!(first.job_id, again.job_id);
    }

    #[tokio::test]
    async fn a_finished_export_cannot_be_cancelled_and_a_running_one_can() {
        let bytes = Arc::new(Bytes::default());
        let exports = exports(&bytes);
        let trail = Trail::of(10);
        let organization = trail.organization();
        let job = queued(&trail, &exports).await;

        let cancelled = exports
            .cancel(&trail, organization, &principal(), &job.job_id)
            .await
            .expect("a cancelled export");
        assert_eq!(cancelled.state, JobState::Cancelled);
        assert!(matches!(
            exports
                .cancel(&trail, organization, &principal(), &job.job_id)
                .await,
            Err(Error::Conflict(_))
        ));
        assert!(matches!(
            exports
                .rows(&trail, organization, &principal(), &job.job_id, 0, 10)
                .await,
            Err(Error::Conflict(_))
        ));
    }

    #[tokio::test]
    async fn an_export_cancelled_while_it_ran_is_not_reported_as_finished() {
        let bytes = Arc::new(Bytes::default());
        let exports = exports(&bytes);
        let trail = Trail::of(10);
        let organization = trail.organization();
        let job = queued(&trail, &exports).await;

        // Cancelled after the worker claimed it and before it wrote anything —
        // ten entries never reach a shard boundary.
        exports
            .write(
                &exports.job_key(organization, &job.job_id),
                &AuditExportJob {
                    state: JobState::Cancelled,
                    finished_at: Some(OffsetDateTime::now_utc()),
                    claimed_by: "worker-a".to_owned(),
                    claimed_at: Some(OffsetDateTime::now_utc()),
                    ..job.clone()
                },
            )
            .await
            .expect("a cancelled job");

        let seen = exports
            .run(&trail, organization, &job.job_id, "worker-a")
            .await
            .expect("a cancelled export");
        assert_eq!(seen.state, JobState::Cancelled);
        assert!(seen.version.is_none(), "nothing was named");
    }

    #[tokio::test]
    async fn an_export_of_an_empty_range_is_refused_rather_than_queued() {
        let bytes = Arc::new(Bytes::default());
        let exports = exports(&bytes);
        let trail = Trail::of(10);
        assert!(matches!(
            exports
                .create(
                    &trail,
                    trail.organization(),
                    &principal(),
                    AuditExportRequest {
                        reason: String::new(),
                        after_sequence: 10,
                    },
                )
                .await,
            Err(Error::Conflict(_))
        ));
    }

    #[tokio::test]
    async fn a_queued_job_is_claimable_and_a_finished_one_is_not() {
        let bytes = Arc::new(Bytes::default());
        let exports = exports(&bytes);
        let trail = Trail::of(10);
        let organization = trail.organization();
        let job = queued(&trail, &exports).await;

        let now = OffsetDateTime::now_utc();
        assert_eq!(
            exports.claimable(now).await.expect("a listing"),
            vec![(organization, job.job_id.clone())]
        );
        exports
            .run(&trail, organization, &job.job_id, "worker-a")
            .await
            .expect("a finished export");
        assert!(exports.claimable(now).await.expect("a listing").is_empty());
    }

    #[tokio::test]
    async fn a_running_job_whose_lease_ran_out_is_claimable_again() {
        let bytes = Arc::new(Bytes::default());
        let exports = exports(&bytes);
        let trail = Trail::of(10);
        let organization = trail.organization();
        let job = queued(&trail, &exports).await;
        let stale =
            OffsetDateTime::now_utc() - time::Duration::seconds(aiwatcher_jobs::LEASE_SECONDS + 1);
        exports
            .write(
                &exports.job_key(organization, &job.job_id),
                &AuditExportJob {
                    state: JobState::Running,
                    claimed_by: "a process that died".to_owned(),
                    claimed_at: Some(stale),
                    ..job.clone()
                },
            )
            .await
            .expect("a stale claim");

        assert_eq!(
            exports
                .claimable(OffsetDateTime::now_utc())
                .await
                .expect("a listing"),
            vec![(organization, job.job_id)]
        );
    }
}
