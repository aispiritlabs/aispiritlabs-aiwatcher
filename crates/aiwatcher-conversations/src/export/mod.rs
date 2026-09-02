//! Freezing a selection of the archive into an immutable, reproducible corpus.
//!
//! Asynchronous, because the synchronous exporter this replaced could not be
//! anything else: it read every event of every run of a conversation into one
//! process, built one JSON body and posted it, and the 1 000-row and 4 MiB caps
//! that made that safe are not caps a real corpus fits inside. Making the body
//! bigger would only move the number.
//!
//! ```text
//!   POST  ──►  queued ──► running ──► completed  name@sha256
//!                 ▲          │  │
//!                 └──────────┘  └──► failed      the retry budget ran out
//!                  retryable         cancelled   somebody asked
//! ```
//!
//! Four properties, and each one is a thing the synchronous exporter did not
//! have.
//!
//! **The selection is pinned when the job is created.** The conversation list
//! is resolved once and stored on the job, so a conversation that starts while
//! the export is running does not appear halfway through it — which would make
//! the same request produce a different corpus depending on how long it took.
//!
//! **A shard never splits a conversation.** Rows are buffered until a whole
//! conversation is rendered, whatever the shape asks for, so a chat row — which
//! *is* a conversation — cannot straddle two shards. The cost is a shard that
//! overshoots its target by one conversation's worth of rows, which is a bound
//! on a conversation rather than on the corpus.
//!
//! **A shard is written before the cursor that passes it.** The unit of resume
//! is a shard, so a crash re-does at most one shard's conversations and writes
//! byte-identical bytes to the same key. The reverse ordering would advance
//! past rows that were never stored, which is the one failure an export must
//! not have: a corpus missing rows nothing can tell you about.
//!
//! **The version is a function of the content.** `sha256(request ‖ every shard
//! digest, in order)`, computed from the shards rather than from a running hash
//! nobody could resume. The same request over an unchanged archive produces the
//! same version, which is what makes `name@version` a thing a training run can
//! name — the rule ADR_0018 refuses a promotion without.
//!
//! **Every row that was left out is counted, by reason.** An export that
//! silently produced 40 rows from 4 000 turns is indistinguishable from one
//! that worked; the counts are what turns "the corpus is small" into "3 900
//! turns are still waiting for review".

pub mod format;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;

use crate::archive::{ArchivedTurn, IndexShard};
use crate::policy::TrainingScope;
use crate::redaction::FindingKind;
use crate::review::ReviewState;
use crate::store::Backend;
use crate::turn::{Role, TurnContent, TurnState};
use crate::{
    EXPORT_SHARD_ROWS, Error, MAX_ROW_PAGE, Result, digest, validate_digest, validate_name,
};

pub use aiwatcher_jobs::{JobState, LEASE_SECONDS, MAX_ATTEMPTS, ShardRef};

use format::{ExportFormat, Selected};

/// Times one selection may be exported before the ids run out. A thousand
/// nightly exports of one corpus is nearly three years.
const MAX_GENERATIONS: u32 = 1_000;
/// Individual exclusions kept alongside the counts.
///
/// The counts are complete and the list is a sample, because an export that
/// excluded a million rows would otherwise produce a manifest nobody can open.
/// Which is which is stated on the manifest, so nobody reads the sample as the
/// total.
pub const MAX_EXCLUSION_SAMPLES: usize = 1_000;

/// Which conversations an export considers.
#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportSelection {
    /// Named explicitly. Empty means every conversation in the archive, which
    /// is the common case and the one that needs the window below.
    #[serde(default)]
    pub conversations: Vec<String>,
    /// Only conversations last active at or after this moment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "time::serde::rfc3339::option")]
    #[schema(value_type = Option<String>)]
    pub since: Option<OffsetDateTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "time::serde::rfc3339::option")]
    #[schema(value_type = Option<String>)]
    pub until: Option<OffsetDateTime>,
}

/// What was asked for. The identity of an export, along with the archive's
/// content at the time.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = ConversationExportRequest)]
pub struct ExportRequest {
    /// `training/agent-turns`. The mutable half of `name@version`.
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub format: ExportFormat,
    #[serde(default)]
    pub selection: ExportSelection,
    /// Whether a turn nobody approved may be included. Defaults to `true`,
    /// which means "no" — the annotation registry's rule, and the reason the
    /// review queue exists.
    #[serde(default = "yes")]
    pub require_human_review: bool,
    /// What the recorded consent has to permit. Defaults to
    /// [`TrainingScope::Train`], the widest of the three.
    #[serde(default = "train")]
    pub required_scope: TrainingScope,
    /// Finding kinds that exclude a turn. Defaults to the three that make
    /// content unsafe to train on rather than merely awkward.
    #[serde(default = "unsafe_findings")]
    pub exclude_findings: Vec<FindingKind>,
    /// Roles to include. Empty means every role, which is what a chat shape
    /// wants; a prompt/response shape ignores this and takes what it needs.
    #[serde(default)]
    pub roles: Vec<Role>,
}

fn yes() -> bool {
    true
}

fn train() -> TrainingScope {
    TrainingScope::Train
}

fn unsafe_findings() -> Vec<FindingKind> {
    vec![FindingKind::Secret, FindingKind::Pii, FindingKind::Unsafe]
}

impl ExportRequest {
    /// # Errors
    ///
    /// [`Error::Invalid`] for a name or a conversation id that is unusable.
    pub fn validate(&self) -> Result<()> {
        validate_name(&self.name, "the export name")?;
        for conversation_id in &self.selection.conversations {
            validate_name(conversation_id, "a selected conversation_id")?;
        }
        if let (Some(since), Some(until)) = (self.selection.since, self.selection.until)
            && since > until
        {
            return Err(Error::Invalid(
                "the selection window ends before it starts".to_owned(),
            ));
        }
        Ok(())
    }

    /// The identity of "what was asked", independent of what the archive held.
    #[must_use]
    pub fn digest(&self) -> String {
        digest(&serde_json::to_vec(self).unwrap_or_default())
    }
}

/// Why a turn did not reach the corpus.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize, ToSchema,
)]
#[serde(rename_all = "snake_case")]
#[schema(as = ConversationExclusionReason)]
pub enum ExclusionReason {
    /// Nobody has reviewed it.
    NotReviewed,
    /// A reviewer said no.
    ReviewRejected,
    /// The content is gone: retention ran out, or somebody asked.
    Erased,
    /// No lawful basis was ever recorded.
    ConsentMissing,
    /// The recorded consent does not cover what this export asks for.
    ScopeNotPermitted,
    /// The scanner or a reviewer found something this export excludes on.
    Finding,
    /// The format asked for other roles.
    RoleFiltered,
    /// The head says the content is there and the object is not. A corrupt
    /// archive rather than a policy decision, and counted separately so it
    /// cannot hide among the ordinary exclusions.
    ContentMissing,
}

impl ExclusionReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotReviewed => "not_reviewed",
            Self::ReviewRejected => "review_rejected",
            Self::Erased => "erased",
            Self::ConsentMissing => "consent_missing",
            Self::ScopeNotPermitted => "scope_not_permitted",
            Self::Finding => "finding",
            Self::RoleFiltered => "role_filtered",
            Self::ContentMissing => "content_missing",
        }
    }
}

/// One turn that was left out, named.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[schema(as = ConversationExportExclusion)]
pub struct ExportExclusion {
    pub turn_id: String,
    pub conversation_id: String,
    pub reason: ExclusionReason,
    /// The rule that fired, for a [`ExclusionReason::Finding`].
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, ToSchema)]
#[schema(as = ConversationExportCounts)]
pub struct ExportCounts {
    pub conversations: usize,
    pub turns_considered: usize,
    pub turns_included: usize,
    pub turns_excluded: usize,
    pub rows: usize,
}

/// A resumable export, as it sits in the store.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct ExportJob {
    pub job_id: String,
    pub request: ExportRequest,
    pub request_digest: String,
    pub state: JobState,
    /// The conversations this job will read, resolved once and never re-read.
    #[serde(default)]
    pub conversations: Vec<String>,
    /// How many of them are already in a written shard. The resume point.
    #[serde(default)]
    pub cursor: usize,
    #[serde(default)]
    pub attempts: u32,
    /// The worker currently holding this job: a pod name in a cluster.
    ///
    /// Not an owner — a lease. It expires, which is what lets a job survive the
    /// process that was running it, and it is re-checked at every shard, which
    /// is what stops the process it outlived from writing beside its
    /// replacement.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub claimed_by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "time::serde::rfc3339::option")]
    #[schema(value_type = Option<String>)]
    pub claimed_at: Option<OffsetDateTime>,
    #[serde(default)]
    pub counts: ExportCounts,
    /// Complete. Keyed by [`ExclusionReason::as_str`].
    #[serde(default)]
    pub exclusions: BTreeMap<String, usize>,
    /// A sample, capped at [`MAX_EXCLUSION_SAMPLES`].
    #[serde(default)]
    pub excluded: Vec<ExportExclusion>,
    #[serde(default)]
    pub shards: Vec<ShardRef>,
    /// The immutable reference, once there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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

impl ExportJob {
    /// Whether nobody is demonstrably working on this right now.
    ///
    /// A job with no claim at all is expired by definition: that is either a
    /// queued job or one written before leases existed.
    #[must_use]
    pub fn lease_expired(&self, now: OffsetDateTime) -> bool {
        aiwatcher_jobs::lease_expired(self.claimed_at, now)
    }

    /// What is left to read, as a fraction. `None` before the selection is
    /// resolved, and never a guess: the denominator is a pinned list.
    #[must_use]
    pub fn progress(&self) -> Option<f64> {
        aiwatcher_jobs::progress(self.cursor, self.conversations.len())
    }

    #[must_use]
    pub fn summary(&self) -> ExportJobSummary {
        ExportJobSummary {
            job_id: self.job_id.clone(),
            name: self.request.name.clone(),
            format: self.request.format,
            state: self.state,
            claimed_by: self.claimed_by.clone(),
            conversations: self.conversations.len(),
            cursor: self.cursor,
            counts: self.counts,
            version: self.version.clone(),
            error: self.error.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            finished_at: self.finished_at,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct ExportJobSummary {
    pub job_id: String,
    pub name: String,
    pub format: ExportFormat,
    pub state: JobState,
    /// Which worker holds it, while one does. Answers "why has this been
    /// running for twenty minutes" without opening a log.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub claimed_by: String,
    pub conversations: usize,
    pub cursor: usize,
    pub counts: ExportCounts,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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

#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct ExportJobPage {
    pub jobs: Vec<ExportJobSummary>,
}

/// Why a corpus was withdrawn, and when.
///
/// An export is immutable and a withdrawal does not change that: the manifest
/// keeps every count, digest and exclusion it had. What is gone is the rows,
/// because the alternative is a published corpus holding words somebody asked
/// to have deleted — and an erasure that stopped at the archive would be an
/// erasure in name only.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct Withdrawal {
    /// The conversations whose erasure caused this.
    pub conversations: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub by: String,
    #[serde(with = "time::serde::rfc3339")]
    #[schema(value_type = String)]
    pub at: OffsetDateTime,
}

/// What a finished export is, forever.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[schema(as = ConversationExportManifest)]
pub struct ExportManifest {
    pub name: String,
    pub version: String,
    pub job_id: String,
    /// The conversations this export read, pinned when the job was created.
    ///
    /// Kept on the manifest rather than only on the job, because it is what an
    /// erasure searches: "which published corpora hold this person's words" has
    /// to be answerable from the manifests alone.
    #[serde(default)]
    pub conversations: Vec<String>,
    /// Set when an erasure took this corpus' rows away. The manifest remains.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub withdrawn: Option<Withdrawal>,
    pub request: ExportRequest,
    pub request_digest: String,
    pub counts: ExportCounts,
    pub exclusions: BTreeMap<String, usize>,
    /// A sample of the excluded turns. `excluded_truncated` says whether the
    /// counts above are larger than this list.
    #[serde(default)]
    pub excluded: Vec<ExportExclusion>,
    #[serde(default)]
    pub excluded_truncated: bool,
    pub shards: Vec<ShardRef>,
    #[serde(with = "time::serde::rfc3339")]
    #[schema(value_type = String)]
    pub built_at: OffsetDateTime,
}

impl ExportManifest {
    /// `name@version`, the reference a training run records.
    #[must_use]
    pub fn reference(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct ExportRowsPage {
    #[schema(value_type = Vec<Object>)]
    pub rows: Vec<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<usize>,
    pub total: usize,
}

/// Which versions of one name exist.
#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct ExportIndex {
    pub name: String,
    #[serde(default)]
    pub versions: Vec<ExportVersionSummary>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct ExportVersionSummary {
    pub version: String,
    pub format: ExportFormat,
    pub rows: usize,
    /// True once an erasure has taken this corpus' rows away.
    #[serde(default)]
    pub withdrawn: bool,
    #[serde(with = "time::serde::rfc3339")]
    #[schema(value_type = String)]
    pub built_at: OffsetDateTime,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
#[schema(as = ConversationExportPage)]
pub struct ExportPage {
    pub exports: Vec<ExportIndex>,
}

// ── Operations ───────────────────────────────────────────────────────────────

/// Queue an export, pinning what it will read.
pub(crate) async fn create(
    backend: &Backend,
    request: ExportRequest,
    created_by: &str,
) -> Result<ExportJob> {
    request.validate()?;
    let selection = &request.selection;
    let mut conversations: Vec<String> = if selection.conversations.is_empty() {
        crate::archive::conversations(backend)
            .await?
            .conversations
            .into_iter()
            .filter(|head| {
                selection.since.is_none_or(|since| head.last_seen >= since)
                    && selection.until.is_none_or(|until| head.first_seen <= until)
            })
            .map(|head| head.conversation_id)
            .collect()
    } else {
        selection.conversations.clone()
    };
    // Sorted, so the order a job reads in does not depend on how the object
    // store happened to list its keys.
    conversations.sort_unstable();
    conversations.dedup();

    if conversations.is_empty() {
        return Err(Error::Refused(
            "the selection matched no conversation; an export of nothing is not a corpus"
                .to_owned(),
        ));
    }

    let now = OffsetDateTime::now_utc();
    let request_digest = request.digest();
    // Derived rather than random, so a retried POST joins the job it already
    // started instead of exporting the same thing twice — and *generational*,
    // so asking again after one finished starts a new one. Without the
    // generation, "export again now that another two hundred turns are
    // reviewed" would silently hand back the corpus built before the review,
    // which is the failure that is hardest to notice: the request succeeds and
    // the rows are stale.
    let mut job_id = String::new();
    for generation in 0..MAX_GENERATIONS {
        job_id = digest(
            format!(
                "{request_digest}\0{generation}\0{}",
                conversations.join("\u{0}")
            )
            .as_bytes(),
        );
        match backend.read::<ExportJob>(&backend.job_key(&job_id)).await? {
            Some(existing) if !existing.state.is_finished() => return Ok(existing),
            Some(_) => continue,
            None => break,
        }
    }

    let job = ExportJob {
        job_id,
        request_digest,
        request,
        state: JobState::Queued,
        conversations,
        cursor: 0,
        attempts: 0,
        claimed_by: String::new(),
        claimed_at: None,
        counts: ExportCounts::default(),
        exclusions: BTreeMap::new(),
        excluded: Vec::new(),
        shards: Vec::new(),
        version: None,
        error: None,
        created_by: created_by.to_owned(),
        created_at: now,
        updated_at: now,
        finished_at: None,
    };
    backend.write(&backend.job_key(&job.job_id), &job).await?;
    Ok(job)
}

pub(crate) async fn job(backend: &Backend, job_id: &str) -> Result<ExportJob> {
    validate_digest(job_id, "job_id")?;
    backend
        .read(&backend.job_key(job_id))
        .await?
        .ok_or_else(|| Error::NotFound(format!("the export job {job_id}")))
}

pub(crate) async fn jobs(backend: &Backend) -> Result<ExportJobPage> {
    let entries = backend.list(&backend.jobs_prefix()).await?;
    let mut jobs = Vec::new();
    for entry in entries {
        if let Some(job) = backend.read::<ExportJob>(&entry.key).await? {
            jobs.push(job.summary());
        }
    }
    jobs.sort_by_key(|job| std::cmp::Reverse(job.created_at));
    Ok(ExportJobPage { jobs })
}

pub(crate) async fn cancel(backend: &Backend, job_id: &str) -> Result<ExportJob> {
    let mut job = job(backend, job_id).await?;
    if job.state.is_finished() {
        return Err(Error::Refused(format!(
            "the export job {job_id} already finished as {}",
            job.state.as_str()
        )));
    }
    job.state = JobState::Cancelled;
    job.finished_at = Some(OffsetDateTime::now_utc());
    job.updated_at = job.finished_at.unwrap_or(job.updated_at);
    backend.write(&backend.job_key(job_id), &job).await?;
    Ok(job)
}

/// Whatever is waiting to be worked on, oldest first.
/// Job ids nobody is working on, oldest first.
pub(crate) async fn claimable(backend: &Backend, now: OffsetDateTime) -> Result<Vec<String>> {
    let mut waiting: Vec<(OffsetDateTime, String)> = Vec::new();
    for entry in backend.list(&backend.jobs_prefix()).await? {
        let Some(job) = backend.read::<ExportJob>(&entry.key).await? else {
            continue;
        };
        match job.state {
            JobState::Queued => waiting.push((job.created_at, job.job_id)),
            // `Running` is claimable once its lease has run out, and that is
            // the restart case: a process that died mid-export left its job
            // saying `running`, and nothing else would ever move it. The lease
            // is what tells that apart from a worker that is simply still
            // going.
            JobState::Running if job.lease_expired(now) => {
                waiting.push((job.created_at, job.job_id));
            }
            _ => {}
        }
    }
    waiting.sort();
    Ok(waiting.into_iter().map(|(_, job_id)| job_id).collect())
}

/// Run one job to completion, or until it is cancelled, fails, or is taken
/// over.
///
/// `worker` is this process's identity — a pod name in a cluster. Two things
/// depend on it. A job whose lease is live and held by somebody *else* is left
/// alone, so a rolling update does not put two workers on one export. And the
/// lease is re-checked at every shard boundary, so a worker whose lease expired
/// under it — because a shard took longer than [`LEASE_SECONDS`] — stops
/// instead of writing shards a second worker is also writing.
///
/// Re-checking is what closes the only case where two workers could actually
/// corrupt a corpus. Two deterministic workers over an *unchanged* archive
/// converge: same cursor, same shard index, same bytes, same digest. It is when
/// the archive changes under them — a turn reviewed mid-export — that they
/// produce different bytes for one shard index, and the last job record written
/// would then name digests that do not describe the stored shards.
pub(crate) async fn run(backend: &Backend, job_id: &str, worker: &str) -> Result<ExportJob> {
    let mut job = job(backend, job_id).await?;
    if job.state.is_finished() {
        return Ok(job);
    }
    let now = OffsetDateTime::now_utc();
    // Taking over a live lease held by another worker is the thing the lease
    // exists to prevent. Taking over *our own* is not: a pod that restarted
    // has the same name and the process that held it is gone.
    if job.state == JobState::Running && !job.lease_expired(now) && job.claimed_by != worker {
        tracing::debug!(
            job_id,
            held_by = %job.claimed_by,
            "another worker holds this export's lease"
        );
        return Ok(job);
    }
    job.state = JobState::Running;
    job.claimed_by = worker.to_owned();
    job.claimed_at = Some(now);
    job.updated_at = now;
    backend.write(&backend.job_key(job_id), &job).await?;

    let mut staged = Staged::default();

    while job.cursor + staged.conversations < job.conversations.len() {
        let index = job.cursor + staged.conversations;
        let conversation_id = job.conversations[index].clone();
        match gather(
            backend,
            &job.request,
            &job.counts,
            &mut staged,
            &conversation_id,
        )
        .await
        {
            Ok(()) => {}
            // An unreachable object store is worth coming back for; a corrupt
            // document will be just as corrupt on the third attempt, and
            // spending the retry budget on it only delays the message that
            // says so.
            Err(error) => return park(backend, job, &error, is_retryable(&error)).await,
        }

        if staged.rows.len() >= EXPORT_SHARD_ROWS {
            // A cancellation and a lost lease both land between shards.
            // Checking more often would mean re-reading the job per
            // conversation; checking less often would mean a cancelled job
            // that keeps writing, or two workers writing one shard.
            if let Some(stopped) = interrupted(backend, job_id, worker).await? {
                return Ok(stopped);
            }
            flush(backend, &mut job, &mut staged, worker).await?;
        }
    }
    if !staged.rows.is_empty() || staged.conversations > 0 {
        if let Some(stopped) = interrupted(backend, job_id, worker).await? {
            return Ok(stopped);
        }
        flush(backend, &mut job, &mut staged, worker).await?;
    }
    complete(backend, job, worker).await
}

/// What has been read since the last shard was written.
///
/// Held apart from the job for the same reason the importer holds a page's
/// tally apart from its own: a shard that is never written *will be read
/// again*, and counts already folded into the job record would then describe
/// those conversations twice. A crash the right way round costs one shard of
/// duplicated work; it must not also cost a manifest whose exclusion counts do
/// not add up.
#[derive(Default)]
struct Staged {
    /// Conversations read since the last flush. The amount the cursor moves.
    conversations: usize,
    rows: Vec<serde_json::Value>,
    counts: ExportCounts,
    exclusions: BTreeMap<String, usize>,
    excluded: Vec<ExportExclusion>,
}

/// The stored job, when this worker should stop writing to it.
///
/// `None` means carry on. `Some` is either a cancellation or a lease this
/// worker no longer holds, and in both cases what it does *not* do is write:
/// the caller returns before touching a shard.
async fn interrupted(backend: &Backend, job_id: &str, worker: &str) -> Result<Option<ExportJob>> {
    let current = job(backend, job_id).await?;
    if current.state == JobState::Cancelled {
        return Ok(Some(current));
    }
    if current.claimed_by != worker {
        tracing::warn!(
            job_id,
            taken_by = %current.claimed_by,
            "this export's lease was taken over; stopping rather than writing beside it"
        );
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
/// The cursor is untouched either way, which is the whole point: a job that
/// comes back picks up at the last shard it committed, and one that does not
/// still says how far it got.
async fn park(
    backend: &Backend,
    mut job: ExportJob,
    error: &Error,
    retryable: bool,
) -> Result<ExportJob> {
    job.attempts += 1;
    job.error = Some(error.to_string());
    job.updated_at = OffsetDateTime::now_utc();
    // Released rather than left to expire: a requeued job should be picked up
    // on the next tick, not five minutes later.
    job.claimed_by = String::new();
    job.claimed_at = None;
    job.state = aiwatcher_jobs::after_failure(job.attempts, retryable);
    if job.state == JobState::Failed {
        job.finished_at = Some(job.updated_at);
    }
    backend.write(&backend.job_key(&job.job_id), &job).await?;
    Ok(job)
}

/// One conversation's rows, with every excluded turn counted.
async fn gather(
    backend: &Backend,
    request: &ExportRequest,
    committed: &ExportCounts,
    staged: &mut Staged,
    conversation_id: &str,
) -> Result<()> {
    let Some(head) = crate::archive::conversation_head(backend, conversation_id).await? else {
        staged.counts.conversations += 1;
        staged.conversations += 1;
        return Ok(());
    };

    let mut turns: Vec<ArchivedTurn> = Vec::new();
    for shard in 0..head.shards {
        let entries: IndexShard = backend
            .read(&backend.index_key(conversation_id, shard))
            .await?
            .unwrap_or_default();
        for turn_id in entries.entries {
            if let Some(turn) = backend
                .read::<ArchivedTurn>(&backend.turn_key(conversation_id, &turn_id))
                .await?
            {
                turns.push(turn);
            }
        }
    }

    let mut kept: Vec<(ArchivedTurn, TurnContent)> = Vec::new();
    for turn in turns {
        staged.counts.turns_considered += 1;
        if let Some((reason, detail)) = excluded_for(&turn, request) {
            exclude(staged, committed, &turn, reason, detail);
            continue;
        }
        let Some(content) = backend
            .open::<TurnContent>(&backend.content_key(&turn.turn_id))
            .await?
        else {
            exclude(
                staged,
                committed,
                &turn,
                ExclusionReason::ContentMissing,
                String::new(),
            );
            continue;
        };
        staged.counts.turns_included += 1;
        kept.push((turn, content));
    }

    let selected: Vec<Selected<'_>> = kept
        .iter()
        .map(|(turn, content)| Selected { turn, content })
        .collect();
    let rows = format::rows(request.format, conversation_id, &selected);
    staged.counts.conversations += 1;
    staged.counts.rows += rows.len();
    staged.rows.extend(rows);
    staged.conversations += 1;
    Ok(())
}

/// Every gate, in the order that gives the most useful reason.
///
/// Erasure first, because a turn whose content is gone is not "unreviewed"
/// however true that also is, and a reason that sends somebody to the review
/// queue for a row nothing can produce wastes their afternoon.
fn excluded_for(turn: &ArchivedTurn, request: &ExportRequest) -> Option<(ExclusionReason, String)> {
    if turn.state == TurnState::Erased {
        return Some((ExclusionReason::Erased, String::new()));
    }
    if !request.roles.is_empty() && !request.roles.contains(&turn.role) {
        return Some((ExclusionReason::RoleFiltered, turn.role.to_string()));
    }
    if !turn.policy.consent.basis.is_stated() {
        return Some((ExclusionReason::ConsentMissing, String::new()));
    }
    if !turn.permits(request.required_scope) {
        return Some((
            ExclusionReason::ScopeNotPermitted,
            request.required_scope.as_str().to_owned(),
        ));
    }
    for finding in &turn.findings {
        if request.exclude_findings.contains(&finding.kind) {
            return Some((ExclusionReason::Finding, finding.rule.clone()));
        }
    }
    match turn.review.state {
        ReviewState::Rejected => Some((ExclusionReason::ReviewRejected, String::new())),
        ReviewState::Pending if request.require_human_review => {
            Some((ExclusionReason::NotReviewed, String::new()))
        }
        _ => None,
    }
}

fn exclude(
    staged: &mut Staged,
    committed: &ExportCounts,
    turn: &ArchivedTurn,
    reason: ExclusionReason,
    detail: String,
) {
    staged.counts.turns_excluded += 1;
    *staged
        .exclusions
        .entry(reason.as_str().to_owned())
        .or_default() += 1;
    // The sample is capped across the whole export, so what is already
    // committed counts towards the cap.
    if committed.turns_excluded.min(MAX_EXCLUSION_SAMPLES) + staged.excluded.len()
        < MAX_EXCLUSION_SAMPLES
    {
        staged.excluded.push(ExportExclusion {
            turn_id: turn.turn_id.clone(),
            conversation_id: turn.conversation_id.clone(),
            reason,
            detail,
        });
    }
}

/// Write one shard, then the cursor that passes it. Never the other way round.
async fn flush(
    backend: &Backend,
    job: &mut ExportJob,
    staged: &mut Staged,
    worker: &str,
) -> Result<()> {
    let index = job.shards.len();
    let mut bytes = Vec::new();
    for row in &staged.rows {
        bytes.extend_from_slice(&serde_json::to_vec(row).map_err(|error| Error::Corrupt {
            key: backend.shard_key(&job.job_id, index),
            message: error.to_string(),
        })?);
        bytes.push(b'\n');
    }
    let shard_digest = digest(&bytes);
    backend
        .seal_bytes(&backend.shard_key(&job.job_id, index), &bytes)
        .await?;

    job.shards.push(ShardRef {
        index,
        rows: staged.rows.len(),
        digest: shard_digest,
    });
    // Only now do the conversations behind that shard count. Everything from
    // here to the write below moves together: cursor, counts, exclusions and
    // lease are one record.
    job.cursor += staged.conversations;
    job.counts.conversations += staged.counts.conversations;
    job.counts.turns_considered += staged.counts.turns_considered;
    job.counts.turns_included += staged.counts.turns_included;
    job.counts.turns_excluded += staged.counts.turns_excluded;
    job.counts.rows += staged.counts.rows;
    for (reason, count) in std::mem::take(&mut staged.exclusions) {
        *job.exclusions.entry(reason).or_default() += count;
    }
    job.excluded.append(&mut staged.excluded);
    job.updated_at = OffsetDateTime::now_utc();
    // The cursor and the lease are renewed in one write, which is why the lease
    // bounds a shard rather than an export: as long as shards keep landing, the
    // claim keeps holding.
    job.claimed_by = worker.to_owned();
    job.claimed_at = Some(job.updated_at);
    backend.write(&backend.job_key(&job.job_id), &*job).await?;

    *staged = Staged::default();
    Ok(())
}

/// Seal the version, write the manifest, then index it.
async fn complete(backend: &Backend, mut job: ExportJob, worker: &str) -> Result<ExportJob> {
    let version = aiwatcher_jobs::version_of(&job.request_digest, &job.shards);
    let now = OffsetDateTime::now_utc();

    let manifest = ExportManifest {
        name: job.request.name.clone(),
        version: version.clone(),
        job_id: job.job_id.clone(),
        conversations: job.conversations.clone(),
        withdrawn: None,
        request: job.request.clone(),
        request_digest: job.request_digest.clone(),
        counts: job.counts,
        exclusions: job.exclusions.clone(),
        excluded: job.excluded.clone(),
        excluded_truncated: job.counts.turns_excluded > job.excluded.len(),
        shards: job.shards.clone(),
        built_at: now,
    };

    // The manifest before the index entry that lists it — the ordering every
    // registry in this workspace keeps, for the same reason.
    backend
        .write(
            &backend.manifest_key(&manifest.name, &manifest.version),
            &manifest,
        )
        .await?;

    let index_key = backend.export_index_key(&manifest.name);
    let mut index: ExportIndex = backend
        .read(&index_key)
        .await?
        .unwrap_or_else(|| ExportIndex {
            name: manifest.name.clone(),
            versions: Vec::new(),
        });
    if !index
        .versions
        .iter()
        .any(|summary| summary.version == manifest.version)
    {
        index.versions.insert(
            0,
            ExportVersionSummary {
                version: manifest.version.clone(),
                format: manifest.request.format,
                rows: manifest.counts.rows,
                withdrawn: false,
                built_at: now,
            },
        );
        backend.write(&index_key, &index).await?;
    }

    job.state = JobState::Completed;
    job.version = Some(version);
    job.error = None;
    job.finished_at = Some(now);
    job.updated_at = now;
    // Kept rather than cleared: "which worker built this" is worth having on a
    // finished job, and nothing claims a finished one.
    job.claimed_by = worker.to_owned();
    backend.write(&backend.job_key(&job.job_id), &job).await?;
    Ok(job)
}

pub(crate) async fn manifest(
    backend: &Backend,
    name: &str,
    version: &str,
) -> Result<ExportManifest> {
    validate_name(name, "the export name")?;
    validate_digest(version, "the export version")?;
    backend
        .read(&backend.manifest_key(name, version))
        .await?
        .ok_or_else(|| Error::NotFound(format!("the export {name}@{version}")))
}

pub(crate) async fn exports(backend: &Backend) -> Result<ExportPage> {
    let entries = backend.list(&backend.export_indexes_prefix()).await?;
    let mut exports = Vec::new();
    for entry in entries {
        if let Some(index) = backend.read::<ExportIndex>(&entry.key).await? {
            exports.push(index);
        }
    }
    exports.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(ExportPage { exports })
}

/// One page of an immutable export's rows.
pub(crate) async fn rows(
    backend: &Backend,
    name: &str,
    version: &str,
    offset: usize,
    limit: usize,
) -> Result<ExportRowsPage> {
    let manifest = manifest(backend, name, version).await?;
    if let Some(withdrawal) = &manifest.withdrawn {
        return Err(Error::Erased(
            format!("the corpus {name}@{version}"),
            withdrawal
                .at
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_else(|_| "an unknown date".to_owned()),
        ));
    }
    let limit = limit.clamp(1, MAX_ROW_PAGE);
    let total: usize = manifest.shards.iter().map(|shard| shard.rows).sum();

    let mut rows = Vec::new();
    let mut position = 0;
    for shard in &manifest.shards {
        if position + shard.rows <= offset {
            position += shard.rows;
            continue;
        }
        if rows.len() >= limit {
            break;
        }
        let Some(bytes) = backend
            .open_bytes(&backend.shard_key(&manifest.job_id, shard.index))
            .await?
        else {
            return Err(Error::NotFound(format!(
                "shard {} of {name}@{version}",
                shard.index
            )));
        };
        for line in bytes.split(|byte| *byte == b'\n') {
            if line.is_empty() {
                continue;
            }
            if position >= offset && rows.len() < limit {
                rows.push(
                    serde_json::from_slice(line).map_err(|error| Error::Corrupt {
                        key: backend.shard_key(&manifest.job_id, shard.index),
                        message: error.to_string(),
                    })?,
                );
            }
            position += 1;
        }
    }
    let next = offset + rows.len();
    Ok(ExportRowsPage {
        next_offset: (next < total).then_some(next),
        rows,
        total,
    })
}

/// Take the rows away from every published corpus that read one of these
/// conversations.
///
/// The other half of an erasure, and the half that is easy to forget. Erasing
/// the archive and leaving the corpus is an erasure in name only: the words are
/// still readable, through a different route, under a reference somebody has
/// already written into a training run.
///
/// The manifest survives — its counts, its digests and its exclusions are the
/// record of what existed — and only the shards are deleted. A training run
/// that names a withdrawn corpus therefore still resolves to something that can
/// say what happened to it, which is a better answer than a 404.
///
/// Bounded by the number of published corpora rather than by the number of
/// turns, because the manifest carries the conversation list the job pinned.
pub(crate) async fn withdraw_for(
    backend: &Backend,
    conversations: &std::collections::BTreeSet<String>,
    by: &str,
) -> Result<usize> {
    if conversations.is_empty() {
        return Ok(0);
    }
    let mut withdrawn = 0;
    for entry in backend.list(&backend.export_indexes_prefix()).await? {
        let Some(mut index) = backend.read::<ExportIndex>(&entry.key).await? else {
            continue;
        };
        let mut changed = false;
        for summary in &mut index.versions {
            if summary.withdrawn {
                continue;
            }
            let key = backend.manifest_key(&index.name, &summary.version);
            let Some(mut manifest) = backend.read::<ExportManifest>(&key).await? else {
                continue;
            };
            let affected: Vec<String> = manifest
                .conversations
                .iter()
                .filter(|conversation| conversations.contains(*conversation))
                .cloned()
                .collect();
            if affected.is_empty() {
                continue;
            }
            // The shards first, then the manifest that says they are gone —
            // the same ordering as erasing a turn, and for the same reason: a
            // manifest that says `withdrawn` while the rows are still in the
            // bucket would be a lie.
            for shard in &manifest.shards {
                backend
                    .delete(&backend.shard_key(&manifest.job_id, shard.index))
                    .await?;
            }
            manifest.withdrawn = Some(Withdrawal {
                conversations: affected,
                by: by.to_owned(),
                at: OffsetDateTime::now_utc(),
            });
            backend.write(&key, &manifest).await?;
            summary.withdrawn = true;
            changed = true;
            withdrawn += 1;
        }
        if changed {
            backend.write(&entry.key, &index).await?;
        }
    }
    Ok(withdrawn)
}
