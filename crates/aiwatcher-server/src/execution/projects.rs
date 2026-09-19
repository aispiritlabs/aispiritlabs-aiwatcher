//! One loop for every project's work: discover, bind once, and run each pass.
//!
//! ```text
//!   project_scopes ──► bind (once per project) ──► claim ─► publish ─► timers
//!        every 60 s          ProjectDispatcher            └──► sweep (hourly)
//! ```
//!
//! **Why a supervisor rather than a task per project.** ADR_0033 names the
//! thing that would make its own decision wrong: *"a dispatcher serving many
//! projects in one loop — it would bind per attempt, and a bind that costs a
//! pool, a directory handle or a connection per claim is a bind that has to
//! become a parameter after all."* So the bind happens **once per project** and
//! is kept; what runs per pass is the same four calls every instance-wide loop
//! already makes, against a store that sees one project's work and nothing
//! else. The number to watch is binds per second, and here it is bounded by how
//! often a project is *created*.
//!
//! **Where the scopes come from.** `WorkflowStore::project_scopes` — which
//! projects have run something here, never what they ran. Not from IAM: a
//! background loop that could not run a project's work because the control
//! plane was briefly unreachable would be a second availability story for the
//! same work, and a grant is asked anyway, per claim, by `ProjectGrant`.
//!
//! **What it does not do, by name.** A project's attempt whose runtime no
//! executor here performs is not claimed — a query, a notebook, a worker's
//! Python task. Those are refused when a run is *started* instead
//! (`aiwatcher_execution::start`), because an attempt nothing claims is a run
//! that looks alive for ever.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use aiwatcher_api::state::AppState;
use aiwatcher_bus::MessageSink;
use aiwatcher_core::prompts::ObjectStore;
use aiwatcher_execution::{Performed, WorkflowStore};
use aiwatcher_iam::ProjectScope;
use time::OffsetDateTime;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::project::{ProjectDispatcher, Telemetry};

/// How often the set of projects is read again.
///
/// A minute, because what it changes is how soon a project created just now
/// starts having its work done, and never whether it is done at all. One
/// `select distinct` over an indexed column.
const DISCOVER_EVERY: Duration = Duration::from_secs(60);

/// How often each project's finished executions are swept.
///
/// The instance sweep's own interval — `super::SWEEP_EVERY`, read rather than
/// restated — because a scoped sweep that ran per poll would be a delete
/// transaction per project per second for a window that moves by days.
use super::SWEEP_EVERY;

/// How many projects one process runs work for.
///
/// A ceiling with a name rather than a silent cliff: past it the loop says so
/// on every discovery and runs the first `limit` in scope order, which is
/// stable, so the same projects are served rather than a different set each
/// minute.
const MAX_PROJECTS: usize = 64;

/// What this process brings to a project's work.
///
/// The clients as well as the intervals, and they are here rather than read
/// off `Config` a second time because a project's dispatcher and the work
/// role's own reactor must claim the same runtimes for the same reasons: a
/// judge this process does not hold is a card nothing claims, whichever side
/// of the boundary the card is on.
pub struct Wiring {
    /// How often each bound project is asked whether it has work.
    pub poll: Duration,
    /// How long a project's finished executions are kept, if at all.
    pub retention: Option<Duration>,
    /// The name this process holds a project's leases under, before the
    /// project's own key is appended to it.
    pub owner: String,
    /// The judge this process asks, and how many questions at once.
    pub judge: Option<(Arc<dyn aiwatcher_evaluation::JudgeModel>, usize)>,
    /// The scorer service this process asks, and its ceiling.
    pub scorers: Option<(Arc<dyn aiwatcher_evaluation::ExternalScorers>, usize)>,
}

impl std::fmt::Debug for Wiring {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Wiring")
            .field("poll", &self.poll)
            .field("retention", &self.retention)
            .field("judge", &self.judge.is_some())
            .field("scorers", &self.scorers.is_some())
            .finish_non_exhaustive()
    }
}

/// Start the loop, where this process is the work role and has what a project
/// needs.
///
/// `None` — and a line saying which half is missing — when it does not. A
/// project's work then waits for a process that does, which is every other
/// executor's rule here.
#[must_use]
pub fn spawn(
    state: &AppState,
    store: &Arc<dyn WorkflowStore>,
    sink: &Arc<dyn MessageSink>,
    objects: Option<&Arc<dyn ObjectStore>>,
    wiring: Wiring,
    shutdown: &CancellationToken,
) -> Option<JoinHandle<()>> {
    let Wiring {
        poll,
        retention,
        owner,
        judge,
        scorers,
    } = wiring;
    // Each absence is a working state and each says which one it is, because a
    // process that silently performs no project's work is indistinguishable
    // from one where no project has any.
    let missing = [
        ("AIWATCHER_IAM_POSTGRES_URL", state.iam.is_none()),
        (
            "AIWATCHER_PROMPT_STORE (the evaluation registry)",
            state.evaluations.is_none(),
        ),
        (
            "an object store for a project's artifacts",
            objects.is_none(),
        ),
    ];
    let absent: Vec<&str> = missing
        .iter()
        .filter_map(|(what, missing)| missing.then_some(*what))
        .collect();
    if !absent.is_empty() {
        tracing::info!(
            missing = absent.join(", "),
            "this process performs no project's managed work"
        );
        return None;
    }
    let iam = state.iam.clone()?;
    let evaluations = state.evaluations.clone()?;
    let objects = objects?.clone();
    let context = Context {
        store: Arc::clone(store),
        sink: Arc::clone(sink),
        objects,
        evaluations,
        iam,
        owner,
        retention,
        judge,
        scorers,
        telemetry: Telemetry {
            read_model: Arc::clone(&state.read_model),
            bundles: state.evaluation_bundles.clone(),
            witnesses: state.witnesses.clone(),
            prompts: state.prompts.clone(),
            asked: state.asked.clone(),
            wait: super::scoring::TELEMETRY_WAIT,
        },
    };
    let shutdown = shutdown.clone();
    tracing::info!(
        poll_seconds = poll.as_secs(),
        "this process performs projects' managed work"
    );
    Some(tokio::spawn(
        async move { run(context, poll, shutdown).await },
    ))
}

/// Everything one pass needs, assembled once.
struct Context {
    store: Arc<dyn WorkflowStore>,
    sink: Arc<dyn MessageSink>,
    objects: Arc<dyn ObjectStore>,
    evaluations: Arc<aiwatcher_evaluation::Registry>,
    iam: Arc<dyn aiwatcher_iam::IamStore>,
    owner: String,
    retention: Option<Duration>,
    judge: Option<(Arc<dyn aiwatcher_evaluation::JudgeModel>, usize)>,
    scorers: Option<(Arc<dyn aiwatcher_evaluation::ExternalScorers>, usize)>,
    telemetry: Telemetry,
}

async fn run(context: Context, poll: Duration, shutdown: CancellationToken) {
    let mut bound: BTreeMap<ProjectScope, Arc<ProjectDispatcher>> = BTreeMap::new();
    let mut discovered_at: Option<tokio::time::Instant> = None;
    let mut swept_at = tokio::time::Instant::now();
    loop {
        tokio::select! {
            () = shutdown.cancelled() => {
                tracing::info!("the project dispatcher is stopping");
                return;
            }
            () = tokio::time::sleep(poll) => {}
        }

        if discovered_at.is_none_or(|at| at.elapsed() >= DISCOVER_EVERY) {
            discover(&context, &mut bound).await;
            discovered_at = Some(tokio::time::Instant::now());
        }

        let sweeping = context.retention.is_some() && swept_at.elapsed() >= SWEEP_EVERY;
        for dispatcher in bound.values() {
            pass(&context, dispatcher, sweeping).await;
        }
        if sweeping {
            swept_at = tokio::time::Instant::now();
        }
    }
}

/// Read the set of projects again, bind a dispatcher for each new one and drop
/// the ones that have none left.
///
/// Dropping is safe and is what keeps this bounded: a scope the store does not
/// list holds no execution here, so there is nothing of it to claim, publish,
/// time out or sweep — and an execution an outbox row still speaks for is one
/// `prunable` refuses to forget, so a project with a fact still to publish is
/// still listed. Coming back costs one `for_project`, which shares the backing
/// store.
async fn discover(context: &Context, bound: &mut BTreeMap<ProjectScope, Arc<ProjectDispatcher>>) {
    let scopes = match context.store.project_scopes(MAX_PROJECTS + 1).await {
        Ok(scopes) => scopes,
        // The bound dispatchers keep running. A store that could not answer
        // costs a project created in the meantime, and never a project whose
        // work has already started here.
        Err(error) => {
            tracing::warn!(%error, "the projects that have run here could not be listed");
            return;
        }
    };
    if scopes.len() > MAX_PROJECTS {
        tracing::error!(
            projects = scopes.len(),
            ceiling = MAX_PROJECTS,
            "more projects have run here than this process runs work for; the first \
             {MAX_PROJECTS} in scope order are served and the rest wait for another process",
        );
    }
    let served: Vec<ProjectScope> = scopes.into_iter().take(MAX_PROJECTS).collect();
    bound.retain(|scope, _| served.contains(scope));
    for scope in served {
        if bound.contains_key(&scope) {
            continue;
        }
        let dispatcher = ProjectDispatcher::bind(
            &context.store,
            &context.objects,
            &context.evaluations,
            &context.iam,
            scope,
            format!("{}/project/{}", context.owner, scope.key()),
        );
        match dispatcher {
            Ok(dispatcher) => {
                let mut dispatcher = dispatcher.reading_telemetry(context.telemetry.clone());
                // The same clients the work role's own reactor holds. A card
                // that asks a judge or a framework metric is claimed here or
                // by nothing, because a project's attempt is invisible to
                // every other reactor in this process.
                if let Some((judge, concurrency)) = context.judge.clone() {
                    dispatcher = dispatcher.judged_by(judge, concurrency);
                }
                if let Some((scorers, concurrency)) = context.scorers.clone() {
                    dispatcher = dispatcher.scored_by(scorers, concurrency);
                }
                tracing::info!(
                    project = %scope.key(),
                    runtimes = ?dispatcher.runtimes(),
                    "this process now performs a project's managed work"
                );
                bound.insert(scope, Arc::new(dispatcher));
            }
            // Named and skipped, not fatal: one project whose store or object
            // store refuses the scope must not stop every other project's work.
            Err(error) => tracing::error!(
                project = %scope.key(),
                %error,
                "a project's dispatcher could not be bound; its work waits"
            ),
        }
    }
}

/// One project's pass: claim what is due, publish what committed, deliver what
/// came due, and — on the hour — forget what is past the window.
///
/// The order is the one the instance's own loops keep. Claiming first, because
/// that is what somebody is waiting for; publishing after it, because a fact
/// is published only once the decision that produced it has committed
/// (ADR_0026); and the sweep last, because it is the only one that deletes.
async fn pass(context: &Context, dispatcher: &ProjectDispatcher, sweeping: bool) {
    let now = OffsetDateTime::now_utc();
    match dispatcher.poll_once(now).await {
        Ok(Performed::Idle) => {}
        Ok(_) => {}
        Err(error) => tracing::warn!(
            project = %dispatcher.scope().key(),
            %error,
            "a project's attempt could not be claimed or settled"
        ),
    }
    match dispatcher.publish_once(context.sink.as_ref()).await {
        Ok(published) if published.sent > 0 => tracing::debug!(
            project = %dispatcher.scope().key(),
            rows = published.sent,
            "published a project's execution facts"
        ),
        Ok(_) => {}
        Err(error) => tracing::warn!(
            project = %dispatcher.scope().key(),
            %error,
            "a project's execution facts could not be published; they stay pending"
        ),
    }
    if let Err(error) = dispatcher
        .fire_due_timers(now, aiwatcher_execution::hosted::TIMERS_PER_TICK)
        .await
    {
        tracing::warn!(
            project = %dispatcher.scope().key(),
            %error,
            "a project's timer pass could not run"
        );
    }
    let Some(window) = context.retention.filter(|_| sweeping) else {
        return;
    };
    match dispatcher.prune(now - window, super::SWEEP_BATCH).await {
        Ok(pruned) if !pruned.is_empty() => tracing::info!(
            project = %dispatcher.scope().key(),
            executions = pruned.executions,
            attempts = pruned.attempts,
            "forgot a project's executions past the retention window"
        ),
        Ok(_) => {}
        Err(error) => tracing::warn!(
            project = %dispatcher.scope().key(),
            %error,
            "a project's retention sweep could not run"
        ),
    }
}
