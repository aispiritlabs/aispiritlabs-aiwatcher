//! One transaction per workflow decision.
//!
//! A module behind the `postgres` feature, not a crate. The plan proposed a
//! crate "so sqlx is out of every build that does not set the feature, exactly
//! as `laser` keeps laser_sdk out" — and that reason does not hold, because
//! `laser` is not a crate: `laser_sdk` and the ~360 crates beneath it are kept
//! out of `aiwatcher-bus` by an optional dependency behind a feature, from a
//! module beside `memory` and `wal`. This is the same shape, for the same
//! reason, and it also keeps the rule that a crate is named for its capability
//! and never for a vendor.
//!
//! Everything in [`AppendRequest`] lands together or none of it does — the
//! input, the outputs, the projection, the attempt rows, the outbox and the
//! checkpoint. That is the requirement that settled the backend (ADR_0025): the
//! event log offers ordered offsets and at-least-once delivery and no
//! transaction spanning itself and this state, and the object store cannot do a
//! compare-and-append at all on the store this system ships.
//!
//! Three things are worth reading closely.
//!
//! **The version check is the primary key, not a `SELECT`.** Two deciders
//! racing to write version *N* produce one row and one unique violation, and
//! the violation is [`StoreError::VersionConflict`] rather than a database
//! failure. A `SELECT … FOR UPDATE` on the run row would work and would
//! serialise every command on an execution for the duration of a decision that
//! never blocks.
//!
//! **The inbox is checked inside the transaction.** Outside it, a redelivery
//! arriving beside its first delivery would decide twice.
//!
//! **A claim is `FOR UPDATE SKIP LOCKED`.** The row a claimant sees is one no
//! other transaction is holding, so two workers polling together take two
//! different attempts rather than one of them waiting for the other's lock.

pub mod error;
pub mod schema;

use async_trait::async_trait;
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row as _};
use time::OffsetDateTime;

use crate::claim::{AttemptKey, AttemptRow, ClaimFilter};
use crate::message::{
    Direction, MessageMetadata, OutboxMessage, RecordedMessage, RunProjection, WorkflowMessage,
};
use crate::state::ExecutionId;
use crate::store::{
    AppendOutcome, AppendRequest, ExpectedVersion, StoreCapabilities, StreamSlice, WorkflowStore,
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
            let kind = if message.message.is_event() {
                "event"
            } else {
                "command"
            };
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

        for row in request.attempts {
            upsert_attempt(&mut transaction, &row).await?;
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
                    created_at, started_at, ended_at
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

    async fn mark_published(&self, ids: &[MessageId], at: OffsetDateTime) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let ids: Vec<&str> = ids.iter().map(MessageId::as_str).collect();
        sqlx::query("update outbox_messages set published_at = $1 where message_id = any($2)")
            .bind(at)
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
        if filter.runtimes.is_empty() && filter.queues.is_empty() {
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
        let stale = now - time::Duration::seconds(aiwatcher_jobs::LEASE_SECONDS);

        // `SKIP LOCKED` is what makes two claimants polling together take two
        // attempts rather than one waiting on the other's lock. `queue is null`
        // is the reactor's half and `queue = any($1)` the worker's, and they do
        // not overlap: a pulled attempt belongs to whoever holds its queue.
        let row = sqlx::query(
            "select execution_id, step_id, attempt, runtime, command_id, queue,
                    task_ref, state, lease_owner, previous_owner, claimed_at, not_before
               from step_attempts
              where state not in ('completed', 'failed', 'crashed', 'cancelled',
                                  'awaiting_input')
                and (claimed_at is null or claimed_at <= $3)
                and (not_before is null or not_before <= $4)
                and ( (queue is not null and queue = any($1))
                   or (queue is null and runtime = any($2)) )
              order by updated_at
              for update skip locked
              limit 1",
        )
        .bind(&queues)
        .bind(&runtimes)
        .bind(stale)
        .bind(now)
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
            created_at, started_at, ended_at)
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
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
           started_at = excluded.started_at,
           ended_at = excluded.ended_at",
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
    .bind(projection.started_at)
    .bind(projection.ended_at)
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
        started_at: row.get("started_at"),
        ended_at: row.get("ended_at"),
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
