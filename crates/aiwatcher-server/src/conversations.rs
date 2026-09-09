//! The two background jobs the conversation archive needs, and nothing else.
//!
//! **The export worker** is what makes an export asynchronous rather than a
//! request somebody holds open. It picks up whatever is queued — including a
//! job an earlier process left `running` when it died, which is the whole
//! reason the cursor is durable — and runs it a shard at a time.
//!
//! **The retention sweep** is what makes the archive's clock real. A retention
//! policy nothing enforces is a paragraph, and the difference between the two
//! is a loop that runs every hour and usually finds nothing.
//!
//! **The orphan payload sweep** is what gives a hosted run's sealed words the
//! lifetime they were promised. It rides the same tick as the retention sweep
//! and is the one job here that reads a store outside the archive — see
//! [`forget_orphan_payloads`] for why it is in this role rather than beside the
//! execution retention it mirrors.
//!
//! Both are one task, because they share a shutdown and neither is busy. A
//! replica that is not running them loses nothing: the job state and the expiry
//! are in the object store, so whichever process does run them picks up the
//! work. Running the sweep in several replicas at once is safe too — erasing an
//! already-erased turn is counted as `already_erased` and changes nothing.

use std::sync::Arc;

use aiwatcher_api::state::AppState;
use aiwatcher_conversations::Registry;
use aiwatcher_execution::state::ExecutionId;
use aiwatcher_execution::store::WorkflowStore;
use time::OffsetDateTime;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::config::Config;

/// Start the worker, if this deployment keeps an archive at all.
///
/// `None` when it does not, which is the default — and the reason this returns
/// an option rather than spawning a task that would immediately find nothing to
/// do forever.
#[must_use]
pub fn spawn(
    state: &AppState,
    config: &Config,
    shutdown: CancellationToken,
) -> Option<JoinHandle<()>> {
    let archive = Arc::clone(state.conversations.as_ref()?);
    let notify = Arc::clone(state.export_worker.as_ref()?);
    // The workflow store, for the orphan sweep only. `None` in a process with
    // no execution store, which is a process that seals no payloads either.
    let runs = state
        .executions
        .as_ref()
        .map(|handler| Arc::clone(handler.store()));
    let poll = config.conversation_export_poll;
    let sweep = config.conversation_sweep_interval;
    let worker = worker_id();
    tracing::info!(
        poll_seconds = poll.as_secs(),
        sweep_seconds = sweep.as_secs(),
        %worker,
        "the conversation archive's export worker and retention sweep are running"
    );
    Some(tokio::spawn(async move {
        run(archive, notify, runs, poll, sweep, worker, shutdown).await;
    }))
}

/// What this process calls itself when it claims an export.
///
/// The pod name in a cluster, which is what makes a claim readable in a log —
/// "held by aiwatcher-server-7d9f-x2k" says more than a UUID. A pod that
/// restarts keeps its name and therefore reclaims its own lease immediately,
/// which is right: the process that held it is gone.
fn worker_id() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| format!("pid-{}", std::process::id()))
}

async fn run(
    archive: Arc<Registry>,
    notify: Arc<tokio::sync::Notify>,
    runs: Option<Arc<dyn WorkflowStore>>,
    poll: std::time::Duration,
    sweep: std::time::Duration,
    worker: String,
    shutdown: CancellationToken,
) {
    let mut poll_tick = tokio::time::interval(poll);
    let mut sweep_tick = tokio::time::interval(sweep);
    // `Delay` rather than `Burst`: a missed tick during a long export should
    // not produce a run of catch-up ticks the moment it finishes.
    poll_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    sweep_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            () = shutdown.cancelled() => {
                tracing::info!("the conversation archive worker is stopping");
                return;
            }
            () = notify.notified() => drain(&archive, &worker, &shutdown).await,
            _ = poll_tick.tick() => drain(&archive, &worker, &shutdown).await,
            _ = sweep_tick.tick() => {
                expire(&archive).await;
                forget_orphan_payloads(&archive, runs.as_deref()).await;
            }
        }
    }
}

/// Run every job that is waiting, oldest first.
///
/// The list is re-read after each job rather than taken once: a job queued
/// while this one was running should not wait for the next tick, and a job
/// somebody cancelled in the meantime should not be started.
///
/// A job another worker holds is skipped by `claimable_exports`, and one whose
/// lease was taken over mid-export stops itself — so this loop can be running
/// in every replica without two of them writing one corpus.
async fn drain(archive: &Registry, worker: &str, shutdown: &CancellationToken) {
    loop {
        if shutdown.is_cancelled() {
            return;
        }
        let waiting = match archive.claimable_exports().await {
            Ok(waiting) => waiting,
            Err(error) => {
                tracing::warn!(%error, "cannot list conversation export jobs");
                return;
            }
        };
        let Some(job_id) = waiting.into_iter().next() else {
            return;
        };
        match archive.run_export(&job_id, worker).await {
            // Somebody else claimed it between the listing and the call. Not a
            // failure, and not finished either — the next listing skips it.
            Ok(job) if job.state == aiwatcher_conversations::JobState::Running => {
                tracing::debug!(
                    job_id = %job.job_id,
                    held_by = %job.claimed_by,
                    "another worker took this export"
                );
            }
            // The job's own failure is recorded on the job, not returned, so
            // what reaches here is only "this job is no longer running".
            Ok(job) => tracing::info!(
                job_id = %job.job_id,
                state = job.state.as_str(),
                rows = job.counts.rows,
                version = job.version.as_deref().unwrap_or("-"),
                "conversation export finished"
            ),
            Err(error) => {
                tracing::warn!(%job_id, %error, "cannot run a conversation export");
                // Stop rather than spin: whatever is wrong will be just as
                // wrong for the next job, and the poll tick will try again.
                return;
            }
        }
    }
}

async fn expire(archive: &Registry) {
    match archive.sweep(OffsetDateTime::now_utc()).await {
        Ok(report) if report.turns_erased > 0 => tracing::info!(
            turns = report.turns_erased,
            conversations = report.conversations_touched,
            "conversation content passed its retention and was erased"
        ),
        Ok(_) => tracing::debug!("nothing in the conversation archive has expired"),
        Err(error) => tracing::warn!(%error, "the conversation retention sweep failed"),
    }
}

/// Forget the sealed payloads of runs the workflow store no longer holds.
///
/// A hosted run's `sealed` words live for as long as the run's history does —
/// that is what a deployment turning the policy on is promised, and until this
/// existed nothing kept it. The three things that look as though they would
/// each miss for their own reason, and `aiwatcher_conversations::payload` says
/// which; this is the one that was worth building.
///
/// **Why here rather than beside the retention it mirrors.** The execution
/// retention sweep runs in the `work` role and the archive in `serve`. §43.11
/// already makes those two share the object store *and* the workflow store, so
/// the join costs nothing here and would cost an archive wired into the other
/// role there. A split deployment sweeps from `serve`; a combined one is the
/// same process either way.
///
/// **Why "no projection" is the whole test.** A projection is written by the
/// first command of every run and removed only by `prune`, and
/// `POST …/payloads` refuses a run that has none — so a payload whose run has
/// no projection belongs to a run that has been forgotten. A store that cannot
/// answer stops the pass rather than continuing: an error is not an absence,
/// but a store that is down will be down for the next run too.
async fn forget_orphan_payloads(archive: &Registry, runs: Option<&dyn WorkflowStore>) {
    let Some(runs) = runs else {
        return;
    };
    let sealed = match archive.payload_executions().await {
        Ok(sealed) => sealed,
        Err(error) => {
            tracing::warn!(%error, "cannot list the runs that have sealed payloads");
            return;
        }
    };
    let (mut payloads, mut executions) = (0, 0);
    for execution in sealed {
        match runs.projection(&ExecutionId::new(execution.clone())).await {
            Ok(Some(_)) => {}
            Ok(None) => match archive.erase_payloads_of(&execution).await {
                Ok(erased) => {
                    payloads += erased;
                    executions += 1;
                }
                Err(error) => {
                    tracing::warn!(%execution, %error, "cannot erase a forgotten run's payloads");
                }
            },
            Err(error) => {
                tracing::warn!(%error, "cannot ask whether a run still exists");
                return;
            }
        }
    }
    if executions > 0 {
        tracing::info!(
            payloads,
            executions,
            "sealed payloads went with the runs that were forgotten"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use aiwatcher_conversations::{ArchivePolicy, Keyring};
    use aiwatcher_core::{CausationId, CorrelationId, MessageId};
    use aiwatcher_execution::store::memory::MemoryWorkflowStore;
    use aiwatcher_execution::{
        AppendRequest, ExecutionMode, ExecutionOwner, ExpectedVersion, MessageMetadata,
        PendingMessage, RunProjection, RunState, StateType, WorkflowCommand, WorkflowMessage,
    };
    use aiwatcher_prompts::adapters::memory::MemoryObjectStore;
    use time::OffsetDateTime;

    const KEY: [u8; 32] = [7; 32];

    fn archive() -> Registry {
        Registry::new(
            Arc::new(MemoryObjectStore::new()),
            "conversations",
            Keyring::single("k1", KEY),
            ArchivePolicy::default(),
        )
    }

    /// One running execution, written straight into the store.
    ///
    /// What the sweep reads is the projection, so a start decided through
    /// `decide` would put a plan and three events into a test about a loop.
    fn running(execution: &ExecutionId) -> AppendRequest {
        AppendRequest {
            expected_version: ExpectedVersion::NoStream,
            input: PendingMessage::input(
                WorkflowMessage::Command(WorkflowCommand::PauseExecution),
                MessageMetadata {
                    schema_version: aiwatcher_execution::message::SCHEMA_VERSION,
                    message_id: MessageId::new(format!("{execution}/only")),
                    occurred_at: OffsetDateTime::UNIX_EPOCH,
                    correlation_id: CorrelationId::new(execution.as_str()),
                    causation_id: CausationId::new("test"),
                    trace_id: None,
                    span_id: None,
                    step_id: None,
                    attempt: None,
                },
            ),
            outputs: Vec::new(),
            projection: RunProjection {
                execution_id: execution.clone(),
                plan_id: String::new(),
                definition_name: "hosted".to_owned(),
                owner: ExecutionOwner::Worker,
                mode: ExecutionMode::Hosted,
                payloads: Default::default(),
                state: RunState::of(StateType::Running),
                requested_by: "a test".to_owned(),
                steps: Vec::new(),
                last_message_version: 1,
                created_at: OffsetDateTime::UNIX_EPOCH,
            },
            outbox: Vec::new(),
            checkpoint: None,
            timers: Vec::new(),
            attempts: Vec::new(),
        }
    }

    #[tokio::test]
    async fn a_forgotten_run_loses_its_payloads_and_a_live_one_keeps_its_own() {
        let archive = archive();
        let store = MemoryWorkflowStore::new();
        let live = ExecutionId::new("run-live");
        store
            .append(&live, running(&live))
            .await
            .expect("a running execution");
        let kept = archive
            .seal_payload("run-live", b"still running")
            .await
            .expect("sealing");
        let gone = archive
            .seal_payload("run-forgotten", b"pruned last week")
            .await
            .expect("sealing");

        forget_orphan_payloads(&archive, Some(&store as &dyn WorkflowStore)).await;

        assert!(
            archive.open_payload("run-live", &kept.digest).await.is_ok(),
            "a run the store still holds keeps its words"
        );
        assert!(
            archive
                .open_payload("run-forgotten", &gone.digest)
                .await
                .is_err(),
            "a run the store has forgotten does not"
        );
        assert_eq!(
            archive.payload_executions().await.expect("listing"),
            vec!["run-live".to_owned()]
        );
    }

    #[tokio::test]
    async fn a_process_with_no_execution_store_erases_nothing() {
        // A process with no workflow store seals no payloads either, so `None`
        // is "cannot answer" rather than "every run is gone" — reading it the
        // other way would erase an archive another process is still writing to.
        let archive = archive();
        archive
            .seal_payload("run-a", b"words")
            .await
            .expect("sealing");

        forget_orphan_payloads(&archive, None).await;

        assert_eq!(
            archive.payload_executions().await.expect("listing"),
            vec!["run-a".to_owned()]
        );
    }
}
