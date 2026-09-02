//! The background worker that drains the annotation import queue.
//!
//! The same shape as [`crate::conversations`]'s export worker, deliberately —
//! they run the same primitive ([`aiwatcher_jobs`]) over the same object
//! store, and a reader who has understood one has understood the other. It is
//! a separate task rather than a second arm of that one because the two
//! queues live behind different configuration: an installation that imports
//! corpora while keeping no conversation archive is the ordinary case, and
//! folding them together would mean the importer only ran where somebody had
//! also turned on the archive.
//!
//! A replica that is not running it loses nothing: the job state is in the
//! object store, so whichever process does run one picks the work up. Several
//! replicas running it at once is safe too — a job another worker holds is
//! skipped, and a worker whose lease is taken over mid-import stops at its
//! next page boundary.

use std::sync::Arc;

use aiwatcher_annotations::Registry;
use aiwatcher_api::state::AppState;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::config::Config;

/// Start the worker, if this deployment has an annotation registry at all.
#[must_use]
pub fn spawn(
    state: &AppState,
    config: &Config,
    shutdown: CancellationToken,
) -> Option<JoinHandle<()>> {
    let registry = Arc::clone(state.annotations.as_ref()?);
    let notify = Arc::clone(state.import_worker.as_ref()?);
    let poll = config.import_poll;
    let worker = worker_id();
    tracing::info!(
        poll_seconds = poll.as_secs(),
        %worker,
        "the annotation import worker is running"
    );
    Some(tokio::spawn(async move {
        run(registry, notify, poll, worker, shutdown).await;
    }))
}

/// What this process calls itself when it claims a job.
///
/// The pod name in a cluster, which is what makes a claim readable in a log. A
/// pod that restarts keeps its name and therefore reclaims its own lease
/// immediately, which is right: the process that held it is gone.
fn worker_id() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| format!("pid-{}", std::process::id()))
}

async fn run(
    registry: Arc<Registry>,
    notify: Arc<tokio::sync::Notify>,
    poll: std::time::Duration,
    worker: String,
    shutdown: CancellationToken,
) {
    let mut tick = tokio::time::interval(poll);
    // `Delay` rather than `Burst`: a missed tick during a long import should
    // not produce a run of catch-up ticks the moment it finishes.
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            () = shutdown.cancelled() => {
                tracing::info!("the annotation import worker is stopping");
                return;
            }
            () = notify.notified() => drain(&registry, &worker, &shutdown).await,
            _ = tick.tick() => drain(&registry, &worker, &shutdown).await,
        }
    }
}

/// Run every job that is waiting, oldest first.
///
/// The list is re-read after each job rather than taken once: a job queued
/// while this one was running should not wait for the next tick, and one
/// somebody cancelled in the meantime should not be started.
async fn drain(registry: &Registry, worker: &str, shutdown: &CancellationToken) {
    loop {
        if shutdown.is_cancelled() {
            return;
        }
        let waiting = match registry.claimable_imports().await {
            Ok(waiting) => waiting,
            Err(error) => {
                tracing::warn!(%error, "cannot list annotation import jobs");
                return;
            }
        };
        let Some(job_id) = waiting.into_iter().next() else {
            return;
        };
        match registry.run_import(&job_id, worker).await {
            // Somebody else claimed it between the listing and the call. Not a
            // failure, and not finished either — the next listing skips it.
            Ok(job) if job.state == aiwatcher_jobs::JobState::Running => {
                tracing::debug!(
                    job_id = %job.job_id,
                    held_by = %job.claimed_by,
                    "another worker took this import"
                );
                return;
            }
            // The job's own failure is recorded on the job, not returned, so
            // what reaches here is only "this job is no longer running".
            Ok(job) => tracing::info!(
                job_id = %job.job_id,
                state = job.state.as_str(),
                accepted = job.counts.accepted,
                rejected = job.counts.rejected,
                fetched = job.counts.fetched,
                version = job.version.as_deref().unwrap_or("-"),
                "annotation import finished"
            ),
            Err(error) => {
                tracing::warn!(%job_id, %error, "cannot run an annotation import");
                // Stop rather than spin: whatever is wrong will be just as
                // wrong for the next job, and the poll tick will try again.
                return;
            }
        }
    }
}
