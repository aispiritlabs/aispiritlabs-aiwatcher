//! The prompt registry's HTTP surface.
//!
//! These routes read and write an object store, because a prompt has to
//! outlive the runs that used it. The legacy and project families share
//! handlers; the extractors select authorized storage before any access.
//!
//! Two shapes are worth noticing:
//!
//! * **Publishing is a `POST` to the collection, not a `PUT` to a version.**
//!   The caller does not choose the version id — it is `sha256(text)` — so
//!   there is no address to `PUT` to until the text exists. The response says
//!   whether the version was created or already there.
//! * **Moving a label is its own route.** Storing a prompt and deploying it
//!   are different decisions, so they are different requests. `POST /prompts`
//!   with a `label` is the shorthand for doing both at once, which is what a
//!   first publish wants.

use axum::extract::{Path, Query};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use aiwatcher_core::prompts::{
    OptimizationRecord, PromptHead, PromptName, PromptVersion, PromptVersionId,
};
use aiwatcher_prompts::{OptimizationRequest, PromptFilter, PromptPage, PublishRequest, Published};

use crate::error::{ApiError, ApiResult};
use crate::prompt_scope::{PromptRead, PromptWrite};
use crate::state::AppState;
use utoipa::OpenApi;

/// This module's operations, as the contract they satisfy.
///
/// Derived beside the router rather than listed in the root document, so
/// adding a route and forgetting the contract is a change to one file rather
/// than a change to two files that has to be noticed in the second.
#[derive(OpenApi)]
#[openapi(paths(
    list_prompts,
    publish_prompt,
    get_prompt,
    get_prompt_version,
    set_prompt_label,
    record_optimization,
    get_optimization,
    rebuild_prompt,
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
        .route("/prompts", get(list_prompts).post(publish_prompt))
        .route("/prompts/{name}", get(get_prompt))
        .route(
            "/prompts/{name}/versions/{version_id}",
            get(get_prompt_version),
        )
        .route("/prompts/{name}/labels/{label}", put(set_prompt_label))
        .route("/prompts/{name}/optimizations", post(record_optimization))
        .route(
            "/prompts/{name}/optimizations/{optimization_id}",
            get(get_optimization),
        )
        .route("/prompts/{name}/rebuild", post(rebuild_prompt))
}

#[derive(Deserialize)]
struct PromptPath {
    name: String,
}
#[derive(Deserialize)]
struct VersionPath {
    name: String,
    version_id: String,
}
#[derive(Deserialize)]
struct LabelPath {
    name: String,
    label: String,
}
#[derive(Deserialize)]
struct OptimizationPath {
    name: String,
    optimization_id: String,
}

/// A path segment, as a validated name.
fn name_of(raw: &str) -> ApiResult<PromptName> {
    PromptName::parse(raw).map_err(|error| ApiError::BadRequest(error.to_string()))
}

fn version_of(raw: &str) -> ApiResult<PromptVersionId> {
    PromptVersionId::parse(raw).map_err(|error| ApiError::BadRequest(error.to_string()))
}

/// One prompt, with enough to render its page in one request.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PromptDetail {
    pub head: PromptHead,
    /// The version `production` points at, or the newest when no label has
    /// been moved. Carried with its text so the page paints without a second
    /// round trip — the text is the thing being looked at.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<PromptVersion>,
}

/// Every prompt in the registry.
#[utoipa::path(
    get,
    path = "/api/v1/prompts",
    params(PromptFilter),
    responses(
        (status = 200, body = PromptPage),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "prompts",
)]
async fn list_prompts(
    PromptRead(registry): PromptRead,
    Query(filter): Query<PromptFilter>,
) -> ApiResult<Json<PromptPage>> {
    Ok(Json(registry.list(&filter).await?))
}

/// Publish a version.
///
/// Idempotent on the text: `created` is `false` when this exact prompt was
/// already stored, and the version that comes back is the one that was there,
/// with its original author and notes intact.
#[utoipa::path(
    post,
    path = "/api/v1/prompts",
    request_body = PublishRequest,
    responses(
        (status = 201, body = Published, description = "A new version was stored"),
        (status = 200, body = Published, description = "This text was already stored"),
        (status = 400, body = crate::error::ErrorBody),
        (status = 413, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "prompts",
)]
async fn publish_prompt(
    write: PromptWrite,
    Json(request): Json<PublishRequest>,
) -> ApiResult<(axum::http::StatusCode, Json<Published>)> {
    let registry = write.authorize().await?;
    let published = registry.publish(request).await?;
    let status = if published.created {
        axum::http::StatusCode::CREATED
    } else {
        axum::http::StatusCode::OK
    };
    Ok((status, Json(published)))
}

/// One prompt: its labels, its versions, and what happened to it lately.
#[utoipa::path(
    get,
    path = "/api/v1/prompts/{name}",
    params(("name" = String, Path, description = "The prompt to fetch")),
    responses(
        (status = 200, body = PromptDetail),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "prompts",
)]
async fn get_prompt(
    PromptRead(registry): PromptRead,
    Path(PromptPath { name }): Path<PromptPath>,
) -> ApiResult<Json<PromptDetail>> {
    let name = name_of(&name)?;
    let head = registry
        .head(&name)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("prompt {name}")))?;
    let current = match head.current() {
        Some(version) => registry.version(&name, version).await?,
        None => None,
    };
    Ok(Json(PromptDetail { head, current }))
}

/// One version, with its text.
#[utoipa::path(
    get,
    path = "/api/v1/prompts/{name}/versions/{version_id}",
    params(
        ("name" = String, Path, description = "The prompt"),
        ("version_id" = String, Path, description = "sha256 of the prompt text"),
    ),
    responses(
        (status = 200, body = PromptVersion),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "prompts",
)]
async fn get_prompt_version(
    PromptRead(registry): PromptRead,
    Path(VersionPath { name, version_id }): Path<VersionPath>,
) -> ApiResult<Json<PromptVersion>> {
    let name = name_of(&name)?;
    let version = version_of(&version_id)?;
    registry
        .version(&name, &version)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("version {version_id} of prompt {name}")))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct LabelRequest {
    pub version_id: PromptVersionId,
}

/// Point a label at a version. `production` is the one an SDK reads.
///
/// A `PUT`, because moving a label twice to the same version is the same
/// world as moving it once.
#[utoipa::path(
    put,
    path = "/api/v1/prompts/{name}/labels/{label}",
    params(
        ("name" = String, Path, description = "The prompt"),
        ("label" = String, Path, description = "The label to move, e.g. production"),
    ),
    request_body = LabelRequest,
    responses(
        (status = 200, body = PromptHead),
        (status = 404, body = crate::error::ErrorBody, description = "No such prompt, or no such version"),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "prompts",
)]
async fn set_prompt_label(
    write: PromptWrite,
    Path(LabelPath { name, label }): Path<LabelPath>,
    Json(request): Json<LabelRequest>,
) -> ApiResult<Json<PromptHead>> {
    let registry = write.authorize().await?;
    let name = name_of(&name)?;
    Ok(Json(
        registry
            .set_label(&name, &label, &request.version_id)
            .await?,
    ))
}

/// Record an optimisation and store its candidate.
///
/// The verdict in the response is computed here from the held-out scores and
/// from what the candidate did to the baseline's variables — it is not read
/// from the request. An optimiser selected its candidate by maximising the
/// number it is reporting, which makes it the last thing that should grade it.
#[utoipa::path(
    post,
    path = "/api/v1/prompts/{name}/optimizations",
    params(("name" = String, Path, description = "The prompt that was optimised")),
    request_body = OptimizationRequest,
    responses(
        (status = 201, body = OptimizationRecord),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody, description = "The baseline version is not in the registry"),
        (status = 413, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "prompts",
)]
async fn record_optimization(
    write: PromptWrite,
    Path(PromptPath { name }): Path<PromptPath>,
    Json(request): Json<OptimizationRequest>,
) -> ApiResult<(axum::http::StatusCode, Json<OptimizationRecord>)> {
    let registry = write.authorize().await?;
    let name = name_of(&name)?;
    let record = registry.record_optimization(&name, request).await?;
    Ok((axum::http::StatusCode::CREATED, Json(record)))
}

/// One optimisation, with the report the optimiser attached.
#[utoipa::path(
    get,
    path = "/api/v1/prompts/{name}/optimizations/{optimization_id}",
    params(
        ("name" = String, Path, description = "The prompt"),
        ("optimization_id" = String, Path, description = "The optimisation to fetch"),
    ),
    responses(
        (status = 200, body = OptimizationRecord),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "prompts",
)]
async fn get_optimization(
    PromptRead(registry): PromptRead,
    Path(OptimizationPath {
        name,
        optimization_id,
    }): Path<OptimizationPath>,
) -> ApiResult<Json<OptimizationRecord>> {
    let name = name_of(&name)?;
    registry
        .optimization(&name, &optimization_id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("optimisation {optimization_id}")))
}

/// Re-derive the prompt's index from the objects that are stored.
///
/// The repair path, exposed because the thing it repairs — a head that lost a
/// concurrent write — is invisible until somebody notices a version missing
/// from a list. Labels survive it; a label pointing at a version that is gone
/// does not.
#[utoipa::path(
    post,
    path = "/api/v1/prompts/{name}/rebuild",
    params(("name" = String, Path, description = "The prompt to re-index")),
    responses(
        (status = 200, body = PromptHead),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "prompts",
)]
async fn rebuild_prompt(
    write: PromptWrite,
    Path(PromptPath { name }): Path<PromptPath>,
) -> ApiResult<Json<PromptHead>> {
    let registry = write.authorize().await?;
    let name = name_of(&name)?;
    Ok(Json(registry.rebuild(&name).await?))
}
