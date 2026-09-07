//! One append-only file per execution, plus a lock nobody else may take.
//!
//! The same shape as `aiwatcher-bus`'s write-ahead log, and for the same
//! reason: aiwatcher has to be useful before anybody commits to PostgreSQL.
//! One JSON record per line, stream version = line number — no index to keep in
//! sync, and a half-written tail truncates cleanly because the broken line
//! fails to parse and everything before it is still valid.
//!
//! ## The lock is the point
//!
//! This adapter holds **one process**. A workflow stream has a decider, a
//! reactor and possibly a worker racing to append, and a file offers no
//! compare-and-append across processes — so a second process is refused at
//! `open`, by name, rather than allowed to interleave writes that would each
//! look fine on their own.
//!
//! And [`StoreCapabilities::multi_process`] is `false`, which is what makes a
//! plan needing a worker or a container job refused *before* it starts, naming
//! `AIWATCHER_WORKFLOW_STORE`. A development store must not become a production
//! one by omission.
//!
//! The atomicity is weaker than a transaction and is honest about where: the
//! stream lines for one decision are written with one `write_all` and one
//! `sync_all`, and the projection, outbox and checkpoint files are written
//! after. A crash in between leaves the stream ahead of the projection, which
//! the projection is rebuilt from at open — the same direction as
//! [`aiwatcher_jobs::ORDERING`], where a crash re-does work rather than losing
//! it.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use time::OffsetDateTime;
use tokio::fs::{self, OpenOptions};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use aiwatcher_core::{Checkpoint, MessageId};

use crate::claim::{AttemptKey, AttemptRow, ClaimFilter};
use crate::error::{Result, StoreError};
use crate::message::{OutboxMessage, RecordedMessage, RunProjection};
use crate::state::ExecutionId;

use super::{
    AppendOutcome, AppendRequest, ExpectedVersion, Pruned, StoreCapabilities, StreamSlice,
    WorkflowStore, prunable,
};

const LOCK_FILE: &str = "workflow.lock";
const STREAMS_DIR: &str = "streams";
const PROJECTIONS_DIR: &str = "projections";
const OUTBOX_FILE: &str = "outbox.jsonl";
const CHECKPOINTS_DIR: &str = "checkpoints";
const ATTEMPTS_FILE: &str = "attempts.json";

/// A single-process workflow store under a directory.
#[derive(Debug)]
pub struct FileWorkflowStore {
    root: PathBuf,
    /// Serialises this process's own appends. The lock file keeps other
    /// processes out; this keeps two tasks from interleaving one decision.
    gate: Arc<Mutex<()>>,
    _lock: LockGuard,
}

/// Removes the lock file when the store is dropped.
#[derive(Debug)]
struct LockGuard(PathBuf);

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

impl FileWorkflowStore {
    /// Open (or create) a store at `root`, taking the single-process lock.
    ///
    /// # Errors
    ///
    /// [`StoreError::SingleProcessOnly`] when another process holds it, naming
    /// the variable that selects a store which does not have this limit.
    pub async fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(root.join(STREAMS_DIR)).await?;
        fs::create_dir_all(root.join(PROJECTIONS_DIR)).await?;
        fs::create_dir_all(root.join(CHECKPOINTS_DIR)).await?;

        let lock_path = root.join(LOCK_FILE);
        // `create_new` is the whole mechanism: the filesystem decides who wins,
        // and the loser is told which store to use instead.
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
            .await
        {
            Ok(mut file) => {
                file.write_all(std::process::id().to_string().as_bytes())
                    .await?;
                file.sync_all().await?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(StoreError::SingleProcessOnly);
            }
            Err(error) => return Err(error.into()),
        }

        Ok(Self {
            root,
            gate: Arc::new(Mutex::new(())),
            _lock: LockGuard(lock_path),
        })
    }

    fn stream_path(&self, execution: &ExecutionId) -> PathBuf {
        self.root
            .join(STREAMS_DIR)
            .join(format!("{}.jsonl", sanitise(execution.as_str())))
    }

    fn projection_path(&self, execution: &ExecutionId) -> PathBuf {
        self.root
            .join(PROJECTIONS_DIR)
            .join(format!("{}.json", sanitise(execution.as_str())))
    }

    fn checkpoint_path(&self, processor: &str) -> PathBuf {
        self.root
            .join(CHECKPOINTS_DIR)
            .join(format!("{}.json", sanitise(processor)))
    }

    async fn read_stream(&self, execution: &ExecutionId) -> Result<Vec<RecordedMessage>> {
        let path = self.stream_path(execution);
        let Ok(body) = fs::read_to_string(&path).await else {
            return Ok(Vec::new());
        };
        let mut messages = Vec::new();
        for line in body.lines() {
            if line.trim().is_empty() {
                continue;
            }
            // A half-written final line is a crash mid-append, and everything
            // before it is still a valid stream.
            match serde_json::from_str::<RecordedMessage>(line) {
                Ok(message) => messages.push(message),
                Err(_) => break,
            }
        }
        Ok(messages)
    }

    /// Every attempt row, keyed. One file rather than one per row: a claim
    /// table is small by construction — one live row per running step — and a
    /// single-process store has no reader racing the writer.
    async fn read_attempts(&self) -> Result<BTreeMap<AttemptKey, AttemptRow>> {
        let Ok(body) = fs::read(self.root.join(ATTEMPTS_FILE)).await else {
            return Ok(BTreeMap::new());
        };
        Ok(serde_json::from_slice::<Vec<AttemptRow>>(&body)
            .unwrap_or_default()
            .into_iter()
            .map(|row| (row.key.clone(), row))
            .collect())
    }

    async fn write_attempts(&self, rows: &BTreeMap<AttemptKey, AttemptRow>) -> Result<()> {
        let rows: Vec<&AttemptRow> = rows.values().collect();
        write_atomically(&self.root.join(ATTEMPTS_FILE), &serde_json::to_vec(&rows)?).await
    }

    /// When this store last wrote anything about an execution.
    ///
    /// The stream file's own modification time, because this adapter holds one
    /// process and appends to that file in place — so the filesystem's answer
    /// *is* the last append, without reading a line of it. The epoch when the
    /// file is not there, which is what finishes a prune that was interrupted
    /// between removing the stream and removing the projection.
    async fn last_activity(&self, execution: &ExecutionId) -> OffsetDateTime {
        fs::metadata(self.stream_path(execution))
            .await
            .and_then(|meta| meta.modified())
            .map_or(OffsetDateTime::UNIX_EPOCH, OffsetDateTime::from)
    }

    async fn read_outbox(&self) -> Result<Vec<OutboxMessage>> {
        let Ok(body) = fs::read_to_string(self.root.join(OUTBOX_FILE)).await else {
            return Ok(Vec::new());
        };
        let mut rows = Vec::new();
        for line in body.lines() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<OutboxMessage>(line) {
                Ok(row) => rows.push(row),
                Err(_) => break,
            }
        }
        Ok(rows)
    }

    async fn write_outbox(&self, rows: &[OutboxMessage]) -> Result<()> {
        let mut body = String::new();
        for row in rows {
            body.push_str(&serde_json::to_string(row)?);
            body.push('\n');
        }
        write_atomically(&self.root.join(OUTBOX_FILE), body.as_bytes()).await
    }
}

#[async_trait]
impl WorkflowStore for FileWorkflowStore {
    fn capabilities(&self) -> StoreCapabilities {
        // The whole reason this adapter is not the default anywhere a worker
        // runs. `just dev` gets a real durable store; a deployment is told to
        // pick one that more than one process can hold.
        StoreCapabilities {
            multi_process: false,
            claimable: false,
        }
    }

    async fn load(&self, execution: &ExecutionId) -> Result<StreamSlice> {
        let messages = self.read_stream(execution).await?;
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
        let _gate = self.gate.lock().await;

        let existing = self.read_stream(execution).await?;
        let version = existing.len() as u64;

        if let Some(seen) = existing
            .iter()
            .find(|message| message.metadata.message_id == request.input.metadata.message_id)
        {
            return Ok(AppendOutcome::Duplicate {
                version: seen.stream_version,
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
        let mut body = String::new();
        let mut next = version;
        for message in std::iter::once(request.input).chain(request.outputs) {
            next += 1;
            body.push_str(&serde_json::to_string(&RecordedMessage {
                stream_version: next,
                direction: message.direction,
                message: message.message,
                metadata: message.metadata,
                recorded_at: now,
            })?);
            body.push('\n');
        }

        // One write, one sync: the stream is the truth, and it goes down before
        // anything derived from it. A crash after this and before the
        // projection leaves the projection stale, which `projection` rebuilds
        // — the direction that re-does work rather than losing it.
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.stream_path(execution))
            .await?;
        file.write_all(body.as_bytes()).await?;
        file.sync_all().await?;

        write_atomically(
            &self.projection_path(execution),
            &serde_json::to_vec(&request.projection)?,
        )
        .await?;

        if !request.outbox.is_empty() {
            let mut rows = self.read_outbox().await?;
            rows.extend(request.outbox);
            self.write_outbox(&rows).await?;
        }

        if !request.attempts.is_empty() {
            let mut rows = self.read_attempts().await?;
            for row in request.attempts {
                rows.insert(row.key.clone(), row);
            }
            self.write_attempts(&rows).await?;
        }

        if let Some((processor, checkpoint)) = request.checkpoint {
            write_atomically(
                &self.checkpoint_path(&processor),
                &serde_json::to_vec(&checkpoint)?,
            )
            .await?;
        }

        Ok(AppendOutcome::Appended { version: next })
    }

    async fn projection(&self, execution: &ExecutionId) -> Result<Option<RunProjection>> {
        let Ok(body) = fs::read(self.projection_path(execution)).await else {
            return Ok(None);
        };
        Ok(serde_json::from_slice(&body).ok())
    }

    async fn pending_outbox(&self, limit: usize) -> Result<Vec<OutboxMessage>> {
        Ok(self
            .read_outbox()
            .await?
            .into_iter()
            .filter(|row| row.published_at.is_none())
            .take(limit)
            .collect())
    }

    async fn mark_published(&self, ids: &[MessageId], _at: OffsetDateTime) -> Result<()> {
        let _gate = self.gate.lock().await;
        let mut rows = self.read_outbox().await?;
        // Dropped rather than flagged, which for this adapter is also what
        // keeps the file from growing without bound: it is rewritten whole on
        // every publish, so a kept row is paid for on every pass afterwards.
        rows.retain(|row| !ids.contains(&row.message_id));
        self.write_outbox(&rows).await
    }

    async fn checkpoint(&self, processor: &str) -> Result<Option<Checkpoint>> {
        let Ok(body) = fs::read(self.checkpoint_path(processor)).await else {
            return Ok(None);
        };
        Ok(serde_json::from_slice(&body).ok())
    }

    async fn claim_attempt(
        &self,
        filter: &ClaimFilter,
        owner: &str,
        now: OffsetDateTime,
    ) -> Result<Option<AttemptRow>> {
        // The same gate as an append: this store holds one process, so the
        // mutex is the whole exclusion, and `capabilities().claimable` is
        // `false` precisely because that guarantee stops at the process edge.
        let _gate = self.gate.lock().await;
        let mut rows = self.read_attempts().await?;
        let Some(key) = rows
            .values()
            .find(|row| row.is_claimable(now) && filter.matches(row))
            .map(|row| row.key.clone())
        else {
            return Ok(None);
        };
        let Some(row) = rows.get_mut(&key) else {
            return Ok(None);
        };
        row.claim(owner, now);
        let claimed = row.clone();
        self.write_attempts(&rows).await?;
        Ok(Some(claimed))
    }

    async fn heartbeat(&self, key: &AttemptKey, owner: &str, now: OffsetDateTime) -> Result<bool> {
        let _gate = self.gate.lock().await;
        let mut rows = self.read_attempts().await?;
        let Some(row) = rows.get_mut(key) else {
            return Ok(false);
        };
        if !row.is_held_by(owner, now) {
            return Ok(false);
        }
        row.claimed_at = Some(now);
        self.write_attempts(&rows).await?;
        Ok(true)
    }

    async fn attempt(&self, key: &AttemptKey) -> Result<Option<AttemptRow>> {
        Ok(self.read_attempts().await?.remove(key))
    }

    async fn advance_checkpoint(&self, processor: &str, checkpoint: Checkpoint) -> Result<()> {
        write_atomically(
            &self.checkpoint_path(processor),
            &serde_json::to_vec(&checkpoint)?,
        )
        .await
    }

    /// A sweep reads every projection, which is the cost this adapter accepts.
    ///
    /// There is no index here to ask instead, and building one would be a
    /// second file to keep in sync with the directory that is already the
    /// truth. A store this holds is a development store — one process,
    /// `just dev` — and the pass it pays for is what keeps the *claim* table
    /// small, which is the cost that was actually growing.
    async fn prune(&self, before: OffsetDateTime, limit: usize) -> Result<Pruned> {
        let _guard = self.gate.lock().await;

        let unpublished: HashSet<String> = self
            .read_outbox()
            .await?
            .into_iter()
            .filter(|row| row.published_at.is_none())
            .map(|row| row.partition_key)
            .collect();

        let mut doomed: Vec<RunProjection> = Vec::new();
        let mut entries = fs::read_dir(self.root.join(PROJECTIONS_DIR)).await?;
        while let Some(entry) = entries.next_entry().await? {
            if doomed.len() == limit {
                break;
            }
            let Ok(body) = fs::read(entry.path()).await else {
                continue;
            };
            let Ok(run) = serde_json::from_slice::<RunProjection>(&body) else {
                continue;
            };
            if unpublished.contains(&format!("workflow:{}", run.execution_id)) {
                continue;
            }
            if prunable(&run, self.last_activity(&run.execution_id).await, before) {
                doomed.push(run);
            }
        }

        let mut pruned = Pruned::default();
        for run in &doomed {
            // The stream before the projection: a crash between them leaves a
            // projection with no stream, which the next pass reads as activity
            // at the epoch and finishes. The other order leaves a stream
            // nothing points at and nothing ever looks for.
            let _ = fs::remove_file(self.stream_path(&run.execution_id)).await;
            let _ = fs::remove_file(self.projection_path(&run.execution_id)).await;
            pruned.executions += 1;
        }

        if !doomed.is_empty() {
            let gone: HashSet<&ExecutionId> = doomed.iter().map(|run| &run.execution_id).collect();
            let mut attempts = self.read_attempts().await?;
            let before_count = attempts.len();
            attempts.retain(|key, _| !gone.contains(&key.execution_id));
            pruned.attempts = before_count - attempts.len();
            if pruned.attempts > 0 {
                self.write_attempts(&attempts).await?;
            }
        }
        Ok(pruned)
    }
}

/// Write, sync, then rename. A reader never sees half a projection.
async fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = path.with_extension("tmp");
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&temporary)
        .await?;
    file.write_all(bytes).await?;
    file.sync_all().await?;
    drop(file);
    fs::rename(&temporary, path).await?;
    Ok(())
}

/// An execution id is a caller's string, and it becomes a file name.
///
/// Everything outside `[A-Za-z0-9._-]` becomes `_`, and the original is hashed
/// into a suffix so two ids that sanitise alike stay two files. A traversal
/// aimed at a store's directory is the failure this closes; two executions
/// silently sharing a stream is the one the suffix closes.
fn sanitise(value: &str) -> String {
    let safe: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .take(80)
        .collect();
    format!("{safe}-{}", &crate::digest(value.as_bytes())[..16])
}

/// Whether a directory currently has a live lock. For a caller reporting why
/// a start-up refused, not for deciding anything.
#[must_use]
pub fn is_locked(root: impl AsRef<Path>) -> bool {
    root.as_ref().join(LOCK_FILE).exists()
}
