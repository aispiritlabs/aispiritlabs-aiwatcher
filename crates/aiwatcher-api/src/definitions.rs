//! The authored workflow catalog. Observed graphs still come from the event log.

use crate::{
    auth::Caller,
    definition_scope::{DefinitionRead, DefinitionWrite},
    error::{ApiError, ApiResult},
    state::AppState,
};
use aiwatcher_execution::definition::{SavedWorkflow, WorkflowSpec};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, post},
};
use serde::Deserialize;
use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(paths(register_workflow, list_workflow_definitions, get_workflow_definition))]
struct Api;

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
            "/workflow-definitions",
            post(register_workflow).get(list_workflow_definitions),
        )
        .route("/workflow-definitions/{name}", get(get_workflow_definition))
}

#[utoipa::path(post, path = "/api/v1/workflow-definitions", request_body = WorkflowSpec,
    responses((status = 200, body = SavedWorkflow), (status = 403, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody), (status = 500, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody), (status = 502, body = crate::error::ErrorBody),
        (status = 503, body = crate::error::ErrorBody)), tag = "execution")]
async fn register_workflow(
    State(state): State<AppState>,
    write: DefinitionWrite,
    caller: Caller,
    Json(body): Json<WorkflowSpec>,
) -> ApiResult<Json<SavedWorkflow>> {
    let registry = write.authorize().await?;
    let requested_by = caller.identity().log_subject().to_owned();
    let plan = body.compile().map_err(|error| ApiError::PlanRefused {
        summary: "workflow definition is invalid".to_owned(),
        problems: error.problems().to_vec(),
    })?;
    // What only this deployment can answer: whether it has the template a step
    // names, lists its image and allows what it asks for. Refused before
    // anything is stored, every problem at once. The launcher asks again,
    // because the file is configuration and can change after this (ADR_0029).
    let refused = aiwatcher_execution::pods::refusals(&plan, state.pod_templates.as_deref());
    if !refused.is_empty() {
        return Err(ApiError::PlanRefused {
            summary: "workflow definition asks for a pod this deployment does not allow".to_owned(),
            problems: refused,
        });
    }
    let saved = registry
        .save(body, requested_by, time::OffsetDateTime::now_utc())
        .await?;
    Ok(Json(saved))
}

#[utoipa::path(get, path = "/api/v1/workflow-definitions",
    responses((status = 200, body = Vec<SavedWorkflow>), (status = 500, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody), (status = 502, body = crate::error::ErrorBody),
        (status = 503, body = crate::error::ErrorBody)), tag = "execution")]
async fn list_workflow_definitions(
    DefinitionRead(registry): DefinitionRead,
) -> ApiResult<Json<Vec<SavedWorkflow>>> {
    Ok(Json(registry.list().await?))
}

#[derive(Deserialize)]
struct NamedPath {
    name: String,
}

#[derive(Deserialize, utoipa::IntoParams)]
struct RevisionQuery {
    revision: Option<String>,
}

#[utoipa::path(get, path = "/api/v1/workflow-definitions/{name}",
    params(("name" = String, Path), RevisionQuery),
    responses((status = 200, body = SavedWorkflow), (status = 404, body = crate::error::ErrorBody),
        (status = 500, body = crate::error::ErrorBody), (status = 501, body = crate::error::ErrorBody),
        (status = 502, body = crate::error::ErrorBody), (status = 503, body = crate::error::ErrorBody)), tag = "execution")]
async fn get_workflow_definition(
    DefinitionRead(registry): DefinitionRead,
    Path(NamedPath { name }): Path<NamedPath>,
    Query(query): Query<RevisionQuery>,
) -> ApiResult<Json<SavedWorkflow>> {
    registry
        .get(&name, query.revision.as_deref())
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("workflow definition {name}")))
}
