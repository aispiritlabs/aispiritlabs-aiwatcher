//! The authored workflow catalog. Observed graphs still come from the event log.

use crate::{
    auth::Caller,
    error::{ApiError, ApiResult},
    state::AppState,
};
use aiwatcher_execution::definition::{DefinitionRegistry, SavedWorkflow, WorkflowSpec};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, post},
};
use serde::Deserialize;
use std::sync::Arc;
use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(paths(register_workflow, list_workflow_definitions, get_workflow_definition))]
struct Api;

#[must_use]
pub fn openapi() -> utoipa::openapi::OpenApi {
    Api::openapi()
}
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workflow-definitions",
            post(register_workflow).get(list_workflow_definitions),
        )
        .route(
            "/api/v1/workflow-definitions/{name}",
            get(get_workflow_definition),
        )
}

pub(crate) fn registry(state: &AppState) -> ApiResult<&Arc<DefinitionRegistry>> {
    state
        .workflow_definitions
        .as_ref()
        .ok_or(ApiError::WorkflowDefinitionsDisabled)
}

#[utoipa::path(post, path = "/api/v1/workflow-definitions", request_body = WorkflowSpec,
    responses((status = 200, body = SavedWorkflow), (status = 403, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody), (status = 501, body = crate::error::ErrorBody),
        (status = 503, body = crate::error::ErrorBody)), tag = "execution")]
async fn register_workflow(
    State(state): State<AppState>,
    caller: Caller,
    Json(body): Json<WorkflowSpec>,
) -> ApiResult<Json<SavedWorkflow>> {
    let requested_by = caller
        .require(aiwatcher_auth::Role::Editor)?
        .log_subject()
        .to_owned();
    body.compile().map_err(|error| ApiError::PlanRefused {
        summary: "workflow definition is invalid".to_owned(),
        problems: error.problems().to_vec(),
    })?;
    let saved = registry(&state)?
        .save(body, requested_by, time::OffsetDateTime::now_utc())
        .await
        .map_err(aiwatcher_execution::HandleError::Store)?;
    Ok(Json(saved))
}

#[utoipa::path(get, path = "/api/v1/workflow-definitions",
    responses((status = 200, body = Vec<SavedWorkflow>), (status = 501, body = crate::error::ErrorBody),
        (status = 503, body = crate::error::ErrorBody)), tag = "execution")]
async fn list_workflow_definitions(
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<SavedWorkflow>>> {
    Ok(Json(
        registry(&state)?
            .list()
            .await
            .map_err(aiwatcher_execution::HandleError::Store)?,
    ))
}

#[derive(Deserialize, utoipa::IntoParams)]
struct RevisionQuery {
    revision: Option<String>,
}

#[utoipa::path(get, path = "/api/v1/workflow-definitions/{name}",
    params(("name" = String, Path), RevisionQuery),
    responses((status = 200, body = SavedWorkflow), (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody), (status = 503, body = crate::error::ErrorBody)), tag = "execution")]
async fn get_workflow_definition(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<RevisionQuery>,
) -> ApiResult<Json<SavedWorkflow>> {
    registry(&state)?
        .get(&name, query.revision.as_deref())
        .await
        .map_err(aiwatcher_execution::HandleError::Store)?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("workflow definition {name}")))
}
