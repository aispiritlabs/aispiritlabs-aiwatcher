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
//! Both are one task, because they share a shutdown and neither is busy. A
//! replica that is not running them loses nothing: the job state and the expiry
//! are in the object store, so whichever process does run them picks up the
//! work. Running the sweep in several replicas at once is safe too — erasing an
//! already-erased turn is counted as `already_erased` and changes nothing.

use std::sync::Arc;

use aiwatcher_api::state::AppState;
use aiwatcher_conversations::Registry;
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
        run(archive, notify, poll, sweep, worker, shutdown).await;
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
            _ = sweep_tick.tick() => expire(&archive).await,
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
