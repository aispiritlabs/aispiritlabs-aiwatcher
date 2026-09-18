//! The HTTP surface. Every module here is a facade: `router()` and
//! `openapi()`, nothing else.
//!
//! | Group | Reads | Notes |
//! |---|---|---|
//! | `/runs`, `/spans`, `/dimensions` | read model | what happened |
//! | `/events/stream`, `/live` | projector fan-out | a reconnect closes its own gap — see [`stream`] |
//! | `/prompts` | object store | authored, outlives the runs that used it |
//! | `/evaluation-rubrics`, `/evaluation-assessments` | object store | what somebody judged, in a form somebody declared — see [`assessments`] |
//! | `/evaluation-scorecards` | object store | what an evaluation measures, declared rather than discovered — see [`scorecards`] |
//! | `/evaluation-runs`, `/evaluation-recordings` | object store + workflow store | measuring answers somebody already has — see [`scoring`] |
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
//! | `/labs` | object store | a workshop's exercise: the brief, and the card and cohort it pins — see [`labs`] |
//! | `/system` | configuration | what this instance has wired, and the variable that decides each — see [`system`] |
//!
//! ADR_0021, ADR_0022, ADR_0025, ADR_0026.

mod annotation_scope;
pub mod annotations;
pub mod artifacts;
pub mod assessments;
pub mod auth;
mod bundle_scope;
mod cohorts;
pub mod context;
pub mod conversations;
mod dataset_scope;
pub mod datasets;
mod definition_scope;
pub mod definitions;
pub mod error;
mod evaluation_bundles;
mod evaluation_scope;
pub mod evaluations;
mod evidence_scope;
pub mod executions;
pub mod experiments;
pub mod health;
pub mod iam;
pub mod imports;
pub mod ingest;
pub mod integrations;
mod lab_scope;
pub mod labs;
pub mod live;
pub mod metrics;
pub mod openapi;
mod project_scope;
mod prompt_scope;
pub mod prompts;
mod recordings;
pub mod reviews;
pub mod routes;
pub mod runs;
pub mod schedules;
pub mod scorecards;
pub mod scoring;
pub mod state;
pub mod stream;
pub mod system;
pub mod training;
mod training_scope;
pub mod worker;
pub mod workflows;

pub use auth::Caller;
pub use error::ApiError;
pub use openapi::ApiDoc;
pub use routes::router;
pub use state::{AppState, HealthState};
