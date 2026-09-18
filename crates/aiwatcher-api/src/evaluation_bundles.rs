//! Staged approval files, before evidence admission or execution.
use crate::bundle_scope::{BundleRead, BundleWrite};
use crate::{AppState, error::ApiResult};
use aiwatcher_evaluation::StagedFile;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path},
    routing::{get, put},
};
use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(paths(stage_bundle, list_bundle, discard_bundle))]
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
        .route(
            "/evaluation-approvals/{approval_id}/bundle",
            get(list_bundle).delete(discard_bundle),
        )
        .route(
            "/evaluation-approvals/{approval_id}/bundle/{*name}",
            put(stage_bundle).layer(DefaultBodyLimit::max(100 * 1024 * 1024)),
        )
}
#[derive(serde::Deserialize)]
struct BundlePath {
    approval_id: String,
}
#[derive(serde::Deserialize)]
struct MemberPath {
    approval_id: String,
    name: String,
}

// ── The bytes an approval admits a pair by ───────────────────────────────────

/// Stage one member of a bundle, by the name the declaration gives it.
///
/// This is what lets a *new* pair be admitted with nobody on the server's
/// host. Admin, because approving is: an ingest token is an editor by
/// construction, and bytes that decide what may be published are the operator's
/// act rather than the producer's.
///
/// It admits nothing on its own. The approval that follows resolves the whole
/// bundle and records its digest, so bytes staged after one do not widen it —
/// they stop that pair reading until they are what was admitted again.
#[utoipa::path(put, path = "/api/v1/evaluation-approvals/{approval_id}/bundle/{name}",
    params(("approval_id" = String, Path), ("name" = String, Path,
        description = "One member: `manifest.json`, `scorer.py`, `model-artifacts/<file>`")),
    request_body = Vec<u8>,
    responses((status = 200, body = StagedFile), (status = 403, body = crate::error::ErrorBody),
    (status = 501, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn stage_bundle(
    bundles: BundleWrite,
    Path(MemberPath { approval_id, name }): Path<MemberPath>,
    body: axum::body::Bytes,
) -> ApiResult<Json<StagedFile>> {
    Ok(Json(
        bundles
            .authorize()
            .await?
            .stage(&approval_id, &name, body.to_vec())
            .await?,
    ))
}

/// What is staged for one pair. Names and sizes: the digests are the
/// declaration's, and a second copy of them here could only disagree.
#[utoipa::path(get, path = "/api/v1/evaluation-approvals/{approval_id}/bundle",
    params(("approval_id" = String, Path)),
    responses((status = 200, body = Vec<StagedFile>), (status = 501, body = crate::error::ErrorBody)),
    tag = "evaluation")]
async fn list_bundle(
    BundleRead(bundles): BundleRead,
    Path(BundlePath { approval_id }): Path<BundlePath>,
) -> ApiResult<Json<Vec<StagedFile>>> {
    Ok(Json(bundles.staged(&approval_id).await?))
}

/// Remove every staged member of one pair.
///
/// What it does not do is un-admit anything: an approval is withdrawn through
/// its own route, and evidence already published under this pair keeps reading
/// until the adapter is asked for bytes that are no longer there. Use it to
/// correct a bundle before approving, or to clear one that was withdrawn.
#[utoipa::path(delete, path = "/api/v1/evaluation-approvals/{approval_id}/bundle",
    params(("approval_id" = String, Path)),
    responses((status = 204), (status = 403, body = crate::error::ErrorBody),
    (status = 501, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn discard_bundle(
    bundles: BundleWrite,
    Path(BundlePath { approval_id }): Path<BundlePath>,
) -> ApiResult<axum::http::StatusCode> {
    bundles.authorize().await?.discard(&approval_id).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}
