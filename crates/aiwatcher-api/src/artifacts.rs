//! What a run produced, and what one of those says.
//!
//! ```text
//! /executions/{execution}/artifacts            everything it produced
//! /executions/{execution}/artifacts/{digest}   one of them, read as text
//! ```
//!
//! The catalog has recorded a step's outputs and a pod's log against the
//! attempt that made them since ADR_0029; what was missing was any way to read
//! that back, which made the log a thing this system kept and nobody could
//! open. These are the reader half: the first answers from
//! [`ArtifactCatalog::produced_by`] and the second reads the bytes.
//!
//! **The bytes are proxied, never presigned.** A presigned URL is a bearer
//! credential for a bucket that also holds prompts, datasets, annotations,
//! conversations and training; what this route can check instead is the one
//! thing that matters — that the digest somebody asked for is one *this
//! execution* produced. That check is also what makes the digest safe as the
//! whole address: it is not an oracle over the store, because a digest no row
//! of this run names is a 404 whatever is under it.
//!
//! **A 501 rather than an empty list** when this deployment has no object
//! store, naming the variable. Both the catalog and the store are `Some`
//! exactly when `AIWATCHER_PROMPT_STORE` is set, so their absence is one fact
//! with one fix — the prompt registry's rule, in a sixth place. An empty list
//! would be a different problem with a different fix, and a step whose log was
//! never kept would be indistinguishable from one whose log is simply not
//! there yet.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use utoipa::{OpenApi, ToSchema};

use aiwatcher_core::ArtifactRef;
use aiwatcher_core::ports::AttemptArtifacts;
use aiwatcher_execution::{ArtifactCatalog, CatalogedArtifact, ExecutionId};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// This module's operations, as the contract they satisfy.
#[derive(OpenApi)]
#[openapi(paths(run_artifacts, artifact_content))]
struct Api;

/// The operations this module serves. Composed by [`crate::openapi`].
#[must_use]
pub fn openapi() -> utoipa::openapi::OpenApi {
    Api::openapi()
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/executions/{execution_id}/artifacts",
            get(run_artifacts),
        )
        .route(
            "/api/v1/executions/{execution_id}/artifacts/{digest}",
            get(artifact_content),
        )
}

/// How much of one artifact this route will read as text.
///
/// A pod's log is bounded to 256 KiB where it is written, so this is four
/// times the only thing it is expected to be asked for. It is a bound on the
/// *declared* size as well as on what came back, because the digest is over
/// the whole object and a verified read is therefore a whole read: refusing
/// before the read is the only refusal that costs nothing.
pub const MAX_TEXT_BYTES: u64 = 1024 * 1024;

/// One artifact, read as text.
///
/// Text rather than a stream with the stored content type: everything this
/// route is for is something somebody reads, and serving bytes back under a
/// type the object store was told about is how a stored `text/html` becomes a
/// page on this origin.
#[derive(Debug, Serialize, ToSchema)]
pub struct ArtifactContent {
    /// The pointer, so a reader has the size and the name without the list.
    pub artifact: ArtifactRef,
    /// The bytes, decoded lossily. A pod's stdout is not promised to be UTF-8
    /// and a traceback is still worth reading with one byte mangled in it.
    pub text: String,
}

fn catalog(state: &AppState) -> ApiResult<&Arc<dyn ArtifactCatalog>> {
    state
        .catalog
        .as_ref()
        .ok_or(ApiError::StepArtifactsDisabled)
}

fn store(state: &AppState) -> ApiResult<&Arc<dyn AttemptArtifacts>> {
    state
        .artifacts
        .as_ref()
        .ok_or(ApiError::StepArtifactsDisabled)
}

/// Everything one run produced, in the order it was recorded.
///
/// Each row carries its `produced_by` — the execution, the step and the
/// attempt — which is what lets a step's view show its own outputs and its own
/// log without a second route per noun.
///
/// A run that produced nothing and a run that never existed are the same empty
/// list. The catalog cannot tell them apart and neither could a caller that
/// asked it to: whether a run exists is `GET /executions/{id}`, and answering
/// it twice would be a second read of a second store to describe the same
/// absence.
#[utoipa::path(
    get,
    path = "/api/v1/executions/{execution_id}/artifacts",
    params(("execution_id" = String, Path, description = "The id a start returned")),
    responses(
        (status = 200, body = Vec<CatalogedArtifact>),
        (status = 501, body = crate::error::ErrorBody),
        (status = 503, body = crate::error::ErrorBody),
    ),
    tag = "execution",
)]
async fn run_artifacts(
    State(state): State<AppState>,
    Path(execution_id): Path<String>,
) -> ApiResult<Json<Vec<CatalogedArtifact>>> {
    let produced = catalog(&state)?
        .produced_by(&ExecutionId::new(execution_id))
        .await
        .map_err(aiwatcher_execution::HandleError::Store)?;
    Ok(Json(produced))
}

/// One of this run's artifacts, read as text.
///
/// The digest is checked against what this execution produced *before*
/// anything is read, so the route answers for a run's own outputs and for
/// nothing else in the store.
#[utoipa::path(
    get,
    path = "/api/v1/executions/{execution_id}/artifacts/{digest}",
    params(
        ("execution_id" = String, Path, description = "The id a start returned"),
        ("digest" = String, Path, description = "The artifact's content address, as the list gave it"),
    ),
    responses(
        (status = 200, body = ArtifactContent),
        (status = 404, body = crate::error::ErrorBody),
        (status = 413, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
        (status = 502, body = crate::error::ErrorBody),
        (status = 503, body = crate::error::ErrorBody),
    ),
    tag = "execution",
)]
async fn artifact_content(
    State(state): State<AppState>,
    Path((execution_id, digest)): Path<(String, String)>,
) -> ApiResult<Json<ArtifactContent>> {
    // The run's own list rather than `by_digest`, which would answer for an
    // artifact any execution produced. The cost is a list read per byte read,
    // and it buys the scoping: what makes a bare digest safe as the whole
    // address is that this run has to be the one that produced it.
    let produced = catalog(&state)?
        .produced_by(&ExecutionId::new(execution_id.clone()))
        .await
        .map_err(aiwatcher_execution::HandleError::Store)?;
    let artifact = produced
        .into_iter()
        .map(|row| row.artifact)
        .find(|artifact| artifact.digest == digest)
        .ok_or_else(|| {
            ApiError::NotFound(format!("artifact {digest} of execution {execution_id}"))
        })?;

    if let Some(size) = artifact.size_bytes.filter(|size| *size > MAX_TEXT_BYTES) {
        return Err(too_large(size));
    }
    // `WorkerArtifacts` although the reader is not a worker: the sentence it
    // carries is about the object store and is the same one either way, and
    // the split it makes — unreachable is a 503 worth repeating, a refusal is
    // a 502 that will refuse identically — is the split this route needs. Only
    // the *disabled* case earns a variant of its own, because that one is read
    // by a person deciding what to set.
    let bytes = store(&state)?
        .read_bytes(&artifact)
        .await
        .map_err(ApiError::WorkerArtifacts)?;
    // Again, against what actually came back. The declared size is a field on
    // a pointer and this is the object.
    if bytes.len() as u64 > MAX_TEXT_BYTES {
        return Err(too_large(bytes.len() as u64));
    }
    Ok(Json(ArtifactContent {
        artifact,
        text: String::from_utf8_lossy(&bytes).into_owned(),
    }))
}

fn too_large(size: u64) -> ApiError {
    ApiError::TooLarge {
        what: "this artifact",
        size: usize::try_from(size).unwrap_or(usize::MAX),
        limit: usize::try_from(MAX_TEXT_BYTES).unwrap_or(usize::MAX),
    }
}
