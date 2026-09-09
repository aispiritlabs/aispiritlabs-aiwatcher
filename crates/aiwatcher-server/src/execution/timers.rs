//! The one active thing this engine does for a hosted run.
//!
//! `agentic.workflow.Saga` has had `schedule_timeout`, `due_timeouts` and
//! `fire_timeout` since before any of this existed. What it has never had is
//! something that *wakes up and looks*: a worker holding its own SQLite is not
//! running when the timeout comes due, and nothing else knows the timeout is
//! there. This loop is that something, and it is deliberately the smallest
//! version of it — a deferred append of a message the worker composed.
//!
//! ## Why no cursor
//!
//! The scheduler's tick reads an *interval* and asks a pure function what fell
//! in it, because a slot that nothing noticed must not be lost. A timer is the
//! other shape: it is a row that stays due until something delivers it, so
//! "what is due now" is the whole question and a tick that missed a pass simply
//! finds it on the next one. There is nothing to checkpoint, and a checkpoint
//! would be a second place a timer could be considered handled.
//!
//! ## Why no lease
//!
//! Two replicas that both find a timer due produce one delivery, and neither
//! contention nor a lease is what makes that true: the message is recorded
//! under an id derived from the execution and the timer, so both land on one
//! inbox entry, and the append that delivers it retires the row in the same
//! transaction. See [`ExecutionHandler::fire_due_timers`].

use std::sync::Arc;

use aiwatcher_api::state::AppState;
use aiwatcher_execution::WorkflowStore;
use aiwatcher_execution::hosted::TIMERS_PER_TICK;
use time::OffsetDateTime;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// How often to look.
///
/// Ten seconds rather than the scheduler's minute, and the difference is what
/// the two are for: a schedule fires at a wall-clock time somebody wrote down
/// and a minute of lateness is invisible, while a timeout inside an agent turn
/// is somebody waiting. It is still an operational choice — a timer stays due
/// until it is delivered, so the rate decides lateness and never correctness.
const TICK: std::time::Duration = std::time::Duration::from_secs(10);

/// Start the loop that delivers due timers.
pub fn spawn(
    state: &AppState,
    store: Arc<dyn WorkflowStore>,
    shutdown: CancellationToken,
) -> JoinHandle<()> {
    let state = state.clone();
    tracing::info!(
        seconds = TICK.as_secs(),
        "hosted timers are being watched for"
    );
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = shutdown.cancelled() => {
                    tracing::info!("the timer loop is stopping");
                    return;
                }
                () = tokio::time::sleep(TICK) => {}
            }
            tick(&state, store.as_ref()).await;
        }
    })
}

/// One pass: deliver what is due, and say what happened when anything did.
async fn tick(state: &AppState, store: &dyn WorkflowStore) {
    let Some(handler) = state.executions.as_ref() else {
        return;
    };
    let _ = store;
    match handler
        .fire_due_timers(OffsetDateTime::now_utc(), TIMERS_PER_TICK)
        .await
    {
        Ok(fired) if fired.is_empty() => {}
        Ok(fired) => {
            let delivered = fired.iter().filter(|one| one.delivered).count();
            tracing::info!(
                delivered,
                // A timer whose run had already ended is retired rather than
                // delivered, and counting the two together would hide a graph
                // that is scheduling timeouts nobody will ever receive.
                lapsed = fired.len() - delivered,
                "hosted timers came due"
            );
        }
        // The rows are still due, so the next pass has them. A store that is
        // down costs lateness and never a timeout.
        Err(error) => tracing::warn!(%error, "a timer pass could not run"),
    }
}
