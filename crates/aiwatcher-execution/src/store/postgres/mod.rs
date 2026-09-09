//! One transaction per workflow decision.
//!
//! A module behind the `postgres` feature rather than a crate — the shape
//! `laser` has in `aiwatcher-bus`, which keeps `sqlx` out of every build that
//! does not ask for it and keeps a crate named for its capability.
//!
//! Everything in [`AppendRequest`] lands together or none of it does: the
//! input, the outputs, the projection, the attempt rows, the outbox and the
//! checkpoint.
//!
//! * **The version check is the primary key, not a `SELECT`.** Two deciders
//!   racing to write version *N* produce one row and one unique violation,
//!   surfaced as [`StoreError::VersionConflict`]. `SELECT … FOR UPDATE` would
//!   serialise every command on an execution for a decision that never blocks.
//! * **The inbox is checked inside the transaction.** Outside it, a redelivery
//!   arriving beside its first delivery decides twice.
//! * **A claim is `FOR UPDATE SKIP LOCKED`**, so two workers polling together
//!   take two different attempts rather than one waiting on the other's lock.
//!
//! ADR_0025.

pub mod error;
pub mod schema;

use async_trait::async_trait;
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row as _};
use time::OffsetDateTime;

use crate::claim::{AttemptKey, AttemptRow, AttemptWrite, ClaimFilter};
use crate::hosted::{DeciderLease, LeaseOutcome, Timer, TimerWrite};
use crate::message::{
    Direction, MessageMetadata, OutboxMessage, RecordedMessage, RunProjection, WorkflowEvent,
    WorkflowMessage,
};
use crate::plan::DefinitionKind;
use crate::schedule::slot::{
    SlotAdmission, SlotAdmissionRequest, SlotKey, SlotOutcome, SlotRecord, SlotSettlement,
};
use crate::state::{ExecutionId, StateType};
use crate::store::{
    AppendOutcome, AppendRequest, ExpectedVersion, Pruned, StoreCapabilities, StreamSlice,
    WorkflowStore,
};
use crate::{Result, StoreError};
use aiwatcher_core::{Checkpoint, MessageId};

pub use self::error::PostgresError;

use self::error::is_unique_violation;

/// The workflow store on PostgreSQL.
#[derive(Clone, Debug)]
pub struct PostgresWorkflowStore {
    pool: PgPool,
}

impl PostgresWorkflowStore {
    /// Wrap a pool whose schema is already applied.
    ///
    /// [`crate::connect`] is the usual door; this is for a caller that owns its
    /// own pool.
    #[must_use]
    pub const fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub const fn pool(&self) -> &PgPool {
        &self.pool
    }
}

#[async_trait]
impl WorkflowStore for PostgresWorkflowStore {
    fn capabilities(&self) -> StoreCapabilities {
        // The reason this adapter exists. Everything a managed run needs more
        // than one process for — a worker, a container job, a second API
        // replica — is refused on `file` and allowed here.
        StoreCapabilities {
            multi_process: true,
            claimable: true,
        }
    }

    async fn load(&self, execution: &ExecutionId) -> Result<StreamSlice> {
        let rows = sqlx::query(
            "select stream_version, direction, message, metadata, recorded_at
               from workflow_messages
              where execution_id = $1
              order by stream_version",
        )
        .bind(execution.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(|error| StoreError::Backend(error.to_string()))?;

        let mut messages = Vec::with_capacity(rows.len());
        for row in rows {
            messages.push(recorded_from(&row)?);
        }
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
        // The version comes from its own query rather than from the page's
        // length: a page is a window, and `max(stream_version)` is the only
        // thing that tells a reader whether it is looking at the end.
        let version: i64 = sqlx::query_scalar(
            "select coalesce(max(stream_version), 0) from workflow_messages where execution_id = $1",
        )
        .bind(execution.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(|error| StoreError::Backend(error.to_string()))?;

        let rows = sqlx::query(
            "select stream_version, direction, message, metadata, recorded_at
               from workflow_messages
              where execution_id = $1 and stream_version > $2
              order by stream_version
              limit $3",
        )
        .bind(execution.as_str())
        .bind(after as i64)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| StoreError::Backend(error.to_string()))?;

        let mut messages = Vec::with_capacity(rows.len());
        for row in rows {
            messages.push(recorded_from(&row)?);
        }
        Ok(StreamSlice {
            version: version as u64,
            messages,
        })
    }

    async fn due_timers(&self, now: OffsetDateTime, limit: usize) -> Result<Vec<Timer>> {
        let rows = sqlx::query(
            "select execution_id, timer_id, due_at, message
               from execution_timers
              where due_at <= $1
              order by due_at
              limit $2",
        )
        .bind(now)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| StoreError::Backend(error.to_string()))?;
        rows.iter().map(timer_from).collect()
    }

    async fn timers_of(&self, execution: &ExecutionId) -> Result<Vec<Timer>> {
        let rows = sqlx::query(
            "select execution_id, timer_id, due_at, message
               from execution_timers where execution_id = $1 order by due_at",
        )
        .bind(execution.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(|error| StoreError::Backend(error.to_string()))?;
        rows.iter().map(timer_from).collect()
    }

    async fn recorded_outcome(&self, key: &AttemptKey) -> Result<Option<WorkflowEvent>> {
        // The index `0009` adds, matched exactly: the same expression, the same
        // partial condition. A read that spelled it differently would be a
        // sequential scan over every message of every run — which is the whole
        // of the performance item this replaces.
        let row: Option<serde_json::Value> = sqlx::query_scalar(
            "select message from workflow_messages
              where execution_id = $1
                and message_type in ('step_completed', 'step_failed')
                and message ->> 'step_id' = $2
                and (message ->> 'attempt')::int = $3
                and direction = 'output'
              order by stream_version
              limit 1",
        )
        .bind(key.execution_id.as_str())
        .bind(&key.step_id)
        .bind(i64::from(key.attempt))
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| StoreError::Backend(error.to_string()))?;
        let Some(document) = row else {
            return Ok(None);
        };
        let message: WorkflowMessage =
            serde_json::from_value(document).map_err(StoreError::Encoding)?;
        Ok(message.event().cloned())
    }

    async fn take_decider_lease(
        &self,
        execution: &ExecutionId,
        holder: &str,
        now: OffsetDateTime,
    ) -> Result<LeaseOutcome> {
        // One statement, because two — read, then write — is the race this
        // exists to settle. The `where` is the whole rule: insert when nobody
        // has it, update only when it is already this holder's or has run out.
        // A row somebody else holds live matches neither, so nothing is written
        // and `returning` yields no row, which is how the refusal is told apart
        // from the grant.
        let row = sqlx::query(
            "insert into execution_decider_leases (execution_id, holder, claimed_at)
             values ($1, $2, $3)
             on conflict (execution_id) do update set
               previous_holder = case
                 when execution_decider_leases.holder = excluded.holder
                   then execution_decider_leases.previous_holder
                 else execution_decider_leases.holder
               end,
               holder = excluded.holder,
               claimed_at = excluded.claimed_at
             where execution_decider_leases.holder = excluded.holder
                or execution_decider_leases.claimed_at < $4
             returning holder, previous_holder, claimed_at",
        )
        .bind(execution.as_str())
        .bind(holder)
        .bind(now)
        .bind(now - time::Duration::seconds(aiwatcher_jobs::LEASE_SECONDS))
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| StoreError::Backend(error.to_string()))?;

        if let Some(row) = row {
            return Ok(LeaseOutcome::Taken(lease_from(execution, &row)));
        }

        // Nothing was written, so somebody else holds it and it is live. Who,
        // and until when — the one thing a refused decider can act on.
        let held = self.decider_lease(execution).await?.ok_or_else(|| {
            StoreError::Backend("the lease vanished between two reads".to_owned())
        })?;
        Ok(LeaseOutcome::Held {
            expires_at: held.expires_at(),
            holder: held.holder,
        })
    }

    async fn release_decider_lease(
        &self,
        execution: &ExecutionId,
        holder: &str,
        now: OffsetDateTime,
    ) -> Result<bool> {
        let done = sqlx::query(
            "delete from execution_decider_leases
              where execution_id = $1 and holder = $2 and claimed_at >= $3",
        )
        .bind(execution.as_str())
        .bind(holder)
        .bind(now - time::Duration::seconds(aiwatcher_jobs::LEASE_SECONDS))
        .execute(&self.pool)
        .await
        .map_err(|error| StoreError::Backend(error.to_string()))?;
        Ok(done.rows_affected() > 0)
    }

    async fn decider_lease(&self, execution: &ExecutionId) -> Result<Option<DeciderLease>> {
        let row = sqlx::query(
            "select holder, previous_holder, claimed_at
               from execution_decider_leases where execution_id = $1",
        )
        .bind(execution.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| StoreError::Backend(error.to_string()))?;
        Ok(row.map(|row| lease_from(execution, &row)))
    }

    async fn append(
        &self,
        execution: &ExecutionId,
        request: AppendRequest,
    ) -> Result<AppendOutcome> {
        request.check_payloads()?;
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| StoreError::Backend(error.to_string()))?;

        let version: i64 = sqlx::query_scalar(
            "select coalesce(max(stream_version), 0) from workflow_messages where execution_id = $1",
        )
        .bind(execution.as_str())
        .fetch_one(&mut *transaction)
        .await
        .map_err(|error| StoreError::Backend(error.to_string()))?;
        let version = version as u64;

        // The inbox first, and inside the transaction: a redelivery is not a
        // conflict, and answering it with one would make every at-least-once
        // retry look like a race.
        let seen: Option<i64> = sqlx::query_scalar(
            "select stream_version from workflow_messages
              where execution_id = $1 and message_id = $2",
        )
        .bind(execution.as_str())
        .bind(request.input.metadata.message_id.as_str())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|error| StoreError::Backend(error.to_string()))?;
        if let Some(seen) = seen {
            return Ok(AppendOutcome::Duplicate {
                version: seen as u64,
            });
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
        let mut next = version;
        for message in std::iter::once(request.input.clone()).chain(request.outputs) {
            next += 1;
            let kind = message.message.kind();
            sqlx::query(
                "insert into workflow_messages
                   (execution_id, stream_version, message_id, kind, direction,
                    message_type, message, metadata, occurred_at, recorded_at)
                 values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
            )
            .bind(execution.as_str())
            .bind(next as i64)
            .bind(message.metadata.message_id.as_str())
            .bind(kind)
            .bind(direction_str(message.direction))
            .bind(message.message.name())
            .bind(serde_json::to_value(&message.message).map_err(StoreError::Encoding)?)
            .bind(serde_json::to_value(&message.metadata).map_err(StoreError::Encoding)?)
            .bind(message.metadata.occurred_at)
            .bind(now)
            .execute(&mut *transaction)
            .await
            // The primary key *is* the compare-and-append: whoever loses the
            // race gets this, and it is contention rather than a failure.
            .map_err(|error| {
                if is_unique_violation(&error) {
                    StoreError::VersionConflict {
                        expected: version,
                        actual: next,
                    }
                } else {
                    StoreError::Backend(error.to_string())
                }
            })?;
        }

        upsert_projection(&mut transaction, &request.projection).await?;

        for write in request.timers {
            match write {
                TimerWrite::Schedule(timer) => {
                    sqlx::query(
                        "insert into execution_timers
                           (execution_id, timer_id, due_at, message)
                         values ($1, $2, $3, $4)
                         on conflict (execution_id, timer_id) do update set
                           due_at = excluded.due_at,
                           message = excluded.message",
                    )
                    .bind(timer.execution.as_str())
                    .bind(&timer.timer_id)
                    .bind(timer.due_at)
                    .bind(serde_json::to_value(&timer.message).map_err(StoreError::Encoding)?)
                    .execute(&mut *transaction)
                    .await
                    .map_err(|error| StoreError::Backend(error.to_string()))?;
                }
                TimerWrite::Cancel(id) | TimerWrite::Fire(id) => {
                    sqlx::query(
                        "delete from execution_timers
                          where execution_id = $1 and timer_id = $2",
                    )
                    .bind(execution.as_str())
                    .bind(&id)
                    .execute(&mut *transaction)
                    .await
                    .map_err(|error| StoreError::Backend(error.to_string()))?;
                }
            }
        }

        for write in request.attempts {
            match write {
                AttemptWrite::Dispatch(row) => {
                    upsert_attempt(&mut transaction, &row).await?;
                }
                // A finished attempt is not a row. See `AttemptWrite`.
                AttemptWrite::Retire(key) => {
                    retire_attempt(&mut transaction, &key).await?;
                }
            }
        }

        for row in request.outbox {
            sqlx::query(
                "insert into outbox_messages
                   (message_id, execution_id, event_type, partition_key, payload,
                    available_at, attempts, published_at, last_error)
                 values ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                 on conflict (message_id) do nothing",
            )
            .bind(row.message_id.as_str())
            .bind(execution.as_str())
            .bind(&row.event_type)
            .bind(&row.partition_key)
            .bind(&row.payload)
            .bind(row.available_at)
            .bind(i32::try_from(row.attempts).unwrap_or(i32::MAX))
            .bind(row.published_at)
            .bind(&row.last_error)
            .execute(&mut *transaction)
            .await
            .map_err(|error| StoreError::Backend(error.to_string()))?;
        }

        if let Some((processor, checkpoint)) = request.checkpoint {
            write_checkpoint(&mut transaction, &processor, &checkpoint).await?;
        }

        transaction
            .commit()
            .await
            .map_err(|error| StoreError::Backend(error.to_string()))?;
        Ok(AppendOutcome::Appended { version: next })
    }

    async fn projection(&self, execution: &ExecutionId) -> Result<Option<RunProjection>> {
        let row = sqlx::query(
            "select execution_id, plan_id, definition_name, owner, mode, state_type,
                    state_name, requested_by, steps, last_message_version,
                    created_at
               from execution_runs where execution_id = $1",
        )
        .bind(execution.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| StoreError::Backend(error.to_string()))?;
        row.map(|row| projection_from(&row)).transpose()
    }

    async fn pending_outbox(&self, limit: usize) -> Result<Vec<OutboxMessage>> {
        let rows = sqlx::query(
            "select message_id, event_type, partition_key, payload, available_at,
                    attempts, published_at, last_error
               from outbox_messages
              where published_at is null
              order by available_at, message_id
              limit $1",
        )
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await
        .map_err(|error| StoreError::Backend(error.to_string()))?;
        rows.iter().map(outbox_from).collect()
    }

    async fn mark_published(&self, ids: &[MessageId], _at: OffsetDateTime) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let ids: Vec<&str> = ids.iter().map(MessageId::as_str).collect();
        // Deleted rather than flagged. The fact is on the event log once the
        // sink has taken it, and a published row here would answer no question
        // while the table grew with every step of every run — the one table in
        // this schema whose rows have a reader that finishes with them.
        sqlx::query("delete from outbox_messages where message_id = any($1)")
            .bind(&ids)
            .execute(&self.pool)
            .await
            .map_err(|error| StoreError::Backend(error.to_string()))?;
        Ok(())
    }

    async fn claim_attempt(
        &self,
        filter: &ClaimFilter,
        owner: &str,
        now: OffsetDateTime,
    ) -> Result<Option<AttemptRow>> {
        if filter.is_empty() {
            // A claimant that named nothing takes nothing. Without this the
            // query below would match every row and hand a reactor a worker's
            // attempt.
            return Ok(None);
        }
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| StoreError::Backend(error.to_string()))?;

        let queues: Vec<&str> = filter.queues.iter().map(String::as_str).collect();
        let runtimes: Vec<&str> = filter
            .runtimes
            .iter()
            .map(|runtime| runtime.as_str())
            .collect();
        let tasks: Vec<&str> = filter.tasks.iter().map(String::as_str).collect();
        let stale = now - time::Duration::seconds(aiwatcher_jobs::LEASE_SECONDS);

        // `SKIP LOCKED` is what makes two claimants polling together take two
        // attempts rather than one waiting on the other's lock. `queue is null`
        // is the reactor's half and `queue = any($1)` the worker's, and they do
        // not overlap: a pulled attempt belongs to whoever holds its queue.
        //
        // `task_ref = any($5)` is [`ClaimFilter::matches`]' second condition in
        // SQL, and it has to be here rather than checked after the row comes
        // back: a claim that filtered in Rust would have already taken the
        // lease, and releasing it again is a five-minute stall on a row
        // somebody else could have run.
        let row = sqlx::query(
            "select execution_id, step_id, attempt, runtime, command_id, queue,
                    task_ref, state, lease_owner, previous_owner, claimed_at, not_before
               from step_attempts
              where state not in ('completed', 'failed', 'crashed', 'cancelled',
                                  'awaiting_input')
                and (claimed_at is null or claimed_at <= $3)
                and (not_before is null or not_before <= $4)
                and ($6::text is null or (execution_id = $6 and step_id = $7 and attempt::bigint = $8))
                and ( (queue is not null and queue = any($1)
                       and task_ref is not null and task_ref = any($5))
                   or (queue is null and runtime = any($2)) )
              order by updated_at
              for update skip locked
              limit 1",
        )
        .bind(&queues)
        .bind(&runtimes)
        .bind(stale)
        .bind(now)
        .bind(&tasks)
        .bind(filter.attempt.as_ref().map(|key| key.execution_id.as_str()))
        .bind(filter.attempt.as_ref().map(|key| key.step_id.as_str()))
        .bind(filter.attempt.as_ref().map(|key| i64::from(key.attempt)))
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|error| StoreError::Backend(error.to_string()))?;

        let Some(row) = row else {
            return Ok(None);
        };
        let mut claimed = attempt_from(&row)?;
        claimed.claim(owner, now);
        upsert_attempt(&mut transaction, &claimed).await?;
        transaction
            .commit()
            .await
            .map_err(|error| StoreError::Backend(error.to_string()))?;
        Ok(Some(claimed))
    }

    async fn heartbeat(&self, key: &AttemptKey, owner: &str, now: OffsetDateTime) -> Result<bool> {
        let stale = now - time::Duration::seconds(aiwatcher_jobs::LEASE_SECONDS);
        // The `lease_owner` and freshness checks are in the `where`, not in a
        // read followed by a write: a renewal that read a live lease and wrote
        // after it expired would be a worker renewing its replacement's claim.
        let updated = sqlx::query(
            "update step_attempts
                set claimed_at = $1, updated_at = $1
              where execution_id = $2 and step_id = $3 and attempt = $4
                and lease_owner = $5 and claimed_at > $6",
        )
        .bind(now)
        .bind(key.execution_id.as_str())
        .bind(&key.step_id)
        .bind(i32::try_from(key.attempt).unwrap_or(i32::MAX))
        .bind(owner)
        .bind(stale)
        .execute(&self.pool)
        .await
        .map_err(|error| StoreError::Backend(error.to_string()))?;
        Ok(updated.rows_affected() > 0)
    }

    async fn attempt(&self, key: &AttemptKey) -> Result<Option<AttemptRow>> {
        let row = sqlx::query(
            "select execution_id, step_id, attempt, runtime, command_id, queue,
                    task_ref, state, lease_owner, previous_owner, claimed_at, not_before
               from step_attempts
              where execution_id = $1 and step_id = $2 and attempt = $3",
        )
        .bind(key.execution_id.as_str())
        .bind(&key.step_id)
        .bind(i32::try_from(key.attempt).unwrap_or(i32::MAX))
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| StoreError::Backend(error.to_string()))?;
        row.map(|row| attempt_from(&row)).transpose()
    }

    async fn admit_slot(&self, request: &SlotAdmissionRequest) -> Result<SlotAdmission> {
        // One transaction, and the row lock is what makes it one. `SELECT …
        // FOR UPDATE` on the slot serialises two replicas that both found the
        // same slot due, so the overlap check below cannot be read by both
        // before either writes.
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| StoreError::Backend(error.to_string()))?;

        let held = sqlx::query(
            "select state, execution_id, detail, lease_owner, leased_at
               from schedule_slots
              where definition_kind = $1 and definition_name = $2 and slot = $3
                for update",
        )
        .bind(request.key.definition_kind.as_str())
        .bind(&request.key.definition_name)
        .bind(request.key.slot)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|error| StoreError::Backend(error.to_string()))?;

        let mut detail: Option<String> = None;
        if let Some(row) = &held {
            if let Some(outcome) = outcome_from(row) {
                return Ok(SlotAdmission::Settled { outcome });
            }
            let leased_at: Option<OffsetDateTime> = row.get("leased_at");
            let owner: Option<String> = row.get("lease_owner");
            let live = leased_at.is_some_and(|at| {
                request.now - at < time::Duration::seconds(aiwatcher_jobs::LEASE_SECONDS)
            });
            if live {
                return Ok(SlotAdmission::Held {
                    owner: owner.unwrap_or_default(),
                });
            }
            detail = row.get("detail");
        }

        if request.overlap == crate::OverlapPolicy::Skip {
            // In the same transaction as the claim, and against the projection
            // this store already holds — never the read model, which is
            // asynchronous and empty in the role the tick runs in.
            let running: Option<String> = sqlx::query_scalar(
                "select execution_id from execution_runs
                  where definition_name = $1 and state_type <> all($2)
                  order by execution_id
                  limit 1",
            )
            .bind(&request.key.definition_name)
            .bind(
                StateType::TERMINAL
                    .iter()
                    .map(|state| state.as_str())
                    .collect::<Vec<&str>>(),
            )
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|error| StoreError::Backend(error.to_string()))?;
            if let Some(execution_id) = running {
                return Ok(SlotAdmission::Blocked { execution_id });
            }
        }

        sqlx::query(
            "insert into schedule_slots
               (definition_kind, definition_name, slot, state, execution_id,
                detail, lease_owner, leased_at, updated_at)
             values ($1, $2, $3, null, null, $4, $5, $6, $6)
             on conflict (definition_kind, definition_name, slot) do update set
               detail = excluded.detail,
               lease_owner = excluded.lease_owner,
               leased_at = excluded.leased_at,
               updated_at = excluded.updated_at",
        )
        .bind(request.key.definition_kind.as_str())
        .bind(&request.key.definition_name)
        .bind(request.key.slot)
        .bind(detail)
        .bind(&request.owner)
        .bind(request.now)
        .execute(&mut *transaction)
        .await
        .map_err(|error| StoreError::Backend(error.to_string()))?;

        transaction
            .commit()
            .await
            .map_err(|error| StoreError::Backend(error.to_string()))?;
        Ok(SlotAdmission::Admitted)
    }

    async fn settle_slot(
        &self,
        key: &SlotKey,
        owner: &str,
        settlement: SlotSettlement,
        now: OffsetDateTime,
    ) -> Result<()> {
        let state = settlement.outcome().map(outcome_str);
        let execution_id = settlement.execution_id();
        let detail = settlement.detail();

        // `lease_owner = $5` in the predicate is what makes a settlement from a
        // caller whose lease was taken over a no-op rather than a stale write
        // over a fresh decision.
        sqlx::query(
            "update schedule_slots
                set state = $4, execution_id = $6, detail = $7,
                    lease_owner = null, leased_at = null, updated_at = $8
              where definition_kind = $1 and definition_name = $2 and slot = $3
                and lease_owner = $5 and state is null",
        )
        .bind(key.definition_kind.as_str())
        .bind(&key.definition_name)
        .bind(key.slot)
        .bind(state)
        .bind(owner)
        .bind(execution_id)
        .bind(detail)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|error| StoreError::Backend(error.to_string()))?;
        Ok(())
    }

    async fn recent_slots(
        &self,
        kind: DefinitionKind,
        name: &str,
        limit: usize,
    ) -> Result<Vec<SlotRecord>> {
        let rows = sqlx::query(
            "select definition_name, slot, state, execution_id,
                    detail, lease_owner, leased_at, updated_at
               from schedule_slots
              where definition_kind = $1 and definition_name = $2
              order by slot desc
              limit $3",
        )
        .bind(kind.as_str())
        .bind(name)
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await
        .map_err(|error| StoreError::Backend(error.to_string()))?;
        Ok(rows.iter().map(|row| slot_from(row, kind)).collect())
    }

    async fn checkpoint(&self, processor: &str) -> Result<Option<Checkpoint>> {
        let value: Option<String> = sqlx::query_scalar(
            "select checkpoint from processor_checkpoints where processor_id = $1",
        )
        .bind(processor)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| StoreError::Backend(error.to_string()))?;
        Ok(value.and_then(|value| Checkpoint::parse(&value).ok()))
    }

    async fn advance_checkpoint(&self, processor: &str, checkpoint: Checkpoint) -> Result<()> {
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| StoreError::Backend(error.to_string()))?;
        write_checkpoint(&mut transaction, processor, &checkpoint).await?;
        transaction
            .commit()
            .await
            .map_err(|error| StoreError::Backend(error.to_string()))?;
        Ok(())
    }

    /// One bounded transaction: choose, then delete all four tables together.
    ///
    /// `for update skip locked` is what makes two work replicas sweeping at
    /// once take disjoint sets rather than the same one twice — the same reason
    /// [`Self::claim_attempt`] uses it, and here it keeps the counts honest as
    /// well as the work halved.
    async fn prune(&self, before: OffsetDateTime, limit: usize) -> Result<Pruned> {
        let terminal: Vec<&str> = StateType::TERMINAL.iter().map(|s| s.as_str()).collect();
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| StoreError::Backend(error.to_string()))?;

        // The candidate test, in one place: terminal, quiet for long enough,
        // and not still spoken for by the outbox. The last clause is
        // `aiwatcher_jobs::ORDERING` — the durable copy reaches the log before
        // the explanation behind it is forgotten.
        let doomed: Vec<String> = sqlx::query_scalar(
            "select execution_id from execution_runs
              where state_type = any($1)
                and updated_at < $2
                and not exists (
                      select 1 from outbox_messages o
                       where o.execution_id = execution_runs.execution_id
                         and o.published_at is null)
              -- No `order by`. Oldest-first reads well and is what the index
              -- would have to be sorted into: `state_type = any(...)` over a
              -- composite index returns each state's rows in its own order, so
              -- a global ordering is a sort of every prunable row in the table
              -- to take five hundred of them. The sweep loops until it drains,
              -- so which five hundred is a question with no consequence — and
              -- neither of the other two adapters orders either. Measured on
              -- 50 000 rows: without it the plan is an index scan whose
              -- condition covers both columns and touches only the hundred
              -- prunable rows; with it, a Sort above that scan.
              limit $3
              for update skip locked",
        )
        .bind(&terminal)
        .bind(before)
        .bind(limit as i64)
        .fetch_all(&mut *transaction)
        .await
        .map_err(|error| StoreError::Backend(error.to_string()))?;

        if doomed.is_empty() {
            transaction
                .rollback()
                .await
                .map_err(|error| StoreError::Backend(error.to_string()))?;
            return Ok(Pruned::default());
        }

        let attempts: i64 =
            sqlx::query_scalar("with gone as (delete from step_attempts where execution_id = any($1) returning 1) select count(*) from gone")
                .bind(&doomed)
                .fetch_one(&mut *transaction)
                .await
                .map_err(|error| StoreError::Backend(error.to_string()))?;

        for statement in [
            "delete from workflow_messages where execution_id = any($1)",
            // Published rows only: an unpublished one would have kept its
            // execution out of `doomed` in the first place.
            "delete from outbox_messages where execution_id = any($1)",
            // With the run, never after it: a lease naming an execution this
            // store has forgotten is a row nothing will ever release.
            "delete from execution_decider_leases where execution_id = any($1)",
            // With the run: a timer naming an execution this store has
            // forgotten would fire a message into a stream that is gone.
            "delete from execution_timers where execution_id = any($1)",
            "delete from execution_runs where execution_id = any($1)",
        ] {
            sqlx::query(statement)
                .bind(&doomed)
                .execute(&mut *transaction)
                .await
                .map_err(|error| StoreError::Backend(error.to_string()))?;
        }

        transaction
            .commit()
            .await
            .map_err(|error| StoreError::Backend(error.to_string()))?;
        Ok(Pruned {
            executions: doomed.len(),
            attempts: usize::try_from(attempts).unwrap_or(0),
        })
    }
}

/// The decision a slot row carries, if it carries one.
fn outcome_from(row: &PgRow) -> Option<SlotOutcome> {
    match row.get::<Option<String>, _>("state")?.as_str() {
        "started" => Some(SlotOutcome::Started),
        "skipped" => Some(SlotOutcome::Skipped),
        // A state this build does not know reads as refused rather than as an
        // unsettled slot a newer build already decided — re-running somebody's
        // curation is the worse of the two failures.
        _ => Some(SlotOutcome::Refused),
    }
}

const fn outcome_str(outcome: SlotOutcome) -> &'static str {
    match outcome {
        SlotOutcome::Started => "started",
        SlotOutcome::Skipped => "skipped",
        SlotOutcome::Refused => "refused",
    }
}

fn slot_from(row: &PgRow, kind: DefinitionKind) -> SlotRecord {
    SlotRecord {
        key: SlotKey {
            definition_kind: kind,
            definition_name: row.get("definition_name"),
            slot: row.get("slot"),
        },
        outcome: outcome_from(row),
        execution_id: row.get("execution_id"),
        detail: row.get("detail"),
        lease_owner: row.get("lease_owner"),
        leased_at: row.get("leased_at"),
        updated_at: row.get("updated_at"),
    }
}

type Transaction<'a> = sqlx::Transaction<'a, sqlx::Postgres>;

async fn upsert_projection(
    transaction: &mut Transaction<'_>,
    projection: &RunProjection,
) -> Result<()> {
    sqlx::query(
        "insert into execution_runs
           (execution_id, plan_id, definition_name, owner, mode, state_type,
            state_name, requested_by, steps, last_message_version,
            created_at, updated_at)
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, now())
         on conflict (execution_id) do update set
           plan_id = excluded.plan_id,
           definition_name = excluded.definition_name,
           owner = excluded.owner,
           mode = excluded.mode,
           state_type = excluded.state_type,
           state_name = excluded.state_name,
           requested_by = excluded.requested_by,
           steps = excluded.steps,
           last_message_version = excluded.last_message_version,
           -- The retention clock. A clock read in the store rather than in
           -- `decide`, which is the one place that may not have one: this is
           -- when the row was written, not a fact the fold produced.
           updated_at = now()",
    )
    .bind(projection.execution_id.as_str())
    .bind(&projection.plan_id)
    .bind(&projection.definition_name)
    .bind(projection.owner.as_string())
    .bind(mode_str(projection.mode))
    .bind(projection.state.state_type.as_str())
    .bind(&projection.state.name)
    .bind(&projection.requested_by)
    .bind(serde_json::to_value(&projection.steps).map_err(StoreError::Encoding)?)
    .bind(projection.last_message_version as i64)
    .bind(projection.created_at)
    .execute(&mut **transaction)
    .await
    .map_err(|error| StoreError::Backend(error.to_string()))?;
    Ok(())
}

/// Take a finished attempt out of the claim table.
///
/// A delete rather than a state change, so the table is bounded by how much is
/// unfinished rather than by how much has ever run. `step_attempts_claimable`
/// is still partial, because `awaiting_input` is neither claimable nor an
/// ending and keeps its row.
async fn retire_attempt(transaction: &mut Transaction<'_>, key: &AttemptKey) -> Result<()> {
    sqlx::query(
        "delete from step_attempts
          where execution_id = $1 and step_id = $2 and attempt = $3",
    )
    .bind(key.execution_id.as_str())
    .bind(&key.step_id)
    .bind(i32::try_from(key.attempt).unwrap_or(i32::MAX))
    .execute(&mut **transaction)
    .await
    .map_err(|error| StoreError::Backend(error.to_string()))?;
    Ok(())
}

async fn upsert_attempt(transaction: &mut Transaction<'_>, row: &AttemptRow) -> Result<()> {
    sqlx::query(
        "insert into step_attempts
           (execution_id, step_id, attempt, runtime, command_id, queue, task_ref,
            state, lease_owner, previous_owner, claimed_at, not_before, updated_at)
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, now())
         on conflict (execution_id, step_id, attempt) do update set
           runtime = excluded.runtime,
           command_id = excluded.command_id,
           queue = excluded.queue,
           task_ref = excluded.task_ref,
           state = excluded.state,
           lease_owner = excluded.lease_owner,
           previous_owner = excluded.previous_owner,
           claimed_at = excluded.claimed_at,
           not_before = excluded.not_before,
           updated_at = now()",
    )
    .bind(row.key.execution_id.as_str())
    .bind(&row.key.step_id)
    .bind(i32::try_from(row.key.attempt).unwrap_or(i32::MAX))
    .bind(row.runtime.as_str())
    .bind(row.command_id.as_str())
    .bind(row.queue.as_deref())
    .bind(row.task_ref.as_deref())
    .bind(row.state.as_str())
    .bind(row.lease_owner.as_deref())
    .bind(row.previous_owner.as_deref())
    .bind(row.claimed_at)
    .bind(row.not_before)
    .execute(&mut **transaction)
    .await
    .map_err(|error| StoreError::Backend(error.to_string()))?;
    Ok(())
}

async fn write_checkpoint(
    transaction: &mut Transaction<'_>,
    processor: &str,
    checkpoint: &Checkpoint,
) -> Result<()> {
    sqlx::query(
        "insert into processor_checkpoints (processor_id, checkpoint, updated_at)
         values ($1, $2, now())
         on conflict (processor_id) do update set
           checkpoint = excluded.checkpoint, updated_at = now()",
    )
    .bind(processor)
    .bind(checkpoint.to_string())
    .execute(&mut **transaction)
    .await
    .map_err(|error| StoreError::Backend(error.to_string()))?;
    Ok(())
}

fn timer_from(row: &PgRow) -> Result<Timer> {
    Ok(Timer {
        execution: ExecutionId::new(row.get::<String, _>("execution_id")),
        timer_id: row.get("timer_id"),
        due_at: row.get("due_at"),
        message: serde_json::from_value(row.get("message")).map_err(StoreError::Encoding)?,
    })
}

fn lease_from(execution: &ExecutionId, row: &PgRow) -> DeciderLease {
    DeciderLease {
        execution: execution.clone(),
        holder: row.get("holder"),
        previous_holder: row.get("previous_holder"),
        claimed_at: row.get("claimed_at"),
    }
}

fn recorded_from(row: &PgRow) -> Result<RecordedMessage> {
    let message: WorkflowMessage =
        serde_json::from_value(row.get("message")).map_err(StoreError::Encoding)?;
    let metadata: MessageMetadata =
        serde_json::from_value(row.get("metadata")).map_err(StoreError::Encoding)?;
    Ok(RecordedMessage {
        stream_version: row.get::<i64, _>("stream_version") as u64,
        direction: direction_from(row.get("direction")),
        message,
        metadata,
        recorded_at: row.get("recorded_at"),
    })
}

fn projection_from(row: &PgRow) -> Result<RunProjection> {
    use crate::state::{ExecutionMode, ExecutionOwner, RunState};

    let state_type = state_type_from(row.get("state_type"));
    Ok(RunProjection {
        execution_id: ExecutionId::new(row.get::<String, _>("execution_id")),
        plan_id: row.get("plan_id"),
        definition_name: row.get("definition_name"),
        owner: ExecutionOwner::parse(row.get("owner")),
        mode: if row.get::<String, _>("mode") == "hosted" {
            ExecutionMode::Hosted
        } else {
            ExecutionMode::Compiled
        },
        state: RunState {
            state_type,
            name: row.get("state_name"),
        },
        requested_by: row.get("requested_by"),
        steps: serde_json::from_value(row.get("steps")).map_err(StoreError::Encoding)?,
        last_message_version: row.get::<i64, _>("last_message_version") as u64,
        created_at: row.get("created_at"),
    })
}

fn attempt_from(row: &PgRow) -> Result<AttemptRow> {
    let runtime = runtime_from(row.get("runtime"));
    Ok(AttemptRow {
        key: AttemptKey::new(
            ExecutionId::new(row.get::<String, _>("execution_id")),
            row.get::<String, _>("step_id"),
            row.get::<i32, _>("attempt") as u32,
        ),
        runtime,
        command_id: MessageId::new(row.get::<String, _>("command_id")),
        queue: row.get("queue"),
        task_ref: row.get("task_ref"),
        state: state_type_from(row.get("state")),
        lease_owner: row.get("lease_owner"),
        previous_owner: row.get("previous_owner"),
        claimed_at: row.get("claimed_at"),
        not_before: row.get("not_before"),
    })
}

fn outbox_from(row: &PgRow) -> Result<OutboxMessage> {
    Ok(OutboxMessage {
        message_id: MessageId::new(row.get::<String, _>("message_id")),
        event_type: row.get("event_type"),
        partition_key: row.get("partition_key"),
        payload: row.get("payload"),
        available_at: row.get("available_at"),
        attempts: row.get::<i32, _>("attempts").max(0) as u32,
        published_at: row.get("published_at"),
        last_error: row.get("last_error"),
    })
}

const fn direction_str(direction: Direction) -> &'static str {
    match direction {
        Direction::Input => "input",
        Direction::Output => "output",
    }
}

fn direction_from(value: String) -> Direction {
    if value == "input" {
        Direction::Input
    } else {
        Direction::Output
    }
}

const fn mode_str(mode: crate::state::ExecutionMode) -> &'static str {
    match mode {
        crate::state::ExecutionMode::Compiled => "compiled",
        crate::state::ExecutionMode::Hosted => "hosted",
    }
}

/// A state written by this build, or by a newer one.
///
/// An unrecognised word reads as `crashed` rather than as a panic or a
/// silently-running row: a state this build cannot name is one it must not
/// schedule, and `crashed` is terminal.
fn state_type_from(value: String) -> crate::state::StateType {
    use crate::state::StateType;
    match value.as_str() {
        "scheduled" => StateType::Scheduled,
        "pending" => StateType::Pending,
        "running" => StateType::Running,
        "awaiting_input" => StateType::AwaitingInput,
        "completed" => StateType::Completed,
        "failed" => StateType::Failed,
        "cancelled" => StateType::Cancelled,
        "paused" => StateType::Paused,
        _ => StateType::Crashed,
    }
}

/// Same rule as the state: a runtime this build cannot name is one it must not
/// claim, and `external_workflow` is the binding this process never runs.
fn runtime_from(value: String) -> crate::RuntimeKind {
    use crate::RuntimeKind;
    match value.as_str() {
        "flow_php" => RuntimeKind::FlowPhp,
        "marimo" => RuntimeKind::Marimo,
        "publish_dataset" => RuntimeKind::PublishDataset,
        "python_task" => RuntimeKind::PythonTask,
        "human_input" => RuntimeKind::HumanInput,
        _ => RuntimeKind::ExternalWorkflow,
    }
}

/// How long a caller waits for a connection before it is told the pool is full.
///
/// Short on purpose. A workflow decision is a few short statements, so a wait
/// past this is a pool that is too small or a query that is stuck, and both are
/// better reported than absorbed.
const ACQUIRE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Connect, apply the schema, and hand back the store.
///
/// The schema is applied here rather than by a separate step, because the one
/// thing worse than a migration nobody ran is two replicas disagreeing about
/// whether it ran. [`schema::apply`] holds an advisory lock for exactly that.
///
/// ADR_0009 applies to the database as to every other backend:
/// `detect-stack.py` reports `install | external | none`, and a second
/// PostgreSQL beside an existing one is the mistake that ADR exists to prevent.
/// One already runs in the `planner` namespace this system installs into.
///
/// # Errors
///
/// [`PostgresError::Connect`] when the database is unreachable, or
/// [`PostgresError::Migration`] naming the schema version that failed.
pub async fn connect(url: &str, max_connections: u32) -> Result<PostgresWorkflowStore> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(ACQUIRE_TIMEOUT)
        .connect(url)
        .await
        .map_err(|error| StoreError::Backend(PostgresError::Connect(error).to_string()))?;
    schema::apply(&pool).await?;
    Ok(PostgresWorkflowStore::from_pool(pool))
}
