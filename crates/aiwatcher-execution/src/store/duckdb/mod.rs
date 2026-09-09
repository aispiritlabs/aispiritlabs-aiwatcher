//! The workflow store on DuckDB: what `postgres` is for a deployment, for one
//! machine.
//!
//! It replaces two properties of `file`. That adapter rewrites whole files —
//! the outbox on every publish, the claim table on every claim — which is
//! quadratic where retention is opt-in; and a directory of JSON answers no
//! questions, while `aiwatcher sql` opens this one with no server running.
//!
//! **The rules are not here.** `prunable`, `AttemptRow::is_claimable`,
//! `ClaimFilter::matches` and `SlotRecord::is_available` decide in Rust exactly
//! as they do for the other three adapters; this file loads rows and asks them.
//! A second answer in SQL is what `store::prunable` exists to prevent. The cost
//! is that `claim_attempt` and `admit_slot` read the live rows rather than
//! pushing a predicate down — bounded by concurrency and the tick rather than
//! by retention. A deployment with a real backlog runs `postgres` and its
//! `SKIP LOCKED`.
//!
//! One connection behind a mutex, every call through
//! [`DuckdbWorkflowStore::with`] on `spawn_blocking` — DuckDB's API is
//! synchronous and would otherwise stall the runtime. The mutex is the
//! transaction. DuckDB takes an exclusive file lock, so
//! [`StoreCapabilities::multi_process`] is `false`.

mod schema;

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use duckdb::Connection;
use time::OffsetDateTime;

use aiwatcher_core::{Checkpoint, MessageId};

use crate::claim::{AttemptKey, AttemptRow, AttemptWrite, ClaimFilter};
use crate::error::{Result, StoreError};
use crate::hosted::{DeciderLease, LeaseOutcome};
use crate::message::{OutboxMessage, RecordedMessage, RunProjection};
use crate::plan::DefinitionKind;
use crate::schedule::slot::{
    SlotAdmission, SlotAdmissionRequest, SlotKey, SlotRecord, SlotSettlement,
};
use crate::state::ExecutionId;

use super::{
    AppendOutcome, AppendRequest, ExpectedVersion, Pruned, StoreCapabilities, StreamSlice,
    WorkflowStore, prunable,
};

/// A workflow store in one DuckDB database.
#[derive(Clone, Debug)]
pub struct DuckdbWorkflowStore {
    inner: Arc<Mutex<Connection>>,
}

impl DuckdbWorkflowStore {
    /// Open, or create, the database at `path` and bring its schema up to date.
    ///
    /// # Errors
    ///
    /// [`StoreError::Backend`] when the file cannot be opened — which on this
    /// adapter most often means another process is holding it, and the message
    /// says so, because "one process" is this store's contract rather than a
    /// transient condition to retry.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                StoreError::Backend(format!("creating {}: {error}", parent.display()))
            })?;
        }
        let connection = Connection::open(path).map_err(|error| {
            StoreError::Backend(format!(
                "opening {}: {error}. DuckDB holds one process at a time, which is what \
                 AIWATCHER_WORKFLOW_STORE=duckdb means; a deployment that needs two runs postgres",
                path.display()
            ))
        })?;
        schema::apply(&connection)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(connection)),
        })
    }

    /// A database that lives only as long as this value. For the suite.
    ///
    /// # Errors
    ///
    /// [`StoreError::Backend`] when DuckDB will not start at all.
    pub fn in_memory() -> Result<Self> {
        let connection = Connection::open_in_memory().map_err(|error| {
            StoreError::Backend(format!("opening an in-memory database: {error}"))
        })?;
        schema::apply(&connection)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(connection)),
        })
    }

    /// Run one unit of work against the connection, off the runtime's threads.
    async fn with<T, F>(&self, work: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || {
            // A poisoned mutex means a previous call panicked while holding the
            // connection. Reported rather than propagated as a second panic:
            // the caller can still be told which store failed.
            let guard = inner
                .lock()
                .map_err(|_| StoreError::Backend("the database lock is poisoned".to_owned()))?;
            work(&guard)
        })
        .await
        .map_err(|error| StoreError::Backend(format!("the database task failed: {error}")))?
    }
}

/// Encode a value the way every payload column here is encoded.
fn encode<T: serde::Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value)
        .map_err(|error| StoreError::Backend(format!("encoding a row: {error}")))
}

/// Decode one back.
fn decode<T: serde::de::DeserializeOwned>(raw: &str) -> Result<T> {
    serde_json::from_str(raw)
        .map_err(|error| StoreError::Backend(format!("decoding a row: {error}")))
}

/// Every row of a one-column `varchar` query, decoded.
fn rows<T: serde::de::DeserializeOwned>(
    connection: &Connection,
    sql: &str,
    parameters: impl duckdb::Params,
) -> Result<Vec<T>> {
    let mut statement = connection
        .prepare(sql)
        .map_err(|error| StoreError::Backend(format!("preparing a query: {error}")))?;
    let found = statement
        .query_map(parameters, |row| row.get::<_, String>(0))
        .map_err(|error| StoreError::Backend(format!("running a query: {error}")))?;
    let mut decoded = Vec::new();
    for payload in found {
        let payload =
            payload.map_err(|error| StoreError::Backend(format!("reading a row: {error}")))?;
        decoded.push(decode(&payload)?);
    }
    Ok(decoded)
}

/// One row of a one-column query, if there is one.
fn one<T: serde::de::DeserializeOwned>(
    connection: &Connection,
    sql: &str,
    parameters: impl duckdb::Params,
) -> Result<Option<T>> {
    Ok(rows(connection, sql, parameters)?.into_iter().next())
}

/// A timestamp as this schema stores it.
///
/// Microseconds since the epoch, as a `BIGINT`, rather than DuckDB's own
/// `TIMESTAMPTZ`. The reason is comparison: every ordering and every expiry
/// check here is against a value that came from `OffsetDateTime`, and a round
/// trip through a database's own temporal type is a second representation with
/// its own precision — this way what is compared is exactly what was written.
fn stamp(at: OffsetDateTime) -> i64 {
    i64::try_from(at.unix_timestamp_nanos() / 1_000).unwrap_or(i64::MAX)
}

#[async_trait]
impl WorkflowStore for DuckdbWorkflowStore {
    fn capabilities(&self) -> StoreCapabilities {
        StoreCapabilities {
            // DuckDB takes an exclusive lock on the database file, so the
            // second process does not interleave — it fails to open. Said here
            // so a plan that needs a worker is refused by name at admission
            // rather than discovered three attempts later.
            multi_process: false,
            claimable: true,
        }
    }

    async fn load(&self, execution: &ExecutionId) -> Result<StreamSlice> {
        let key = execution.to_string();
        self.with(move |db| {
            let messages: Vec<RecordedMessage> = rows(
                db,
                "select payload from streams where execution = ? order by version",
                [key],
            )?;
            Ok(StreamSlice {
                version: messages.len() as u64,
                messages,
            })
        })
        .await
    }

    async fn load_page(
        &self,
        execution: &ExecutionId,
        after: u64,
        limit: usize,
    ) -> Result<StreamSlice> {
        let key = execution.to_string();
        self.with(move |db| {
            // The stream's *current* version rather than the page's end, so a
            // reader can tell a short page from the last one without a second
            // call.
            let version: i64 = db
                .query_row(
                    "select count(*) from streams where execution = ?",
                    [key.clone()],
                    |row| row.get(0),
                )
                .map_err(|error| StoreError::Backend(format!("counting a stream: {error}")))?;
            let messages: Vec<RecordedMessage> = rows(
                db,
                "select payload from streams where execution = ? and version > ? \
                 order by version limit ?",
                duckdb::params![key, after as i64, limit as i64],
            )?;
            Ok(StreamSlice {
                version: version as u64,
                messages,
            })
        })
        .await
    }

    async fn take_decider_lease(
        &self,
        execution: &ExecutionId,
        holder: &str,
        now: OffsetDateTime,
    ) -> Result<LeaseOutcome> {
        let key = execution.to_string();
        let execution = execution.clone();
        let holder = holder.to_owned();
        self.with(move |db| {
            let held: Option<DeciderLease> = one(
                db,
                "select payload from decider_leases where execution = ?",
                [key.clone()],
            )?;
            let lease = match held {
                Some(lease) if !(lease.held_by(&holder, now) || lease.expired(now)) => {
                    return Ok(LeaseOutcome::Held {
                        holder: lease.holder.clone(),
                        expires_at: lease.expires_at(),
                    });
                }
                Some(mut lease) => {
                    lease.take(&holder, now);
                    lease
                }
                None => DeciderLease::taken_by(&execution, &holder, now),
            };
            db.execute(
                "insert or replace into decider_leases (execution, payload) values (?, ?)",
                duckdb::params![key, encode(&lease)?],
            )
            .map_err(|error| StoreError::Backend(format!("writing a lease: {error}")))?;
            Ok(LeaseOutcome::Taken(lease))
        })
        .await
    }

    async fn release_decider_lease(
        &self,
        execution: &ExecutionId,
        holder: &str,
        now: OffsetDateTime,
    ) -> Result<bool> {
        let key = execution.to_string();
        let holder = holder.to_owned();
        self.with(move |db| {
            let Some(lease): Option<DeciderLease> = one(
                db,
                "select payload from decider_leases where execution = ?",
                [key.clone()],
            )?
            else {
                return Ok(false);
            };
            if !lease.held_by(&holder, now) {
                return Ok(false);
            }
            db.execute("delete from decider_leases where execution = ?", [key])
                .map_err(|error| StoreError::Backend(format!("releasing a lease: {error}")))?;
            Ok(true)
        })
        .await
    }

    async fn decider_lease(&self, execution: &ExecutionId) -> Result<Option<DeciderLease>> {
        let key = execution.to_string();
        self.with(move |db| {
            one(
                db,
                "select payload from decider_leases where execution = ?",
                [key],
            )
        })
        .await
    }

    async fn append(
        &self,
        execution: &ExecutionId,
        request: AppendRequest,
    ) -> Result<AppendOutcome> {
        request.check_payloads()?;
        let key = execution.to_string();
        self.with(move |db| {
            let version: i64 = db
                .query_row(
                    "select count(*) from streams where execution = ?",
                    [key.clone()],
                    |row| row.get(0),
                )
                .map_err(|error| StoreError::Backend(format!("counting a stream: {error}")))?;
            let version = version as u64;

            // The inbox first: a redelivery is not a conflict, and answering it
            // with one would make every at-least-once retry look like a race.
            let message_id = request.input.metadata.message_id.to_string();
            let seen: Option<i64> = db
                .query_row(
                    "select version from inbox where execution = ? and message_id = ?",
                    duckdb::params![key.clone(), message_id.clone()],
                    |row| row.get(0),
                )
                .ok();
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

            // One transaction around the six writes. The mutex already
            // serialises writers, so what this adds is the other half: a crash
            // in the middle leaves none of them rather than some.
            db.execute_batch("begin transaction")
                .map_err(|error| StoreError::Backend(format!("starting a transaction: {error}")))?;
            let outcome = append_all(db, &key, &message_id, version, request);
            match &outcome {
                Ok(_) => db.execute_batch("commit"),
                Err(_) => db.execute_batch("rollback"),
            }
            .map_err(|error| StoreError::Backend(format!("closing a transaction: {error}")))?;
            outcome
        })
        .await
    }

    async fn projection(&self, execution: &ExecutionId) -> Result<Option<RunProjection>> {
        let key = execution.to_string();
        self.with(move |db| {
            one(
                db,
                "select payload from projections where execution = ?",
                [key],
            )
        })
        .await
    }

    async fn pending_outbox(&self, limit: usize) -> Result<Vec<OutboxMessage>> {
        self.with(move |db| {
            rows(
                db,
                "select payload from outbox order by sequence limit ?",
                [limit as i64],
            )
        })
        .await
    }

    async fn mark_published(&self, ids: &[MessageId], _at: OffsetDateTime) -> Result<()> {
        let ids: Vec<String> = ids.iter().map(ToString::to_string).collect();
        self.with(move |db| {
            // Deleted rather than flagged: the fact is on the event log, and a
            // second copy here would grow with every step of every run.
            let mut statement = db
                .prepare("delete from outbox where message_id = ?")
                .map_err(|error| StoreError::Backend(format!("preparing a delete: {error}")))?;
            for id in ids {
                statement
                    .execute([id])
                    .map_err(|error| StoreError::Backend(format!("publishing a row: {error}")))?;
            }
            Ok(())
        })
        .await
    }

    async fn admit_slot(&self, request: &SlotAdmissionRequest) -> Result<SlotAdmission> {
        let request = request.clone();
        self.with(move |db| {
            let key = encode(&request.key)?;
            let held: Option<SlotRecord> =
                one(db, "select payload from slots where key = ?", [key.clone()])?;

            if let Some(held) = &held {
                if let Some(outcome) = &held.outcome {
                    return Ok(SlotAdmission::Settled { outcome: *outcome });
                }
                if !held.is_available(request.now) {
                    return Ok(SlotAdmission::Held {
                        owner: held.lease_owner.clone().unwrap_or_default(),
                    });
                }
            }

            // The overlap check reads this store's own projection, in the same
            // critical section that takes the slot — never the read model,
            // which is an asynchronous fold and is empty in the `work` role
            // where the tick runs.
            if request.overlap == crate::OverlapPolicy::Skip {
                let running: Vec<RunProjection> = rows(
                    db,
                    "select payload from projections where definition_name = ? and not terminal \
                     order by execution",
                    [request.key.definition_name.clone()],
                )?;
                if let Some(running) = running
                    .iter()
                    .map(|run| run.execution_id.to_string())
                    .min()
                {
                    return Ok(SlotAdmission::Blocked {
                        execution_id: running,
                    });
                }
            }

            let record = SlotRecord {
                key: request.key.clone(),
                outcome: None,
                execution_id: None,
                lease_owner: Some(request.owner.clone()),
                leased_at: Some(request.now),
                detail: held.and_then(|record| record.detail),
                updated_at: request.now,
            };
            db.execute(
                "insert or replace into slots (key, definition_kind, definition_name, slot, payload) \
                 values (?, ?, ?, ?, ?)",
                duckdb::params![
                    key,
                    request.key.definition_kind.as_str(),
                    request.key.definition_name.clone(),
                    stamp(request.key.slot),
                    encode(&record)?,
                ],
            )
            .map_err(|error| StoreError::Backend(format!("taking a slot: {error}")))?;
            Ok(SlotAdmission::Admitted)
        })
        .await
    }

    async fn settle_slot(
        &self,
        key: &SlotKey,
        owner: &str,
        settlement: SlotSettlement,
        now: OffsetDateTime,
    ) -> Result<()> {
        let key = key.clone();
        let owner = owner.to_owned();
        self.with(move |db| {
            let encoded = encode(&key)?;
            let Some(mut record): Option<SlotRecord> = one(
                db,
                "select payload from slots where key = ?",
                [encoded.clone()],
            )?
            else {
                return Ok(());
            };
            // A caller whose lease was taken over says nothing. A slow tick
            // overwriting a fresh decision with a stale one is review R3 in a
            // second place.
            if record.lease_owner.as_deref() != Some(owner.as_str()) {
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
            db.execute(
                "update slots set payload = ? where key = ?",
                duckdb::params![encode(&record)?, encoded],
            )
            .map_err(|error| StoreError::Backend(format!("settling a slot: {error}")))?;
            Ok(())
        })
        .await
    }

    async fn recent_slots(
        &self,
        kind: DefinitionKind,
        name: &str,
        limit: usize,
    ) -> Result<Vec<SlotRecord>> {
        let name = name.to_owned();
        let kind = kind.as_str();
        self.with(move |db| {
            rows(
                db,
                "select payload from slots where definition_kind = ? and definition_name = ? \
                 order by slot desc limit ?",
                duckdb::params![kind, name, limit as i64],
            )
        })
        .await
    }

    async fn checkpoint(&self, processor: &str) -> Result<Option<Checkpoint>> {
        let processor = processor.to_owned();
        self.with(move |db| {
            one(
                db,
                "select payload from checkpoints where processor = ?",
                [processor],
            )
        })
        .await
    }

    async fn claim_attempt(
        &self,
        filter: &ClaimFilter,
        owner: &str,
        now: OffsetDateTime,
    ) -> Result<Option<AttemptRow>> {
        let filter = filter.clone();
        let owner = owner.to_owned();
        self.with(move |db| {
            // `order by updated_at`, which is what the PostgreSQL adapter
            // orders by and what stops a backlog being served newest-first
            // while its head starves. The predicate is `ClaimFilter`'s and
            // `AttemptRow`'s, in Rust, because a second copy of it in SQL is a
            // second answer to which row is claimable.
            let candidates: Vec<AttemptRow> = rows(
                db,
                "select payload from attempts order by updated_at, execution, step, attempt",
                [],
            )?;
            let Some(mut row) = candidates
                .into_iter()
                .find(|row| row.is_claimable(now) && filter.matches(row))
            else {
                return Ok(None);
            };
            row.claim(&owner, now);
            write_attempt(db, &row)?;
            Ok(Some(row))
        })
        .await
    }

    async fn heartbeat(&self, key: &AttemptKey, owner: &str, now: OffsetDateTime) -> Result<bool> {
        let key = key.clone();
        let owner = owner.to_owned();
        self.with(move |db| {
            let Some(mut row) = read_attempt(db, &key)? else {
                return Ok(false);
            };
            if !row.is_held_by(&owner, now) {
                return Ok(false);
            }
            row.claimed_at = Some(now);
            write_attempt(db, &row)?;
            Ok(true)
        })
        .await
    }

    async fn attempt(&self, key: &AttemptKey) -> Result<Option<AttemptRow>> {
        let key = key.clone();
        self.with(move |db| read_attempt(db, &key)).await
    }

    async fn advance_checkpoint(&self, processor: &str, checkpoint: Checkpoint) -> Result<()> {
        let processor = processor.to_owned();
        self.with(move |db| {
            db.execute(
                "insert or replace into checkpoints (processor, payload) values (?, ?)",
                duckdb::params![processor, encode(&checkpoint)?],
            )
            .map_err(|error| StoreError::Backend(format!("advancing a checkpoint: {error}")))?;
            Ok(())
        })
        .await
    }

    async fn prune(&self, before: OffsetDateTime, limit: usize) -> Result<Pruned> {
        self.with(move |db| {
            // Which executions the outbox still speaks for. Read before
            // anything is chosen, because the answer has to be the same for
            // every candidate in one pass — a pending row is a fact that has
            // not reached the log, and deleting the decision behind it leaves
            // the publisher a message with no explanation.
            let speaking: HashSet<String> = db
                .prepare("select distinct partition_key from outbox")
                .and_then(|mut statement| {
                    statement
                        .query_map([], |row| row.get::<_, String>(0))
                        .map(|found| found.filter_map(std::result::Result::ok).collect())
                })
                .map_err(|error| StoreError::Backend(format!("reading the outbox: {error}")))?;

            let candidates: Vec<(String, RunProjection, i64)> = {
                let mut statement = db
                    .prepare(
                        "select p.execution, p.payload, \
                            coalesce((select max(s.recorded_at) from streams s \
                                      where s.execution = p.execution), 0) \
                         from projections p",
                    )
                    .map_err(|error| StoreError::Backend(format!("preparing a scan: {error}")))?;
                let found = statement
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    })
                    .map_err(|error| StoreError::Backend(format!("scanning: {error}")))?;
                let mut decoded = Vec::new();
                for candidate in found {
                    let (execution, payload, activity) = candidate
                        .map_err(|error| StoreError::Backend(format!("reading a row: {error}")))?;
                    decoded.push((execution, decode::<RunProjection>(&payload)?, activity));
                }
                decoded
            };

            let doomed: Vec<String> = candidates
                .into_iter()
                .filter(|(_, run, activity)| prunable(run, from_stamp(*activity), before))
                .filter(|(execution, _, _)| !speaking.contains(&format!("workflow:{execution}")))
                .map(|(execution, _, _)| execution)
                .take(limit)
                .collect();

            let mut pruned = Pruned::default();
            for execution in &doomed {
                // The whole execution together: a kept projection whose stream
                // is gone is a run the panel lists and cannot open.
                for table in ["streams", "inbox", "projections"] {
                    db.execute(
                        &format!("delete from {table} where execution = ?"),
                        [execution.clone()],
                    )
                    .map_err(|error| StoreError::Backend(format!("pruning: {error}")))?;
                }
                let attempts = db
                    .execute(
                        "delete from attempts where execution = ?",
                        [execution.clone()],
                    )
                    .map_err(|error| StoreError::Backend(format!("pruning attempts: {error}")))?;
                pruned.attempts += attempts;
                pruned.executions += 1;
            }
            Ok(pruned)
        })
        .await
    }
}

/// The six writes of one decision, inside the caller's transaction.
fn append_all(
    db: &Connection,
    key: &str,
    message_id: &str,
    version: u64,
    request: AppendRequest,
) -> Result<AppendOutcome> {
    let now = OffsetDateTime::now_utc();
    // The input's own version is what the inbox remembers: it is where the
    // previous result can be looked up, and it stays true as the stream grows
    // past it.
    let input_version = version + 1;
    let mut next = version;
    for message in std::iter::once(request.input.clone()).chain(request.outputs.clone()) {
        next += 1;
        let recorded = RecordedMessage {
            stream_version: next,
            direction: message.direction,
            message: message.message,
            metadata: message.metadata,
            recorded_at: now,
        };
        db.execute(
            "insert into streams (execution, version, direction, recorded_at, payload) \
             values (?, ?, ?, ?, ?)",
            duckdb::params![
                key,
                next as i64,
                format!("{:?}", recorded.direction),
                stamp(now),
                encode(&recorded)?
            ],
        )
        .map_err(|error| StoreError::Backend(format!("appending a message: {error}")))?;
    }

    db.execute(
        "insert into inbox (execution, message_id, version) values (?, ?, ?)",
        duckdb::params![key, message_id, input_version as i64],
    )
    .map_err(|error| StoreError::Backend(format!("recording an input: {error}")))?;

    db.execute(
        "insert or replace into projections (execution, definition_name, terminal, payload) \
         values (?, ?, ?, ?)",
        duckdb::params![
            key,
            request.projection.definition_name.clone(),
            request.projection.state.state_type.is_terminal(),
            encode(&request.projection)?
        ],
    )
    .map_err(|error| StoreError::Backend(format!("writing a projection: {error}")))?;

    for write in request.attempts {
        match write {
            AttemptWrite::Dispatch(row) => write_attempt(db, &row)?,
            // A finished attempt is not a row. See `AttemptWrite`.
            AttemptWrite::Retire(key) => {
                db.execute(
                    "delete from attempts where execution = ? and step = ? and attempt = ?",
                    duckdb::params![
                        key.execution_id.to_string(),
                        key.step_id.clone(),
                        i64::from(key.attempt)
                    ],
                )
                .map_err(|error| StoreError::Backend(format!("retiring an attempt: {error}")))?;
            }
        }
    }

    for row in request.outbox {
        db.execute(
            "insert into outbox (sequence, message_id, partition_key, payload) \
             values (nextval('outbox_sequence'), ?, ?, ?)",
            duckdb::params![
                row.message_id.to_string(),
                row.partition_key.clone(),
                encode(&row)?
            ],
        )
        .map_err(|error| StoreError::Backend(format!("writing an outbox row: {error}")))?;
    }

    if let Some((processor, checkpoint)) = request.checkpoint {
        db.execute(
            "insert or replace into checkpoints (processor, payload) values (?, ?)",
            duckdb::params![processor, encode(&checkpoint)?],
        )
        .map_err(|error| StoreError::Backend(format!("advancing a checkpoint: {error}")))?;
    }

    Ok(AppendOutcome::Appended { version: next })
}

fn read_attempt(db: &Connection, key: &AttemptKey) -> Result<Option<AttemptRow>> {
    one(
        db,
        "select payload from attempts where execution = ? and step = ? and attempt = ?",
        duckdb::params![
            key.execution_id.to_string(),
            key.step_id.clone(),
            i64::from(key.attempt)
        ],
    )
}

/// Write one attempt row, stamping when it was last touched.
///
/// `updated_at` is written here rather than taken from the row because the row
/// does not carry one: it is a property of the *record*, not of the attempt, and
/// it exists for one reason — `claim_attempt` orders by it, so a claim reaches
/// the least recently touched row first. The PostgreSQL adapter's column of the
/// same name does the same job.
fn write_attempt(db: &Connection, row: &AttemptRow) -> Result<()> {
    db.execute(
        "insert or replace into attempts \
         (execution, step, attempt, updated_at, payload) values (?, ?, ?, ?, ?)",
        duckdb::params![
            row.key.execution_id.to_string(),
            row.key.step_id.clone(),
            i64::from(row.key.attempt),
            stamp(OffsetDateTime::now_utc()),
            encode(row)?
        ],
    )
    .map_err(|error| StoreError::Backend(format!("writing an attempt: {error}")))?;
    Ok(())
}

/// The inverse of [`stamp`].
///
/// The epoch for a stream that is not there, which is prunable by every window
/// — a projection with no stream behind it is the half-state a crash between
/// the two writes leaves, and keeping it forever is how it becomes permanent.
fn from_stamp(microseconds: i64) -> OffsetDateTime {
    if microseconds == 0 {
        return OffsetDateTime::UNIX_EPOCH;
    }
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(microseconds) * 1_000)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH)
}
