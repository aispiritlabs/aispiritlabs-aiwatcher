//! The one transactional operation, and the port behind it.
//!
//! Handling one workflow input is six writes that have to happen together:
//!
//! ```text
//! BEGIN
//!   load the stream at the expected version
//!   if this message_id is already recorded: return what it produced
//!   fold the events, run decide()
//!   append the input, then every output
//!   update the run and step projection
//!   insert the outbox rows
//!   advance the processor checkpoint, when the input came from the log
//! COMMIT
//! ```
//!
//! Splitting them is the dual-write gap: an outbox row with no decision behind
//! it publishes a `step.completed` for an attempt the store does not consider
//! complete, and a decision with no outbox row is a run the panel never sees
//! finish. That requirement is what settles the backend — the event log offers
//! ordered offsets and at-least-once delivery and no transaction spanning
//! itself and this state (ADR_0025).
//!
//! ## Three adapters, one contract
//!
//! `memory` for tests, `file` for `just dev`, and `postgres` for a deployment —
//! the pattern `memory | wal | laser` and `none | memory | file | s3` already
//! set. The suite in `tests/store_contract.rs` runs against every one of them,
//! because an adapter that passes a *different* suite is an adapter that is
//! correct about something else.
//!
//! The `file` adapter holds **one process**, and says so by name
//! ([`StoreError::SingleProcessOnly`]) rather than by corrupting quietly. A run
//! that needs a worker, a container job or a second API replica is refused on
//! it, so that a development store never becomes a production one by omission.

pub mod file;
pub mod memory;
#[cfg(feature = "postgres")]
pub mod postgres;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use aiwatcher_core::{Checkpoint, MessageId};

use crate::claim::{AttemptKey, AttemptRow, AttemptWrite, ClaimFilter};
use crate::error::{Result, StoreError};
use crate::message::{
    Direction, MAX_PAYLOAD_BYTES, OutboxMessage, PendingMessage, RecordedMessage, RunProjection,
};
use crate::state::ExecutionId;

/// What the caller believed the stream was at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExpectedVersion {
    /// Append whatever the stream is at. For an input whose decision does not
    /// depend on the state — there are none yet, and the variant exists so a
    /// caller has to *choose* rather than pass a number it guessed.
    Any,
    /// The stream must be empty. What `StartExecution` uses, so two callers
    /// racing to start one execution produce one execution and one conflict.
    #[default]
    NoStream,
    Exact(u64),
}

/// Everything one decision writes, in one call.
#[derive(Clone, Debug)]
pub struct AppendRequest {
    pub expected_version: ExpectedVersion,
    /// The message that caused this decision. Its `message_id` is the durable
    /// inbox key: re-delivering it returns the first outcome rather than
    /// deciding again.
    pub input: PendingMessage,
    pub outputs: Vec<PendingMessage>,
    /// The inline projection after this decision. Written in the same
    /// transaction, so a caller that reads it never sees a state the stream
    /// does not justify.
    pub projection: RunProjection,
    /// What the outbox publishes to the event log, after commit. Never before
    /// (ADR_0026).
    pub outbox: Vec<OutboxMessage>,
    /// `(processor_id, checkpoint)` when this input came from the log.
    pub checkpoint: Option<(String, Checkpoint)>,
    /// What this decision does to the claim table.
    ///
    /// Applied in the same transaction as the decision that authorised it, so a
    /// claimable row always has a `StepScheduled` behind it and a retirement
    /// always has its completion. A row inserted separately would be work
    /// nobody decided on.
    pub attempts: Vec<AttemptWrite>,
}

impl AppendRequest {
    /// The bounds every message has to be inside.
    ///
    /// Checked here rather than trusted, because the payload that breaks the
    /// limit is somebody inlining a result on the day the corpus grew.
    ///
    /// # Errors
    ///
    /// [`StoreError::PayloadTooLarge`] naming the size and the limit.
    pub fn check_payloads(&self) -> Result<()> {
        for message in std::iter::once(&self.input).chain(&self.outputs) {
            let size = message.payload_size();
            if size > MAX_PAYLOAD_BYTES {
                return Err(StoreError::PayloadTooLarge {
                    size,
                    limit: MAX_PAYLOAD_BYTES,
                });
            }
        }
        Ok(())
    }
}

/// What an append did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppendOutcome {
    Appended {
        version: u64,
    },
    /// This input was already recorded. The version is where the *input
    /// message* landed the first time — where the previous result is looked up,
    /// and a number that stays true as the stream grows past it. The outputs it
    /// produced are already in the stream, and the outbox already holds their
    /// rows.
    Duplicate {
        version: u64,
    },
}

impl AppendOutcome {
    #[must_use]
    pub const fn version(&self) -> u64 {
        match self {
            Self::Appended { version } | Self::Duplicate { version } => *version,
        }
    }

    #[must_use]
    pub const fn is_duplicate(&self) -> bool {
        matches!(self, Self::Duplicate { .. })
    }
}

/// One execution's stream, as loaded.
#[derive(Clone, Debug, Default)]
pub struct StreamSlice {
    pub version: u64,
    pub messages: Vec<RecordedMessage>,
}

impl StreamSlice {
    /// The facts, in order. What [`crate::decide::replay`] folds.
    pub fn events(&self) -> impl Iterator<Item = &crate::message::WorkflowEvent> {
        self.messages
            .iter()
            .filter(|recorded| recorded.direction == Direction::Output)
            .filter_map(|recorded| recorded.message.event())
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

/// What an adapter can do, so a caller can refuse rather than discover.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreCapabilities {
    /// Whether more than one process may hold this store at once. `false` for
    /// `file`, and the reason a managed run needing a worker is refused on it.
    pub multi_process: bool,
    /// Whether an attempt can be claimed by pulling. `false` until a store can
    /// hand one row to exactly one claimant.
    pub claimable: bool,
}

/// What one pass of a retention sweep removed.
///
/// Counted rather than named: a sweep that ran for an hour would otherwise
/// return a list nobody reads, and what an operator wants from the log line is
/// whether the store is shrinking.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pruned {
    pub executions: usize,
    pub attempts: usize,
}

impl Pruned {
    /// Whether anything went. What a sweeper logs on, so a store that is
    /// already inside its window is silent rather than hourly.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.executions == 0 && self.attempts == 0
    }
}

/// Whether one execution may be forgotten, given a cutoff.
///
/// The rule the three adapters share, written once. An adapter that decided
/// this for itself would keep a *different* retention policy, and the property
/// saying a running execution survives would only prove it about whichever one
/// it was written against.
///
/// `last_activity` is when the store last wrote anything about this execution,
/// and each adapter reads it from what it already has — the last recorded
/// message for `memory`, the stream file's own modification time for `file`,
/// an indexed column for `postgres`. Its meaning is the same in all three,
/// which is what the contract suite checks; its source is not, which is what
/// stops one adapter paying for another's shape.
///
/// It is deliberately *not* [`RunProjection::ended_at`], and the first reason
/// is that nothing writes that field: `evolve` reads no clock and the terminal
/// events carry no timestamp, so it has been `None` since it was added. The
/// second reason is that it would be the wrong clock anyway — a crashed run may
/// never get a terminal event, and a retried one has to start the window again.
/// "Nothing has been written here for N days" answers both on its own, and is
/// what a retention window means when somebody says it out loud.
#[must_use]
pub fn prunable(
    run: &RunProjection,
    last_activity: OffsetDateTime,
    before: OffsetDateTime,
) -> bool {
    run.state.state_type.is_terminal() && last_activity < before
}

/// The transactional store an execution's history lives in.
#[async_trait]
pub trait WorkflowStore: Send + Sync + std::fmt::Debug {
    /// What this adapter can do. Read before a plan is accepted, never after.
    fn capabilities(&self) -> StoreCapabilities;

    /// The whole stream for one execution.
    ///
    /// # Errors
    ///
    /// Whatever the backend could not do.
    async fn load(&self, execution: &ExecutionId) -> Result<StreamSlice>;

    /// The one atomic operation. Everything in `request` lands, or none of it.
    ///
    /// # Errors
    ///
    /// [`StoreError::VersionConflict`] when somebody appended in between — the
    /// caller re-reads and decides again, which is a bounded retry around a
    /// pure function rather than an error a caller surfaces.
    async fn append(
        &self,
        execution: &ExecutionId,
        request: AppendRequest,
    ) -> Result<AppendOutcome>;

    /// The inline projection, for accepting the next command and for the run's
    /// own page. Never for a list the event log's folds already serve.
    ///
    /// # Errors
    ///
    /// Whatever the backend could not do.
    async fn projection(&self, execution: &ExecutionId) -> Result<Option<RunProjection>>;

    /// Rows the publisher has not yet put on the log, oldest first.
    ///
    /// # Errors
    ///
    /// Whatever the backend could not do.
    async fn pending_outbox(&self, limit: usize) -> Result<Vec<OutboxMessage>>;

    /// Forget rows the log has accepted.
    ///
    /// **Deleted, not flagged.** The row's only reader is the publisher, and
    /// once the sink has taken it the fact lives on the event log — which is
    /// the durable copy, the one every fold reads, and the one a person looks
    /// at. A second copy in this store would answer no question and would grow
    /// with every step of every run for as long as the deployment lives.
    ///
    /// `at` is what the deletion happened at, kept in the signature because an
    /// adapter that wanted to tombstone rather than delete would need it, and
    /// because a caller must not read a clock to call this.
    ///
    /// Safe to repeat: a row published twice is a redelivery, which the
    /// projector already deduplicates by message id, and deleting a row that is
    /// already gone is a no-op rather than an error.
    ///
    /// # Errors
    ///
    /// Whatever the backend could not do.
    async fn mark_published(&self, ids: &[MessageId], at: OffsetDateTime) -> Result<()>;

    /// Take the oldest claimable attempt this claimant would run, if there is
    /// one, and hold it for [`aiwatcher_jobs::LEASE_SECONDS`].
    ///
    /// Exactly one claimant gets a given row. Everything about *which* row is
    /// [`ClaimFilter`]'s: a reactor claims by runtime and a worker by queue,
    /// and neither takes the other's work.
    ///
    /// # Errors
    ///
    /// Whatever the backend could not do.
    async fn claim_attempt(
        &self,
        filter: &ClaimFilter,
        owner: &str,
        now: OffsetDateTime,
    ) -> Result<Option<AttemptRow>>;

    /// Renew a lease. `false` when the caller no longer holds it, which is the
    /// signal to stop rather than to try harder.
    ///
    /// # Errors
    ///
    /// Whatever the backend could not do.
    async fn heartbeat(&self, key: &AttemptKey, owner: &str, now: OffsetDateTime) -> Result<bool>;

    /// One attempt row, for a caller re-checking a lease at a boundary.
    ///
    /// # Errors
    ///
    /// Whatever the backend could not do.
    async fn attempt(&self, key: &AttemptKey) -> Result<Option<AttemptRow>>;

    /// Where a processor got to.
    ///
    /// # Errors
    ///
    /// Whatever the backend could not do.
    async fn checkpoint(&self, processor: &str) -> Result<Option<Checkpoint>>;

    /// Move a processor's cursor on its own, outside any decision.
    ///
    /// For the one case a decision cannot cover: an input from the log that the
    /// inbox says was already handled. The decision committed long ago, so
    /// there is nothing to be atomic with — and advancing afterwards is the
    /// right way round by [`aiwatcher_jobs::ORDERING`]. A crash in between
    /// re-reads a message the inbox already knows, which is the failure this
    /// system is built to absorb.
    ///
    /// Everything else passes its checkpoint in [`AppendRequest`], where it
    /// moves with the decision or not at all.
    ///
    /// # Errors
    ///
    /// Whatever the backend could not do.
    async fn advance_checkpoint(&self, processor: &str, checkpoint: Checkpoint) -> Result<()>;

    /// Forget finished executions that ended before `before`.
    ///
    /// Section 43.25. The stream is the *explanation* of a run — the commands,
    /// the decisions, the attempt that failed and the one that did not — and it
    /// is the one thing the event log does not carry, so this is a deletion of
    /// something no other store holds. That is why the window is configuration
    /// and why its default keeps everything.
    ///
    /// Three rules, and each is a way of not leaving a half-run behind:
    ///
    /// * **Terminal only**, by [`StateType::is_terminal`](crate::StateType::is_terminal). A run with no end is
    ///   `Running` and this system never decides one has died — the same rule
    ///   the projector keeps for agent runs. Age is not evidence.
    /// * **Nothing the outbox still holds.** A pending row is a fact that has
    ///   not reached the log; deleting the decision behind it would leave the
    ///   publisher a message with no explanation and the log a gap with no
    ///   record of one. [`aiwatcher_jobs::ORDERING`] in a sixth place — the
    ///   durable copy first, and only then the thing it was derived from.
    /// * **All of one execution together**: stream, inbox, projection, attempt
    ///   rows. A kept projection whose stream is gone is a run the panel lists
    ///   and cannot open, which is the guardrail about actions that would be
    ///   refused, arriving as a link instead of a button.
    ///
    /// Plans and checkpoints are untouched. A plan is content-addressed and
    /// shared by every run of one revision; a checkpoint belongs to a processor
    /// rather than to an execution, and forgetting one would re-read the log.
    ///
    /// `limit` bounds one pass, so turning retention on against a store with a
    /// year of history is many short transactions rather than one that holds a
    /// table lock for a minute. A caller that wants to catch up sweeps again
    /// while the count comes back at the limit.
    ///
    /// ## The window has a floor nothing here can check
    ///
    /// The inbox goes with the stream, so a message redelivered after its
    /// execution was pruned is decided again rather than recognised. Delivery
    /// is at-least-once and the log is what redelivers, so the window has to be
    /// longer than the log's own retention. That is a deployment fact this
    /// crate cannot read, which is why it is written here rather than asserted.
    ///
    /// # Errors
    ///
    /// Whatever the backend could not do.
    async fn prune(&self, before: OffsetDateTime, limit: usize) -> Result<Pruned>;
}

/// Sharing one store between the parts of a process that hold it.
///
/// The API accepts the next command, the outbox publisher drains what the last
/// one wrote and a reactor claims what it dispatched — three readers of one
/// store, in one process, and none of them owns it. Without this every one of
/// them would have to carry the concrete adapter as a type parameter, which
/// puts `postgres` in the signature of a struct that is built whether or not
/// the feature is on.
#[async_trait]
impl<T: WorkflowStore + ?Sized> WorkflowStore for std::sync::Arc<T> {
    fn capabilities(&self) -> StoreCapabilities {
        (**self).capabilities()
    }

    async fn load(&self, execution: &ExecutionId) -> Result<StreamSlice> {
        (**self).load(execution).await
    }

    async fn append(
        &self,
        execution: &ExecutionId,
        request: AppendRequest,
    ) -> Result<AppendOutcome> {
        (**self).append(execution, request).await
    }

    async fn projection(&self, execution: &ExecutionId) -> Result<Option<RunProjection>> {
        (**self).projection(execution).await
    }

    async fn pending_outbox(&self, limit: usize) -> Result<Vec<OutboxMessage>> {
        (**self).pending_outbox(limit).await
    }

    async fn mark_published(&self, ids: &[MessageId], at: OffsetDateTime) -> Result<()> {
        (**self).mark_published(ids, at).await
    }

    async fn claim_attempt(
        &self,
        filter: &ClaimFilter,
        owner: &str,
        now: OffsetDateTime,
    ) -> Result<Option<AttemptRow>> {
        (**self).claim_attempt(filter, owner, now).await
    }

    async fn heartbeat(&self, key: &AttemptKey, owner: &str, now: OffsetDateTime) -> Result<bool> {
        (**self).heartbeat(key, owner, now).await
    }

    async fn attempt(&self, key: &AttemptKey) -> Result<Option<AttemptRow>> {
        (**self).attempt(key).await
    }

    async fn checkpoint(&self, processor: &str) -> Result<Option<Checkpoint>> {
        (**self).checkpoint(processor).await
    }

    async fn advance_checkpoint(&self, processor: &str, checkpoint: Checkpoint) -> Result<()> {
        (**self).advance_checkpoint(processor, checkpoint).await
    }

    async fn prune(&self, before: OffsetDateTime, limit: usize) -> Result<Pruned> {
        (**self).prune(before, limit).await
    }
}

/// A row on its way to the log, with the ordering rule already applied.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OutboxBatch {
    pub messages: Vec<OutboxMessage>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_caller_has_to_choose_an_expectation_rather_than_guess_a_number() {
        // `NoStream` is the default because starting an execution is the one
        // append whose expectation is not a number somebody read: two callers
        // racing to start one execution must produce one execution and one
        // conflict.
        assert_eq!(ExpectedVersion::default(), ExpectedVersion::NoStream);
    }
}
