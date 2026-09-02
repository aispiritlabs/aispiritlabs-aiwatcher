//! Aggregates over the runs the projector still holds.
//!
//! Served from the read model rather than from a metrics backend, so the page
//! renders with no PromQL and no dependency on VictoriaMetrics being reachable.

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use utoipa::OpenApi;

use aiwatcher_projector::{MetricsFilter, MetricsSummary};

use crate::state::AppState;

/// This module's operations, as the contract they satisfy.
#[derive(OpenApi)]
#[openapi(paths(get_metrics,))]
struct Api;

/// The operations this module serves. Composed by [`crate::openapi`].
#[must_use]
pub fn openapi() -> utoipa::openapi::OpenApi {
    Api::openapi()
}

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/metrics", get(get_metrics))
}

// ── Metrics ──────────────────────────────────────────────────────────────────

/// Aggregates over the runs the projector still holds.
///
/// Served from the read model rather than from a metrics backend: the numbers
/// are a fold over data already in memory, so the page renders with no PromQL
/// and no dependency on VictoriaMetrics being reachable. The window is bounded
/// by retention — `window.runs_retained` against `window.retention_limit` tells
/// a caller whether it is looking at everything or at a truncated tail.
#[utoipa::path(
    get,
    path = "/api/v1/metrics",
    params(MetricsFilter),
    responses((status = 200, body = MetricsSummary)),
    tag = "metrics",
)]
async fn get_metrics(
    State(state): State<AppState>,
    Query(filter): Query<MetricsFilter>,
) -> Json<MetricsSummary> {
    Json(state.read_model.metrics(&filter).await)
}
