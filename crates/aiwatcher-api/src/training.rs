//! Training runs and the model versions they produce.
//!
//! Its own module, and — unlike every other group here — its own *shape*.
//! Nothing in this file touches the event log, the live hub or the span
//! assembler: a training run opens, accumulates a curve and closes, all
//! against a durable record. See ADR_0018 for why that stopped being the
//! log's problem.
//!
//! The write path is deliberately three routes rather than seven. A trainer
//! buffers locally and flushes one batch per epoch, so epochs, sampled points,
//! checkpoints and profiler summaries arrive together in `progress` — one
//! request per epoch rather than four, and one place where a retry is made
//! idempotent.

use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;

use aiwatcher_training::{
    FinishRunRequest, ModelDetail, ModelHead, ModelLabelRequest, ModelPage, ProgressRequest,
    RegisterModelRequest, RegisteredModel, RunFilter, StartRunRequest, TrainingRun,
    TrainingRunPage, TrainingRunSummary, TrainingStatus,
};

use crate::error::ApiResult;
use crate::state::AppState;
use crate::training_scope::{TrainingPromote, TrainingRead, TrainingWrite};
use utoipa::OpenApi;

/// This module's operations, as the contract they satisfy.
///
/// Derived beside the router rather than listed in the root document, so
/// adding a route and forgetting the contract is a change to one file rather
/// than a change to two files that has to be noticed in the second.
#[derive(OpenApi)]
#[openapi(paths(
    list_training_runs,
    get_training_run,
    start_training_run,
    record_training_progress,
    finish_training_run,
    list_models,
    get_model,
    register_model,
    set_model_label,
))]
struct Api;

/// The operations this module serves. Composed by [`crate::openapi`].
#[must_use]
pub fn openapi() -> utoipa::openapi::OpenApi {
    crate::project_scope::openapi(Api::openapi())
}

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
    Router::new()
        .route(
            "/training-runs",
            get(list_training_runs).post(start_training_run),
        )
        .route("/training-runs/{run_id}", get(get_training_run))
        .route(
            "/training-runs/{run_id}/progress",
            post(record_training_progress),
        )
        .route("/training-runs/{run_id}/finish", post(finish_training_run))
        .route("/models", get(list_models).post(register_model))
        .route("/models/{name}", get(get_model))
        .route("/models/{name}/labels", post(set_model_label))
}

#[derive(Deserialize)]
struct RunPath {
    run_id: String,
}
#[derive(Deserialize)]
struct ModelPath {
    name: String,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct TrainingRunsQuery {
    pub model: Option<String>,
    pub status: Option<TrainingStatus>,
    /// An exact `project@version`, or a bare project name to match every
    /// export of it.
    pub dataset: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct ModelVersionQuery {
    /// Omitted resolves `production`, then the newest version.
    pub version: Option<String>,
}

// ── Runs ─────────────────────────────────────────────────────────────────────

/// Training runs, newest first.
#[utoipa::path(
    get,
    path = "/api/v1/training-runs",
    params(TrainingRunsQuery),
    responses(
        (status = 200, body = TrainingRunPage),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "training",
)]
async fn list_training_runs(
    TrainingRead(registry): TrainingRead,
    Query(query): Query<TrainingRunsQuery>,
) -> ApiResult<Json<TrainingRunPage>> {
    let filter = RunFilter {
        model: query.model,
        status: query.status,
        dataset: query.dataset,
    };
    Ok(Json(
        registry.runs(&filter, query.limit.unwrap_or(50)).await?,
    ))
}

/// One run, with its whole curve.
#[utoipa::path(
    get,
    path = "/api/v1/training-runs/{run_id}",
    params(("run_id" = String, Path, description = "The run id the trainer chose")),
    responses(
        (status = 200, body = TrainingRun),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "training",
)]
async fn get_training_run(
    TrainingRead(registry): TrainingRead,
    Path(RunPath { run_id }): Path<RunPath>,
) -> ApiResult<Json<TrainingRun>> {
    Ok(Json(registry.run(&run_id).await?))
}

/// Open a training run.
///
/// Answered before the first epoch on purpose: if this instance is going to
/// refuse the run, a trainer should find out now rather than after six GPU
/// hours. Re-opening an already-open run returns it, so a retried start does
/// not lose the curve it already wrote.
#[utoipa::path(
    post,
    path = "/api/v1/training-runs",
    request_body = StartRunRequest,
    responses(
        (status = 201, body = TrainingRun),
        (status = 400, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody, description = "The run id already finished"),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "training",
)]
async fn start_training_run(
    write: TrainingWrite,
    Json(request): Json<StartRunRequest>,
) -> ApiResult<(StatusCode, Json<TrainingRun>)> {
    let registry = write.authorize().await?;
    Ok((StatusCode::CREATED, Json(registry.start(request).await?)))
}

/// One batch of progress: epochs, sampled points, checkpoints, profiles.
///
/// Returns the summary rather than the whole record, because a trainer flushing
/// every epoch does not want its own curve back every time.
#[utoipa::path(
    post,
    path = "/api/v1/training-runs/{run_id}/progress",
    params(("run_id" = String, Path, description = "The run id")),
    request_body = ProgressRequest,
    responses(
        (status = 200, body = TrainingRunSummary),
        (status = 400, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody, description = "The run has finished"),
        (status = 413, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "training",
)]
async fn record_training_progress(
    write: TrainingWrite,
    Path(RunPath { run_id }): Path<RunPath>,
    Json(request): Json<ProgressRequest>,
) -> ApiResult<Json<TrainingRunSummary>> {
    let registry = write.authorize().await?;
    let run = registry.progress(&run_id, request).await?;
    Ok(Json(run.summary()))
}

/// Close a run.
#[utoipa::path(
    post,
    path = "/api/v1/training-runs/{run_id}/finish",
    params(("run_id" = String, Path, description = "The run id")),
    request_body = FinishRunRequest,
    responses(
        (status = 200, body = TrainingRunSummary),
        (status = 400, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "training",
)]
async fn finish_training_run(
    write: TrainingWrite,
    Path(RunPath { run_id }): Path<RunPath>,
    Json(request): Json<FinishRunRequest>,
) -> ApiResult<Json<TrainingRunSummary>> {
    let registry = write.authorize().await?;
    let run = registry.finish(&run_id, request).await?;
    Ok(Json(run.summary()))
}

// ── Models ───────────────────────────────────────────────────────────────────

/// Every registered model.
#[utoipa::path(
    get,
    path = "/api/v1/models",
    responses(
        (status = 200, body = ModelPage),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "training",
)]
async fn list_models(TrainingRead(registry): TrainingRead) -> ApiResult<Json<ModelPage>> {
    Ok(Json(registry.models().await?))
}

/// One model: its versions, its labels, and one version's record.
#[utoipa::path(
    get,
    path = "/api/v1/models/{name}",
    params(("name" = String, Path, description = "The model name"), ModelVersionQuery),
    responses(
        (status = 200, body = ModelDetail),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "training",
)]
async fn get_model(
    TrainingRead(registry): TrainingRead,
    Path(ModelPath { name }): Path<ModelPath>,
    Query(query): Query<ModelVersionQuery>,
) -> ApiResult<Json<ModelDetail>> {
    Ok(Json(registry.model(&name, query.version.as_deref()).await?))
}

/// Register what a run produced.
///
/// The provenance — dataset, framework, code — is read from the run rather
/// than from this request, so a version cannot claim a lineage the run it
/// names does not have. A version that cannot be promoted is still recorded,
/// and the reason comes back with it.
#[utoipa::path(
    post,
    path = "/api/v1/models",
    request_body = RegisterModelRequest,
    responses(
        (status = 201, body = RegisteredModel, description = "A new version was stored"),
        (status = 200, body = RegisteredModel, description = "This exact version already existed"),
        (status = 400, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "training",
)]
async fn register_model(
    write: TrainingWrite,
    Json(request): Json<RegisterModelRequest>,
) -> ApiResult<(StatusCode, Json<RegisteredModel>)> {
    let registry = write.authorize().await?;
    let registered = registry.register_model(request).await?;
    let status = if registered.created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(registered)))
}

/// Point a label at a version.
///
/// The one write here that changes what a service loads next, so it needs
/// instance `admin` on legacy routes or project `admin` on scoped routes
/// — and the one the registry itself can refuse: a version with no
/// held-out measurement, or one trained on a dataset name nobody can
/// reconstruct, is not promotable however much anybody wants it to be.
#[utoipa::path(
    post,
    path = "/api/v1/models/{name}/labels",
    params(("name" = String, Path, description = "The model name")),
    request_body = ModelLabelRequest,
    responses(
        (status = 200, body = ModelHead),
        (status = 400, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody, description = "The version may not be promoted"),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "training",
)]
async fn set_model_label(
    TrainingPromote(write): TrainingPromote,
    Path(ModelPath { name }): Path<ModelPath>,
    Json(request): Json<ModelLabelRequest>,
) -> ApiResult<Json<ModelHead>> {
    let registry = write.authorize().await?;
    Ok(Json(registry.set_label(&name, request).await?))
}
