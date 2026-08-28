//! One error type, one JSON shape.
//!
//! Every failure the API can return renders as the same object, so the
//! generated TypeScript client has one error type to handle rather than a
//! different shape per endpoint.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("{0} not found")]
    NotFound(String),

    #[error("invalid request: {0}")]
    BadRequest(String),

    #[error("the event log is unavailable: {0}")]
    LogUnavailable(String),

    #[error("ingest is not enabled on this instance")]
    IngestDisabled,

    #[error("this instance has no prompt registry configured (AIWATCHER_PROMPT_STORE)")]
    RegistryDisabled,

    #[error("{what} is too large: {size} bytes, over the {limit} byte limit")]
    TooLarge {
        what: &'static str,
        size: usize,
        limit: usize,
    },

    #[error(transparent)]
    Registry(#[from] aiwatcher_prompts::RegistryError),

    #[error(transparent)]
    Bus(#[from] aiwatcher_bus::BusError),

    #[error(transparent)]
    Core(#[from] aiwatcher_core::CoreError),
}

/// The body every error response carries.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ErrorBody {
    /// Stable machine-readable discriminator. Switch on this, not on `message`.
    pub code: &'static str,
    pub message: String,
}

impl ApiError {
    fn parts(&self) -> (StatusCode, &'static str) {
        match self {
            Self::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            Self::BadRequest(_) | Self::Core(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            Self::IngestDisabled => (StatusCode::FORBIDDEN, "ingest_disabled"),
            // 501, not 403: the endpoint exists in the contract and this
            // deployment did not wire a store behind it. A client can tell
            // "you may not" from "nobody can here", which is the difference
            // between a permission problem and a configuration one.
            Self::RegistryDisabled => (StatusCode::NOT_IMPLEMENTED, "registry_disabled"),
            Self::TooLarge { .. } => (StatusCode::PAYLOAD_TOO_LARGE, "too_large"),
            Self::LogUnavailable(_) => (StatusCode::SERVICE_UNAVAILABLE, "log_unavailable"),
            Self::Bus(error) if error.is_retryable() => {
                (StatusCode::SERVICE_UNAVAILABLE, "log_unavailable")
            }
            Self::Bus(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            Self::Registry(error) => registry_parts(error),
        }
    }
}

/// A registry failure, as a status the caller can act on.
///
/// The distinction that matters is retryable against not: an unreachable
/// object store is a 503 the client should come back from, and a rejected
/// request is a 4xx that will be rejected identically forever.
fn registry_parts(error: &aiwatcher_prompts::RegistryError) -> (StatusCode, &'static str) {
    use aiwatcher_prompts::RegistryError;
    match error {
        RegistryError::UnknownPrompt(_)
        | RegistryError::UnknownVersion { .. }
        | RegistryError::UnknownOptimization { .. } => (StatusCode::NOT_FOUND, "not_found"),
        RegistryError::Invalid(_) | RegistryError::InvalidIdentifier { .. } => {
            (StatusCode::BAD_REQUEST, "bad_request")
        }
        RegistryError::TooLarge { .. } => (StatusCode::PAYLOAD_TOO_LARGE, "too_large"),
        RegistryError::Store(store) if store.is_retryable() => {
            (StatusCode::SERVICE_UNAVAILABLE, "registry_unavailable")
        }
        RegistryError::Store(_) => (StatusCode::BAD_GATEWAY, "registry_rejected"),
        // A stored object that will not parse is this system's fault, not the
        // caller's, and it is not going to fix itself on a retry.
        RegistryError::Corrupt { .. } => (StatusCode::INTERNAL_SERVER_ERROR, "registry_corrupt"),
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = self.parts();
        if status.is_server_error() {
            tracing::error!(error = %self, "request failed");
        } else {
            tracing::debug!(error = %self, "request rejected");
        }
        (
            status,
            Json(ErrorBody {
                code,
                message: self.to_string(),
            }),
        )
            .into_response()
    }
}

pub type ApiResult<T> = std::result::Result<T, ApiError>;
