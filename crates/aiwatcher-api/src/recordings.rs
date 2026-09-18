//! Recorded answer bytes in the currently authorized evaluation registry.
use crate::evaluation_scope::{EvaluationRead, EvaluationWrite};
use crate::{AppState, error::ApiResult};
use aiwatcher_core::ArtifactRef;
use aiwatcher_evaluation::RecordedAnswers;
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path},
    http::header,
    response::IntoResponse,
    routing::{get, put},
};
use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(paths(stage_recording, get_recording))]
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
                header::CACHE_CONTROL,
                axum::http::HeaderValue::from_static("no-store"),
            )),
    )
}
fn resource_router() -> Router<AppState> {
    Router::new()
        .route(
            "/evaluation-recordings/{name}",
            put(stage_recording).layer(DefaultBodyLimit::max(100 * 1024 * 1024)),
        )
        .route(
            "/evaluation-recordings/{digest}/content",
            get(get_recording),
        )
}
#[derive(serde::Deserialize)]
struct RecordingName {
    name: String,
}
#[derive(serde::Deserialize)]
struct RecordingDigest {
    digest: String,
}

/// Keep answers and return a reference to their exact bytes. The name is display
/// metadata, never a storage key. Retrying the same bytes preserves their digest.
/// On project routes the reference is interpreted only in that same project.
#[utoipa::path(put, path = "/api/v1/evaluation-recordings/{name}",
    params(("name" = String, Path, description = "What a reader calls this recording")),
    request_body = Vec<u8>,
    responses((status = 200, body = ArtifactRef), (status = 400, body = crate::error::ErrorBody),
    (status = 403, body = crate::error::ErrorBody), (status = 501, body = crate::error::ErrorBody)),
    tag = "evaluation")]
async fn stage_recording(
    evaluation: EvaluationWrite,
    Path(RecordingName { name }): Path<RecordingName>,
    body: Bytes,
) -> ApiResult<Json<ArtifactRef>> {
    Ok(Json(
        evaluation
            .authorize()
            .await?
            .stage_recording(&name, body.to_vec())
            .await?,
    ))
}

/// Download a staged answer document after verifying its SHA-256 digest.
/// Returns the original JSON bytes, preserving number spelling and whitespace.
#[utoipa::path(get, path = "/api/v1/evaluation-recordings/{digest}/content",
    params(("digest" = String, Path, description = "SHA-256 of the original recording bytes")),
    responses((status = 200, body = RecordedAnswers, content_type = "application/json"),
    (status = 400, body = crate::error::ErrorBody),
    (status = 404, body = crate::error::ErrorBody), (status = 501, body = crate::error::ErrorBody),
    (status = 503, body = crate::error::ErrorBody)),
    tag = "evaluation")]
async fn get_recording(
    EvaluationRead(registry): EvaluationRead,
    Path(RecordingDigest { digest }): Path<RecordingDigest>,
) -> ApiResult<impl IntoResponse> {
    let bytes = registry
        .recording_bytes(&digest)
        .await
        .map_err(|error| match error {
            aiwatcher_evaluation::EvaluationError::Unavailable(
                aiwatcher_evaluation::EvidenceState::MissingArtifact,
            ) => crate::ApiError::NotFound(format!("recording {digest}")),
            other => other.into(),
        })?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        bytes,
    ))
}
