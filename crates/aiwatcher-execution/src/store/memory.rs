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

use crate::claim::{AttemptKey, AttemptRow, AttemptWrite, ClaimFilter};
use crate::error::{Result, StoreError};
use crate::message::{Direction, OutboxMessage, RecordedMessage, RunProjection};
use crate::plan::DefinitionKind;
use crate::schedule::slot::{
    SlotAdmission, SlotAdmissionRequest, SlotKey, SlotRecord, SlotSettlement,
};
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
}

/// An in-memory workflow store.
#[derive(Clone, Debug, Default)]
pub struct MemoryWorkflowStore {
    inner: Arc<Mutex<Inner>>,
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

    async fn load(&self, execution: &ExecutionId) -> Result<StreamSlice> {
        let inner = self.inner.lock().await;
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

        // The inbox first: a redelivery is not a conflict, and answering it
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
                // A finished attempt is not a row. See `AttemptWrite`.
                AttemptWrite::Retire(key) => {
                    inner.attempts.remove(&key);
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
        Ok(self
            .inner
            .lock()
            .await
            .projections
            .get(execution.as_str())
            .cloned())
    }

    async fn pending_outbox(&self, limit: usize) -> Result<Vec<OutboxMessage>> {
        Ok(self
            .inner
            .lock()
            .await
            .outbox
            .iter()
            .filter(|row| row.published_at.is_none())
            .take(limit)
            .cloned()
            .collect())
    }

    async fn mark_published(&self, ids: &[MessageId], _at: OffsetDateTime) -> Result<()> {
        // Dropped rather than flagged: the fact is on the log, and a second
        // copy here would grow with every step of every run.
        self.inner
            .lock()
            .await
            .outbox
            .retain(|row| !ids.contains(&row.message_id));
        Ok(())
    }

    async fn admit_slot(&self, request: &SlotAdmissionRequest) -> Result<SlotAdmission> {
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

    async fn checkpoint(&self, processor: &str) -> Result<Option<Checkpoint>> {
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
        let Some(key) = inner
            .attempts
            .values()
            .find(|row| row.is_claimable(now) && filter.matches(row))
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
        Ok(self.inner.lock().await.attempts.get(key).cloned())
    }

    async fn advance_checkpoint(&self, processor: &str, checkpoint: Checkpoint) -> Result<()> {
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

        let doomed: Vec<String> = inner
            .projections
            .iter()
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
