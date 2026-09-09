//! The work role: the outbox publisher, and the reactors.
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
//! `work` is the only role that opens a socket to Flow, a notebook runtime, an
//! engine or the cluster.
//!
//! `publish_dataset` runs in `serve` because it executes nothing: it writes a
//! content-addressed version through the object store that role already holds.
//! Putting it in `work` would give the role that reaches Flow the registry's
//! credentials for no capability it lacks.
//!
//! The projector stays in `serve` because it *is* the read model the API
//! answers from, in process, under `AIWATCHER_MAX_SPANS_TOTAL`. A `serve` role
//! without it would answer every read from an empty fold.

pub mod artifacts;
pub mod editor;
pub mod flow;
pub mod marimo;
pub mod measure;
pub mod publish;
pub mod scheduler;
pub mod timers;

use std::sync::Arc;
use std::time::Duration;

use aiwatcher_api::state::AppState;
use aiwatcher_bus::MessageSink;
use aiwatcher_execution::{
    ArtifactCatalog, ExecutionHandler, ObjectArtifactCatalog, Performed, Pruned, Reactor,
    WorkflowStore, publish_pending,
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
    pub retention: Option<JoinHandle<()>>,
    pub scheduler: Option<JoinHandle<()>>,
    /// The hourly walk of the artifact prefix. A measurement, so it is the one
    /// task here whose loss on shutdown costs nothing.
    pub storage: Option<JoinHandle<()>>,
    pub timers: Option<JoinHandle<()>>,
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
        if let Some(task) = self.timers {
            match tokio::time::timeout(grace, task).await {
                Ok(Ok(())) => tracing::info!("the timer loop stopped"),
                Ok(Err(error)) => tracing::error!(%error, "the timer loop panicked"),
                // Every timer it did not deliver is still due, so the next
                // process to run this loop delivers it. Late, never lost.
                Err(_) => tracing::warn!("the timer loop did not stop within the grace"),
            }
        }
        if let Some(task) = self.scheduler {
            match tokio::time::timeout(grace, task).await {
                Ok(Ok(())) => tracing::info!("the scheduler stopped"),
                Ok(Err(error)) => tracing::error!(%error, "the scheduler panicked"),
                // Nothing is half-started: a slot the cursor has not passed is
                // a slot the next tick covers.
                Err(_) => tracing::warn!("the scheduler did not stop within the grace"),
            }
        }
        if let Some(task) = self.storage {
            // Aborted rather than waited for. It is a `list` over a whole
            // prefix and it holds nothing: a measurement interrupted is one
            // reading missed, and making a shutdown wait an hour for a graph is
            // the wrong trade.
            task.abort();
        }
        if let Some(task) = self.retention {
            match tokio::time::timeout(grace, task).await {
                Ok(Ok(())) => tracing::info!("the retention sweep stopped"),
                Ok(Err(error)) => tracing::error!(%error, "the retention sweep panicked"),
                // Nothing is half-deleted: each pass is one transaction, and a
                // pass that never ran is a pass the next one does.
                Err(_) => tracing::warn!("the retention sweep did not stop within the grace"),
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
    metrics: &Arc<dyn aiwatcher_core::ports::MetricSink>,
    shutdown: &CancellationToken,
) -> Tasks {
    let Some(notify) = state.execution_worker.as_ref().map(Arc::clone) else {
        return Tasks::default();
    };
    let artifacts = objects.map(|store| artifacts::Artifacts::new(Arc::clone(store)));
    // The manifest beside the bytes, and the cache index beside both. `None`
    // when this deployment has no object store, which runs everything and
    // remembers nothing: deleting the index loses nothing authoritative,
    // taken to its limit.
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
        // In the work role because the rule it has to keep is the outbox's: an
        // execution whose facts have not reached the log is not forgotten. The
        // loop that publishes them and the loop that forgets are then one
        // process's problem rather than two processes' agreement.
        tasks.retention = config
            .workflow_retention
            .map(|window| spawn_retention(Arc::clone(store), window, shutdown.clone()));

        // Schedules live in the object store, so a deployment with none keeps
        // none — and the loop that would read them is not started rather than
        // started to find nothing. Absence is a working state, as it is for
        // every runtime executor.
        tasks.scheduler = objects.map(|objects| {
            scheduler::spawn(
                state,
                Arc::new(aiwatcher_execution::ScheduleStore::new(Arc::clone(objects))),
                Arc::clone(store),
                Arc::clone(metrics),
                shutdown.clone(),
            )
        });

        // Beside the scheduler because it answers the other half of one
        // question — how far behind is this, and how much is piling up — and
        // because both are the gate for a design rather than a feature. Only
        // where there is an object store to walk.
        tasks.storage = objects.map(|objects| {
            measure::spawn_storage_sweep(Arc::clone(objects), Arc::clone(metrics), shutdown.clone())
        });

        // Beside the scheduler and for the same reason it is in this role: a
        // hosted timer's delivery is an append, and the loops that write to the
        // store belong with the outbox that publishes what they wrote.
        tasks.timers = Some(timers::spawn(state, Arc::clone(store), shutdown.clone()));

        tasks.outbox = Some(spawn_outbox(
            Arc::clone(store),
            Arc::clone(sink),
            Arc::clone(&notify),
            config.execution_poll,
            shutdown.clone(),
        ));

        // One registry per address, merged: each executor's "no address is a
        // working state" stays local to it, and a deployment may run managed
        // Flow steps and no notebooks or the other way round.
        let executors =
            flow::executors(config, artifacts).merge(marimo::executors(config, artifacts));
        if executors.is_empty() {
            tracing::info!(
                "the work role holds no runtime executor; nothing is claimed \
                 (AIWATCHER_FLOW_URL, AIWATCHER_ML_PIPELINE_URL)"
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

/// How often a sweep looks, and how much it takes at once.
///
/// A retention window is days, so an hour is often enough that a store never
/// carries more than an hour of history past its window, and rare enough that a
/// deployment inside its window logs nothing. The batch bounds one transaction:
/// turning retention on against a store with a year in it is many short
/// deletes, not one that holds a table for a minute.
const SWEEP_EVERY: Duration = Duration::from_secs(3_600);
const SWEEP_BATCH: usize = 500;

/// Forget finished executions past the retention window.
///
/// The window is the deployment's; the three rules it keeps are
/// [`WorkflowStore::prune`]'s, and none of them is age alone.
fn spawn_retention(
    store: Arc<dyn WorkflowStore>,
    window: Duration,
    shutdown: CancellationToken,
) -> JoinHandle<()> {
    tracing::info!(
        days = window.as_secs() / 86_400,
        "finished executions are forgotten after this long"
    );
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = shutdown.cancelled() => {
                    tracing::info!("the retention sweep is stopping");
                    return;
                }
                // Before the first sweep as well as between them. Nothing is
                // urgent here, and a process that restarts every few minutes
                // must not sweep on every start.
                () = tokio::time::sleep(SWEEP_EVERY) => {}
            }
            if let Err(error) = sweep(store.as_ref(), window, SWEEP_BATCH).await {
                // The rows stay. A store that is down is a store that keeps
                // its history, which is the failure worth having.
                tracing::warn!(%error, "the retention sweep could not run");
            }
        }
    })
}

/// Take batches until a pass comes back short of one.
///
/// Short means the store ran out of candidates rather than out of budget, which
/// is the only signal here that does not need the store to count what it left —
/// and it cannot spin, because a pass that deletes nothing is short by
/// definition.
async fn sweep(
    store: &dyn WorkflowStore,
    window: Duration,
    batch: usize,
) -> aiwatcher_execution::Result<()> {
    let before = OffsetDateTime::now_utc() - window;
    let mut total = Pruned::default();
    loop {
        let pass = store.prune(before, batch).await?;
        total.executions += pass.executions;
        total.attempts += pass.attempts;
        if pass.executions < batch {
            if !total.is_empty() {
                tracing::info!(
                    executions = total.executions,
                    attempts = total.attempts,
                    "forgot executions past the retention window"
                );
            }
            return Ok(());
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    use aiwatcher_core::{CausationId, CorrelationId, MessageId};
    use aiwatcher_execution::store::memory::MemoryWorkflowStore;
    use aiwatcher_execution::{
        AppendRequest, ExecutionId, ExecutionMode, ExecutionOwner, ExpectedVersion,
        MessageMetadata, PendingMessage, RunProjection, RunState, StateType, WorkflowCommand,
        WorkflowMessage,
    };

    /// One finished execution, written straight into the store.
    ///
    /// A start decided through `decide` would do, and would put a plan, an
    /// outbox row and three events in a test about a loop. What the loop reads
    /// is the projection.
    fn finished(execution: &ExecutionId) -> AppendRequest {
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
                definition_name: "swept".to_owned(),
                owner: ExecutionOwner::Local,
                mode: ExecutionMode::Compiled,
                payloads: Default::default(),
                state: RunState::of(StateType::Completed),
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
    async fn a_sweep_takes_batches_until_one_comes_back_short() {
        // The signal the loop stops on, and the only one available without the
        // store counting what it left behind: a pass that filled its budget may
        // have more waiting, and a pass that did not cannot.
        let store = MemoryWorkflowStore::new();
        for index in 0..5 {
            let execution = ExecutionId::new(format!("sweep-{index}"));
            store
                .append(&execution, finished(&execution))
                .await
                .expect("a finished execution");
        }

        // A window of zero makes every write older than the cutoff, which is
        // what lets this run in milliseconds rather than in a retention period.
        sweep(&store, Duration::ZERO, 2)
            .await
            .expect("a sweep over five executions in batches of two");

        for index in 0..5 {
            let execution = ExecutionId::new(format!("sweep-{index}"));
            assert!(
                store
                    .projection(&execution)
                    .await
                    .expect("reading it back")
                    .is_none(),
                "sweep-{index} survived a sweep that had budget left"
            );
        }
    }

    #[tokio::test]
    async fn a_sweep_over_a_store_inside_its_window_stops_at_once() {
        let store = MemoryWorkflowStore::new();
        let execution = ExecutionId::new("sweep-recent");
        store
            .append(&execution, finished(&execution))
            .await
            .expect("a finished execution");

        sweep(&store, Duration::from_secs(86_400), 2)
            .await
            .expect("a sweep");

        assert!(
            store
                .projection(&execution)
                .await
                .expect("reading it back")
                .is_some(),
            "forgot an execution that finished inside the window"
        );
    }
}
