//! What every handler is given.

use std::sync::Arc;

use aiwatcher_bus::{MessageSink, MessageSource};
use aiwatcher_projector::{LiveHub, ReadModel};
use aiwatcher_prompts::Registry;

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
    pub health: HealthState,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("source", &self.source)
            .field("ingest_enabled", &self.sink.is_some())
            .field("prompt_registry", &self.prompts.is_some())
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
