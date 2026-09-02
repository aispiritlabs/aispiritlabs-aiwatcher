//! What a long job over an object store is, once you have written two of them.
//!
//! The conversation export (ADR_0021) was the first: a request that could not
//! be a request, because it read a whole archive, wrote a corpus and had to
//! survive the process that started it. The Hub importer is the second, and
//! it is the same machine with different rows in it — which is exactly the
//! moment [plan.md](../../../plan.md) said to decide, because deciding after
//! writing the second one is how the two drift.
//!
//! ```text
//!   POST  ──►  queued ──► running ──► completed   name@sha256
//!                 ▲          │  │
//!                 └──────────┘  └──► failed       the retry budget ran out
//!                  retryable         cancelled    somebody asked
//! ```
//!
//! # What is shared, and what deliberately is not
//!
//! **The rules are shared.** They are the part that must not drift, because
//! each of them is a silent corruption when it is wrong in one copy:
//!
//! * a shard is written **before** the cursor that passes it, so a crash
//!   re-does one shard and never skips rows nothing can tell you about;
//!   [`ORDERING`] states it and every caller's `flush` keeps it;
//! * a job's version is [`version_of`] — `sha256(request ‖ every shard digest,
//!   in order)` — so the same request over unchanged inputs is the same
//!   reference, and a shard swapped for another is a different one;
//! * a lease is renewed per shard and re-checked at every shard boundary
//!   ([`lease_expired`]), so a worker that lost its claim stops rather than
//!   writing beside its replacement;
//! * a retryable failure is requeued until [`MAX_ATTEMPTS`], a rejection is
//!   failed immediately ([`after_failure`]) — getting that backwards either
//!   spins forever or discards good work, which is `PortError`'s rule one
//!   layer up.
//!
//! **The records are not shared**, and that is not an oversight. An export job
//! pins a conversation list and counts exclusions by reason; an import job
//! pins a staged batch and counts rejected rows by reason. A generic
//! `Job<Payload>` would either flatten those into the JSON — which produces a
//! TypeScript intersection the panel cannot narrow, the same reason
//! `ModelDetail` nests rather than flattens — or hide them behind a trait with
//! a dozen accessors, which is more machinery than the thing it abstracts.
//! What each record owes this crate is that it *keeps the rules*, and the
//! rules are functions rather than a base class so that keeping them is a call
//! rather than an inheritance.
//!
//! See ADR_0022.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use utoipa::ToSchema;

/// The ordering every caller of this crate keeps, in one sentence, so a reader
/// of a `flush` that does it the other way round has something to be wrong
/// against.
///
/// It is the same ordering as the pipeline's checkpoint (`flush` commits after
/// the durable write), the prompt registry's head (the version object before
/// the head that indexes it) and the annotation registry's export (the
/// manifest before the index entry). Three registries, one rule, and the
/// sharpest consequence here: a crash the right way round re-does one shard
/// and writes byte-identical bytes, and a crash the wrong way round leaves an
/// artifact missing rows that nothing can tell you about.
pub const ORDERING: &str = "write the shard, then the cursor that passes it";

/// Attempts a job gets before it is failed rather than requeued.
///
/// Three, because the failures worth retrying are transient by definition and
/// the ones that are not will be just as broken on the tenth attempt — while
/// the message that says so waits behind them.
pub const MAX_ATTEMPTS: u32 = 3;

/// How long a worker's claim on a job is trusted, in seconds.
///
/// Renewed at every shard, so this bounds *one shard* rather than a whole job.
/// Five minutes is generous for a shard and short enough that a pod killed
/// without draining is picked up again within a poll or two — and the cost of
/// getting it wrong in either direction is duplicated work, never a corrupted
/// artifact, because a worker whose lease expired under it stops at its next
/// shard boundary rather than writing beside its replacement.
pub const LEASE_SECONDS: i64 = 300;

/// Where a job is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    #[default]
    Queued,
    Running,
    Completed,
    /// The retry budget ran out. The record's `error` says what kept failing.
    Failed,
    Cancelled,
}

impl JobState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    #[must_use]
    pub const fn is_finished(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// One written shard of a staged artifact.
///
/// The unit of resume and the unit of proof. `digest` is over the shard's
/// plaintext bytes — before any encryption, so an archive that seals its
/// shards and an importer that does not produce comparable references — and it
/// is what [`version_of`] builds the artifact's identity from.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct ShardRef {
    pub index: usize,
    pub rows: usize,
    /// `sha256` of the shard's plaintext JSONL. What proves a shard was not
    /// swapped for another.
    pub digest: String,
}

/// Whether nobody is demonstrably working on this right now.
///
/// A job with no claim at all is expired by definition: that is either a queued
/// job or one written before leases existed.
#[must_use]
pub fn lease_expired(claimed_at: Option<OffsetDateTime>, now: OffsetDateTime) -> bool {
    claimed_at.is_none_or(|at| now - at > time::Duration::seconds(LEASE_SECONDS))
}

/// What is done, as a fraction — and never a guess.
///
/// `None` before there is a denominator, which is the honest answer while a
/// job has not resolved what it will read. A bar drawn from a guessed total is
/// the thing `training` refuses to draw, and for the same reason: nothing
/// knows how many epochs a run intends, while a job *does* know how many
/// conversations or pages it pinned.
#[must_use]
pub fn progress(cursor: usize, total: usize) -> Option<f64> {
    (total > 0).then(|| {
        #[allow(clippy::cast_precision_loss)]
        {
            cursor as f64 / total as f64
        }
    })
}

/// Where a job goes after an attempt failed.
///
/// The retryability decision is the caller's, because it is a claim about the
/// error rather than about the job: an unreachable object store is worth
/// coming back for, and a document that will not parse will not parse on the
/// third attempt either — spending the budget on it only delays the message
/// that says so.
#[must_use]
pub const fn after_failure(attempts: u32, retryable: bool) -> JobState {
    if retryable && attempts < MAX_ATTEMPTS {
        JobState::Queued
    } else {
        JobState::Failed
    }
}

/// The immutable reference a finished job is named by.
///
/// `sha256(request_digest ‖ every shard digest, in order)`, computed from the
/// shards rather than from a running hash nobody could resume. Two properties
/// follow, and both are load-bearing: the same request over unchanged inputs
/// reaches the same version, which is what lets a training run *name* a
/// dataset; and a version pins the content rather than the question, so
/// "export again now that another two hundred rows are reviewed" produces a
/// different reference rather than silently returning the old corpus.
#[must_use]
pub fn version_of(request_digest: &str, shards: &[ShardRef]) -> String {
    let mut material = request_digest.to_owned();
    for shard in shards {
        material.push('\0');
        material.push_str(&shard.digest);
    }
    digest(material.as_bytes())
}

/// `sha256`, hex, lower case. The one everything here is addressed by.
#[must_use]
pub fn digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex(&hasher.finalize())
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Duration;

    #[test]
    fn a_job_nobody_ever_claimed_is_claimable() {
        assert!(lease_expired(None, OffsetDateTime::now_utc()));
    }

    #[test]
    fn a_claim_younger_than_the_lease_is_left_alone() {
        let now = OffsetDateTime::now_utc();
        assert!(!lease_expired(Some(now - Duration::seconds(10)), now));
        assert!(lease_expired(
            Some(now - Duration::seconds(LEASE_SECONDS + 1)),
            now
        ));
    }

    #[test]
    fn a_rejection_fails_immediately_and_an_outage_is_retried() {
        assert_eq!(after_failure(0, false), JobState::Failed);
        assert_eq!(after_failure(1, true), JobState::Queued);
        assert_eq!(after_failure(MAX_ATTEMPTS, true), JobState::Failed);
    }

    #[test]
    fn two_jobs_over_the_same_shards_reach_the_same_version() {
        let shards = vec![
            ShardRef {
                index: 0,
                rows: 2,
                digest: "aa".repeat(32),
            },
            ShardRef {
                index: 1,
                rows: 1,
                digest: "bb".repeat(32),
            },
        ];
        assert_eq!(version_of("req", &shards), version_of("req", &shards));
    }

    #[test]
    fn a_shard_swapped_for_another_is_a_different_version() {
        let one = vec![ShardRef {
            index: 0,
            rows: 2,
            digest: "aa".repeat(32),
        }];
        let other = vec![ShardRef {
            index: 0,
            rows: 2,
            digest: "cc".repeat(32),
        }];
        assert_ne!(version_of("req", &one), version_of("req", &other));
    }

    #[test]
    fn reordering_two_shards_is_a_different_version() {
        let forward = vec![
            ShardRef {
                index: 0,
                rows: 1,
                digest: "aa".repeat(32),
            },
            ShardRef {
                index: 1,
                rows: 1,
                digest: "bb".repeat(32),
            },
        ];
        let mut backward = forward.clone();
        backward.reverse();
        assert_ne!(version_of("req", &forward), version_of("req", &backward));
    }

    #[test]
    fn a_job_with_no_denominator_reports_no_progress_rather_than_zero() {
        assert_eq!(progress(0, 0), None);
        assert_eq!(progress(1, 4), Some(0.25));
    }

    #[test]
    fn the_digest_is_the_one_hashlib_produces() {
        // `python3 -c "import hashlib; print(hashlib.sha256(b'').hexdigest())"`
        assert_eq!(
            digest(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
