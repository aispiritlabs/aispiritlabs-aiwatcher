//! Aggregates over the runs the projector still holds.
//!
//! Served from the read model rather than from a metrics backend, so the page
//! renders with no PromQL and no dependency on VictoriaMetrics being reachable.

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use utoipa::OpenApi;

use aiwatcher_projector::{MetricsFilter, MetricsSummary};

use crate::run_scope::RunRead;
use crate::state::AppState;

/// This module's operations, as the contract they satisfy.
#[derive(OpenApi)]
#[openapi(paths(get_metrics,))]
struct Api;

/// The operations this module serves, on both route families. Composed by
/// [`crate::openapi`].
#[must_use]
pub fn openapi() -> utoipa::openapi::OpenApi {
    crate::project_scope::openapi(Api::openapi())
}

/// One route, served twice — see [`crate::runs::router`] for why.
pub fn router() -> Router<AppState> {
    Router::new().nest("/api/v1", resource_router()).nest(
        "/api/v1/orgs/{organization}/projects/{project}",
        resource_router()
            .layer(axum::Extension(crate::project_scope::ScopedRoute))
            .layer(tower_http::set_header::SetResponseHeaderLayer::overriding(
                axum::http::header::CACHE_CONTROL,
                axum::http::HeaderValue::from_static("no-store"),
            )),
    )
}

fn resource_router() -> Router<AppState> {
    Router::new().route("/metrics", get(get_metrics))
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
    read: RunRead,
    Query(filter): Query<MetricsFilter>,
) -> Json<MetricsSummary> {
    Json(state.read_model.metrics(read.scope, &filter).await)
}
