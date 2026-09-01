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

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;

use aiwatcher_training::{
    FinishRunRequest, ModelLabelRequest, ModelDetail, ModelHead, ModelPage, ProgressRequest,
    RegisterModelRequest, RegisteredModel, Registry, RunFilter, StartRunRequest, TrainingRun,
    TrainingRunPage, TrainingRunSummary, TrainingStatus,
};

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/training-runs", get(list_training_runs).post(start_training_run))
        .route("/api/v1/training-runs/{run_id}", get(get_training_run))
        .route("/api/v1/training-runs/{run_id}/progress", post(record_training_progress))
        .route("/api/v1/training-runs/{run_id}/finish", post(finish_training_run))
        .route("/api/v1/models", get(list_models).post(register_model))
        .route("/api/v1/models/{name}", get(get_model))
        .route("/api/v1/models/{name}/labels", post(set_model_label))
}

fn registry(state: &AppState) -> ApiResult<&Arc<Registry>> {
    state
        .training
        .as_ref()
        .ok_or(ApiError::TrainingRegistryDisabled)
}

/// Writing here is an editor's. A trainer runs where nobody can complete an
/// interactive sign-in, so it carries an ingest token — which is capped at
/// editor for exactly this reason, and cannot promote anything to production
/// because a *label* is the one write in this module that changes what a
/// deployment loads.
fn may_write(caller: &Caller) -> ApiResult<()> {
    caller.require(aiwatcher_auth::Role::Editor).map(|_| ())
}

/// Moving a label is what changes which weights a service loads next. Same
/// role as a rerun and a launch, and for the same reason: it is the write that
/// reaches outside aiwatcher.
fn may_promote(caller: &Caller) -> ApiResult<()> {
    caller.require(aiwatcher_auth::Role::Admin).map(|_| ())
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
pub async fn list_training_runs(
    State(state): State<AppState>,
    Query(query): Query<TrainingRunsQuery>,
) -> ApiResult<Json<TrainingRunPage>> {
    let filter = RunFilter {
        model: query.model,
        status: query.status,
        dataset: query.dataset,
    };
    Ok(Json(
        registry(&state)?
            .runs(&filter, query.limit.unwrap_or(50))
            .await?,
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
pub async fn get_training_run(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
) -> ApiResult<Json<TrainingRun>> {
    Ok(Json(registry(&state)?.run(&run_id).await?))
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
pub async fn start_training_run(
    State(state): State<AppState>,
    caller: Caller,
    Json(request): Json<StartRunRequest>,
) -> ApiResult<(StatusCode, Json<TrainingRun>)> {
    may_write(&caller)?;
    Ok((
        StatusCode::CREATED,
        Json(registry(&state)?.start(request).await?),
    ))
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
pub async fn record_training_progress(
    State(state): State<AppState>,
    caller: Caller,
    Path(run_id): Path<String>,
    Json(request): Json<ProgressRequest>,
) -> ApiResult<Json<TrainingRunSummary>> {
    may_write(&caller)?;
    let run = registry(&state)?.progress(&run_id, request).await?;
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
pub async fn finish_training_run(
    State(state): State<AppState>,
    caller: Caller,
    Path(run_id): Path<String>,
    Json(request): Json<FinishRunRequest>,
) -> ApiResult<Json<TrainingRunSummary>> {
    may_write(&caller)?;
    let run = registry(&state)?.finish(&run_id, request).await?;
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
pub async fn list_models(State(state): State<AppState>) -> ApiResult<Json<ModelPage>> {
    Ok(Json(registry(&state)?.models().await?))
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
pub async fn get_model(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<ModelVersionQuery>,
) -> ApiResult<Json<ModelDetail>> {
    Ok(Json(
        registry(&state)?
            .model(&name, query.version.as_deref())
            .await?,
    ))
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
pub async fn register_model(
    State(state): State<AppState>,
    caller: Caller,
    Json(request): Json<RegisterModelRequest>,
) -> ApiResult<(StatusCode, Json<RegisteredModel>)> {
    may_write(&caller)?;
    let registered = registry(&state)?.register_model(request).await?;
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
/// `admin` — and the one the registry itself can refuse: a version with no
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
pub async fn set_model_label(
    State(state): State<AppState>,
    caller: Caller,
    Path(name): Path<String>,
    Json(request): Json<ModelLabelRequest>,
) -> ApiResult<Json<ModelHead>> {
    may_promote(&caller)?;
    Ok(Json(registry(&state)?.set_label(&name, request).await?))
}
