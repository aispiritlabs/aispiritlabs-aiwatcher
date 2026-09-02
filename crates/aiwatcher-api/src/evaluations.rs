//! Evaluation reports: suites, scores, regressions.
//!
//! An `eval.*` event rides the same log and forms **no span**, and these
//! routes read a bounded projection of its own rather than the runs list.
//! See ADR_0010.

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use utoipa::OpenApi;

use aiwatcher_projector::{EvaluationDetail, EvaluationFilter, EvaluationPage, SuitePage};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// This module's operations, as the contract they satisfy.
#[derive(OpenApi)]
#[openapi(paths(list_evaluations, get_evaluation, list_evaluation_suites,))]
struct Api;

/// The operations this module serves. Composed by [`crate::openapi`].
#[must_use]
pub fn openapi() -> utoipa::openapi::OpenApi {
    Api::openapi()
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/evaluations", get(list_evaluations))
        .route("/api/v1/evaluations/{evaluation_id}", get(get_evaluation))
        .route("/api/v1/evaluation-suites", get(list_evaluation_suites))
}

// ── Evaluations ──────────────────────────────────────────────────────────────

/// Evaluation reports, newest first.
///
/// The other half of the loop the traces come from: a trace says what one run
/// did, an evaluation says whether the thing producing those runs is getting
/// better. They arrive on the same log and are folded apart — an `eval.*`
/// event produces no span and no row in the runs list. See
/// `aiwatcher_projector::evaluations`.
#[utoipa::path(
    get,
    path = "/api/v1/evaluations",
    params(EvaluationFilter),
    responses((status = 200, body = EvaluationPage)),
    tag = "evaluation",
)]
async fn list_evaluations(
    State(state): State<AppState>,
    Query(filter): Query<EvaluationFilter>,
) -> Json<EvaluationPage> {
    Json(state.read_model.evaluations(&filter).await)
}

/// One evaluation: its parameters, its metrics, its cases, the document the
/// producer attached, and the previous evaluation of the same suite on the
/// same dataset.
#[utoipa::path(
    get,
    path = "/api/v1/evaluations/{evaluation_id}",
    params(("evaluation_id" = String, Path, description = "The evaluation to fetch")),
    responses(
        (status = 200, body = EvaluationDetail),
        (status = 404, body = crate::error::ErrorBody),
    ),
    tag = "evaluation",
)]
async fn get_evaluation(
    State(state): State<AppState>,
    Path(evaluation_id): Path<String>,
) -> ApiResult<Json<EvaluationDetail>> {
    state
        .read_model
        .evaluation(&evaluation_id)
        .await
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("evaluation {evaluation_id}")))
}

/// Suites: the level above a report, and what MLflow calls an experiment.
///
/// A separate resource rather than `/evaluations/suites`, so an evaluation
/// that happens to be called `suites` is still reachable.
#[utoipa::path(
    get,
    path = "/api/v1/evaluation-suites",
    responses((status = 200, body = SuitePage)),
    tag = "evaluation",
)]
async fn list_evaluation_suites(State(state): State<AppState>) -> Json<SuitePage> {
    Json(state.read_model.evaluation_suites().await)
}
