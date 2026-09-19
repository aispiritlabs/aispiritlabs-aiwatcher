//! What a run produced, and what one of those says.
//!
//! ```text
//! /executions/{execution}/artifacts            everything it produced
//! /executions/{execution}/artifacts/{digest}   one of them, read as text
//! ```
//!
//! The reader half of what ADR_0029 has been recording: the first route
//! answers from [`ArtifactCatalog::produced_by`] and the second reads bytes.
//!
//! **The bytes are proxied, never presigned.** A presigned URL is a bearer
//! credential for a bucket that also holds prompts, datasets, annotations,
//! conversations and training; what this route checks instead is that the
//! digest asked for is one *this execution* produced. That is also what makes
//! a bare digest safe as the whole address: it is no oracle over the store,
//! because a digest no row of this run names is a 404 whatever is under it.
//!
//! **A 501 rather than an empty list** when this deployment has no object
//! store, naming the variable — the prompt registry's rule, in a sixth place.
//! An empty list is a different problem with a different fix, and a step whose
//! log was never kept would be indistinguishable from one not written yet.
//!
//! **Served twice** (ADR_0033). Before IAM-03's D4 a project member held
//! `viewer` and could read their bytes on the instance's own list; D4 takes
//! that away, so without the twin nothing would read a pod's log. Three stores
//! bind from one answer — the workflow store ([`RunHandle`], where the grant
//! is asked), the catalog and the bytes — because a manifest under one prefix
//! with its bytes under another is a row nobody can open.

use std::sync::Arc;

use axum::extract::{FromRequestParts, Path};
use axum::http::request::Parts;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use utoipa::{OpenApi, ToSchema};

use aiwatcher_core::ArtifactRef;
use aiwatcher_core::ports::AttemptArtifacts;
use aiwatcher_execution::{ArtifactCatalog, CatalogedArtifact, ExecutionId};

use crate::error::{ApiError, ApiResult};
use crate::execution_scope::RunHandle;
use crate::state::AppState;

/// This module's operations, as the contract they satisfy.
#[derive(OpenApi)]
#[openapi(paths(run_artifacts, artifact_content))]
struct Api;

/// The operations this module serves, on both route families. Composed by
/// [`crate::openapi`].
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
        .route("/executions/{execution_id}/artifacts", get(run_artifacts))
        .route(
            "/executions/{execution_id}/artifacts/{digest}",
            get(artifact_content),
        )
}

/// Named rather than `Path<String>`, so the scoped family's `organization` and
/// `project` need no spelling out in a handler that ignores them.
#[derive(Deserialize)]
struct RunPath {
    execution_id: String,
}

#[derive(Deserialize)]
struct ArtifactPath {
    execution_id: String,
    digest: String,
}

/// The three stores one run's artifacts are read through, on one side.
///
/// [`RunHandle`] resolves the workflow store and, on a project's routes, asks
/// the grant; the catalog and the bytes follow it. Binding all three from one
/// answer is what stops a read that took the run from one side and its bytes
/// from the other.
struct RunArtifacts {
    run: RunHandle,
    catalog: Arc<dyn ArtifactCatalog>,
    store: Arc<dyn AttemptArtifacts>,
}

impl RunArtifacts {
    /// Everything this execution produced, once the side has been settled.
    async fn produced_by(&self, execution: &ExecutionId) -> ApiResult<Vec<CatalogedArtifact>> {
        self.on_this_side(execution).await?;
        self.catalog
            .produced_by(execution)
            .await
            .map_err(|error| aiwatcher_execution::HandleError::Store(error).into())
    }

    /// Refuse an execution this side is not the one for.
    ///
    /// Without it the list would come back **empty** rather than refused, which
    /// reads as "this run produced nothing" and is exactly the silence
    /// ADR_0033 pt. 7 exists to prevent. The question is asked of the workflow
    /// store, which already answers it: a handle refuses the other side's
    /// execution by name and says `None` for an id nobody has used.
    async fn on_this_side(&self, execution: &ExecutionId) -> ApiResult<()> {
        // A refusal renders 404 and a bad moment renders 503; the split is
        // `execution_parts`' and is not repeated here.
        self.run
            .handler()
            .store()
            .ownership(execution)
            .await
            .map(|_| ())
            .map_err(|error| aiwatcher_execution::HandleError::Store(error).into())
    }
}

impl FromRequestParts<AppState> for RunArtifacts {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        let run = RunHandle::from_request_parts(parts, state).await?;
        let catalog = state
            .catalog
            .as_ref()
            .ok_or(ApiError::StepArtifactsDisabled)?;
        let store = state
            .artifacts
            .as_ref()
            .ok_or(ApiError::StepArtifactsDisabled)?;
        let Some(scope) = run.project_scope() else {
            return Ok(Self {
                run,
                catalog: Arc::clone(catalog),
                store: Arc::clone(store),
            });
        };
        Ok(Self {
            run,
            catalog: catalog
                .for_project(scope)
                .map_err(aiwatcher_execution::HandleError::Store)?,
            store: store
                .for_project(scope.on_the_log())
                .map_err(ApiError::WorkerArtifacts)?,
        })
    }
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
    artifacts: RunArtifacts,
    Path(RunPath { execution_id }): Path<RunPath>,
) -> ApiResult<Json<Vec<CatalogedArtifact>>> {
    let produced = artifacts
        .produced_by(&ExecutionId::new(execution_id))
        .await?;
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
    artifacts: RunArtifacts,
    Path(ArtifactPath {
        execution_id,
        digest,
    }): Path<ArtifactPath>,
) -> ApiResult<Json<ArtifactContent>> {
    // The run's own list rather than `by_digest`, which would answer for an
    // artifact any execution produced. The cost is a list read per byte read,
    // and it buys the scoping: what makes a bare digest safe as the whole
    // address is that this run has to be the one that produced it.
    let produced = artifacts
        .produced_by(&ExecutionId::new(execution_id.clone()))
        .await?;
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
    let bytes = artifacts
        .store
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
