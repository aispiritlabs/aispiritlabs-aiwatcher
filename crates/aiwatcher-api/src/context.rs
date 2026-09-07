//! What a block was, so the panel does not have to work it out.
//!
//! Section 19, and its first sentence is the whole reason this module exists:
//! *the panel must not reconstruct upstream context*. Every part of the answer
//! is somewhere the browser is not — the pinned plan, the artifacts a parent
//! produced, the attempt a staging key is named after — so a canvas that
//! guessed would guess from the draft on screen, which is the one thing that is
//! certainly not what an old run read.
//!
//! Two routes for two questions, and their shapes differ in a way that matters:
//!
//! ```text
//! /executions/{execution}/steps/{step}/context        what it did read
//! /curation-pipelines/{name}/revisions/{rev}/blocks/{block}/context   what it would
//! ```
//!
//! The first is exact — a run pins a plan and a plan is immutable, so opening a
//! step of last week's execution shows last week's script over last week's
//! rows, whatever the definition has become since. The second has no input
//! artifacts and no state, and says so with empty fields rather than borrowing
//! the newest run's.
//!
//! ## Why this is not two modules
//!
//! A module is a facade and the thing it owns is a noun. The noun here is
//! *context*, and the two routes return one type: splitting them by path prefix
//! would put one answer in two files and give a reader two places to look when
//! a runtime gains a field.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use utoipa::OpenApi;

use aiwatcher_datasets::Registry as DatasetRegistry;
use aiwatcher_execution::compile::CompileOptions;
use aiwatcher_execution::{
    ContextSnapshot, ExecutionHandler, ExecutionId, WorkflowStore, compile_curation, replay,
};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// This module's operations, as the contract they satisfy.
#[derive(OpenApi)]
#[openapi(paths(step_context, block_context))]
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
