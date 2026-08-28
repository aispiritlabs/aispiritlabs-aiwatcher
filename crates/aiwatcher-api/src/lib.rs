//! The HTTP surface.
//!
//! Three kinds of endpoint, and they exist for three different reasons:
//!
//! * **Reads** (`/api/v1/runs`, `/api/v1/runs/{id}`) answer "what happened",
//!   served from the projector's read model so the panel's first paint does not
//!   wait on a trace store.
//! * **Live** (`/api/v1/runs/{id}/stream`, `/api/v1/live`) answer "what is
//!   happening", straight off the projector's fan-out. A reconnect closes its
//!   own gap — see [`stream`].
//! * **Ingest** (`/api/v1/events`) exists so a client that cannot reach Laser
//!   directly — a browser, a serverless function, anything behind a firewall —
//!   still has a way in. Python and TypeScript SDKs publish to Laser directly;
//!   this is the fallback, not the main path.

pub mod error;
pub mod openapi;
pub mod routes;
pub mod state;
pub mod stream;

pub use error::ApiError;
pub use openapi::ApiDoc;
pub use routes::router;
pub use state::{AppState, HealthState};
