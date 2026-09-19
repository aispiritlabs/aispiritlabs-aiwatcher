//! The workflow graph's HTTP surface.
//!
//! Its own module for the same reason the prompt registry has one: five of
//! these routes read the log's projection like everything else, and the sixth
//! does not read anything at all. `POST …/rerun` asks an orchestrator to do
//! work. Keeping it beside `list_runs` would bury the one route in this API
//! that has an effect on the world outside aiwatcher.
//!
//! The read routes come in two levels, and the split is the feature:
//!
//! * `/workflows` is the **catalog** — the graphs a producer declared, plus the
//!   ones only ever observed. It is what a picker lists, and it answers before
//!   anything has run.
//! * `/workflow-executions` is the **traversals** — what one attempt at a graph
//!   actually did, including the nodes it has not reached yet.
//!
//! An execution is not a run, and the id in these paths is a
//! `workflow_run_id`, not a `run_id`. A stage-per-pod orchestrator gives every
//! stage its own process; the join is the whole reason these routes exist
//! rather than a filter on `/runs`.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{KeepAlive, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::stream::{Stream, StreamExt};
use serde::Deserialize;

use aiwatcher_core::ports::{RerunAccepted, RerunRequest, WorkflowRunner};
use aiwatcher_projector::{
    ExecutionDetail, ExecutionFilter, ExecutionPage, WorkflowDefinition, WorkflowFilter,
    WorkflowPage,
};

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::live::{StreamQuery, resume_point};
use crate::run_scope::RunRead;
use crate::state::AppState;
use crate::stream::{Scope, Subscription, as_sse, catch_up, live_tail};
use utoipa::OpenApi;

/// This module's operations, as the contract they satisfy.
///
/// Derived beside the router rather than listed in the root document, so
/// adding a route and forgetting the contract is a change to one file rather
/// than a change to two files that has to be noticed in the second.
#[derive(OpenApi)]
#[openapi(paths(
    list_workflows,
    get_workflow,
    list_workflow_executions,
    get_workflow_execution,
    stream_workflow_execution,
))]
struct Api;

/// This module's one instance-only operation.
///
/// Apart rather than in [`Api`] because it has **no scoped twin** and must not
/// grow one from `crate::project_scope::openapi`: a rerun is a dispatch to an
/// endpoint this deployment configured, asked for by an instance admin. See
/// [`rerun_workflow`].
#[derive(OpenApi)]
#[openapi(paths(rerun_workflow))]
struct RerunApi;

/// The operations this module serves. Composed by [`crate::openapi`].
#[must_use]
pub fn openapi() -> utoipa::openapi::OpenApi {
    let mut api = crate::project_scope::openapi(Api::openapi());
    api.merge(RerunApi::openapi());
    api
}

/// One set of routes, served twice — see [`crate::runs::router`].
///
/// The fold behind them carries the project in the row (IAM-02 E2's
/// remainder), so what differs between the two families is the side
/// [`RunRead`] resolves and nothing else.
pub fn router() -> Router<AppState> {
    Router::new()
        .nest("/api/v1", resource_router())
        .nest(
            "/api/v1/orgs/{organization}/projects/{project}",
            resource_router()
                .layer(axum::Extension(crate::project_scope::ScopedRoute))
                .layer(tower_http::set_header::SetResponseHeaderLayer::overriding(
                    axum::http::header::CACHE_CONTROL,
                    axum::http::HeaderValue::from_static("no-store"),
                )),
        )
        // Instance-only, and the one route here that is. It asks another
        // system to do work from an address this deployment configured, and
        // the role that may ask is the instance's `admin` — there is no
        // project form of either, so a scoped twin would be a project role
        // deciding an instance act.
        .route(
            "/api/v1/workflows/{workflow_id}/rerun",
            post(rerun_workflow),
        )
}

fn resource_router() -> Router<AppState> {
    Router::new()
        .route("/workflows", get(list_workflows))
        .route("/workflows/{workflow_id}", get(get_workflow))
        .route("/workflow-executions", get(list_workflow_executions))
        .route(
            "/workflow-executions/{workflow_run_id}",
            get(get_workflow_execution),
        )
        .route(
            "/workflow-executions/{workflow_run_id}/stream",
            get(stream_workflow_execution),
        )
}

/// Named rather than `Path<String>` so the scoped family's `organization` and
/// `project` need no spelling out in a handler that ignores them.
#[derive(Deserialize)]
struct WorkflowPath {
    workflow_id: String,
}

#[derive(Deserialize)]
struct ExecutionPath {
    workflow_run_id: String,
}

// ── The catalog ──────────────────────────────────────────────────────────────

/// Every workflow the projection knows, most recently active first.
///
/// Includes workflows nothing ever declared: a producer that has been setting
/// `workflow_id` since before there was a graph to draw still belongs in the
/// picker, with whatever shape its runs revealed.
#[utoipa::path(
    get,
    path = "/api/v1/workflows",
    params(WorkflowFilter),
    responses((status = 200, body = WorkflowPage)),
    tag = "workflow",
)]
async fn list_workflows(
    State(state): State<AppState>,
    read: RunRead,
    Query(filter): Query<WorkflowFilter>,
) -> Json<WorkflowPage> {
    Json(state.read_model.workflows(read.scope, &filter).await)
}

/// One workflow's declared shape.
#[utoipa::path(
    get,
    path = "/api/v1/workflows/{workflow_id}",
    params(("workflow_id" = String, Path, description = "The workflow to fetch")),
    responses(
        (status = 200, body = WorkflowDefinition),
        (status = 404, body = crate::error::ErrorBody),
    ),
    tag = "workflow",
)]
async fn get_workflow(
    State(state): State<AppState>,
    read: RunRead,
    Path(WorkflowPath { workflow_id }): Path<WorkflowPath>,
) -> ApiResult<Json<WorkflowDefinition>> {
    state
        .read_model
        .workflow(read.scope, &workflow_id)
        .await
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("workflow {workflow_id}")))
}

// ── Traversals ───────────────────────────────────────────────────────────────

/// Executions, newest first.
#[utoipa::path(
    get,
    path = "/api/v1/workflow-executions",
    params(ExecutionFilter),
    responses((status = 200, body = ExecutionPage)),
    tag = "workflow",
)]
async fn list_workflow_executions(
    State(state): State<AppState>,
    read: RunRead,
    Query(filter): Query<ExecutionFilter>,
) -> Json<ExecutionPage> {
    Json(
        state
            .read_model
            .workflow_executions(read.scope, &filter)
            .await,
    )
}

/// One execution: its nodes, its declared edges, and the messages observed.
#[utoipa::path(
    get,
    path = "/api/v1/workflow-executions/{workflow_run_id}",
    params(("workflow_run_id" = String, Path, description = "The execution to fetch")),
    responses(
        (status = 200, body = ExecutionDetail),
        (status = 404, body = crate::error::ErrorBody),
    ),
    tag = "workflow",
)]
async fn get_workflow_execution(
    State(state): State<AppState>,
    read: RunRead,
    Path(ExecutionPath { workflow_run_id }): Path<ExecutionPath>,
) -> ApiResult<Json<ExecutionDetail>> {
    state
        .read_model
        .workflow_execution(read.scope, &workflow_run_id)
        .await
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("workflow execution {workflow_run_id}")))
}

/// Server-sent events for one execution: history, a catch-up marker, then live.
///
/// Scoped by `workflow_run_id` rather than by run, which is what makes a
/// stage-per-pod workflow watchable: the pod that has not started yet is the
/// interesting one, and a subscription resolved to today's run ids would never
/// see it.
#[utoipa::path(
    get,
    path = "/api/v1/workflow-executions/{workflow_run_id}/stream",
    params(
        ("workflow_run_id" = String, Path, description = "The execution to follow"),
        StreamQuery,
    ),
    responses((status = 200, description = "text/event-stream of LiveFrame")),
    tag = "live",
)]
async fn stream_workflow_execution(
    State(state): State<AppState>,
    read: RunRead,
    Path(ExecutionPath { workflow_run_id }): Path<ExecutionPath>,
    Query(query): Query<StreamQuery>,
    headers: HeaderMap,
) -> ApiResult<Sse<impl Stream<Item = Result<axum::response::sse::Event, Infallible>>>> {
    let from = resume_point(&headers, query.from.as_deref())?;
    let subscription = Subscription {
        project: read.scope,
        scope: Scope::WorkflowRun(workflow_run_id),
    };
    let (history, boundary) = catch_up(&state, from.as_ref(), &subscription).await?;
    let tail = live_tail(&state.live, boundary, subscription);
    // Asked again every thirty seconds and closed with a `revoked` frame, as
    // every scoped stream is (ADR_0033, amended). A graph's stream lives as
    // long as the traversal it draws, which for a stage-per-pod workflow is
    // hours.
    let frames = futures::stream::iter(history).chain(crate::live::while_held(read.held, tail));

    Ok(Sse::new(as_sse(frames)).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    ))
}

// ── The one route that makes something happen ────────────────────────────────

/// What a caller may ask for.
///
/// Note what is *not* here: an endpoint. The orchestrator to talk to comes
/// from this deployment's configuration, never from the request and never from
/// the log. aiwatcher runs inside the cluster, so a caller-supplied URL is a
/// request to reach the cluster's network on that caller's behalf.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RerunBody {
    /// The execution to repeat. Omit for a fresh run of the workflow.
    #[serde(default)]
    pub workflow_run_id: Option<String>,
    /// Resume from this node. Advisory — whether an orchestrator can start
    /// mid-graph is its business, and aiwatcher cannot verify that it did.
    #[serde(default)]
    pub from_node: Option<String>,
    /// Passed through untouched.
    #[serde(default)]
    #[schema(value_type = Object)]
    pub inputs: serde_json::Value,
}

/// Ask the configured orchestrator to run this workflow again.
///
/// `202`, not `200`: nothing has run yet. What comes back is an
/// acknowledgement, and the evidence that the rerun happened is the events it
/// publishes onto the same log as everything else.
#[utoipa::path(
    post,
    path = "/api/v1/workflows/{workflow_id}/rerun",
    params(("workflow_id" = String, Path, description = "The workflow to run again")),
    request_body = RerunBody,
    responses(
        (status = 202, body = RerunAccepted),
        (status = 501, body = crate::error::ErrorBody),
        (status = 502, body = crate::error::ErrorBody),
        (status = 503, body = crate::error::ErrorBody),
    ),
    tag = "workflow",
)]
async fn rerun_workflow(
    State(state): State<AppState>,
    caller: Caller,
    Path(workflow_id): Path<String>,
    Json(body): Json<RerunBody>,
) -> ApiResult<(StatusCode, Json<RerunAccepted>)> {
    // Admin, and it is the only route in this API that needs it. Everything
    // else here reports what happened; this asks another system to make
    // something happen, inside the cluster, on the caller's behalf.
    let requester = caller
        .require(aiwatcher_auth::Role::Admin)?
        .log_subject()
        .to_owned();
    let runner = runner(&state)?;

    // Asking to rerun a workflow nothing has ever heard of is almost always a
    // typo, and dispatching it would turn that typo into a request to another
    // system. A workflow that has run is in the catalog even when nobody
    // declared it, so this refuses very little that was real.
    // The instance's own catalog: this route has no project family, so the
    // graph it refuses to dispatch for is the one an instance admin can see.
    if state
        .read_model
        .workflow(aiwatcher_projector::ReadScope::Global, &workflow_id)
        .await
        .is_none()
    {
        return Err(ApiError::NotFound(format!("workflow {workflow_id}")));
    }

    let accepted = runner
        .rerun(RerunRequest {
            workflow_id: workflow_id.clone(),
            workflow_run_id: body.workflow_run_id.clone(),
            from_node: body.from_node.clone(),
            inputs: body.inputs,
        })
        .await
        .map_err(ApiError::Runner)?;

    tracing::info!(
        %workflow_id,
        workflow_run_id = body.workflow_run_id.as_deref().unwrap_or("-"),
        from_node = body.from_node.as_deref().unwrap_or("-"),
        reference = accepted.reference.as_deref().unwrap_or("-"),
        // Who asked. The point of having identities at all: a rerun is the
        // one thing here with a consequence outside aiwatcher, and before SSO
        // this line could only ever have said "somebody".
        requested_by = %requester,
        "dispatched a workflow rerun"
    );

    Ok((StatusCode::ACCEPTED, Json(accepted)))
}

/// The runner, or a 501 explaining that this deployment has none.
fn runner(state: &AppState) -> ApiResult<&Arc<dyn WorkflowRunner>> {
    state.runner.as_ref().ok_or(ApiError::RunnerDisabled)
}
