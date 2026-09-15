//! Rubrics, and the typed judgements made under them.
//!
//! A score is only readable beside the form it was given on, so the form is a
//! resource of its own and an assessment names the version of it. Both belong
//! to Evaluation and are served from its store: a judgement about a case is
//! meaningless beside a result somebody else holds.

use aiwatcher_evaluation::{
    Assessment, AssessmentHistory, AssessmentPage, AssessmentRequest, AssessmentTargetQuery,
    Rubric, RubricPage, RubricVersion,
};
use axum::extract::{Path, Query};
use axum::routing::get;
use axum::{Json, Router};
use utoipa::OpenApi;

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::evaluation_scope::{EvaluationRead, EvaluationWrite};
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
            "/evaluation-rubrics",
            get(list_rubrics).post(publish_rubric),
        )
        .route("/evaluation-rubrics/{name}", get(get_rubric))
        .route(
            "/evaluation-assessments",
            get(list_assessments).post(record_assessment),
        )
        .route(
            "/evaluation-assessments/{target_id}/{standing_id}",
            get(get_assessment_history),
        )
}

#[derive(serde::Deserialize)]
struct AssessmentPath {
    target_id: String,
    standing_id: String,
}

#[derive(serde::Deserialize)]
struct NamedPath {
    name: String,
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
    write: EvaluationWrite,
    caller: Caller,
    Json(rubric): Json<Rubric>,
) -> ApiResult<Json<RubricVersion>> {
    let registry = write.authorize().await?;
    let identity = &caller.0;
    Ok(Json(
        registry
            .publish_rubric(&rubric, &identity.subject, now())
            .await?,
    ))
}

/// Every form this instance holds, at its current version.
#[utoipa::path(get, path = "/api/v1/evaluation-rubrics",
    responses((status = 200, body = RubricPage), (status = 501, body = crate::error::ErrorBody)),
    tag = "evaluation")]
async fn list_rubrics(EvaluationRead(registry): EvaluationRead) -> ApiResult<Json<RubricPage>> {
    Ok(Json(RubricPage {
        rubrics: registry.rubrics().await?,
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
    EvaluationRead(registry): EvaluationRead,
    Path(NamedPath { name }): Path<NamedPath>,
    Query(query): Query<VersionQuery>,
) -> ApiResult<Json<RubricVersion>> {
    registry
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
///
/// Safe to send twice: repeating what the current revision already says lands
/// on that revision rather than recording a second one.
#[utoipa::path(post, path = "/api/v1/evaluation-assessments", request_body = AssessmentRequest,
    responses((status = 200, body = Assessment), (status = 400, body = crate::error::ErrorBody),
    (status = 403, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody),
    (status = 501, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn record_assessment(
    write: EvaluationWrite,
    caller: Caller,
    Json(request): Json<AssessmentRequest>,
) -> ApiResult<Json<Assessment>> {
    let registry = write.authorize().await?;
    let identity = &caller.0;
    Ok(Json(
        registry.assess(&request, &identity.subject, now()).await?,
    ))
}

/// Every standing judgement about one thing, at its current revision.
#[utoipa::path(get, path = "/api/v1/evaluation-assessments", params(AssessmentTargetQuery),
    responses((status = 200, body = AssessmentPage), (status = 400, body = crate::error::ErrorBody),
    (status = 501, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn list_assessments(
    EvaluationRead(registry): EvaluationRead,
    Query(query): Query<AssessmentTargetQuery>,
) -> ApiResult<Json<AssessmentPage>> {
    Ok(Json(registry.assessments(&query.target()?).await?))
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
    EvaluationRead(registry): EvaluationRead,
    Path(AssessmentPath {
        target_id,
        standing_id,
    }): Path<AssessmentPath>,
    Query(query): Query<HistoryQuery>,
) -> ApiResult<Json<AssessmentHistory>> {
    Ok(Json(
        registry
            .assessment_history(&target_id, &standing_id, query.before, query.limit)
            .await?,
    ))
}
