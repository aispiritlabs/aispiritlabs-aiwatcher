//! The Kubernetes probes.
//!
//! Liveness and readiness are separate on purpose: a projector still replaying
//! a backlog is alive but not ready, and restarting it would only make the
//! replay start over.

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use utoipa::OpenApi;

use crate::state::AppState;

/// This module's operations, as the contract they satisfy.
#[derive(OpenApi)]
#[openapi(paths(livez, readyz,))]
struct Api;

/// The operations this module serves. Composed by [`crate::openapi`].
#[must_use]
pub fn openapi() -> utoipa::openapi::OpenApi {
    Api::openapi()
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/livez", get(livez))
        .route("/healthz", get(livez))
        .route("/readyz", get(readyz))
}

// ── Health ───────────────────────────────────────────────────────────────────

/// The process is running.
#[utoipa::path(get, path = "/livez", responses((status = 200)), tag = "health")]
async fn livez() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

/// The process can serve traffic.
///
/// Distinct from liveness: a projector replaying a backlog is alive but not
/// ready, and restarting it would make the replay start over.
#[utoipa::path(
    get,
    path = "/readyz",
    responses((status = 200), (status = 503)),
    tag = "health",
)]
async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    if state.health.is_ready() {
        (StatusCode::OK, "ready")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "not ready")
    }
}
