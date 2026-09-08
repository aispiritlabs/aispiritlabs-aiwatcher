//! One append-only file per execution, plus a lock nobody else may take.
//!
//! The same shape as `aiwatcher-bus`'s write-ahead log, and for the same
//! reason: aiwatcher has to be useful before anybody commits to PostgreSQL.
//! One JSON record per line, stream version = line number — no index to keep in
//! sync, and a half-written tail is cut away before anything is written behind
//! it, because a good record behind a broken one would freeze every later read
//! at the break.
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
//! The lock is the operating system's, held on the open file rather than
//! asserted by the file's existence, so the kernel releases it however the
//! process ends — `SIGKILL` included. See [`LockGuard`].
//!
//! ## One decision is journalled, then applied
//!
//! The atomicity is weaker than a transaction and is honest about where. A
//! decision touches five files — stream, projection, outbox, attempts,
//! checkpoint — and a filesystem writes one at a time. This used to write them
//! in sequence and hope, which was wrong in a way no successful-path test could
//! see: a crash after the stream left the input's message id recorded with none
//! of its consequences on disk, and because that id *is* the inbox key, the
//! retry was answered `Duplicate`. The run then had no outbox row to publish
//! and no attempt row to claim, and nothing anywhere said so (review A1).
//!
//! So [`PendingCommit`] is written whole and `fsync`ed into `commits/` first,
//! and that rename is the commit point. Everything after it is derived, every
//! step of it is idempotent, and the record is deleted only once they are all
//! done. A crash leaves either no record — nothing was accepted — or one that
//! [`FileWorkflowStore::recover`] finishes, at the next `open` and at the top
//! of the next `append`. The second matters as much as the first: A1's
//! reproduction never restarted anything.
//!
//! It is [`aiwatcher_jobs::ORDERING`] again, in one more place: the intent,
//! then the work, then the cursor that says the work is done. A crash re-does
//! rather than loses.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use time::OffsetDateTime;
use tokio::fs::{self, OpenOptions};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use aiwatcher_core::{Checkpoint, MessageId};

use crate::claim::{AttemptKey, AttemptRow, AttemptWrite, ClaimFilter};
use crate::error::{Result, StoreError};
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

const LOCK_FILE: &str = "workflow.lock";
const STREAMS_DIR: &str = "streams";
const PROJECTIONS_DIR: &str = "projections";
const OUTBOX_FILE: &str = "outbox.jsonl";
const CHECKPOINTS_DIR: &str = "checkpoints";
const ATTEMPTS_FILE: &str = "attempts.json";
/// One record per decision that has been accepted and not yet fully applied.
const COMMITS_DIR: &str = "commits";
/// One definition's slots. Keyed like a stream, so a schedule with a name full
/// of separators does not become a path.
const SLOTS_DIR: &str = "slots";

/// A single-process workflow store under a directory.
#[derive(Debug)]
pub struct FileWorkflowStore {
    root: PathBuf,
    /// Serialises this process's own appends. The lock file keeps other
    /// processes out; this keeps two tasks from interleaving one decision.
    gate: Arc<Mutex<()>>,
    _lock: LockGuard,
}

/// Holds the operating system's advisory lock on `workflow.lock`.
///
/// The lock lives on the open file, not on the file's existence, which is the
/// whole reason it is this and not `create_new`: the kernel releases it when
/// the process dies **however** it dies, so a `SIGKILL` leaves a directory the
/// next start can open. The previous shape released it in `Drop`, which is
/// exactly the code a `SIGKILL` does not run — after one, every later start
/// refused a store no process was holding, and the only way out was to delete a
/// file by hand.
///
/// The file is not removed on release, deliberately. Unlinking it while holding
/// the lock lets the next process create a *new* inode and take a lock on that
/// one, after which two processes each hold "the" lock on two different files.
/// A stale path costs nothing; [`is_locked`] asks the kernel rather than the
/// directory listing.
#[derive(Debug)]
struct LockGuard(std::fs::File);

impl Drop for LockGuard {
    fn drop(&mut self) {
        // Closing the file would release it anyway. Saying so is the point:
        // this is the orderly path, and the kernel doing the same thing on a
        // process that never reached here is the reason the lock is held this
        // way at all.
        let _ = self.0.unlock();
    }
}

/// One accepted decision, written whole before any part of it is applied.
///
/// This is what closes A1. The five files a decision touches — stream,
/// projection, outbox, attempts, checkpoint — cannot be written in one
/// operation on a filesystem, and the old code wrote them in sequence and
/// hoped. A crash between the first and the second left the input's message id
/// in the stream with none of its consequences on disk, and because the inbox
/// key *was* the stream, the retry was recognised as a duplicate and returned
/// the first outcome: no outbox row to publish, no attempt row to claim, and
/// nothing anywhere that could tell you the run was stranded.
///
/// So the record below is written, `fsync`ed and renamed into place first, and
/// that rename is the commit point. Everything after it is derived and every
/// step of it is idempotent, so [`FileWorkflowStore::apply`] may be re-run any
/// number of times and the record is deleted only once it has been. A crash
/// anywhere leaves either no record — nothing was accepted — or a record that
/// the next `open` or the next `append` finishes.
///
/// It is the same direction as [`aiwatcher_jobs::ORDERING`] one layer down: the
/// intent, then the work, then the cursor that says the work is done.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct PendingCommit {
    execution: ExecutionId,
    /// The stream lines, already numbered and stamped, so a replay writes the
    /// bytes the first attempt would have written rather than new ones.
    records: Vec<RecordedMessage>,
    projection: RunProjection,
    outbox: Vec<OutboxMessage>,
    attempts: Vec<AttemptWrite>,
    checkpoint: Option<(String, Checkpoint)>,
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

        fs::create_dir_all(root.join(COMMITS_DIR)).await?;
        fs::create_dir_all(root.join(SLOTS_DIR)).await?;

        // The kernel decides who wins, and the loser is told which store to use
        // instead. `try_lock` is `std`'s since 1.89 and this workspace is on
        // 1.98, so it costs no dependency.
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join(LOCK_FILE))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Err(StoreError::SingleProcessOnly),
            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
        }
        // Diagnostics only — nothing decides anything from this. Truncated
        // first, so a previous holder's id is not read as this one's.
        lock.set_len(0)?;
        {
            use std::io::Write as _;
            let mut lock = &lock;
            let _ = lock.write_all(std::process::id().to_string().as_bytes());
            let _ = lock.flush();
        }

        let store = Self {
            root,
            gate: Arc::new(Mutex::new(())),
            _lock: LockGuard(lock),
        };
        // Before anything reads this store: finish whatever the last process
        // was in the middle of, then bring an old claim table up to the shape
        // this build maintains.
        store.recover().await?;
        store.retire_finished_attempts().await?;
        Ok(store)
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
        Ok(self.read_stream_valid(execution).await?.0)
    }

    /// Every complete record, and how many bytes of the file they occupy.
    ///
    /// The second half is what makes a torn tail recoverable rather than
    /// permanent. A crash mid-`write_all` leaves a partial final line; reading
    /// stops there, which is correct, but appending *after* it would put a
    /// valid record behind a broken one and every later read would stop at the
    /// break — the run's history silently frozen at the moment of the crash.
    /// So the caller truncates to this length before it appends.
    async fn read_stream_valid(
        &self,
        execution: &ExecutionId,
    ) -> Result<(Vec<RecordedMessage>, u64)> {
        let path = self.stream_path(execution);
        let Ok(body) = fs::read_to_string(&path).await else {
            return Ok((Vec::new(), 0));
        };
        let mut messages = Vec::new();
        let mut valid = 0_u64;
        for line in body.split_inclusive('\n') {
            if line.trim().is_empty() {
                valid += line.len() as u64;
                continue;
            }
            // A line with no newline after it is a crash mid-append however
            // well it parses, so it is not counted as written.
            if !line.ends_with('\n') {
                break;
            }
            match serde_json::from_str::<RecordedMessage>(line.trim_end()) {
                Ok(message) => {
                    messages.push(message);
                    valid += line.len() as u64;
                }
                Err(_) => break,
            }
        }
        Ok((messages, valid))
    }

    /// Every attempt row, keyed. One file rather than one per row, and that
    /// is affordable because the table really is one row per *unfinished*
    /// step: a settled attempt is retired rather than stored, so this file is
    /// bounded by concurrency and not by history (section 43.34). A
    /// single-process store also has no reader racing the writer.
    ///
    /// It was not always: while a completion wrote a terminal row, every claim
    /// and every heartbeat read, parsed, re-serialised and `fsync`ed the whole
    /// of it — measured at 458 ms per claim over 50 000 rows, growing without
    /// limit because retention is opt-in.
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

    fn commit_path(&self, sequence: u128) -> PathBuf {
        // Zero-padded so the directory sorts in the order the records were
        // written. Normally there is at most one — the gate serialises an
        // append from the journal write to the journal delete — but recovery
        // may not depend on that being true of a directory it did not write.
        self.root
            .join(COMMITS_DIR)
            .join(format!("{sequence:039}.json"))
    }

    /// Apply everything one accepted decision writes. Safe to run again.
    ///
    /// Every step is idempotent, and each one says how:
    ///
    /// - the stream is appended only if its input is not already in it, after
    ///   truncating a torn tail;
    /// - the projection and the checkpoint are whole-file writes, so the last
    ///   one wins and re-running writes the same bytes;
    /// - an outbox row already present by `message_id` is not added twice;
    /// - a dispatch is an insert by key and a retirement is a remove, which are
    ///   both already idempotent (section 43.34).
    async fn apply(&self, commit: &PendingCommit) -> Result<()> {
        let (existing, valid) = self.read_stream_valid(&commit.execution).await?;
        let already = commit.records.first().is_some_and(|first| {
            existing
                .iter()
                .any(|message| message.metadata.message_id == first.metadata.message_id)
        });

        if !already {
            let path = self.stream_path(&commit.execution);
            let mut body = String::new();
            for record in &commit.records {
                body.push_str(&serde_json::to_string(record)?);
                body.push('\n');
            }
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .await?;
            // A partial line from a crashed append is cut away before anything
            // is written behind it. `set_len` on a file opened for append is
            // the whole of it: the next write starts at the new end.
            if file.metadata().await?.len() > valid {
                file.set_len(valid).await?;
            }
            file.write_all(body.as_bytes()).await?;
            file.sync_all().await?;
        }

        write_atomically(
            &self.projection_path(&commit.execution),
            &serde_json::to_vec(&commit.projection)?,
        )
        .await?;

        if !commit.outbox.is_empty() {
            let mut rows = self.read_outbox().await?;
            let held: HashSet<MessageId> = rows.iter().map(|row| row.message_id.clone()).collect();
            rows.extend(
                commit
                    .outbox
                    .iter()
                    .filter(|row| !held.contains(&row.message_id))
                    .cloned(),
            );
            self.write_outbox(&rows).await?;
        }

        if !commit.attempts.is_empty() {
            let mut rows = self.read_attempts().await?;
            for write in &commit.attempts {
                match write {
                    AttemptWrite::Dispatch(row) => {
                        rows.insert(row.key.clone(), row.clone());
                    }
                    // A finished attempt is not a row. See `AttemptWrite`.
                    AttemptWrite::Retire(key) => {
                        rows.remove(key);
                    }
                }
            }
            self.write_attempts(&rows).await?;
        }

        if let Some((processor, checkpoint)) = &commit.checkpoint {
            write_atomically(
                &self.checkpoint_path(processor),
                &serde_json::to_vec(checkpoint)?,
            )
            .await?;
        }
        Ok(())
    }

    /// Finish every decision a previous process accepted and did not apply.
    ///
    /// Run at `open` and again at the top of every `append`. The second is not
    /// belt-and-braces: the review's reproduction never restarted anything — a
    /// write failed mid-decision, the caller retried the identical input in the
    /// same process, and the retry was answered `Duplicate` from a stream that
    /// already held the input. Repairing before the duplicate check is what
    /// turns that answer back into a true one.
    async fn recover(&self) -> Result<()> {
        let directory = self.root.join(COMMITS_DIR);
        let Ok(mut entries) = fs::read_dir(&directory).await else {
            return Ok(());
        };
        let mut pending: Vec<PathBuf> = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                pending.push(path);
            }
        }
        pending.sort();

        for path in pending {
            let body = fs::read(&path).await?;
            match serde_json::from_slice::<PendingCommit>(&body) {
                Ok(commit) => self.apply(&commit).await?,
                // A record that does not parse was never renamed into place
                // whole, so nothing was accepted and there is nothing to
                // finish. Removing it is what stops it being read forever.
                Err(_) => {
                    let _ = fs::remove_file(&path).await;
                    continue;
                }
            }
            fs::remove_file(&path).await?;
        }
        Ok(())
    }

    fn slots_path(&self, kind: DefinitionKind, name: &str) -> PathBuf {
        self.root
            .join(SLOTS_DIR)
            .join(format!("{}-{}.json", kind.as_str(), sanitise(name)))
    }

    async fn read_slots(&self, kind: DefinitionKind, name: &str) -> Result<Vec<SlotRecord>> {
        let Ok(body) = fs::read(self.slots_path(kind, name)).await else {
            return Ok(Vec::new());
        };
        Ok(serde_json::from_slice::<Vec<SlotRecord>>(&body).unwrap_or_default())
    }

    async fn write_slots(
        &self,
        kind: DefinitionKind,
        name: &str,
        rows: &[SlotRecord],
    ) -> Result<()> {
        write_atomically(&self.slots_path(kind, name), &serde_json::to_vec(rows)?).await
    }

    /// Whether any execution of this definition has not finished.
    ///
    /// A scan of the projection directory, which is what this adapter already
    /// does for `prune` and is affordable for the same reason: a store one
    /// process holds is a development store. What matters is that it happens
    /// under the same gate as the slot write, so the check and the claim are
    /// one critical section.
    async fn running_execution_of(&self, definition: &str) -> Result<Option<String>> {
        let mut entries = fs::read_dir(self.root.join(PROJECTIONS_DIR)).await?;
        let mut running: Option<String> = None;
        while let Some(entry) = entries.next_entry().await? {
            let Ok(body) = fs::read(entry.path()).await else {
                continue;
            };
            let Ok(run) = serde_json::from_slice::<RunProjection>(&body) else {
                continue;
            };
            if run.definition_name == definition && !run.state.state_type.is_terminal() {
                let found = run.execution_id.to_string();
                if running.as_ref().is_none_or(|held| &found < held) {
                    running = Some(found);
                }
            }
        }
        Ok(running)
    }

    /// Bring a claim table written by an older build up to this one's shape.
    ///
    /// The file-store half of migration 0004. Until section 43.34 a completion
    /// overwrote its attempt with a terminal row; a finished attempt is now
    /// retired instead, so a store that has been open under an older build
    /// carries rows this one never writes and nothing ever reads. They are
    /// excluded from every claim by `is_claimable`, so this is bounded growth
    /// rather than a correctness bug — but the growth is what made a claim cost
    /// 458 ms over fifty thousand rows.
    ///
    /// `is_terminal` rather than a list written out here, so the states this
    /// drops are the states the rest of the system calls finished.
    /// `awaiting_input` is not one of them and keeps its row: it is not an
    /// ending, and such an attempt resumes on the answer it asked for.
    async fn retire_finished_attempts(&self) -> Result<()> {
        let mut rows = self.read_attempts().await?;
        let before = rows.len();
        rows.retain(|_, row| !row.state.is_terminal());
        if rows.len() != before {
            self.write_attempts(&rows).await?;
        }
        Ok(())
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

        // Before the duplicate check, never after it: a decision the last
        // attempt accepted and did not finish is repaired here, so the inbox
        // key it left in the stream stops being an answer with nothing behind
        // it. See `recover`.
        self.recover().await?;

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
        let mut records = Vec::new();
        let mut next = version;
        for message in std::iter::once(request.input).chain(request.outputs) {
            next += 1;
            records.push(RecordedMessage {
                stream_version: next,
                direction: message.direction,
                message: message.message,
                metadata: message.metadata,
                recorded_at: now,
            });
        }

        let commit = PendingCommit {
            execution: execution.clone(),
            records,
            projection: request.projection,
            outbox: request.outbox,
            attempts: request.attempts,
            checkpoint: request.checkpoint,
        };

        // The commit point. Nothing above this is on disk and nothing below it
        // may be lost: after this rename the decision is accepted, and every
        // file it touches is derived from a record that survives a crash.
        let journal = self.commit_path(now.unix_timestamp_nanos().unsigned_abs());
        write_atomically(&journal, &serde_json::to_vec(&commit)?).await?;

        self.apply(&commit).await?;
        fs::remove_file(&journal).await?;

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

    async fn admit_slot(&self, request: &SlotAdmissionRequest) -> Result<SlotAdmission> {
        // The same gate as an append. This store holds one process, so the
        // mutex is the whole exclusion — which is also why
        // `capabilities().multi_process` is `false` and a deployment that wants
        // two ticks is told to pick another store.
        let _gate = self.gate.lock().await;

        let kind = request.key.definition_kind;
        let mut rows = self.read_slots(kind, &request.key.definition_name).await?;
        if let Some(held) = rows.iter().find(|row| row.key == request.key) {
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
            && let Some(running) = self
                .running_execution_of(&request.key.definition_name)
                .await?
        {
            return Ok(SlotAdmission::Blocked {
                execution_id: running,
            });
        }

        let detail = rows
            .iter()
            .find(|row| row.key == request.key)
            .and_then(|row| row.detail.clone());
        rows.retain(|row| row.key != request.key);
        rows.push(SlotRecord {
            key: request.key.clone(),
            outcome: None,
            execution_id: None,
            lease_owner: Some(request.owner.clone()),
            leased_at: Some(request.now),
            detail,
            updated_at: request.now,
        });
        rows.sort_by_key(|row| row.key.slot);
        self.write_slots(kind, &request.key.definition_name, &rows)
            .await?;
        Ok(SlotAdmission::Admitted)
    }

    async fn settle_slot(
        &self,
        key: &SlotKey,
        owner: &str,
        settlement: SlotSettlement,
        now: OffsetDateTime,
    ) -> Result<()> {
        let _gate = self.gate.lock().await;
        let mut rows = self
            .read_slots(key.definition_kind, &key.definition_name)
            .await?;
        let Some(record) = rows.iter_mut().find(|row| &row.key == key) else {
            return Ok(());
        };
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
        self.write_slots(key.definition_kind, &key.definition_name, &rows)
            .await
    }

    async fn recent_slots(
        &self,
        kind: DefinitionKind,
        name: &str,
        limit: usize,
    ) -> Result<Vec<SlotRecord>> {
        let mut rows = self.read_slots(kind, name).await?;
        rows.sort_by_key(|row| std::cmp::Reverse(row.key.slot));
        rows.truncate(limit);
        Ok(rows)
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
    /// `just dev`, sweeping a directory it can hold.
    ///
    /// It is not what bounds the claim table. That was true while a completion
    /// wrote a terminal row; a finished attempt is now retired rather than
    /// stored, so the table is the size of what is unfinished whether this ever
    /// runs or not (section 43.34). What a sweep still reclaims is streams and
    /// projections, which are the history and are meant to be kept until a
    /// deployment says otherwise.
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
///
/// Asks the kernel rather than the directory listing. The lock file outlives
/// the process that held it — see [`LockGuard`] for why it is not removed — so
/// its existence answers a different question than the one anybody means.
#[must_use]
pub fn is_locked(root: impl AsRef<Path>) -> bool {
    let Ok(file) = std::fs::File::open(root.as_ref().join(LOCK_FILE)) else {
        return false;
    };
    match file.try_lock() {
        Ok(()) => {
            let _ = file.unlock();
            false
        }
        Err(std::fs::TryLockError::WouldBlock) => true,
        // Unknowable rather than false: a lock this cannot test is not a lock
        // this may report as absent.
        Err(std::fs::TryLockError::Error(_)) => true,
    }
}

#[cfg(test)]
mod tests {
    //! What is true of *this* adapter and of no other.
    //!
    //! The contract suite in `tests/store_contract.rs` proves a successful
    //! `append`; these prove the unsuccessful ones. That distinction is what
    //! A1 was: every property held for a decision that completed, and nothing
    //! anywhere held for one that stopped half way.
    //!
    //! The failures are injected through the filesystem rather than through a
    //! hook in the adapter, which is how the review reproduced the original
    //! defect: a directory standing where `write_atomically` wants to put its
    //! temporary file makes that write, and only that write, fail. Nothing in
    //! `file.rs` knows it is being tested.

    use super::*;
    use crate::claim::{AttemptKey, AttemptRow};
    use crate::message::{MessageMetadata, PendingMessage, SCHEMA_VERSION};
    use crate::plan::RuntimeKind;
    use crate::state::{ExecutionMode, ExecutionOwner, RunState, StateType};
    use crate::{WorkflowCommand, WorkflowMessage};
    use aiwatcher_core::{CausationId, CorrelationId};

    /// A directory that removes itself.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "aiwatcher-file-store-{name}-{}-{}",
                std::process::id(),
                OffsetDateTime::now_utc().unix_timestamp_nanos()
            ));
            std::fs::create_dir_all(&path).expect("a scratch directory");
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// One decision that writes into every file this adapter has: a stream
    /// record, a projection, an outbox row, an attempt row and a checkpoint.
    /// Anything less would leave a boundary untested.
    fn decision(execution: &ExecutionId, message_id: &str) -> AppendRequest {
        AppendRequest {
            expected_version: ExpectedVersion::Any,
            input: PendingMessage::input(
                WorkflowMessage::Command(WorkflowCommand::PauseExecution),
                MessageMetadata {
                    schema_version: SCHEMA_VERSION,
                    message_id: MessageId::new(message_id),
                    occurred_at: OffsetDateTime::UNIX_EPOCH,
                    correlation_id: CorrelationId::new(execution.as_str()),
                    causation_id: CausationId::new(message_id),
                    trace_id: None,
                    span_id: None,
                    step_id: None,
                    attempt: None,
                },
            ),
            outputs: Vec::new(),
            projection: RunProjection {
                execution_id: execution.clone(),
                plan_id: "plan-1".to_owned(),
                definition_name: "import".to_owned(),
                owner: ExecutionOwner::Local,
                mode: ExecutionMode::Compiled,
                state: RunState::of(StateType::Running),
                requested_by: "somebody".to_owned(),
                steps: Vec::new(),
                last_message_version: 1,
                created_at: OffsetDateTime::UNIX_EPOCH,
            },
            outbox: vec![OutboxMessage {
                message_id: MessageId::new(format!("{message_id}/out")),
                event_type: "workflow.declared".to_owned(),
                partition_key: format!("workflow:{execution}"),
                payload: serde_json::json!({ "workflow_run_id": execution.as_str() }),
                available_at: OffsetDateTime::UNIX_EPOCH,
                attempts: 0,
                published_at: None,
                last_error: None,
            }],
            checkpoint: Some(("projector".to_owned(), Checkpoint::from_global_position(42))),
            attempts: vec![AttemptWrite::Dispatch(AttemptRow::claimable(
                AttemptKey::new(execution.clone(), "extract", 1),
                RuntimeKind::FlowPhp,
                MessageId::new(format!("{message_id}/cmd")),
            ))],
        }
    }

    /// Everything the decision above should have left behind.
    async fn assert_fully_applied(store: &FileWorkflowStore, execution: &ExecutionId, at: &str) {
        assert_eq!(
            store.load(execution).await.expect("a load").version,
            1,
            "the stream is missing its record after {at}"
        );
        assert!(
            store.projection(execution).await.expect("a read").is_some(),
            "the projection is missing after {at}"
        );
        assert_eq!(
            store.pending_outbox(10).await.expect("an outbox").len(),
            1,
            "the outbox row is missing after {at} — nothing would ever publish this run's facts"
        );
        assert!(
            store
                .attempt(&AttemptKey::new(execution.clone(), "extract", 1))
                .await
                .expect("a read")
                .is_some(),
            "the attempt row is missing after {at} — the run has no work anybody can claim"
        );
        assert_eq!(
            store.checkpoint("projector").await.expect("a read"),
            Some(Checkpoint::from_global_position(42)),
            "the checkpoint is missing after {at}"
        );
    }

    /// Stand a directory where a write wants to put its temporary file. That
    /// write fails; every other one in the same decision still works.
    fn obstruct(path: &Path) {
        std::fs::create_dir_all(path).expect("an obstruction");
    }

    fn unobstruct(path: &Path) {
        std::fs::remove_dir_all(path).expect("the obstruction is removed");
    }

    #[tokio::test]
    async fn a_decision_interrupted_after_any_write_is_finished_by_the_next_one() {
        // A1's exit, one boundary at a time. Each pass fails a different write
        // inside one decision, then retries the identical input — which is what
        // the caller does, and what used to be answered `Duplicate` over a run
        // with no outbox row and no attempt.
        let execution = ExecutionId::new("interrupted");
        for boundary in ["projection", "outbox", "attempts", "checkpoint"] {
            let scratch = Scratch::new(boundary);
            let store = FileWorkflowStore::open(&scratch.0).await.expect("a store");

            let obstruction = match boundary {
                "projection" => store.projection_path(&execution).with_extension("tmp"),
                "outbox" => scratch.0.join(OUTBOX_FILE).with_extension("tmp"),
                "attempts" => scratch.0.join(ATTEMPTS_FILE).with_extension("tmp"),
                _ => store.checkpoint_path("projector").with_extension("tmp"),
            };
            obstruct(&obstruction);

            let failed = store.append(&execution, decision(&execution, "m-1")).await;
            assert!(
                failed.is_err(),
                "the {boundary} write was supposed to fail and did not"
            );

            unobstruct(&obstruction);

            // The retry the caller makes. It carries the same message id, so
            // before the journal it was recognised as a duplicate and repaired
            // nothing.
            store
                .append(&execution, decision(&execution, "m-1"))
                .await
                .expect("the retry");
            assert_fully_applied(&store, &execution, boundary).await;
        }
    }

    #[tokio::test]
    async fn a_decision_interrupted_before_it_was_applied_is_finished_when_the_store_reopens() {
        // The same failure, without the retry: the process died instead. This
        // is the path `open` covers, and it is the one that matters for a run
        // nobody touches again — a scheduled execution whose caller has gone.
        let scratch = Scratch::new("reopen");
        let execution = ExecutionId::new("reopened");
        let obstruction = {
            let store = FileWorkflowStore::open(&scratch.0).await.expect("a store");
            let obstruction = scratch.0.join(OUTBOX_FILE).with_extension("tmp");
            obstruct(&obstruction);
            store
                .append(&execution, decision(&execution, "m-1"))
                .await
                .expect_err("the outbox write fails");
            obstruction
        };
        unobstruct(&obstruction);

        let reopened = FileWorkflowStore::open(&scratch.0)
            .await
            .expect("a restarted process");
        assert_fully_applied(&reopened, &execution, "a restart").await;
    }

    #[tokio::test]
    async fn a_journal_record_that_never_landed_whole_is_discarded_rather_than_read_forever() {
        // `write_atomically` renames, so a half-written record is never at the
        // path recovery reads. A truncated one is still worth an answer: it
        // describes no accepted decision, and leaving it would make every later
        // `open` fail on the same bytes.
        let scratch = Scratch::new("torn-journal");
        {
            let _store = FileWorkflowStore::open(&scratch.0).await.expect("a store");
        }
        let torn = scratch.0.join(COMMITS_DIR).join(format!("{:039}.json", 1));
        std::fs::write(&torn, b"{\"execution\":\"half").expect("a torn record");

        FileWorkflowStore::open(&scratch.0)
            .await
            .expect("a store that steps over it");
        assert!(!torn.exists(), "the unreadable record is removed");
    }

    #[tokio::test]
    async fn a_torn_final_line_is_cut_away_rather_than_written_behind() {
        // A crash inside `write_all` leaves a partial line. Reading stops there
        // — correct — but an append *after* it would put a good record behind a
        // broken one, and every later read would stop at the break. The run's
        // history would be frozen at the moment of the crash with nothing
        // saying so.
        let scratch = Scratch::new("torn-line");
        let execution = ExecutionId::new("torn");
        let store = FileWorkflowStore::open(&scratch.0).await.expect("a store");
        store
            .append(&execution, decision(&execution, "m-1"))
            .await
            .expect("one good record");

        let path = store.stream_path(&execution);
        let mut body = std::fs::read(&path).expect("the stream");
        body.extend_from_slice(b"{\"stream_version\":2,\"direct");
        std::fs::write(&path, &body).expect("a torn tail");

        store
            .append(&execution, decision(&execution, "m-2"))
            .await
            .expect("an append over the torn tail");

        let slice = store.load(&execution).await.expect("a load");
        assert_eq!(slice.version, 2, "both records are readable");
        assert_eq!(
            slice.messages[1].metadata.message_id,
            MessageId::new("m-2"),
            "the second record is the one just written, not the torn one"
        );
    }

    #[tokio::test]
    async fn a_lock_left_by_a_killed_process_does_not_refuse_the_next_start() {
        // What `SIGKILL` leaves: the file is still there, because nothing ran
        // to remove it, and the lock on it is gone, because the kernel released
        // it. The previous shape read the first half and refused every start
        // afterwards until somebody deleted the file by hand.
        let scratch = Scratch::new("killed");
        std::fs::write(scratch.0.join(LOCK_FILE), b"999999").expect("a stale lock file");

        let store = FileWorkflowStore::open(&scratch.0)
            .await
            .expect("a store, because no process holds that lock");
        assert!(is_locked(&scratch.0), "and this one does hold it");
        drop(store);
        assert!(
            !is_locked(&scratch.0),
            "released when the store went away, even though the file is still there"
        );
    }

    #[tokio::test]
    async fn a_second_store_is_still_refused_while_the_first_one_lives() {
        // The guarantee the lock exists for, unchanged by how it is taken.
        let scratch = Scratch::new("held");
        let _held = FileWorkflowStore::open(&scratch.0)
            .await
            .expect("the first");
        let second = FileWorkflowStore::open(&scratch.0)
            .await
            .expect_err("a second");
        assert!(matches!(second, StoreError::SingleProcessOnly), "{second}");
    }

    #[tokio::test]
    async fn an_old_attempts_file_loses_its_finished_rows_and_keeps_the_waiting_one() {
        // The file-store half of migration 0004. `awaiting_input` is not an
        // ending and keeps its row; a sweep that took it would strand every run
        // waiting on a person.
        let scratch = Scratch::new("attempts-upgrade");
        let execution = ExecutionId::new("old");
        let rows: Vec<AttemptRow> = [
            (StateType::Completed, "done"),
            (StateType::Failed, "broke"),
            (StateType::Crashed, "died"),
            (StateType::Cancelled, "stopped"),
            (StateType::AwaitingInput, "asking"),
            (StateType::Running, "going"),
        ]
        .into_iter()
        .map(|(state, step)| {
            let mut row = AttemptRow::claimable(
                AttemptKey::new(execution.clone(), step, 1),
                RuntimeKind::FlowPhp,
                MessageId::new(format!("cmd-{step}")),
            );
            row.state = state;
            row
        })
        .collect();
        std::fs::write(
            scratch.0.join(ATTEMPTS_FILE),
            serde_json::to_vec(&rows).expect("an old attempts file"),
        )
        .expect("a write");

        let store = FileWorkflowStore::open(&scratch.0).await.expect("a store");
        let left = store.read_attempts().await.expect("a read");
        let mut steps: Vec<&str> = left.keys().map(|key| key.step_id.as_str()).collect();
        steps.sort_unstable();
        assert_eq!(steps, vec!["asking", "going"]);
    }
}
