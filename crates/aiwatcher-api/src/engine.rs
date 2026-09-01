//! The pipeline engine's HTTP surface: what can be started, and starting it.
//!
//! Its own module for the reason [`crate::workflows`] is: one of these routes
//! makes something happen outside aiwatcher. The other three read an
//! orchestrator's inventory, which is the only thing in this API that is
//! neither the log nor an authored store.
//!
//! ## Why this is not `/api/v1/workflows`
//!
//! `/workflows` is the catalog of graphs this instance has *seen*, folded from
//! `workflow.declared`. `/engine/workflows` is the catalog of things the
//! orchestrator can *start*. The two sets overlap by coincidence: a workflow
//! declared last week may have been deleted from the engine, and a launch plan
//! registered this morning has published nothing. Serving both from one route
//! would produce a picker that offers what cannot run and hides what has never
//! run — and neither list could then say which it was.
//!
//! ## Why a launch needs `admin`
//!
//! The same reason the rerun does, and it is the only other route here that
//! needs it. Everything else in this API reports what happened; these two ask
//! another system to make something happen, inside the cluster, on the
//! caller's behalf. An ingest token is deliberately capped at editor, so a
//! leaked agent environment cannot start a training run.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;

use aiwatcher_core::engine::{
    CatalogQuery, EngineCatalog, EngineDescription, EngineExecution, EngineRef, EngineWorkflow,
    LaunchAccepted, LaunchRequest, PipelineStage, WorkflowEngine,
};

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// What one catalog page may cost.
///
/// The engine adapter reads one entity per row, so a large page is a large
/// number of requests to somebody else's control plane. Twenty is a screen.
const DEFAULT_LIMIT: usize = 20;
const MAX_LIMIT: usize = 100;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/engine", get(describe_engine))
        .route("/api/v1/engine/workflows", get(list_engine_workflows))
        .route(
            "/api/v1/engine/workflows/{workflow_id}",
            get(get_engine_workflow),
        )
        .route("/api/v1/engine/launches", post(launch_workflow))
        .route("/api/v1/engine/launches/{reference}", get(get_launch))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct CatalogParams {
    /// Case-insensitive substring over name and description.
    pub search: Option<String>,
    /// Overrides the configured project and domain for this request.
    pub project: Option<String>,
    pub domain: Option<String>,
    /// `curation | training | evaluation | inference`. A hint the engine
    /// derived from the entity's name — see `core::engine::PipelineStage`.
    pub stage: Option<String>,
    pub limit: Option<usize>,
    /// The engine's own continuation token, from a previous `next_token`.
    pub token: Option<String>,
}

/// What a caller may ask to start.
///
/// Note what is not here, and it is the same absence as `RerunBody`: no
/// endpoint, no image, no command. `deny_unknown_fields` so an attempt to
/// supply one is a 400 rather than a field that is silently ignored and reads
/// as accepted.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LaunchBody {
    /// The engine reference from the catalog, e.g.
    /// `lp:planner:production:house_dataset_curation:v7`.
    pub workflow: String,
    /// Parameter name to value, bound to the types the engine declares.
    #[serde(default)]
    #[schema(value_type = Object)]
    pub inputs: std::collections::BTreeMap<String, serde_json::Value>,
    /// Supply one to join this execution to events a producer will publish
    /// under an id it already knows. Left out, aiwatcher mints one and returns
    /// it, which is what the panel follows.
    #[serde(default)]
    pub workflow_run_id: Option<String>,
}

fn engine(state: &AppState) -> ApiResult<&Arc<dyn WorkflowEngine>> {
    state.engine.as_ref().ok_or(ApiError::EngineDisabled)
}

/// Which engine this instance can reach, if any.
///
/// A 501 rather than an empty body when none is configured, so a client can
/// tell "this deployment has no orchestrator" from "the orchestrator has
/// nothing to run" — different problems with different fixes.
#[utoipa::path(
    get,
    path = "/api/v1/engine",
    responses(
        (status = 200, body = EngineDescription),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "engine",
)]
pub async fn describe_engine(State(state): State<AppState>) -> ApiResult<Json<EngineDescription>> {
    Ok(Json(engine(&state)?.describe()))
}

/// One page of what the engine could start.
#[utoipa::path(
    get,
    path = "/api/v1/engine/workflows",
    params(CatalogParams),
    responses(
        (status = 200, body = EngineCatalog),
        (status = 400, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
        (status = 502, body = crate::error::ErrorBody),
        (status = 503, body = crate::error::ErrorBody),
    ),
    tag = "engine",
)]
pub async fn list_engine_workflows(
    State(state): State<AppState>,
    Query(params): Query<CatalogParams>,
) -> ApiResult<Json<EngineCatalog>> {
    let stage = params
        .stage
        .as_deref()
        .filter(|stage| !stage.is_empty())
        .map(str::parse::<PipelineStage>)
        .transpose()
        .map_err(ApiError::BadRequest)?;
    let catalog = engine(&state)?
        .catalog(&CatalogQuery {
            search: params.search,
            project: params.project,
            domain: params.domain,
            stage,
            limit: params.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT),
            token: params.token,
        })
        .await
        .map_err(ApiError::Engine)?;
    Ok(Json(catalog))
}

/// One entity and the inputs it declares.
///
/// Read again at launch time by the adapter, so this is what a form renders
/// and never what a launch is validated against — a panel open since before a
/// redeploy is showing an interface that no longer exists.
#[utoipa::path(
    get,
    path = "/api/v1/engine/workflows/{workflow_id}",
    params(("workflow_id" = String, Path, description = "An engine reference, e.g. lp:project:domain:name:version")),
    responses(
        (status = 200, body = EngineWorkflow),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "engine",
)]
pub async fn get_engine_workflow(
    State(state): State<AppState>,
    Path(workflow_id): Path<String>,
) -> ApiResult<Json<EngineWorkflow>> {
    let reference: EngineRef =
        workflow_id
            .parse()
            .map_err(|error: aiwatcher_core::engine::BadEngineRef| {
                ApiError::BadRequest(error.to_string())
            })?;
    engine(&state)?
        .workflow(&reference)
        .await
        .map_err(ApiError::Engine)?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("engine workflow {workflow_id}")))
}

/// Start one.
///
/// `202`, not `200`: nothing has run yet. What comes back is an
/// acknowledgement carrying the engine's reference, a link into its console,
/// and the `workflow_run_id` the events this execution publishes are expected
/// to arrive under — which is what lets the panel open a live stream for work
/// that has not started.
#[utoipa::path(
    post,
    path = "/api/v1/engine/launches",
    request_body = LaunchBody,
    responses(
        (status = 202, body = LaunchAccepted),
        (status = 400, body = crate::error::ErrorBody, description = "The engine refused it: an undeclared input, a missing one, a value that will not bind"),
        (status = 403, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
        (status = 502, body = crate::error::ErrorBody),
        (status = 503, body = crate::error::ErrorBody),
    ),
    tag = "engine",
)]
pub async fn launch_workflow(
    State(state): State<AppState>,
    caller: Caller,
    Json(body): Json<LaunchBody>,
) -> ApiResult<(StatusCode, Json<LaunchAccepted>)> {
    // Admin, for the same reason the rerun is: this is aiwatcher asking
    // another system to do work inside the cluster on the caller's behalf.
    let requester = caller
        .require(aiwatcher_auth::Role::Admin)?
        .log_subject()
        .to_owned();
    let engine = engine(&state)?;

    // Minted here rather than in the adapter, because the caller needs it in
    // the response whether or not the engine chose to carry it: it is the id
    // the panel subscribes to. A hyphen-free rendering keeps it inside what a
    // Kubernetes label — which is what it becomes on the execution — accepts.
    let workflow_run_id = body
        .workflow_run_id
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| uuid::Uuid::now_v7().simple().to_string());

    let accepted = engine
        .launch(LaunchRequest {
            workflow: body.workflow.clone(),
            inputs: body.inputs,
            workflow_run_id: Some(workflow_run_id),
            requested_by: requester.clone(),
        })
        .await
        // A refusal here is the request being wrong, not the gateway being
        // broken: the adapter binds inputs to the types the engine declares
        // and refuses what does not fit before anything leaves the process,
        // and Flyte's own 4xx means the same thing. Either way the message is
        // what a form shows beside the field.
        .map_err(|error| match error {
            aiwatcher_core::ports::PortError::Rejected { message, .. } => {
                ApiError::LaunchRefused(message)
            }
            other => ApiError::Engine(other),
        })?;

    tracing::info!(
        workflow = %body.workflow,
        reference = %accepted.reference,
        workflow_run_id = accepted.workflow_run_id.as_deref().unwrap_or("-"),
        // Who asked. A launch is one of two things here with a consequence
        // outside aiwatcher, and before SSO this line could only have said
        // "somebody".
        requested_by = %requester,
        "launched a pipeline workflow"
    );

    Ok((StatusCode::ACCEPTED, Json(accepted)))
}

/// Where one launched execution has got to, as the engine sees it.
///
/// A second opinion, not the truth: aiwatcher's own view comes from the log
/// and answers a different question. When they disagree the disagreement is
/// the finding — an execution the engine calls `succeeded` that published no
/// events is a producer nobody instrumented.
#[utoipa::path(
    get,
    path = "/api/v1/engine/launches/{reference}",
    params(("reference" = String, Path, description = "The reference from a launch, e.g. project:domain:execution")),
    responses(
        (status = 200, body = EngineExecution),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "engine",
)]
pub async fn get_launch(
    State(state): State<AppState>,
    Path(reference): Path<String>,
) -> ApiResult<Json<EngineExecution>> {
    engine(&state)?
        .execution(&reference)
        .await
        .map_err(ApiError::Engine)?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("engine execution {reference}")))
}
