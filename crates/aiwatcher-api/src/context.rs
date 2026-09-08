//! What a block was, so the panel does not have to work it out.
//!
//! Section 19, and its first sentence is the whole reason this module exists:
//! *the panel must not reconstruct upstream context*. Every part of the answer
//! is somewhere the browser is not — the pinned plan, the artifacts a parent
//! produced, the attempt a staging key is named after — so a canvas that
//! guessed would guess from the draft on screen, which is the one thing that is
//! certainly not what an old run read.
//!
//! Three routes for three questions, and their shapes differ in ways that
//! matter:
//!
//! ```text
//! /executions/{execution}/steps/{step}/context        what it did read
//! /curation-pipelines/{name}/revisions/{rev}/blocks/{block}/context   what it would
//! /executions/{execution}/blocks                      which blocks became which steps
//! ```
//!
//! The first is exact — a run pins a plan and a plan is immutable, so opening a
//! step of last week's execution shows last week's script over last week's
//! rows, whatever the definition has become since. The second has no input
//! artifacts and no state, and says so with empty fields rather than borrowing
//! the newest run's.
//!
//! The third is the same rule at the grain of a whole run, and it is what lets
//! a canvas light up. A run reports `step.started` for a *step*; a person is
//! looking at *blocks*, and three of them fold into one Flow query. Working
//! that out in the browser would mean working it out from the draft on screen,
//! which is certainly not what the run compiled — so the answer carries the
//! authored revision the plan was compiled from, and a canvas whose draft was
//! saved as a different one shows drift instead of borrowing this run's states.
//!
//! ## Why this is not two modules
//!
//! A module is a facade and the thing it owns is a noun. The noun here is
//! *context*, and the two routes return one type: splitting them by path prefix
//! would put one answer in two files and give a reader two places to look when
//! a runtime gains a field.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use utoipa::OpenApi;

use aiwatcher_core::ports::{EditorRequest, EditorSession};
use aiwatcher_datasets::Registry as DatasetRegistry;
use aiwatcher_execution::compile::CompileOptions;
use aiwatcher_execution::plan::{
    DefinitionKind, DefinitionRevision, PlanId, RuntimeBinding, StepBlocks,
};
use aiwatcher_execution::{
    ContextSnapshot, ExecutionHandler, ExecutionId, WorkflowStore, compile_curation, replay,
};

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// This module's operations, as the contract they satisfy.
#[derive(OpenApi)]
#[openapi(paths(step_context, block_context, run_blocks, open_editor))]
struct Api;

/// The operations this module serves. Composed by [`crate::openapi`].
#[must_use]
pub fn openapi() -> utoipa::openapi::OpenApi {
    Api::openapi()
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/executions/{execution_id}/steps/{step_id}/context",
            get(step_context),
        )
        .route(
            "/api/v1/curation-pipelines/{name}/revisions/{revision}/blocks/{block_id}/context",
            get(block_context),
        )
        .route("/api/v1/executions/{execution_id}/blocks", get(run_blocks))
        .route(
            "/api/v1/executions/{execution_id}/steps/{step_id}/editor",
            post(open_editor),
        )
}

/// Which authored blocks a run's steps cover, and what they were authored as.
///
/// Every field comes from the **pinned plan**. `definition_revision` is what
/// makes the rest safe to draw with: a canvas holding a different revision is
/// holding different blocks, and lighting them from these step ids would be
/// borrowing one run's outcome for another run's drawing.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct RunBlocks {
    pub definition_kind: DefinitionKind,
    pub definition_name: String,
    /// The authored revision this run compiled from. Compare it with the one
    /// the canvas was loaded at; unequal means drift, and drift means the
    /// states below belong to blocks that are not the ones on screen.
    pub definition_revision: DefinitionRevision,
    /// The compiled plan, addressed over the executable fields only. Two
    /// revisions that differ by a dragged block share this.
    pub plan_id: PlanId,
    pub steps: Vec<StepBlocks>,
}

fn handler(state: &AppState) -> ApiResult<&Arc<ExecutionHandler<Arc<dyn WorkflowStore>>>> {
    state
        .executions
        .as_ref()
        .ok_or(ApiError::ExecutionsDisabled)
}

fn definitions(state: &AppState) -> ApiResult<&Arc<DatasetRegistry>> {
    state
        .datasets
        .as_ref()
        .ok_or(ApiError::DatasetRegistryDisabled)
}

/// One step of one run, exactly as it was.
///
/// Read from the stream rather than from the definition: the run pinned a plan,
/// and the definition may have moved on several revisions since. That is the
/// difference between reopening a block and reopening *this* block.
#[utoipa::path(
    get,
    path = "/api/v1/executions/{execution_id}/steps/{step_id}/context",
    params(
        ("execution_id" = String, Path, description = "The id a start returned"),
        ("step_id" = String, Path, description = "A step of that run's pinned plan"),
    ),
    responses(
        (status = 200, body = ContextSnapshot),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
        (status = 503, body = crate::error::ErrorBody),
    ),
    tag = "execution",
)]
async fn step_context(
    State(state): State<AppState>,
    Path((execution_id, step_id)): Path<(String, String)>,
) -> ApiResult<Json<ContextSnapshot>> {
    let execution = ExecutionId::new(execution_id.clone());
    let slice = handler(&state)?
        .store()
        .load(&execution)
        .await
        .map_err(aiwatcher_execution::HandleError::Store)?;
    let state_of = replay(slice.events());
    let run = state_of
        .active()
        .ok_or_else(|| ApiError::NotFound(format!("execution {execution_id}")))?;

    ContextSnapshot::of_step(run, &step_id)
        .map(Json)
        .ok_or_else(|| {
            // The plan is the run's, so a step it does not have is a name that
            // never ran here — not a step somebody has yet to reach.
            ApiError::NotFound(format!("step {step_id} of execution {execution_id}"))
        })
}

/// One authored block of one revision, as it would run.
///
/// The revision is pinned in the path and never defaulted to the head: opening
/// a block of a saved pipeline means the pipeline as it was saved, and reading
/// the head instead would be the same answer for every revision.
#[utoipa::path(
    get,
    path = "/api/v1/curation-pipelines/{name}/revisions/{revision}/blocks/{block_id}/context",
    params(
        ("name" = String, Path, description = "The saved pipeline"),
        ("revision" = String, Path, description = "The immutable revision to read"),
        ("block_id" = String, Path, description = "A block on that revision's canvas"),
    ),
    responses(
        (status = 200, body = ContextSnapshot),
        (status = 404, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody, description = "The revision does not compile; every reason is in `details`"),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "data-curation",
)]
async fn block_context(
    State(state): State<AppState>,
    Path((name, revision, block_id)): Path<(String, String, String)>,
) -> ApiResult<Json<ContextSnapshot>> {
    let pipeline = definitions(&state)?
        .pipeline(&name, Some(&revision))
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("pipeline {name} at {revision}")))?;

    // Compiled with no window, because a revision has no run and therefore no
    // resolved bounds. The consequence is stated on `ContextSnapshot::plan_id`:
    // this is the plan the definition compiles to, not the one a run pinned.
    let plan = compile_curation(&pipeline, CompileOptions::default()).map_err(|error| {
        ApiError::PlanRefused {
            summary: format!("{name} at {revision} does not compile"),
            problems: error.problems().to_vec(),
        }
    })?;

    ContextSnapshot::of_block(&plan, &block_id)
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("block {block_id} of {name} at {revision}")))
}

/// Which authored blocks became which steps, for the run the canvas is following.
///
/// One request per execution rather than one per frame: a plan is immutable, so
/// this answer cannot change while the run does. The states themselves come
/// from `GET /executions/{id}`, which is re-read on every frame.
#[utoipa::path(
    get,
    path = "/api/v1/executions/{execution_id}/blocks",
    params(("execution_id" = String, Path, description = "The id a start returned")),
    responses(
        (status = 200, body = RunBlocks),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
        (status = 503, body = crate::error::ErrorBody),
    ),
    tag = "execution",
)]
async fn run_blocks(
    State(state): State<AppState>,
    Path(execution_id): Path<String>,
) -> ApiResult<Json<RunBlocks>> {
    let execution = ExecutionId::new(execution_id.clone());
    let slice = handler(&state)?
        .store()
        .load(&execution)
        .await
        .map_err(aiwatcher_execution::HandleError::Store)?;
    let state_of = replay(slice.events());
    let run = state_of
        .active()
        .ok_or_else(|| ApiError::NotFound(format!("execution {execution_id}")))?;

    Ok(Json(RunBlocks {
        definition_kind: run.plan.definition_kind,
        definition_name: run.plan.definition_name.clone(),
        definition_revision: run.plan.revision.clone(),
        plan_id: run.plan.plan_id.clone(),
        steps: run.plan.blocks_by_step(),
    }))
}

/// Open this step's editor on the rows this step actually read.
///
/// §16.3, resolved server-side and without a token — see
/// `aiwatcher_core::ports::EditorHost` for why the token it described would
/// have been ceremony. The gate is here: this route decides who may open which
/// run's rows, reads them from the object store itself, and hands the runtime
/// a staging request. `Editor`, not `Viewer`, because staging replaces what
/// every other person looking at that notebook's live app is shown.
///
/// It stages and stops. And the live app serves the notebook's *head*, so what
/// opens is this run's rows under today's code — `code_revision` in the answer
/// is what ran, read beside it through the runtime's revision route.
#[utoipa::path(
    post,
    path = "/api/v1/executions/{execution_id}/steps/{step_id}/editor",
    params(
        ("execution_id" = String, Path, description = "The id a start returned"),
        ("step_id" = String, Path, description = "A step of that run's plan"),
    ),
    responses(
        (status = 200, body = EditorSession),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody, description = "That step is not a notebook"),
        (status = 501, body = crate::error::ErrorBody),
        (status = 502, body = crate::error::ErrorBody),
        (status = 503, body = crate::error::ErrorBody),
    ),
    tag = "execution",
)]
async fn open_editor(
    State(state): State<AppState>,
    caller: Caller,
    Path((execution_id, step_id)): Path<(String, String)>,
) -> ApiResult<Json<EditorSession>> {
    caller.require(aiwatcher_auth::Role::Editor)?;
    let host = state
        .editor
        .as_ref()
        .ok_or(ApiError::EditorDisabled)?
        .clone();

    let execution = ExecutionId::new(execution_id.clone());
    let slice = handler(&state)?
        .store()
        .load(&execution)
        .await
        .map_err(aiwatcher_execution::HandleError::Store)?;
    let state_of = replay(slice.events());
    let run = state_of
        .active()
        .ok_or_else(|| ApiError::NotFound(format!("execution {execution_id}")))?;

    let context = ContextSnapshot::of_step(run, &step_id)
        .ok_or_else(|| ApiError::NotFound(format!("step {step_id} of execution {execution_id}")))?;
    // Only a notebook has an editor. A 422 rather than a 404: the step is
    // there, and "that step is not a notebook" is a different sentence from
    // "there is no such step".
    let RuntimeBinding::Marimo(spec) = &context.runtime else {
        return Err(ApiError::BadRequest(format!(
            "step {step_id} runs {} and has no notebook editor",
            context.runtime.kind().as_str()
        )));
    };

    let session = host
        .open(EditorRequest {
            context_id: context.context_id.clone(),
            notebook: spec.notebook.clone(),
            code_revision: spec.code_revision.clone(),
            params: serde_json::to_value(&spec.params).unwrap_or_default(),
            // The rows a parent produced, and never the newest ones: the point
            // of opening this is to see what *this* attempt read.
            input: context.input_artifacts.first().cloned(),
        })
        .await
        .map_err(ApiError::Editor)?;

    Ok(Json(session))
}
