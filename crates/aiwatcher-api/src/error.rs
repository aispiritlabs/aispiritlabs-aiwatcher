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

    #[error("this instance has no annotation registry configured (AIWATCHER_PROMPT_STORE)")]
    AnnotationRegistryDisabled,

    #[error("this instance has no training registry configured (AIWATCHER_PROMPT_STORE)")]
    TrainingRegistryDisabled,

    #[error("this instance has no workflow runner configured (AIWATCHER_WORKFLOW_RUNNER)")]
    RunnerDisabled,

    #[error("this instance has no pipeline engine configured (AIWATCHER_ENGINE)")]
    EngineDisabled,

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

    #[error(transparent)]
    AnnotationRegistry(#[from] aiwatcher_annotations::Error),

    #[error(transparent)]
    TrainingRegistry(#[from] aiwatcher_training::Error),

    /// A rerun the orchestrator would not take. Distinct from every other
    /// variant here in one way that matters: it is the only failure that is
    /// about work aiwatcher asked somebody else to do.
    #[error("the workflow runner refused the rerun: {0}")]
    Runner(aiwatcher_core::ports::PortError),

    /// The engine would not answer. Same split as `Runner`: down is a 503
    /// worth repeating, and a refusal is a 502 that will refuse identically
    /// forever.
    #[error("the pipeline engine could not serve that: {0}")]
    Engine(aiwatcher_core::ports::PortError),

    /// A launch the engine refused, or the adapter refused on its behalf: an
    /// input the entity does not declare, a required one left out, a timestamp
    /// that will not parse.
    ///
    /// Its own variant rather than [`Self::Engine`] because of who is at
    /// fault. This is the request being wrong — a 400, and the message is what
    /// a form puts beside the field. Answering 502 would tell somebody who
    /// mistyped a date that the gateway is broken.
    #[error("{0}")]
    LaunchRefused(String),

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
    /// One line per problem, where a request can fail in more than one way at
    /// once. An annotation is the case that needs it: a labeller fixing one
    /// error per round trip stops using the tool, so every problem in a
    /// drawing is reported together. Absent everywhere else, which keeps the
    /// one JSON shape one shape.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<String>,
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
            Self::RegistryDisabled
            | Self::DatasetRegistryDisabled
            | Self::AnnotationRegistryDisabled
            | Self::TrainingRegistryDisabled => {
                (StatusCode::NOT_IMPLEMENTED, "registry_disabled")
            }
            // Same reasoning, and the message names the variable to set. A
            // null runner that answered 202 would be worse than this: it would
            // report success for a rerun that never happened.
            Self::RunnerDisabled => (StatusCode::NOT_IMPLEMENTED, "runner_disabled"),
            // And again for the engine: the routes exist in the contract and
            // this deployment wired no orchestrator behind them. The message
            // names the variable to set.
            Self::EngineDisabled => (StatusCode::NOT_IMPLEMENTED, "engine_disabled"),
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
            Self::AnnotationRegistry(error) => annotation_registry_parts(error),
            Self::TrainingRegistry(error) => training_registry_parts(error),
            // The same retryable/not split the registry makes, for the same
            // reason: an orchestrator that is down is a 503 worth repeating,
            // and one that refused the request is a 502 that will refuse it
            // identically forever.
            Self::Runner(error) if error.is_retryable() => {
                (StatusCode::SERVICE_UNAVAILABLE, "runner_unavailable")
            }
            Self::Runner(_) => (StatusCode::BAD_GATEWAY, "runner_rejected"),
            Self::Engine(error) if error.is_retryable() => {
                (StatusCode::SERVICE_UNAVAILABLE, "engine_unavailable")
            }
            Self::Engine(_) => (StatusCode::BAD_GATEWAY, "engine_rejected"),
            Self::LaunchRefused(_) => (StatusCode::BAD_REQUEST, "launch_refused"),
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

/// An annotation failure, as a status the caller can act on.
///
/// One extra outcome over the other two registries: a drawing that did not
/// validate is a 422 rather than a 400, because the request was well formed and
/// the *content* was refused. The panel keeps the shape on the canvas and
/// renders the reasons beside it; a 400 would read as "the tool is broken".
fn annotation_registry_parts(error: &aiwatcher_annotations::Error) -> (StatusCode, &'static str) {
    use aiwatcher_annotations::Error;
    match error {
        Error::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
        Error::Invalid(_) => (StatusCode::BAD_REQUEST, "bad_request"),
        Error::Rejected(_) => (StatusCode::UNPROCESSABLE_ENTITY, "annotation_rejected"),
        Error::TooLarge { .. } => (StatusCode::PAYLOAD_TOO_LARGE, "too_large"),
        Error::Store(store) if store.is_retryable() => {
            (StatusCode::SERVICE_UNAVAILABLE, "registry_unavailable")
        }
        Error::Store(_) => (StatusCode::BAD_GATEWAY, "registry_rejected"),
        Error::Corrupt { .. } => (StatusCode::INTERNAL_SERVER_ERROR, "registry_corrupt"),
    }
}

/// A training failure, as a status the caller can act on.
///
/// Two outcomes this registry has that the others do not. A run id that has
/// already finished is a 409: the request is well formed, and the state it
/// assumed is gone. A promotion the registry refused is a 422 with the reason,
/// because "this model has no held-out score" is a finding rather than a typo.
fn training_registry_parts(error: &aiwatcher_training::Error) -> (StatusCode, &'static str) {
    use aiwatcher_training::Error;
    match error {
        Error::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
        Error::Invalid(_) => (StatusCode::BAD_REQUEST, "bad_request"),
        // A closed run and a reused run id are conflicts with state, not bad
        // requests; a refused promotion is a decision about content.
        Error::Refused(message) if message.contains("run") => {
            (StatusCode::CONFLICT, "run_closed")
        }
        Error::Refused(_) => (StatusCode::UNPROCESSABLE_ENTITY, "promotion_refused"),
        Error::TooLarge { .. } => (StatusCode::PAYLOAD_TOO_LARGE, "too_large"),
        Error::Store(store) if store.is_retryable() => {
            (StatusCode::SERVICE_UNAVAILABLE, "registry_unavailable")
        }
        Error::Store(_) => (StatusCode::BAD_GATEWAY, "registry_rejected"),
        Error::Corrupt { .. } => (StatusCode::INTERNAL_SERVER_ERROR, "registry_corrupt"),
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
        let details = match &self {
            Self::AnnotationRegistry(aiwatcher_annotations::Error::Rejected(problems)) => {
                problems.clone()
            }
            _ => Vec::new(),
        };
        let mut response = (
            status,
            Json(ErrorBody {
                code,
                message: self.to_string(),
                details,
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
