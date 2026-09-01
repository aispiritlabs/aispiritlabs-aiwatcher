//! What every handler is given.

use std::sync::Arc;

use aiwatcher_annotations::Registry as AnnotationRegistry;
use aiwatcher_auth::Authenticator;
use aiwatcher_bus::{MessageSink, MessageSource};
use aiwatcher_core::engine::WorkflowEngine;
use aiwatcher_core::ports::WorkflowRunner;
use aiwatcher_datasets::Registry as DatasetRegistry;
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
    /// Vector image annotations and the training exports built from them.
    /// Same store, third prefix, and the same reason all three are here rather
    /// than on the log: a training label has to outlive every run that used
    /// it. See ADR_0017.
    pub annotations: Option<Arc<AnnotationRegistry>>,
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
    /// `None` when no identity provider is configured, which is the default.
    /// Unlike `prompts` and `runner`, absence here is not a 501 on a few
    /// routes — it is every caller being [`aiwatcher_auth::Identity::anonymous`]
    /// and every role check passing, which is what `AIWATCHER_AUTH_MODE=none`
    /// means. The 501 is reserved for the sign-in routes, which cannot do
    /// anything useful without a provider.
    pub auth: Option<Arc<Authenticator>>,
    pub health: HealthState,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("source", &self.source)
            .field("ingest_enabled", &self.sink.is_some())
            .field("prompt_registry", &self.prompts.is_some())
            .field("dataset_registry", &self.datasets.is_some())
            .field("annotation_registry", &self.annotations.is_some())
            .field("training_registry", &self.training.is_some())
            .field("workflow_runner", &self.runner)
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
