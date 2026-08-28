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
            Self::LogUnavailable(_) => (StatusCode::SERVICE_UNAVAILABLE, "log_unavailable"),
            Self::Bus(error) if error.is_retryable() => {
                (StatusCode::SERVICE_UNAVAILABLE, "log_unavailable")
            }
            Self::Bus(_) => (StatusCode::BAD_REQUEST, "bad_request"),
        }
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
