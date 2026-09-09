//! What every handler is given.

use std::sync::Arc;

use aiwatcher_annotations::Registry as AnnotationRegistry;
use aiwatcher_annotations::SourceCatalog;
use aiwatcher_annotations::integrations::hubs::Hubs;
use aiwatcher_auth::Authenticator;
use aiwatcher_bus::{MessageSink, MessageSource};
use aiwatcher_conversations::Registry as ConversationArchive;
use aiwatcher_core::engine::WorkflowEngine;
use aiwatcher_core::ports::{AttemptArtifacts, EditorHost, WorkflowRunner};
use aiwatcher_datasets::Registry as DatasetRegistry;
use aiwatcher_execution::{ArtifactCatalog, ExecutionHandler, WorkflowStore};
use aiwatcher_projector::{LiveHub, ReadModel};
use aiwatcher_prompts::Registry;
use aiwatcher_training::Registry as TrainingRegistry;

/// Shared application state.
///
/// The bus is held behind trait objects so the same router runs over the
/// in-memory bus in tests, the write-ahead log in development and Laser in
/// production, with no conditional compilation.
#[derive(Clone)]
pub struct AppState {
    pub read_model: Arc<ReadModel>,
    pub live: Arc<LiveHub>,
    pub source: Arc<dyn MessageSource>,
    /// `None` disables the HTTP ingest endpoint. A deployment whose producers
    /// all publish to Laser directly should leave it off rather than expose a
    /// second write path.
    pub sink: Option<Arc<dyn MessageSink>>,
    /// `None` when no prompt store is configured, which makes every
    /// `/api/v1/prompts` route answer 501 rather than 404. The registry is the
    /// one thing here that outlives retention, so running without it is a
    /// deliberate choice and the API says so instead of pretending the routes
    /// do not exist.
    pub prompts: Option<Arc<Registry>>,
    /// The durable half of Data Curation: saved Flow recipes and the immutable
    /// rows each execution produced. It shares the configured object store
    /// with prompts, under a separate key prefix.
    pub datasets: Option<Arc<DatasetRegistry>>,

    /// When a definition runs unattended. `None` when this deployment has no
    /// object store, which is the same condition that leaves it no definitions
    /// to schedule.
    ///
    /// Its own field rather than a method on the dataset registry: a schedule
    /// is keyed by [`DefinitionKind`](aiwatcher_execution::plan::DefinitionKind)
    /// and will hold a workflow's as readily as a pipeline's, and it lives
    /// under its own prefix for that reason.
    pub schedules: Option<Arc<aiwatcher_execution::ScheduleStore>>,
    /// Authored Python workflows, versioned outside execution retention.
    pub workflow_definitions: Option<Arc<aiwatcher_execution::definition::DefinitionRegistry>>,
    /// Vector image annotations and the training exports built from them.
    /// Same store, third prefix, and the same reason all three are here rather
    /// than on the log: a training label has to outlive every run that used
    /// it. See ADR_0017.
    pub annotations: Option<Arc<AnnotationRegistry>>,
    /// The governed conversation archive, and the one authored store whose
    /// absence is the *default*. Every other `Option` here is off because a
    /// deployment did not wire something; this one is off because keeping
    /// somebody's words is a decision that has to be made rather than
    /// inherited. See ADR_0021.
    pub conversations: Option<Arc<ConversationArchive>>,
    /// How a handler tells the export worker that there is something to do.
    ///
    /// `None` outside the server binary — a router built for a test has no
    /// worker, and a notify nobody waits on would be a silent no-op rather
    /// than an obvious one.
    pub export_worker: Option<Arc<tokio::sync::Notify>>,
    /// Managed execution: the transactional store, behind the handler that is
    /// the only way to write to it.
    ///
    /// Never `None` in the server binary — an execution store needs no more
    /// configuration than a directory, so there is no "this deployment has
    /// none" to report. It is an `Option` for the routers a test builds, which
    /// have no store and should answer 501 rather than hold one.
    ///
    /// What the API does with it is deliberately narrow: accept a command, and
    /// read one run's own page. Never a *list* — the workflow fold already
    /// serves that from the log, and a second list would be the second picture
    /// of one run that ADR_0026 refuses.
    pub executions: Option<Arc<ExecutionHandler<Arc<dyn WorkflowStore>>>>,
    /// How a handler tells this process's work role that there is something to
    /// do.
    ///
    /// Two loops wait on it — the outbox publisher and a reactor — so this one
    /// wakes *every* waiter rather than one of them, unlike the two queues
    /// above. A loop that was not yet waiting misses the nudge and picks the
    /// work up on its next poll, which is why the poll still exists.
    pub execution_worker: Option<Arc<tokio::sync::Notify>>,
    /// The same, for the annotation import queue.
    ///
    /// A second notify rather than a shared one: the two queues live in
    /// different stores behind different configuration, and a deployment that
    /// imports corpora while keeping no conversation archive is the ordinary
    /// case rather than an odd one.
    pub import_worker: Option<Arc<tokio::sync::Notify>>,
    /// `None` when no dataset hub is configured, which makes
    /// `/api/v1/dataset-hubs` answer 501 naming the variable. Unlike every
    /// other option here this one is *outbound*: it is the only thing in this
    /// state that reaches a service aiwatcher does not run, for a question
    /// whose answer it deliberately refuses to trust — see
    /// [`aiwatcher_annotations::integrations::hubs`].
    pub hubs: Option<Arc<Hubs>>,
    /// The corpora somebody read the licence of, loaded from
    /// `AIWATCHER_DATASET_SOURCES`.
    ///
    /// Not an `Option`: an empty catalogue is a working state rather than a
    /// disabled one. Nothing matches, every hub result stays `unclear`, and an
    /// import records unknown rights — the safe direction, reached by
    /// configuring nothing. See `aiwatcher_annotations::sources`.
    pub sources: Arc<SourceCatalog>,
    /// Training runs and the model versions they produce. Same store, fourth
    /// prefix — and the one registry here whose contents never came from the
    /// event log at all. See ADR_0018.
    pub training: Option<Arc<TrainingRegistry>>,
    /// `None` when no orchestrator is configured, which makes the rerun route
    /// answer 501 rather than 404 — the same reasoning as `prompts`, with a
    /// sharper edge. This is the only thing here that makes something happen
    /// rather than reporting that it did, so the disabled case must be
    /// unmistakable: a no-op adapter would acknowledge a rerun nobody ran.
    pub runner: Option<Arc<dyn WorkflowRunner>>,
    /// `None` when no orchestrator is configured, which makes every
    /// `/api/v1/engine` route answer 501. Same reasoning as `runner`, and the
    /// same sharp edge: this is the other thing here that makes something
    /// happen. It is a separate field rather than the same one because the
    /// two ports answer different questions — a deployment can perfectly well
    /// dispatch reruns to a webhook while having no inventory to browse.
    pub engine: Option<Arc<dyn WorkflowEngine>>,
    /// `None` when this process has no notebook runtime address or no object
    /// store, which makes `POST /executions/{id}/steps/{step}/editor` answer
    /// 501 naming the variable. The third port here that makes something
    /// happen, and the third whose absence has to be unmistakable: an editor
    /// that acknowledged without staging would send somebody to a live app
    /// showing last week's rows and say nothing.
    pub editor: Option<Arc<dyn EditorHost>>,
    /// The bytes a worker reads and writes, proxied through this process.
    ///
    /// `None` when there is no object store, which makes every artifact route
    /// under `/api/v1/worker` answer 501 naming the variable. A worker can
    /// still claim and settle without one — a task that takes its parameters
    /// and returns a bounded value needs no artifact at all — so this is a
    /// separate field from the handler rather than a condition on it.
    pub artifacts: Option<Arc<dyn AttemptArtifacts>>,
    /// Where a cache hit is looked up and a result is recorded.
    ///
    /// The same catalog the work role's reactors hold, because a worker's step
    /// is cached and traced like any other. `None` runs everything and
    /// remembers nothing, which is the state a deployment with no object store
    /// is in: deleting the index never loses an authoritative result, taken to
    /// its limit.
    pub catalog: Option<Arc<dyn ArtifactCatalog>>,
    /// `None` when no identity provider is configured, which is the default.
    /// Unlike `prompts` and `runner`, absence here is not a 501 on a few
    /// routes — it is every caller being [`aiwatcher_auth::Identity::anonymous`]
    /// and every role check passing, which is what `AIWATCHER_AUTH_MODE=none`
    /// means. The 501 is reserved for the sign-in routes, which cannot do
    /// anything useful without a provider.
    pub auth: Option<Arc<Authenticator>>,
    pub health: HealthState,
}

impl AppState {
    /// Wake the export worker, if this process runs one.
    ///
    /// Queueing an export and then waiting for a poll interval would make the
    /// panel's "queued" state last fifteen seconds for no reason. A missing
    /// worker is not an error: the job is durable, and whichever process does
    /// run one will pick it up.
    pub fn notify_export_worker(&self) {
        if let Some(worker) = &self.export_worker {
            worker.notify_one();
        }
    }

    /// Wake the import worker, if this process runs one.
    pub fn notify_import_worker(&self) {
        if let Some(worker) = &self.import_worker {
            worker.notify_one();
        }
    }

    /// Wake this process's work role, if it runs one.
    ///
    /// `notify_waiters` rather than `notify_one`: a decision produces both an
    /// outbox row and, usually, a claimable attempt, and the two are drained by
    /// different loops. Waking one of them would leave the other on its poll.
    pub fn notify_execution_worker(&self) {
        if let Some(worker) = &self.execution_worker {
            worker.notify_waiters();
        }
    }
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("source", &self.source)
            .field("ingest_enabled", &self.sink.is_some())
            .field("prompt_registry", &self.prompts.is_some())
            .field("dataset_registry", &self.datasets.is_some())
            .field("annotation_registry", &self.annotations.is_some())
            .field("conversation_archive", &self.conversations.is_some())
            .field("execution_store", &self.executions.is_some())
            .field("training_registry", &self.training.is_some())
            .field("dataset_hubs", &self.hubs.is_some())
            .field("dataset_sources", &self.sources.sources.len())
            .field("workflow_runner", &self.runner)
            .field("editor", &self.editor)
            .field("engine", &self.engine)
            .field("auth", &self.auth)
            .finish_non_exhaustive()
    }
}

/// Liveness and readiness, kept separate on purpose.
///
/// Liveness means "the process is not wedged" — a failing liveness probe gets
/// the container killed. Readiness means "it can serve traffic right now"; a
/// projector still replaying a backlog is alive but not ready, and restarting
/// it would only make the replay start over.
#[derive(Clone, Debug, Default)]
pub struct HealthState {
    ready: Arc<std::sync::atomic::AtomicBool>,
}

impl HealthState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mark_ready(&self) {
        self.ready.store(true, std::sync::atomic::Ordering::Release);
    }

    pub fn mark_unready(&self) {
        self.ready
            .store(false, std::sync::atomic::Ordering::Release);
    }

    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.ready.load(std::sync::atomic::Ordering::Acquire)
    }
}
