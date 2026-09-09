//! The one transactional operation, and the port behind it.
//!
//! Handling one workflow input is six writes that must land together:
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
//! finish.
//!
//! Four adapters, one contract — `memory`, `file`, `postgres` and `duckdb`,
//! the last two behind cargo features. `tests/store_contract.rs` runs against
//! every one: an adapter that passes a *different* suite is correct about
//! something else.
//!
//! `file` and `duckdb` hold **one process** and say so by name
//! ([`StoreError::SingleProcessOnly`]). A run needing a worker, a container job
//! or a second replica is refused on them, so a development store never becomes
//! a production one by omission.
//!
//! ADR_0025.

#[cfg(feature = "duckdb")]
pub mod duckdb;
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
use crate::hosted::{DeciderLease, LeaseOutcome};
use crate::message::{
    Direction, MAX_PAYLOAD_BYTES, OutboxMessage, PendingMessage, RecordedMessage, RunProjection,
};
use crate::plan::DefinitionKind;
use crate::schedule::slot::{
    SlotAdmission, SlotAdmissionRequest, SlotKey, SlotRecord, SlotSettlement,
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

    /// One page of a stream: the messages after `after`, at most `limit` of
    /// them, in order.
    ///
    /// [`Self::load`] is what the decider needs — a decision is a fold over the
    /// whole history and there is no page of it. A *reader* is the other case,
    /// and a hosted agent graph is where the two stop being the same size: an
    /// execution that runs for a day is a stream neither the API nor the
    /// browser can hold, which is the event log's own rule about
    /// `read_stream_page` arriving in a second store.
    ///
    /// [`StreamSlice::version`] is the stream's current version rather than the
    /// page's end, so a reader can tell a short page from the last one without
    /// a second call. `after` is a version, not an offset: it is what the
    /// previous page's last message reported, so a page that arrives while the
    /// stream grows continues rather than shifts.
    ///
    /// # Errors
    ///
    /// Whatever the backend could not do.
    async fn load_page(
        &self,
        execution: &ExecutionId,
        after: u64,
        limit: usize,
    ) -> Result<StreamSlice>;

    /// Take, or renew, the decider lease on one hosted execution.
    ///
    /// `agentic.workflow`'s `ProcessorLock`, in the store this system already
    /// keeps. Taking it again as the same holder is the renewal, so a heartbeat
    /// and a first claim are one call and cannot disagree.
    ///
    /// It is **not** what keeps the history correct — the expected version is,
    /// and it holds whether or not anybody leases anything. This keeps two
    /// deciders from doing one turn's model call twice.
    ///
    /// # Errors
    ///
    /// Whatever the backend could not do.
    async fn take_decider_lease(
        &self,
        execution: &ExecutionId,
        holder: &str,
        now: OffsetDateTime,
    ) -> Result<LeaseOutcome>;

    /// Give up the decider lease. `false` when it was not this holder's to give.
    ///
    /// A worker that finished its turn releases rather than waiting out the
    /// lease, which is the difference between a replacement starting now and
    /// starting in five minutes.
    ///
    /// # Errors
    ///
    /// Whatever the backend could not do.
    async fn release_decider_lease(
        &self,
        execution: &ExecutionId,
        holder: &str,
        now: OffsetDateTime,
    ) -> Result<bool>;

    /// Who holds it, if anybody still does.
    ///
    /// `None` once it has run out, because "expired" and "never taken" are the
    /// same answer to every caller: it is free. The row survives so that a
    /// takeover can report [`DeciderLease::previous_holder`].
    ///
    /// # Errors
    ///
    /// Whatever the backend could not do.
    async fn decider_lease(&self, execution: &ExecutionId) -> Result<Option<DeciderLease>>;

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

    /// One attempt row, by key.
    ///
    /// Written for the contract suite, and it stayed the right shape when a
    /// second caller arrived. The properties worth proving about a claim table
    /// are all statements about a row that no other operation returns: that a
    /// dispatch put one there, that a settlement took it away again (section
    /// 43.34), that `awaiting_input` kept its own. A suite with no way to read
    /// a row proves those by asking `claim_attempt` what it would hand out
    /// next, which is a different question — it cannot tell a retired row from
    /// one that is merely not claimable yet, and those are the two outcomes
    /// most worth distinguishing.
    ///
    /// The production caller is
    /// [`Reactor::resume`](crate::reactor::Reactor::resume), which rebuilds a
    /// claim an HTTP worker holds across two requests and needs the row's
    /// `command_id` and holder rather than a yes-or-no. A caller that only
    /// wants to know whether it still holds something wants
    /// [`WorkflowStore::heartbeat`], which renews as it answers.
    ///
    /// # Errors
    ///
    /// Whatever the backend could not do.
    async fn attempt(&self, key: &AttemptKey) -> Result<Option<AttemptRow>>;

    /// Take one schedule slot, or say why not.
    ///
    /// **The transactional half of the scheduler** (review R1, R2, R3). Three
    /// things happen inside one transaction and none of them can be split:
    /// the slot is created if it does not exist, it is refused if somebody
    /// already settled it or holds a live lease on it, and — when the policy
    /// is [`OverlapPolicy::Skip`] — the definition is checked for a run that
    /// has not finished.
    ///
    /// The overlap check is here rather than in the caller because the caller
    /// that had it was reading `state.read_model`, an asynchronous fold that is
    /// *empty in the `work` role* where the tick runs, so `skip` never skipped.
    /// A projection this store already holds, read in the transaction that
    /// takes the slot, is the only answer two replicas cannot both get wrong.
    ///
    /// Exactly one caller is [`SlotAdmission::Admitted`] for a given slot until
    /// it is settled or the lease expires. Everything else is an answer rather
    /// than a failure: a second replica racing the first is the expected shape.
    ///
    /// # Errors
    ///
    /// Whatever the backend could not do.
    async fn admit_slot(&self, request: &SlotAdmissionRequest) -> Result<SlotAdmission>;

    /// Write down what happened to a slot this caller holds.
    ///
    /// [`SlotSettlement::Decided`] is terminal and the slot is never taken
    /// again. [`SlotSettlement::TryAgain`] drops the lease and leaves it due,
    /// which is R2's whole point: an infrastructure failure is not a decision,
    /// and the release before this one wrote one down and then moved the cursor
    /// past the slot, so a store that was briefly unreachable at 09:00 cost the
    /// day's run.
    ///
    /// A settlement from somebody who no longer holds the lease is ignored
    /// rather than an error — it is a slow caller whose work was taken over,
    /// and the holder's answer is the one that counts.
    ///
    /// # Errors
    ///
    /// Whatever the backend could not do.
    async fn settle_slot(
        &self,
        key: &SlotKey,
        owner: &str,
        settlement: SlotSettlement,
        now: OffsetDateTime,
    ) -> Result<()>;

    /// One definition's slots, newest first.
    ///
    /// What the panel reads for "when this last fired", which used to be a
    /// field on the schedule object the tick wrote back over. Bounded by
    /// `limit`, because this is a list that grows with time.
    ///
    /// # Errors
    ///
    /// Whatever the backend could not do.
    async fn recent_slots(
        &self,
        kind: DefinitionKind,
        name: &str,
        limit: usize,
    ) -> Result<Vec<SlotRecord>>;

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
    /// The stream is the *explanation* of a run and the one thing the event log
    /// does not carry, so this deletes something no other store holds. That is
    /// why the window is configuration and why its default keeps everything.
    ///
    /// * **Terminal only**, by
    ///   [`StateType::is_terminal`](crate::StateType::is_terminal). A run with
    ///   no end is `Running`, and age is not evidence that one has died.
    /// * **Nothing the outbox still holds.** A pending row is a fact that has
    ///   not reached the log ([`aiwatcher_jobs::ORDERING`]).
    /// * **All of one execution together** — stream, inbox, projection,
    ///   attempts. A kept projection whose stream is gone is a run the panel
    ///   lists and cannot open.
    ///
    /// Plans and checkpoints are untouched: a plan is shared by every run of
    /// one revision, and a checkpoint belongs to a processor.
    ///
    /// `limit` bounds one pass, so turning retention on against a year of
    /// history is many short transactions rather than one long table lock.
    ///
    /// The window has a floor nothing here can check: the inbox goes with the
    /// stream, so it must outlast the log's own retention or a redelivery is
    /// decided again instead of recognised.
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

    async fn load_page(
        &self,
        execution: &ExecutionId,
        after: u64,
        limit: usize,
    ) -> Result<StreamSlice> {
        (**self).load_page(execution, after, limit).await
    }

    async fn take_decider_lease(
        &self,
        execution: &ExecutionId,
        holder: &str,
        now: OffsetDateTime,
    ) -> Result<LeaseOutcome> {
        (**self).take_decider_lease(execution, holder, now).await
    }

    async fn release_decider_lease(
        &self,
        execution: &ExecutionId,
        holder: &str,
        now: OffsetDateTime,
    ) -> Result<bool> {
        (**self).release_decider_lease(execution, holder, now).await
    }

    async fn decider_lease(&self, execution: &ExecutionId) -> Result<Option<DeciderLease>> {
        (**self).decider_lease(execution).await
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

    async fn admit_slot(&self, request: &SlotAdmissionRequest) -> Result<SlotAdmission> {
        (**self).admit_slot(request).await
    }

    async fn settle_slot(
        &self,
        key: &SlotKey,
        owner: &str,
        settlement: SlotSettlement,
        now: OffsetDateTime,
    ) -> Result<()> {
        (**self).settle_slot(key, owner, settlement, now).await
    }

    async fn recent_slots(
        &self,
        kind: DefinitionKind,
        name: &str,
        limit: usize,
    ) -> Result<Vec<SlotRecord>> {
        (**self).recent_slots(kind, name, limit).await
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
