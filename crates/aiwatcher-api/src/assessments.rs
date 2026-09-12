//! Rubrics, and the typed judgements made under them.
//!
//! A score is only readable beside the form it was given on, so the form is a
//! resource of its own and an assessment names the version of it. Both belong
//! to Evaluation and are served from its store: a judgement about a case is
//! meaningless beside a result somebody else holds.

use aiwatcher_auth::Role;
use aiwatcher_evaluation::{
    Assessment, AssessmentHistory, AssessmentPage, AssessmentRequest, AssessmentTargetQuery,
    Rubric, RubricPage, RubricVersion,
};
use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use utoipa::OpenApi;

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// This module's operations, as the contract they satisfy.
#[derive(OpenApi)]
#[openapi(paths(
    publish_rubric,
    list_rubrics,
    get_rubric,
    record_assessment,
    list_assessments,
    get_assessment_history,
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
            "/api/v1/evaluation-rubrics",
            get(list_rubrics).post(publish_rubric),
        )
        .route("/api/v1/evaluation-rubrics/{name}", get(get_rubric))
        .route(
            "/api/v1/evaluation-assessments",
            get(list_assessments).post(record_assessment),
        )
        .route(
            "/api/v1/evaluation-assessments/{target_id}/{standing_id}",
            get(get_assessment_history),
        )
}

fn registry(state: &AppState) -> ApiResult<&aiwatcher_evaluation::Registry> {
    state
        .evaluations
        .as_deref()
        .ok_or(ApiError::EvaluationDisabled)
}

fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

/// Publish a form. Idempotent by content: the same words answer with the
/// version that is already there, and the head moves to it either way.
#[utoipa::path(post, path = "/api/v1/evaluation-rubrics", request_body = Rubric,
    responses((status = 200, body = RubricVersion), (status = 400, body = crate::error::ErrorBody),
    (status = 403, body = crate::error::ErrorBody), (status = 501, body = crate::error::ErrorBody)),
    tag = "evaluation")]
async fn publish_rubric(
    State(state): State<AppState>,
    caller: Caller,
    Json(rubric): Json<Rubric>,
) -> ApiResult<Json<RubricVersion>> {
    let identity = caller.require(Role::Editor)?;
    Ok(Json(
        registry(&state)?
            .publish_rubric(&rubric, &identity.subject, now())
            .await?,
    ))
}

/// Every form this instance holds, at its current version.
#[utoipa::path(get, path = "/api/v1/evaluation-rubrics",
    responses((status = 200, body = RubricPage), (status = 501, body = crate::error::ErrorBody)),
    tag = "evaluation")]
async fn list_rubrics(
    State(state): State<AppState>,
    caller: Caller,
) -> ApiResult<Json<RubricPage>> {
    caller.require(Role::Viewer)?;
    Ok(Json(RubricPage {
        rubrics: registry(&state)?.rubrics().await?,
    }))
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
struct VersionQuery {
    /// Absent means the current one. An assessment names a concrete version,
    /// so this is how a reader opens the form somebody actually answered.
    version: Option<String>,
}

/// One form, at the version asked for or at the head.
#[utoipa::path(get, path = "/api/v1/evaluation-rubrics/{name}",
    params(("name" = String, Path), VersionQuery),
    responses((status = 200, body = RubricVersion), (status = 404, body = crate::error::ErrorBody),
    (status = 501, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn get_rubric(
    State(state): State<AppState>,
    caller: Caller,
    Path(name): Path<String>,
    Query(query): Query<VersionQuery>,
) -> ApiResult<Json<RubricVersion>> {
    caller.require(Role::Viewer)?;
    registry(&state)?
        .rubric(&name, query.version.as_deref())
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("rubric {name}")))
}

/// Record one judgement.
///
/// The caller is who filed it, always: a person's judgement is attributed to
/// the session, and a judge's names the judge while the session still says who
/// ran it. Nothing here checks the target exists — a judgement outlives the
/// trace it is about, which is the whole reason it is written down.
#[utoipa::path(post, path = "/api/v1/evaluation-assessments", request_body = AssessmentRequest,
    responses((status = 200, body = Assessment), (status = 400, body = crate::error::ErrorBody),
    (status = 403, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody),
    (status = 501, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn record_assessment(
    State(state): State<AppState>,
    caller: Caller,
    Json(request): Json<AssessmentRequest>,
) -> ApiResult<Json<Assessment>> {
    let identity = caller.require(Role::Editor)?;
    Ok(Json(
        registry(&state)?
            .assess(&request, &identity.subject, now())
            .await?,
    ))
}

/// Every standing judgement about one thing, at its current revision.
#[utoipa::path(get, path = "/api/v1/evaluation-assessments", params(AssessmentTargetQuery),
    responses((status = 200, body = AssessmentPage), (status = 400, body = crate::error::ErrorBody),
    (status = 501, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn list_assessments(
    State(state): State<AppState>,
    caller: Caller,
    Query(query): Query<AssessmentTargetQuery>,
) -> ApiResult<Json<AssessmentPage>> {
    caller.require(Role::Viewer)?;
    Ok(Json(registry(&state)?.assessments(&query.target()?).await?))
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
struct HistoryQuery {
    /// Continue below this revision.
    before: Option<u32>,
    limit: Option<u32>,
}

/// What one standing judgement said over time, newest first.
///
/// Both IDs came from the listing. A browser deriving either would be a second
/// answer to what one thing is, which is the rule an approval's address keeps.
#[utoipa::path(get, path = "/api/v1/evaluation-assessments/{target_id}/{standing_id}",
    params(("target_id" = String, Path), ("standing_id" = String, Path), HistoryQuery),
    responses((status = 200, body = AssessmentHistory), (status = 400, body = crate::error::ErrorBody),
    (status = 501, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn get_assessment_history(
    State(state): State<AppState>,
    caller: Caller,
    Path((target_id, standing_id)): Path<(String, String)>,
    Query(query): Query<HistoryQuery>,
) -> ApiResult<Json<AssessmentHistory>> {
    caller.require(Role::Viewer)?;
    Ok(Json(
        registry(&state)?
            .assessment_history(&target_id, &standing_id, query.before, query.limit)
            .await?,
    ))
}
