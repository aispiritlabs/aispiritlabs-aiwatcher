//! The HTTP surface. Every module here is a facade: `router()` and
//! `openapi()`, nothing else.
//!
//! | Group | Reads | Notes |
//! |---|---|---|
//! | `/runs`, `/spans`, `/dimensions` | read model | what happened |
//! | `/events/stream`, `/live` | projector fan-out | a reconnect closes its own gap — see [`stream`] |
//! | `/prompts` | object store | authored, outlives the runs that used it |
//! | `/annotation-*` | object store | the only routes taking image bytes; refusals carry a list |
//! | `/annotation-import-*` | object store | a staged batch and a resumable job — see [`imports`] |
//! | `/conversation-*` | encrypted store | `admin` reads content, `editor` writes, `viewer` sees everything but the words |
//! | `/training-runs`, `/models` | object store | touches no log, no live hub, no assembler |
//! | `.../context` | plan + artifacts | so the panel need not reconstruct one |
//! | `.../artifacts` | catalog + object store | what a run produced, a pod's log included — see [`artifacts`] |
//! | `/executions` | workflow store | the only transactional store here; serves no list |
//! | `/workflows` | the log | a graph rather than a run. Its rerun is the one route that asks another system to work |
//! | `/auth` | — | about the caller. Its layer runs in front of everything |
//! | `/events` (POST) | — | the way in for a client that cannot reach the log directly |
//!
//! ADR_0021, ADR_0022, ADR_0025, ADR_0026.

pub mod annotations;
pub mod artifacts;
pub mod auth;
pub mod context;
pub mod conversations;
pub mod datasets;
pub mod definitions;
pub mod error;
pub mod evaluations;
pub mod executions;
pub mod health;
pub mod imports;
pub mod ingest;
pub mod integrations;
pub mod live;
pub mod metrics;
pub mod openapi;
pub mod prompts;
pub mod routes;
pub mod runs;
pub mod schedules;
pub mod state;
pub mod stream;
pub mod training;
pub mod worker;
pub mod workflows;

pub use auth::Caller;
pub use error::ApiError;
pub use openapi::ApiDoc;
pub use routes::router;
pub use state::{AppState, HealthState};
