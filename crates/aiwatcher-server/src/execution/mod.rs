//! The work role: the outbox publisher, and the reactors.
//!
//! Section 27 splits this binary in two. `serve` holds the API, the read model
//! and the object store; `work` holds the loops in this module and is the only
//! role that opens a socket to Flow, a notebook runtime, an engine or the
//! cluster. That is how ADR_0008's "the binary does not know the optional
//! services exist" survives — as a statement about the *API*, which is where it
//! was load-bearing.
//!
//! ```text
//!   serve                        work
//!   ─────                        ────
//!   POST /executions             outbox ──► the event log
//!   the read model               reactor ──► Flow, and back into the store
//!   publish_dataset reactor
//!            └──────── one WorkflowStore ────────┘
//! ```
//!
//! ## Why `publish_dataset` runs in `serve`
//!
//! It is the one binding that executes nothing: it writes a content-addressed
//! dataset version through the object store the serve role already holds
//! (ADR_0025). Putting it in `work` would give the role that reaches Flow a
//! second reason to hold the registry's credentials, for no capability it does
//! not already have.
//!
//! ## Why the projector stays in `serve`
//!
//! Section 27 says `work` "holds the consumers". In this codebase the
//! projector *is* the read model the API serves from, in process, under a
//! memory contract (`AIWATCHER_MAX_SPANS_TOTAL`). A `serve` role without it
//! would answer every read from an empty fold. Moving the folds out of process
//! is Phase 8, deferred behind its own gate — so until then the consumer that
//! runs here is the outbox, and the plan is corrected rather than followed.

pub mod artifacts;
pub mod flow;
pub mod publish;

use std::sync::Arc;
use std::time::Duration;

use aiwatcher_api::state::AppState;
use aiwatcher_bus::MessageSink;
use aiwatcher_execution::{
    ArtifactCatalog, ExecutionHandler, ObjectArtifactCatalog, Performed, Reactor, WorkflowStore,
    publish_pending,
};
use time::OffsetDateTime;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::config::Config;

/// How long a loop waits after an error it cannot act on.
///
/// Longer than the poll, so a store that is down is asked about once a
/// half-minute rather than once a second. The error is logged on every pass
/// either way; what this bounds is how often.
const BACKOFF: Duration = Duration::from_secs(30);

/// Every background task the roles in this process run.
///
/// A struct rather than a `Vec<JoinHandle>` so the shutdown path can say which
/// one did not stop, and so an empty one reads as "this role runs none".
#[derive(Debug, Default)]
pub struct Tasks {
    pub outbox: Option<JoinHandle<()>>,
    pub reactors: Vec<(&'static str, JoinHandle<()>)>,
}

impl Tasks {
    /// Wait for each to finish, or say which did not.
    pub async fn drain(self, grace: Duration) {
        if let Some(task) = self.outbox {
            match tokio::time::timeout(grace, task).await {
                Ok(Ok(())) => tracing::info!("the execution outbox stopped"),
                Ok(Err(error)) => tracing::error!(%error, "the execution outbox panicked"),
                // Every row it did not send is still pending, so the next
                // process to run one sends it. A fact published twice is a
                // redelivery the projector already deduplicates; one lost is
                // the failure ADR_0026 is about, and this ordering is what
                // makes the first outcome the one that happens.
                Err(_) => tracing::warn!("the execution outbox did not stop within the grace"),
            }
        }
        for (role, task) in self.reactors {
            match tokio::time::timeout(grace, task).await {
                Ok(Ok(())) => tracing::info!(role, "a reactor stopped"),
                Ok(Err(error)) => tracing::error!(role, %error, "a reactor panicked"),
                // An attempt in flight keeps its lease until it expires, and
                // is then claimable again — by this process on its next start,
                // or by another. `previous_owner` is what makes the next
                // claimant ask the runtime before it repeats the work.
                Err(_) => tracing::warn!(role, "a reactor did not stop within the grace"),
            }
        }
    }
}

/// Start what this process's role is responsible for.
///
/// Reads [`Config::role`](crate::config::Config): both halves in one process is
/// the default and is what the `file` store requires, since it holds one
/// process and both halves need it.
#[must_use]
pub fn spawn(
    state: &AppState,
    config: &Config,
    store: &Arc<dyn WorkflowStore>,
    sink: &Arc<dyn MessageSink>,
    objects: Option<&Arc<dyn aiwatcher_core::prompts::ObjectStore>>,
    shutdown: &CancellationToken,
) -> Tasks {
    let Some(notify) = state.execution_worker.as_ref().map(Arc::clone) else {
        return Tasks::default();
    };
    let artifacts = objects.map(|store| artifacts::Artifacts::new(Arc::clone(store)));
    // The manifest beside the bytes, and the cache index beside both. `None`
    // when this deployment has no object store, which runs everything and
    // remembers nothing — section 18's "deleting the index loses nothing
    // authoritative", taken to its limit.
    let catalog: Option<Arc<dyn ArtifactCatalog>> = objects.map(|store| {
        Arc::new(ObjectArtifactCatalog::new(Arc::clone(store))) as Arc<dyn ArtifactCatalog>
    });
    let artifacts = artifacts.as_ref();
    let mut tasks = Tasks::default();

    if config.role.serves() {
        // The one executor that runs where the ingress is, because it executes
        // nothing: it writes a dataset version through the object store this
        // role already holds.
        let executors = publish::executors(state, artifacts);
        if !executors.is_empty() {
            tasks.reactors.push((
                "serve",
                spawn_reactor(
                    with_catalog(
                        Reactor::new(
                            ExecutionHandler::new(Arc::clone(store)),
                            executors,
                            owner_of(config, "serve"),
                        ),
                        catalog.clone(),
                    ),
                    Arc::clone(&notify),
                    config.execution_poll,
                    "serve",
                    shutdown.clone(),
                ),
            ));
        }
    }

    if config.role.works() {
        tasks.outbox = Some(spawn_outbox(
            Arc::clone(store),
            Arc::clone(sink),
            Arc::clone(&notify),
            config.execution_poll,
            shutdown.clone(),
        ));

        let executors = flow::executors(config, artifacts);
        if executors.is_empty() {
            tracing::info!(
                "the work role holds no runtime executor; nothing is claimed \
                 (AIWATCHER_FLOW_URL)"
            );
        } else {
            tasks.reactors.push((
                "work",
                spawn_reactor(
                    with_catalog(
                        Reactor::new(
                            ExecutionHandler::new(Arc::clone(store)),
                            executors,
                            owner_of(config, "work"),
                        ),
                        catalog.clone(),
                    ),
                    notify,
                    config.execution_poll,
                    "work",
                    shutdown.clone(),
                ),
            ));
        }
    }

    tasks
}

/// A reactor with an index behind it, when there is one to give it.
///
/// A free function rather than an `Option` inside the reactor's constructor,
/// so that "this deployment keeps no lineage" is one branch in one place rather
/// than a `None` threaded through two call sites.
fn with_catalog<S: WorkflowStore>(
    reactor: Reactor<S>,
    catalog: Option<Arc<dyn ArtifactCatalog>>,
) -> Reactor<S> {
    match catalog {
        Some(catalog) => reactor.with_catalog(catalog),
        None => reactor,
    }
}

/// The name this process holds its leases under.
///
/// Unique per process *and per role*: the two reactors in a combined process
/// claim disjoint runtimes, but they renew leases under their own names, and
/// two of them sharing one would each believe they held the other's — the one
/// thing a lease exists to prevent.
fn owner_of(config: &Config, role: &str) -> String {
    match &config.reactor_owner {
        Some(owner) => format!("{owner}/{role}"),
        None => format!(
            "{}/{}/{role}",
            std::env::var("HOSTNAME")
                .ok()
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "local".to_owned()),
            std::process::id()
        ),
    }
}

/// Drain committed facts onto the event log, after they committed.
///
/// ADR_0026's one strict ordering, as a loop. It publishes nothing the handler
/// did not already write down, and it never publishes from inside a decision —
/// which is what stops a `step.completed` on the log for an attempt the store
/// does not consider complete.
fn spawn_outbox(
    store: Arc<dyn WorkflowStore>,
    sink: Arc<dyn MessageSink>,
    notify: Arc<tokio::sync::Notify>,
    poll: Duration,
    shutdown: CancellationToken,
) -> JoinHandle<()> {
    tracing::info!(
        poll_seconds = poll.as_secs(),
        "the execution outbox is publishing to the event log"
    );
    tokio::spawn(async move {
        loop {
            let wait = match drain_outbox(store.as_ref(), sink.as_ref()).await {
                Ok(()) => poll,
                Err(error) => {
                    // The rows stay pending, so nothing is lost by waiting.
                    tracing::warn!(%error, "the execution outbox could not publish");
                    BACKOFF
                }
            };
            tokio::select! {
                () = shutdown.cancelled() => {
                    // One last pass, so a clean stop does not leave a fact
                    // behind a poll interval. A crash would be absorbed by the
                    // next process; this is the tidy case.
                    let _ = drain_outbox(store.as_ref(), sink.as_ref()).await;
                    tracing::info!("the execution outbox is stopping");
                    return;
                }
                () = notify.notified() => {}
                () = tokio::time::sleep(wait) => {}
            }
        }
    })
}

/// Send every pending row, in batches, until there are none.
///
/// A loop rather than one batch: a backlog after a broker outage is worked
/// through as fast as the sink accepts it, and stopping at one batch per poll
/// would drain it at [`aiwatcher_execution::outbox::BATCH`] rows a second.
async fn drain_outbox(
    store: &dyn WorkflowStore,
    sink: &dyn MessageSink,
) -> aiwatcher_execution::Result<()> {
    loop {
        let published = publish_pending(store, sink, OffsetDateTime::now_utc()).await?;
        if published.undecodable > 0 {
            // Left pending rather than dropped: a row written by a newer build
            // is a fact this process must not silently discard.
            tracing::warn!(
                rows = published.undecodable,
                "the outbox holds rows this build cannot decode; they stay pending"
            );
        }
        if published.sent == 0 {
            return Ok(());
        }
        tracing::debug!(rows = published.sent, "published execution facts");
    }
}

/// Claim one attempt at a time, until there is nothing left to claim.
///
/// One attempt per pass is [`Reactor::poll_once`]'s contract, and the pacing is
/// here so the process can be drained: a loop inside the reactor would be one
/// that cannot be stopped between two steps.
fn spawn_reactor<S: WorkflowStore + 'static>(
    reactor: Reactor<S>,
    notify: Arc<tokio::sync::Notify>,
    poll: Duration,
    role: &'static str,
    shutdown: CancellationToken,
) -> JoinHandle<()> {
    tracing::info!(role, "a reactor is claiming attempts");
    tokio::spawn(async move {
        loop {
            let wait = match reactor.poll_once(OffsetDateTime::now_utc()).await {
                // Something ran. Ask again immediately rather than waiting a
                // poll interval: a three-step chain would otherwise take three
                // seconds of doing nothing.
                Ok(Performed::Reported { .. }) => Duration::ZERO,
                Ok(Performed::Idle) => poll,
                Ok(Performed::LeaseLost { step_id, attempt }) => {
                    // Reported rather than retried here. The work is lost on
                    // purpose: whoever holds the lease now is the one party
                    // that may write for this attempt.
                    tracing::warn!(
                        role,
                        step_id,
                        attempt,
                        "a claim expired while the step was running; its replacement owns it"
                    );
                    Duration::ZERO
                }
                Ok(Performed::Released { step_id, reason }) => {
                    tracing::warn!(role, step_id, reason, "released a claimed attempt");
                    poll
                }
                Err(error) => {
                    tracing::warn!(role, %error, "a reactor could not claim or report");
                    BACKOFF
                }
            };
            if wait.is_zero() {
                if shutdown.is_cancelled() {
                    tracing::info!(role, "a reactor is stopping");
                    return;
                }
                continue;
            }
            tokio::select! {
                () = shutdown.cancelled() => {
                    tracing::info!(role, "a reactor is stopping");
                    return;
                }
                () = notify.notified() => {}
                () = tokio::time::sleep(wait) => {}
            }
        }
    })
}
