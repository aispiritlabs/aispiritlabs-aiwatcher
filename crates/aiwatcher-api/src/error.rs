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

    #[error("this instance has no dataset registry configured (AIWATCHER_PROMPT_STORE)")]
    DatasetRegistryDisabled,

    #[error("this instance has no workflow runner configured (AIWATCHER_WORKFLOW_RUNNER)")]
    RunnerDisabled,

    #[error("this instance has no identity provider configured (AIWATCHER_AUTH_MODE)")]
    AuthDisabled,

    #[error("authentication is required")]
    Unauthenticated,

    /// Authenticated, and not allowed to do this. Names the role required and
    /// the one held, because the fix is a group membership in the identity
    /// provider and "forbidden" alone does not say which one.
    #[error("this needs the {needed} role; you have {held}")]
    Forbidden {
        needed: aiwatcher_auth::Role,
        held: aiwatcher_auth::Role,
    },

    #[error(transparent)]
    Auth(#[from] aiwatcher_auth::AuthError),

    #[error("{what} is too large: {size} bytes, over the {limit} byte limit")]
    TooLarge {
        what: &'static str,
        size: usize,
        limit: usize,
    },

    #[error(transparent)]
    Registry(#[from] aiwatcher_prompts::RegistryError),

    #[error(transparent)]
    DatasetRegistry(#[from] aiwatcher_datasets::RegistryError),

    /// A rerun the orchestrator would not take. Distinct from every other
    /// variant here in one way that matters: it is the only failure that is
    /// about work aiwatcher asked somebody else to do.
    #[error("the workflow runner refused the rerun: {0}")]
    Runner(aiwatcher_core::ports::PortError),

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
            Self::RegistryDisabled | Self::DatasetRegistryDisabled => {
                (StatusCode::NOT_IMPLEMENTED, "registry_disabled")
            }
            // Same reasoning, and the message names the variable to set. A
            // null runner that answered 202 would be worse than this: it would
            // report success for a rerun that never happened.
            Self::RunnerDisabled => (StatusCode::NOT_IMPLEMENTED, "runner_disabled"),
            // Same shape again, and the same reason: the sign-in routes exist
            // in the contract and this deployment configured no provider.
            Self::AuthDisabled => (StatusCode::NOT_IMPLEMENTED, "auth_disabled"),
            Self::Unauthenticated => (StatusCode::UNAUTHORIZED, "unauthenticated"),
            Self::Forbidden { .. } => (StatusCode::FORBIDDEN, "forbidden"),
            Self::Auth(error) => auth_parts(error),
            Self::TooLarge { .. } => (StatusCode::PAYLOAD_TOO_LARGE, "too_large"),
            Self::LogUnavailable(_) => (StatusCode::SERVICE_UNAVAILABLE, "log_unavailable"),
            Self::Bus(error) if error.is_retryable() => {
                (StatusCode::SERVICE_UNAVAILABLE, "log_unavailable")
            }
            Self::Bus(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            Self::Registry(error) => registry_parts(error),
            Self::DatasetRegistry(error) => dataset_registry_parts(error),
            // The same retryable/not split the registry makes, for the same
            // reason: an orchestrator that is down is a 503 worth repeating,
            // and one that refused the request is a 502 that will refuse it
            // identically forever.
            Self::Runner(error) if error.is_retryable() => {
                (StatusCode::SERVICE_UNAVAILABLE, "runner_unavailable")
            }
            Self::Runner(_) => (StatusCode::BAD_GATEWAY, "runner_rejected"),
        }
    }
}

/// An authentication failure, as a status the caller can act on.
///
/// Three outcomes, and which one it is decides what the panel does: 401 means
/// sign in, 403 means ask an administrator for a group, and 5xx means the
/// identity provider is the problem and signing in again will not help.
fn auth_parts(error: &aiwatcher_auth::AuthError) -> (StatusCode, &'static str) {
    use aiwatcher_auth::AuthError;
    match error {
        // Authenticated by the provider and granted nothing here. A 401 would
        // send the panel back to a sign-in that would succeed and land in the
        // same place, which is the loop this distinction exists to avoid.
        AuthError::NotEntitled(_) => (StatusCode::FORBIDDEN, "forbidden"),
        error if error.is_retryable() => (
            StatusCode::SERVICE_UNAVAILABLE,
            "identity_provider_unavailable",
        ),
        error if error.is_caller_fault() => (StatusCode::UNAUTHORIZED, "unauthenticated"),
        // A misconfiguration reaching a request: the instance started, and
        // something about the provider does not work. Not the caller's problem
        // and not fixable by retrying.
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "auth_unavailable"),
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

fn dataset_registry_parts(error: &aiwatcher_datasets::RegistryError) -> (StatusCode, &'static str) {
    use aiwatcher_datasets::RegistryError;
    match error {
        RegistryError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
        RegistryError::Invalid(_) => (StatusCode::BAD_REQUEST, "bad_request"),
        RegistryError::TooLarge { .. } => (StatusCode::PAYLOAD_TOO_LARGE, "too_large"),
        RegistryError::Store(store) if store.is_retryable() => {
            (StatusCode::SERVICE_UNAVAILABLE, "registry_unavailable")
        }
        RegistryError::Store(_) => (StatusCode::BAD_GATEWAY, "registry_rejected"),
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
        let mut response = (
            status,
            Json(ErrorBody {
                code,
                message: self.to_string(),
            }),
        )
            .into_response();

        // What RFC 9110 asks a 401 to carry. No browser dialog: `Bearer` does
        // not trigger one, unlike `Basic`.
        if status == StatusCode::UNAUTHORIZED {
            response.headers_mut().insert(
                axum::http::header::WWW_AUTHENTICATE,
                axum::http::HeaderValue::from_static("Bearer"),
            );
        }
        response
    }
}

pub type ApiResult<T> = std::result::Result<T, ApiError>;
