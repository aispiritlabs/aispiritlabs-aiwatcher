//! Derive pinned evaluation cases from native datasets in the admitted scope.
use crate::evaluation_scope::{CohortWrite, EvaluationRead};
use crate::{ApiError, AppState, Caller, error::ApiResult};
use aiwatcher_auth::Role;
use aiwatcher_evaluation::{CohortRequest, DerivedCohort};
use axum::{
    Json, Router,
    extract::Path,
    routing::{get, post},
};
use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(paths(derive_cohort, get_derived_cohort))]
struct Api;

pub(crate) fn openapi() -> utoipa::openapi::OpenApi {
    crate::project_scope::openapi(Api::openapi())
}
pub(crate) fn router() -> Router<AppState> {
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
        .route("/evaluation-cohorts", post(derive_cohort))
        .route("/evaluation-cohorts/{cases}", get(get_derived_cohort))
}
#[derive(serde::Deserialize)]
struct CohortPath {
    cases: String,
}
fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

/// Take a cohort from a dataset version this deployment owns.
///
/// The first `limit` cases of a version's split, as the owner holds them, with
/// the three files a cohort pins derived here — so a declaration names them
/// and nobody writes or stages them. Admitting the pair derives them again from
/// the owner. A conversation corpus's cases are content, so an admin takes a
/// cohort from one, as an admin reads its cases. Project routes currently support
/// curation versions and annotation exports only; conversations remain unavailable.
#[utoipa::path(post, path = "/api/v1/evaluation-cohorts", request_body = CohortRequest,
    responses((status = 200, body = DerivedCohort), (status = 400, body = crate::error::ErrorBody),
    (status = 403, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody),
    (status = 501, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn derive_cohort(
    CohortWrite(evaluation): CohortWrite,
    caller: Caller,
    Json(request): Json<CohortRequest>,
) -> ApiResult<Json<DerivedCohort>> {
    let requester = caller.identity().log_subject().to_owned();
    let (registry, content_access) = evaluation
        .authorize_with_admin(caller.require(Role::Admin).is_ok())
        .await?;
    Ok(Json(
        registry
            .as_ref()
            .clone()
            .with_content_access(content_access)
            .derive_cohort(&request, &requester, now())
            .await?,
    ))
}

/// Where the cases under one digest were derived from, if this deployment
/// derived them.
#[utoipa::path(get, path = "/api/v1/evaluation-cohorts/{cases}",
    params(("cases" = String, Path, description = "The digest of a cohort's cases")),
    responses((status = 200, body = DerivedCohort), (status = 400, body = crate::error::ErrorBody),
    (status = 404, body = crate::error::ErrorBody), (status = 501, body = crate::error::ErrorBody)),
    tag = "evaluation")]
async fn get_derived_cohort(
    EvaluationRead(registry): EvaluationRead,
    Path(CohortPath { cases }): Path<CohortPath>,
) -> ApiResult<Json<DerivedCohort>> {
    registry
        .derived_cohort(&cases)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("a cohort derived with cases {cases}")))
}
