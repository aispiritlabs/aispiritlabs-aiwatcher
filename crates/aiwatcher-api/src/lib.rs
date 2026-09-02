//! The HTTP surface.
//!
//! Three kinds of endpoint, and they exist for three different reasons:
//!
//! * **Reads** (`/api/v1/runs`, `/api/v1/runs/{id}`) answer "what happened",
//!   served from the projector's read model so the panel's first paint does not
//!   wait on a trace store.
//! * **Live** (`/api/v1/events/stream`, `/api/v1/runs/{id}/stream`,
//!   `/api/v1/live`) answer "what is happening", straight off the projector's
//!   fan-out. A reconnect closes its own gap — see [`stream`].
//! * **The registry** (`/api/v1/prompts`) is the exception to all three: it
//!   reads and writes an object store rather than the log, because a prompt is
//!   authored rather than observed and has to outlive the runs that used it.
//!   See [`prompts`].
//! * **Annotations** (`/api/v1/annotation-*`) are authored in the same sense
//!   and go further: they are the only routes here that accept image bytes,
//!   and the only ones whose refusal carries a list rather than a sentence.
//!   See [`annotations`].
//! * **Training** (`/api/v1/training-runs`, `/api/v1/models`) is the group
//!   that touches none of the machinery above: no log, no live hub, no span
//!   assembler. A training run is a record that grows in place and a model
//!   version is what it produced. See [`training`] and ADR_0018.
//! * **The engine** (`/api/v1/engine`) is the only group here that reads
//!   neither the log nor an authored store: it asks the orchestrator what it
//!   could start, and starts one. See [`engine`].
//! * **The workflow graph** (`/api/v1/workflows`) reads the same log as the
//!   reads above, one level up: a graph rather than a run. Its rerun route is
//!   the only endpoint in this API that asks another system to do work — see
//!   [`workflows`].
//! * **Signing in** (`/api/v1/auth`) is the only group here that is about the
//!   caller rather than about the data. It is also the only one whose layer
//!   runs in front of everything else: see [`auth`].
//! * **Ingest** (`/api/v1/events`) exists so a client that cannot reach Laser
//!   directly — a browser, a serverless function, anything behind a firewall —
//!   still has a way in. Python and TypeScript SDKs publish to Laser directly;
//!   this is the fallback, not the main path.

pub mod annotations;
pub mod auth;
pub mod datasets;
pub mod engine;
pub mod error;
pub mod evaluations;
pub mod health;
pub mod ingest;
pub mod integrations;
pub mod live;
pub mod metrics;
pub mod openapi;
pub mod prompts;
pub mod routes;
pub mod runs;
pub mod state;
pub mod stream;
pub mod training;
pub mod workflows;

pub use auth::Caller;
pub use error::ApiError;
pub use openapi::ApiDoc;
pub use routes::router;
pub use state::{AppState, HealthState};
