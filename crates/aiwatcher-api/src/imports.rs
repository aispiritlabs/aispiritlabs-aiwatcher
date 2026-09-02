//! Staging a corpus-sized import, and the job that reads it.
//!
//! Its own module rather than four more routes in [`crate::annotations`],
//! because it is a different noun with a different lifetime. The routes there
//! answer about *a picture*; these answer about a *job* — one that outlives
//! the request that queued it, survives the process that was running it, and
//! leaves a receipt whether it succeeded or not.
//!
//! The shape is the conversation export's, which is the point:
//!
//! ```text
//!  POST annotation-import-batches   open a batch: project, rights, evidence,
//!                                   the pinned Hub revision/config/split
//!  POST annotation-import-rows      append a page — repeat, and retry freely
//!  POST annotation-import-jobs      seal the batch and queue the job
//!  GET  annotation-import-job       progress, counts, rejects by reason
//!  GET  annotation-import-rejects   the rows it refused, paged, with reasons
//! ```
//!
//! The synchronous `POST /api/v1/annotation-imports` stays exactly where it
//! was and is still the right answer for a catalogue of six hundred. What it
//! cannot be is the answer for a million, and making its body cap larger only
//! moves the number.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;

use aiwatcher_annotations::Registry;
use aiwatcher_annotations::imports::staging::{
    AppendReport, AppendRowsRequest, BatchPage, StageBatchRequest, StagedBatch,
};
use aiwatcher_annotations::imports::{
    ImportIndex, ImportJob, ImportJobPage, ImportJobRequest, ImportManifest, RejectPage,
};

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use utoipa::OpenApi;

/// This module's operations, as the contract they satisfy.
#[derive(OpenApi)]
#[openapi(paths(
    stage_import_batch,
    list_import_batches,
    get_import_batch,
    append_import_rows,
    queue_import_job,
    list_import_jobs,
    get_import_job,
    cancel_import_job,
    list_import_rejects,
    list_import_manifests,
    get_import_manifest,
))]
struct Api;

/// The operations this module serves. Composed by [`crate::openapi`].
#[must_use]
pub fn openapi() -> utoipa::openapi::OpenApi {
    Api::openapi()
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/annotation-import-batches",
            get(list_import_batches).post(stage_import_batch),
        )
        .route("/api/v1/annotation-import-batch", get(get_import_batch))
        .route("/api/v1/annotation-import-rows", post(append_import_rows))
        .route(
            "/api/v1/annotation-import-jobs",
            get(list_import_jobs).post(queue_import_job),
        )
        .route("/api/v1/annotation-import-job", get(get_import_job))
        .route(
            "/api/v1/annotation-import-job/cancel",
            post(cancel_import_job),
        )
        .route(
            "/api/v1/annotation-import-rejects",
            get(list_import_rejects),
        )
        .route(
            "/api/v1/annotation-import-manifests",
            get(list_import_manifests),
        )
        .route(
            "/api/v1/annotation-import-manifest",
            get(get_import_manifest),
        )
}

fn registry(state: &AppState) -> ApiResult<&Arc<Registry>> {
    state
        .annotations
        .as_ref()
        .ok_or(ApiError::AnnotationRegistryDisabled)
}

/// Staging rows that are about to become training data is an editor's job,
/// exactly as registering one image is.
fn may_author(caller: &Caller) -> ApiResult<()> {
    caller.require(aiwatcher_auth::Role::Editor).map(|_| ())
}

fn author(caller: &Caller) -> String {
    let identity = caller.identity();
    if identity.subject.is_empty() {
        "anonymous".to_owned()
    } else {
        identity.subject.clone()
    }
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct BatchQuery {
    pub batch: String,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct JobQuery {
    pub job_id: String,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct RejectsQuery {
    pub job_id: String,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct ManifestQuery {
    pub version: String,
}

// ── Staging ──────────────────────────────────────────────────────────────────

/// Open a batch: what it is for, what may be done with it, and where it came
/// from.
///
/// Rights, evidence and the Hub pin are decided here rather than per page,
/// because they are properties of the corpus. `revision` is the one worth
/// filling in even when it feels redundant: a dataset id is a moving target,
/// and an import that recorded only the name has recorded provenance nobody
/// can go back to.
#[utoipa::path(
    post,
    path = "/api/v1/annotation-import-batches",
    request_body = StageBatchRequest,
    responses(
        (status = 200, body = StagedBatch),
        (status = 400, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody, description = "No such project"),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "annotations",
)]
async fn stage_import_batch(
    State(state): State<AppState>,
    caller: Caller,
    Json(request): Json<StageBatchRequest>,
) -> ApiResult<Json<StagedBatch>> {
    may_author(&caller)?;
    Ok(Json(
        registry(&state)?
            .stage_import(request, &author(&caller))
            .await?,
    ))
}

/// Add one page of rows to an open batch.
///
/// Number the page when the client can. A numbered append is idempotent —
/// identical bytes are an acknowledged retry, different bytes for a page
/// already stored are a 400 naming it — which is what makes a million rows
/// over a flaky link a thing that can be re-sent rather than reconciled.
#[utoipa::path(
    post,
    path = "/api/v1/annotation-import-rows",
    request_body = AppendRowsRequest,
    responses(
        (status = 200, body = AppendReport),
        (status = 400, body = crate::error::ErrorBody, description = "Sealed, out of order, or a page rewritten with different rows"),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 413, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "annotations",
)]
async fn append_import_rows(
    State(state): State<AppState>,
    caller: Caller,
    Json(request): Json<AppendRowsRequest>,
) -> ApiResult<Json<AppendReport>> {
    may_author(&caller)?;
    Ok(Json(registry(&state)?.append_import_rows(request).await?))
}

#[utoipa::path(
    get,
    path = "/api/v1/annotation-import-batches",
    responses(
        (status = 200, body = BatchPage),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "annotations",
)]
async fn list_import_batches(State(state): State<AppState>) -> ApiResult<Json<BatchPage>> {
    Ok(Json(registry(&state)?.import_batches().await?))
}

#[utoipa::path(
    get,
    path = "/api/v1/annotation-import-batch",
    params(BatchQuery),
    responses(
        (status = 200, body = StagedBatch),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "annotations",
)]
async fn get_import_batch(
    State(state): State<AppState>,
    Query(query): Query<BatchQuery>,
) -> ApiResult<Json<StagedBatch>> {
    Ok(Json(registry(&state)?.import_batch(&query.batch).await?))
}

// ── The job ──────────────────────────────────────────────────────────────────

/// Seal the batch and queue the job that reads it.
///
/// A 200 rather than a 201: what comes back is a job, and the thing it will
/// produce does not exist yet. The rights check runs here, before anything is
/// registered — a claim contradicting what a human recorded about the corpus
/// is a decision to reverse, not a row to skip — so a 400 from this route is
/// about the corpus rather than about the request's shape.
///
/// Always queue a `dry_run` first from a UI. Six hundred thousand images with
/// the split key mapped from a filename is not something to discover after the
/// fact, and a dry run costs the downloads and nothing else.
#[utoipa::path(
    post,
    path = "/api/v1/annotation-import-jobs",
    request_body = ImportJobRequest,
    responses(
        (status = 200, body = ImportJob),
        (status = 400, body = crate::error::ErrorBody, description = "Empty batch, wrong project, or rights the curated table contradicts"),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "annotations",
)]
async fn queue_import_job(
    State(state): State<AppState>,
    caller: Caller,
    Json(request): Json<ImportJobRequest>,
) -> ApiResult<Json<ImportJob>> {
    may_author(&caller)?;
    let job = registry(&state)?
        .queue_import(request, &author(&caller))
        .await?;
    // Nudge the worker rather than waiting for its next tick, so a queued job
    // does not sit at "queued" for a poll interval for no reason.
    state.notify_import_worker();
    Ok(Json(job))
}

#[utoipa::path(
    get,
    path = "/api/v1/annotation-import-jobs",
    responses(
        (status = 200, body = ImportJobPage),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "annotations",
)]
async fn list_import_jobs(State(state): State<AppState>) -> ApiResult<Json<ImportJobPage>> {
    Ok(Json(registry(&state)?.import_jobs().await?))
}

#[utoipa::path(
    get,
    path = "/api/v1/annotation-import-job",
    params(JobQuery),
    responses(
        (status = 200, body = ImportJob),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "annotations",
)]
async fn get_import_job(
    State(state): State<AppState>,
    Query(query): Query<JobQuery>,
) -> ApiResult<Json<ImportJob>> {
    Ok(Json(registry(&state)?.import_job(&query.job_id).await?))
}

/// Stop a job. What it already registered stays registered.
///
/// Images are content-addressed, so nothing is half-written; what a cancelled
/// job does not get is a manifest, which is what stops it appearing as a
/// completed import.
#[utoipa::path(
    post,
    path = "/api/v1/annotation-import-job/cancel",
    params(JobQuery),
    responses(
        (status = 200, body = ImportJob),
        (status = 400, body = crate::error::ErrorBody, description = "It already finished"),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "annotations",
)]
async fn cancel_import_job(
    State(state): State<AppState>,
    caller: Caller,
    Query(query): Query<JobQuery>,
) -> ApiResult<Json<ImportJob>> {
    may_author(&caller)?;
    Ok(Json(registry(&state)?.cancel_import(&query.job_id).await?))
}

/// The rows a job refused, and why.
///
/// The dead-letter half, and the reason it is a route rather than a field on
/// the job: an import that rejected four hundred thousand rows cannot put them
/// in a response, and "read the counts, then page the rows" is the difference
/// between a diagnosable import and a number.
#[utoipa::path(
    get,
    path = "/api/v1/annotation-import-rejects",
    params(RejectsQuery),
    responses(
        (status = 200, body = RejectPage),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "annotations",
)]
async fn list_import_rejects(
    State(state): State<AppState>,
    Query(query): Query<RejectsQuery>,
) -> ApiResult<Json<RejectPage>> {
    Ok(Json(
        registry(&state)?
            .import_rejects(
                &query.job_id,
                query.offset.unwrap_or(0),
                query.limit.unwrap_or(50),
            )
            .await?,
    ))
}

/// Every published import, newest first.
#[utoipa::path(
    get,
    path = "/api/v1/annotation-import-manifests",
    responses(
        (status = 200, body = ImportIndex),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "annotations",
)]
async fn list_import_manifests(State(state): State<AppState>) -> ApiResult<Json<ImportIndex>> {
    Ok(Json(registry(&state)?.imports().await?))
}

/// One import's receipt: what it read, what it registered, what it refused,
/// and on what terms.
#[utoipa::path(
    get,
    path = "/api/v1/annotation-import-manifest",
    params(ManifestQuery),
    responses(
        (status = 200, body = ImportManifest),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "annotations",
)]
async fn get_import_manifest(
    State(state): State<AppState>,
    Query(query): Query<ManifestQuery>,
) -> ApiResult<Json<ImportManifest>> {
    Ok(Json(
        registry(&state)?.import_manifest(&query.version).await?,
    ))
}
