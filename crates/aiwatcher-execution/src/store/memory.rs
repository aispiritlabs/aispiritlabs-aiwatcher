//! Everything in one lock. What the tests and `just check` run against.
//!
//! The transaction is the mutex: `append` takes it, checks the version and the
//! inbox, and either writes all six pieces or none. That is the same guarantee
//! PostgreSQL gives with a transaction, reached the only way a process can
//! reach it — which is why the same contract suite runs against both.
//!
//! It is multi-process only in the sense that it is not: one process holds the
//! whole thing, and it claims [`StoreCapabilities::multi_process`] anyway,
//! because a test that spawns two logical workers inside one process is
//! exercising exactly the race the flag is about. `file` is the adapter that
//! says no, because there the second process is real.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use time::OffsetDateTime;
use tokio::sync::Mutex;

use aiwatcher_core::{Checkpoint, MessageId};

use crate::claim::{
    AttemptKey, AttemptRow, AttemptWrite, ClaimFilter, claimable_of, tally_unclaimed,
};
use crate::error::{Result, StoreError};
use crate::hosted::{DeciderLease, LeaseOutcome, Timer, TimerWrite};
use crate::message::{Direction, OutboxMessage, RecordedMessage, RunProjection, WorkflowEvent};
use crate::plan::{DefinitionKind, RuntimeKind};
use crate::schedule::slot::{
    SlotAdmission, SlotAdmissionRequest, SlotKey, SlotRecord, SlotSettlement,
};
use crate::scope::{ExecutionOwnership, ExecutionScope, ScopeBinding};
use crate::state::ExecutionId;

use super::{
    AppendOutcome, AppendRequest, ExpectedVersion, Pruned, StoreCapabilities, StreamSlice,
    WorkflowStore, prunable,
};

#[derive(Debug, Default)]
struct Inner {
    streams: HashMap<String, Vec<RecordedMessage>>,
    /// `execution -> message_id -> the version that message landed at`. The
    /// durable inbox: permanent, unlike a processor's own dedup window, because
    /// re-deciding one input years later would still be wrong.
    seen: HashMap<String, HashMap<String, u64>>,
    projections: HashMap<String, RunProjection>,
    outbox: Vec<OutboxMessage>,
    checkpoints: HashMap<String, Checkpoint>,
    /// Ordered so `claim_attempt` takes the oldest, which is what stops a
    /// backlog from being served newest-first while its head starves.
    attempts: BTreeMap<AttemptKey, AttemptRow>,
    /// Ordered by `(kind, name, slot)`, so one definition's slots are a
    /// contiguous range rather than a scan of every definition's.
    slots: BTreeMap<SlotKey, SlotRecord>,
    /// One decider at a time, per hosted execution. Kept after it expires so a
    /// takeover can name who was interrupted.
    decider_leases: HashMap<String, DeciderLease>,
    /// Deferred appends, by execution and the worker's own timer id.
    timers: BTreeMap<(String, String), Timer>,
    /// Who owns each execution that anybody owns. Absent is a global run, which
    /// is what every stream written before this record existed is.
    ownership: HashMap<String, ExecutionOwnership>,
}

impl Inner {
    /// Who owns one execution, as this store holds it.
    fn owner_of(&self, execution: &str) -> Option<&ExecutionOwnership> {
        self.ownership.get(execution)
    }

    /// Whether this store holds any history for that id.
    ///
    /// An empty vector counts as absent: `append` reaches for the entry before
    /// it knows whether it may write, so a refused start must not leave behind
    /// an id that reads as a global run.
    fn holds(&self, execution: &str) -> bool {
        self.streams
            .get(execution)
            .is_some_and(|stream| !stream.is_empty())
    }

    /// Whether a binding may touch that execution at all.
    fn admits(&self, binding: ScopeBinding, execution: &str) -> bool {
        binding.admits(self.owner_of(execution), self.holds(execution))
    }

    /// The same question, as the refusal a caller returns.
    fn check(&self, binding: ScopeBinding, execution: &ExecutionId) -> Result<()> {
        binding.check(
            execution,
            self.owner_of(execution.as_str()),
            self.holds(execution.as_str()),
        )
    }

    /// Whether a row addressed by `workflow:<execution>` is this side's.
    ///
    /// The outbox is keyed by partition rather than by execution, so the one
    /// place a scope has to be read back out of a string is here.
    fn admits_partition(&self, binding: ScopeBinding, partition_key: &str) -> bool {
        partition_key
            .strip_prefix("workflow:")
            .is_some_and(|execution| self.admits(binding, execution))
    }
}

/// An in-memory workflow store.
///
/// `binding` narrows what may be reached through this handle without copying
/// anything: a project-bound store shares the same `Inner` and sees only that
/// project's executions. One lock, one set of rows, two doors — which is what
/// makes a test that holds both able to prove that neither reaches the other's.
#[derive(Clone, Debug, Default)]
pub struct MemoryWorkflowStore {
    inner: Arc<Mutex<Inner>>,
    binding: ScopeBinding,
}

impl MemoryWorkflowStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Every outbox row, published or not. For a test that wants to prove the
    /// ordering rather than the publishing.
    pub async fn outbox(&self) -> Vec<OutboxMessage> {
        self.inner.lock().await.outbox.clone()
    }
}

#[async_trait]
impl WorkflowStore for MemoryWorkflowStore {
    fn capabilities(&self) -> StoreCapabilities {
        StoreCapabilities {
            multi_process: true,
            claimable: true,
        }
    }

    fn scope(&self) -> ExecutionScope {
        self.binding.scope()
    }

    fn for_project(&self, scope: aiwatcher_iam::ProjectScope) -> Result<Arc<dyn WorkflowStore>> {
        Ok(Arc::new(Self {
            inner: Arc::clone(&self.inner),
            binding: self.binding.bind(scope)?,
        }))
    }

    async fn ownership(&self, execution: &ExecutionId) -> Result<Option<ExecutionOwnership>> {
        let inner = self.inner.lock().await;
        inner.check(self.binding, execution)?;
        Ok(inner.owner_of(execution.as_str()).cloned())
    }

    async fn load(&self, execution: &ExecutionId) -> Result<StreamSlice> {
        let inner = self.inner.lock().await;
        inner.check(self.binding, execution)?;
        let messages = inner
            .streams
            .get(execution.as_str())
            .cloned()
            .unwrap_or_default();
        Ok(StreamSlice {
            version: messages.len() as u64,
            messages,
        })
    }

    async fn load_page(
        &self,
        execution: &ExecutionId,
        after: u64,
        limit: usize,
    ) -> Result<StreamSlice> {
        let inner = self.inner.lock().await;
        inner.check(self.binding, execution)?;
        let stream = inner.streams.get(execution.as_str());
        let version = stream.map_or(0, Vec::len) as u64;
        let messages = stream
            .into_iter()
            .flatten()
            .filter(|recorded| recorded.stream_version > after)
            .take(limit)
            .cloned()
            .collect();
        Ok(StreamSlice { version, messages })
    }

    async fn due_timers(&self, now: OffsetDateTime, limit: usize) -> Result<Vec<Timer>> {
        let inner = self.inner.lock().await;
        let mut due: Vec<Timer> = inner
            .timers
            .values()
            .filter(|timer| {
                timer.is_due(now) && inner.admits(self.binding, timer.execution.as_str())
            })
            .cloned()
            .collect();
        // Oldest first, so a backlog drains in the order it accumulated rather
        // than in whatever order the map happens to hold.
        due.sort_by_key(|timer| timer.due_at);
        due.truncate(limit);
        Ok(due)
    }

    async fn timers_of(&self, execution: &ExecutionId) -> Result<Vec<Timer>> {
        let inner = self.inner.lock().await;
        inner.check(self.binding, execution)?;
        let mut waiting: Vec<Timer> = inner
            .timers
            .values()
            .filter(|timer| timer.execution.as_str() == execution.as_str())
            .cloned()
            .collect();
        waiting.sort_by_key(|timer| timer.due_at);
        Ok(waiting)
    }

    async fn recorded_outcome(&self, key: &AttemptKey) -> Result<Option<WorkflowEvent>> {
        let inner = self.inner.lock().await;
        inner.check(self.binding, &key.execution_id)?;
        Ok(inner
            .streams
            .get(key.execution_id.as_str())
            .into_iter()
            .flatten()
            .filter(|recorded| recorded.direction == Direction::Output)
            .filter_map(|recorded| recorded.message.event())
            .find(|event| crate::store::settles(event, &key.step_id, key.attempt))
            .cloned())
    }

    async fn take_decider_lease(
        &self,
        execution: &ExecutionId,
        holder: &str,
        now: OffsetDateTime,
    ) -> Result<LeaseOutcome> {
        let mut inner = self.inner.lock().await;
        inner.check(self.binding, execution)?;
        match inner.decider_leases.get_mut(execution.as_str()) {
            Some(lease) if lease.held_by(holder, now) || lease.expired(now) => {
                lease.take(holder, now);
                Ok(LeaseOutcome::Taken(lease.clone()))
            }
            Some(lease) => Ok(LeaseOutcome::Held {
                holder: lease.holder.clone(),
                expires_at: lease.expires_at(),
            }),
            None => {
                let lease = DeciderLease::taken_by(execution, holder, now);
                inner
                    .decider_leases
                    .insert(execution.as_str().to_owned(), lease.clone());
                Ok(LeaseOutcome::Taken(lease))
            }
        }
    }

    async fn release_decider_lease(
        &self,
        execution: &ExecutionId,
        holder: &str,
        now: OffsetDateTime,
    ) -> Result<bool> {
        let mut inner = self.inner.lock().await;
        inner.check(self.binding, execution)?;
        let Some(lease) = inner.decider_leases.get(execution.as_str()) else {
            return Ok(false);
        };
        if !lease.held_by(holder, now) {
            return Ok(false);
        }
        inner.decider_leases.remove(execution.as_str());
        Ok(true)
    }

    async fn decider_lease(&self, execution: &ExecutionId) -> Result<Option<DeciderLease>> {
        let inner = self.inner.lock().await;
        inner.check(self.binding, execution)?;
        Ok(inner.decider_leases.get(execution.as_str()).cloned())
    }

    async fn append(
        &self,
        execution: &ExecutionId,
        request: AppendRequest,
    ) -> Result<AppendOutcome> {
        request.check_payloads()?;
        let mut inner = self.inner.lock().await;
        let key = execution.as_str().to_owned();
        let stream = inner.streams.entry(key.clone()).or_default();
        let version = stream.len() as u64;

        // Scope before everything, including the inbox: a redelivery of a
        // project's message down the unscoped path is still that path handling
        // a project's execution, and answering it `Duplicate` would be the
        // silent fallback this boundary exists to refuse.
        let establishing = self.binding.appending(
            execution,
            request.ownership.as_ref(),
            inner.ownership.get(&key),
            version == 0,
        )?;

        // The inbox next: a redelivery is not a conflict, and answering it
        // with one would make every at-least-once retry look like a race.
        if let Some(seen) = inner
            .seen
            .get(&key)
            .and_then(|seen| seen.get(request.input.metadata.message_id.as_str()))
        {
            return Ok(AppendOutcome::Duplicate { version: *seen });
        }

        match request.expected_version {
            ExpectedVersion::Any => {}
            ExpectedVersion::NoStream if version != 0 => {
                return Err(StoreError::VersionConflict {
                    expected: 0,
                    actual: version,
                });
            }
            ExpectedVersion::NoStream => {}
            ExpectedVersion::Exact(expected) if expected != version => {
                return Err(StoreError::VersionConflict {
                    expected,
                    actual: version,
                });
            }
            ExpectedVersion::Exact(_) => {}
        }

        let now = OffsetDateTime::now_utc();
        // Before the stream, so a crash-free reader never sees one message of a
        // project execution without knowing whose it is. One lock holds both,
        // so the order is a statement about intent rather than about recovery.
        if let Some(owner) = establishing {
            inner.ownership.insert(key.clone(), owner);
        }
        let stream = inner.streams.entry(key.clone()).or_default();
        // The input's own version is what the inbox remembers: it is where the
        // previous result can be looked up, and it stays true as the stream
        // grows past it.
        let input_version = version + 1;
        let mut next = version;
        for message in std::iter::once(request.input.clone()).chain(request.outputs.clone()) {
            next += 1;
            stream.push(RecordedMessage {
                stream_version: next,
                direction: message.direction,
                message: message.message,
                metadata: message.metadata,
                recorded_at: now,
            });
        }

        inner
            .seen
            .entry(key.clone())
            .or_default()
            .insert(request.input.metadata.message_id.to_string(), input_version);
        inner.projections.insert(key, request.projection);
        for write in request.attempts {
            match write {
                AttemptWrite::Dispatch(row) => {
                    inner.attempts.insert(row.key.clone(), row);
                }
                // A question is not an ending: the row stays and the lease
                // goes. Absent is a no-op rather than an insert — a step the
                // decider parks when it schedules it was never dispatched
                // here, so there is nothing of its to release.
                AttemptWrite::Park(key) => {
                    if let Some(row) = inner.attempts.get_mut(&key) {
                        row.park();
                    }
                }
                // A finished attempt is not a row. See `AttemptWrite`.
                AttemptWrite::Retire(key) => {
                    inner.attempts.remove(&key);
                }
            }
        }
        for write in request.timers {
            match write {
                // Scheduling the same id twice is one timer, which is what
                // makes a decider's retry safe.
                TimerWrite::Schedule(timer) => {
                    inner.timers.insert(
                        (timer.execution.as_str().to_owned(), timer.timer_id.clone()),
                        timer,
                    );
                }
                // A fired timer is not a row, for the reason a finished attempt
                // is not one: nothing reads it back, and keeping it would mean
                // every read of what is due filtering out history.
                TimerWrite::Cancel(id) | TimerWrite::Fire(id) => {
                    inner.timers.remove(&(execution.as_str().to_owned(), id));
                }
            }
        }
        inner.outbox.extend(request.outbox);
        if let Some((processor, checkpoint)) = request.checkpoint {
            inner.checkpoints.insert(processor, checkpoint);
        }
        Ok(AppendOutcome::Appended { version: next })
    }

    async fn projection(&self, execution: &ExecutionId) -> Result<Option<RunProjection>> {
        let inner = self.inner.lock().await;
        inner.check(self.binding, execution)?;
        Ok(inner.projections.get(execution.as_str()).cloned())
    }

    async fn pending_outbox(&self, limit: usize) -> Result<Vec<OutboxMessage>> {
        let inner = self.inner.lock().await;
        Ok(inner
            .outbox
            .iter()
            .filter(|row| {
                row.published_at.is_none()
                    && inner.admits_partition(self.binding, &row.partition_key)
            })
            .take(limit)
            .cloned()
            .collect())
    }

    async fn mark_published(&self, ids: &[MessageId], _at: OffsetDateTime) -> Result<()> {
        // Dropped rather than flagged: the fact is on the log, and a second
        // copy here would grow with every step of every run.
        //
        // Scoped like the read that produced the ids, so a publisher that was
        // handed one from the other side of the boundary deletes nothing.
        let mut inner = self.inner.lock().await;
        let binding = self.binding;
        let doomed: HashSet<MessageId> = inner
            .outbox
            .iter()
            .filter(|row| {
                ids.contains(&row.message_id) && inner.admits_partition(binding, &row.partition_key)
            })
            .map(|row| row.message_id.clone())
            .collect();
        inner.outbox.retain(|row| !doomed.contains(&row.message_id));
        Ok(())
    }

    async fn admit_slot(&self, request: &SlotAdmissionRequest) -> Result<SlotAdmission> {
        refuse_instance_wide(self.binding, "admit a schedule slot")?;
        // The mutex is the transaction, exactly as it is for `append`: the
        // overlap check and the claim are one critical section, so two logical
        // workers cannot both read "nothing is running" and both start.
        let mut inner = self.inner.lock().await;

        if let Some(held) = inner.slots.get(&request.key) {
            if let Some(outcome) = &held.outcome {
                return Ok(SlotAdmission::Settled { outcome: *outcome });
            }
            if !held.is_available(request.now) {
                return Ok(SlotAdmission::Held {
                    owner: held.lease_owner.clone().unwrap_or_default(),
                });
            }
        }

        if request.overlap == crate::OverlapPolicy::Skip
            && let Some(running) = inner
                .projections
                .values()
                .filter(|run| {
                    run.definition_name == request.key.definition_name
                        && !run.state.state_type.is_terminal()
                })
                .map(|run| run.execution_id.to_string())
                .min()
        {
            return Ok(SlotAdmission::Blocked {
                execution_id: running,
            });
        }

        let detail = inner
            .slots
            .get(&request.key)
            .and_then(|record| record.detail.clone());
        inner.slots.insert(
            request.key.clone(),
            SlotRecord {
                key: request.key.clone(),
                outcome: None,
                execution_id: None,
                lease_owner: Some(request.owner.clone()),
                leased_at: Some(request.now),
                detail,
                updated_at: request.now,
            },
        );
        Ok(SlotAdmission::Admitted)
    }

    async fn settle_slot(
        &self,
        key: &SlotKey,
        owner: &str,
        settlement: SlotSettlement,
        now: OffsetDateTime,
    ) -> Result<()> {
        refuse_instance_wide(self.binding, "settle a schedule slot")?;
        let mut inner = self.inner.lock().await;
        let Some(record) = inner.slots.get_mut(key) else {
            return Ok(());
        };
        // A caller whose lease was taken over says nothing. The holder's answer
        // is the one that counts, and a slow tick overwriting a fresh decision
        // with a stale one is exactly the shape of R3 in a second place.
        if record.lease_owner.as_deref() != Some(owner) {
            return Ok(());
        }
        match settlement {
            SlotSettlement::TryAgain { detail } => {
                record.lease_owner = None;
                record.leased_at = None;
                record.detail = Some(detail);
            }
            decided => {
                record.outcome = decided.outcome();
                record.execution_id = decided.execution_id().map(str::to_owned);
                record.detail = decided.detail().map(str::to_owned);
                record.lease_owner = None;
                record.leased_at = None;
            }
        }
        record.updated_at = now;
        Ok(())
    }

    async fn recent_slots(
        &self,
        kind: DefinitionKind,
        name: &str,
        limit: usize,
    ) -> Result<Vec<SlotRecord>> {
        refuse_instance_wide(self.binding, "read schedule slots")?;
        let inner = self.inner.lock().await;
        let mut found: Vec<SlotRecord> = inner
            .slots
            .values()
            .filter(|record| {
                record.key.definition_kind == kind && record.key.definition_name == name
            })
            .cloned()
            .collect();
        found.sort_by_key(|row| std::cmp::Reverse(row.key.slot));
        found.truncate(limit);
        Ok(found)
    }

    async fn project_scopes(&self, limit: usize) -> Result<Vec<aiwatcher_iam::ProjectScope>> {
        refuse_instance_wide(self.binding, "list the projects that have run here")?;
        let inner = self.inner.lock().await;
        let mut scopes: Vec<aiwatcher_iam::ProjectScope> = inner
            .ownership
            .values()
            .map(|owner| owner.scope)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        scopes.truncate(limit);
        Ok(scopes)
    }

    async fn checkpoint(&self, processor: &str) -> Result<Option<Checkpoint>> {
        refuse_instance_wide(self.binding, "read a processor checkpoint")?;
        Ok(self.inner.lock().await.checkpoints.get(processor).cloned())
    }

    async fn claim_attempt(
        &self,
        filter: &ClaimFilter,
        owner: &str,
        now: OffsetDateTime,
    ) -> Result<Option<AttemptRow>> {
        let mut inner = self.inner.lock().await;
        // The lock is the `SELECT … FOR UPDATE SKIP LOCKED`: whoever holds it
        // sees the row unclaimed and leaves it claimed, so two claimants
        // racing produce one claim and one `None`.
        //
        // The scope is checked before the filter and not by it: a claimant says
        // what it can *run*, and which executions it may reach at all is the
        // store's binding rather than something a claim filter could assert
        // about itself.
        let binding = self.binding;
        let Some(key) = inner
            .attempts
            .values()
            .find(|row| {
                row.is_claimable(now)
                    && filter.matches(row)
                    && inner.admits(binding, row.key.execution_id.as_str())
            })
            .map(|row| row.key.clone())
        else {
            return Ok(None);
        };
        let row = inner.attempts.get_mut(&key).map(|row| {
            row.claim(owner, now);
            row.clone()
        });
        Ok(row)
    }

    async fn heartbeat(&self, key: &AttemptKey, owner: &str, now: OffsetDateTime) -> Result<bool> {
        let mut inner = self.inner.lock().await;
        inner.check(self.binding, &key.execution_id)?;
        let Some(row) = inner.attempts.get_mut(key) else {
            return Ok(false);
        };
        if !row.is_held_by(owner, now) {
            return Ok(false);
        }
        row.claimed_at = Some(now);
        Ok(true)
    }

    async fn attempt(&self, key: &AttemptKey) -> Result<Option<AttemptRow>> {
        let inner = self.inner.lock().await;
        inner.check(self.binding, &key.execution_id)?;
        Ok(inner.attempts.get(key).cloned())
    }

    async fn unclaimed_attempts(&self, now: OffsetDateTime) -> Result<BTreeMap<RuntimeKind, u64>> {
        let inner = self.inner.lock().await;
        let binding = self.binding;
        Ok(tally_unclaimed(
            inner
                .attempts
                .values()
                .filter(|row| inner.admits(binding, row.key.execution_id.as_str())),
            now,
        ))
    }

    async fn claimable_attempts(
        &self,
        runtime: RuntimeKind,
        now: OffsetDateTime,
        limit: usize,
    ) -> Result<Vec<AttemptRow>> {
        let inner = self.inner.lock().await;
        let binding = self.binding;
        Ok(claimable_of(
            inner
                .attempts
                .values()
                .filter(|row| inner.admits(binding, row.key.execution_id.as_str())),
            runtime,
            now,
            limit,
        ))
    }

    async fn advance_checkpoint(&self, processor: &str, checkpoint: Checkpoint) -> Result<()> {
        refuse_instance_wide(self.binding, "advance a processor checkpoint")?;
        self.inner
            .lock()
            .await
            .checkpoints
            .insert(processor.to_owned(), checkpoint);
        Ok(())
    }

    async fn prune(&self, before: OffsetDateTime, limit: usize) -> Result<Pruned> {
        let mut inner = self.inner.lock().await;

        // Which executions the outbox still speaks for. Collected before
        // anything is chosen, because the answer has to be the same for every
        // candidate in one pass.
        let unpublished: HashSet<String> = inner
            .outbox
            .iter()
            .filter(|row| row.published_at.is_none())
            .map(|row| row.partition_key.clone())
            .collect();

        let binding = self.binding;
        let doomed: Vec<String> = inner
            .projections
            .iter()
            .filter(|(execution, _)| inner.admits(binding, execution))
            .filter(|(execution, run)| {
                prunable(run, last_activity(inner.streams.get(*execution)), before)
            })
            .filter(|(execution, _)| !unpublished.contains(&format!("workflow:{execution}")))
            .map(|(execution, _)| execution.clone())
            .take(limit)
            .collect();

        let mut pruned = Pruned::default();
        for execution in &doomed {
            inner.streams.remove(execution);
            inner.seen.remove(execution);
            inner.projections.remove(execution);
            // With the execution, never before or after it: an ownership row
            // left behind names a run that no longer exists, and one removed
            // early leaves a live run readable from the unscoped path.
            inner.ownership.remove(execution);
            pruned.executions += 1;
        }
        let doomed: HashSet<&String> = doomed.iter().collect();
        let before_count = inner.attempts.len();
        inner
            .attempts
            .retain(|key, _| !doomed.contains(&key.execution_id.to_string()));
        pruned.attempts = before_count - inner.attempts.len();
        Ok(pruned)
    }
}

/// Refuse what a project-bound store has no answer for.
///
/// Processor checkpoints and schedule slots are instance-wide: there is one
/// cursor per processor and one slot per definition, and neither has a scoped
/// form yet. A bound store says so by name rather than answering for the global
/// one, which would be the fallback this boundary is about.
fn refuse_instance_wide(binding: ScopeBinding, what: &str) -> Result<()> {
    if binding.scope().is_global() {
        return Ok(());
    }
    Err(StoreError::NotInThisScope {
        what: format!("{what} while bound to {}", binding.scope().label()),
    })
}

/// When this store last wrote anything about an execution.
///
/// The epoch for a stream that is not there, which is prunable by every window
/// — a projection with no stream behind it is the half-state a crash between
/// the two writes leaves, and keeping it forever is how it becomes permanent.
fn last_activity(stream: Option<&Vec<RecordedMessage>>) -> OffsetDateTime {
    stream
        .and_then(|messages| messages.last())
        .map_or(OffsetDateTime::UNIX_EPOCH, |message| message.recorded_at)
}

/// The messages one decision produced, from a loaded slice — used by the
/// contract suite to prove that input and outputs land together.
#[must_use]
pub fn outputs_at(slice: &StreamSlice, after: u64) -> Vec<&RecordedMessage> {
    slice
        .messages
        .iter()
        .filter(|message| message.stream_version > after && message.direction == Direction::Output)
        .collect()
}
