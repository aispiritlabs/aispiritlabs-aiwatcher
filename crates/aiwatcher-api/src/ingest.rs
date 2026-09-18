//! The HTTP fallback for publishing events.
//!
//! `None` for the sink disables it: a deployment whose producers all publish
//! to the broker directly should not expose a second write path.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use utoipa::OpenApi;

use aiwatcher_core::{Checkpoint, EventEnvelope};

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::project_scope::on_the_log as project_scope;
use crate::state::AppState;

/// This module's operations, as the contract they satisfy.
#[derive(OpenApi)]
#[openapi(paths(ingest,))]
struct Api;

/// The operations this module serves. Composed by [`crate::openapi`].
#[must_use]
pub fn openapi() -> utoipa::openapi::OpenApi {
    Api::openapi()
}

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/events", post(ingest))
}

// ── Ingest ───────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct IngestRequest {
    /// A batch. Publishing several events in one call is what keeps a chatty
    /// producer from paying a round trip per token.
    pub events: Vec<EventEnvelope>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct IngestResponse {
    pub accepted: usize,
    /// The checkpoint of the last event written. A client can hand this to the
    /// live stream to pick up exactly where its own write landed.
    pub last_checkpoint: Checkpoint,
}

/// Publish events over HTTP.
///
/// The fallback path for clients that cannot reach Laser. Returns 403 when the
/// instance is configured without a sink.
///
/// Writer, not viewer: this is the one read-model route that puts something in
/// the durable log, and an agent that publishes here is a machine identity —
/// an authentik service account holding a token for this audience, or a
/// bearer the operator issued. Reading runs and writing them are different
/// permissions in every deployment that has more than one team.
///
/// Every event is recorded as published by that identity — the token's name or
/// the person's subject — whatever the body says, which is what lets a reader
/// tell one publisher's word from another's.
///
/// The same rule decides **which project** an event belongs to, and here it is
/// load-bearing rather than descriptive. The credential's scope is written onto
/// every envelope in the batch, including when the credential has none, so a
/// producer that named a project is ignored and one that named somebody else's
/// has written nothing into it. Absence means the global log, which is what
/// every token without a project does and what every event before this field
/// existed is (ADR_0033 pt. 3).
#[utoipa::path(
    post,
    path = "/api/v1/events",
    request_body = IngestRequest,
    responses(
        (status = 202, body = IngestResponse),
        (status = 400, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
    ),
    tag = "ingest",
)]
async fn ingest(
    State(state): State<AppState>,
    caller: Caller,
    Json(request): Json<IngestRequest>,
) -> ApiResult<(StatusCode, Json<IngestResponse>)> {
    let identity = caller.require(aiwatcher_auth::Role::Editor)?;
    let publisher = identity.log_subject().to_owned();
    let project = identity.project.map(project_scope);
    let Some(sink) = state.sink.as_ref() else {
        return Err(ApiError::IngestDisabled);
    };
    if request.events.is_empty() {
        return Err(ApiError::BadRequest("no events in the batch".to_owned()));
    }
    for envelope in &request.events {
        envelope.validate()?;
    }

    let accepted = request.events.len();
    let events = request
        .events
        .into_iter()
        .map(|envelope| EventEnvelope {
            published_by: Some(publisher.clone()),
            // Assigned, never merged: `project` is overwritten with the
            // credential's whatever arrived, so a body naming one is discarded
            // rather than honoured. Writing `envelope.project.or(project)`
            // here — or checking it only when the credential has a scope —
            // would turn an envelope field into a way of publishing into
            // somebody else's project.
            project,
            ..envelope
        })
        .collect();
    let result = sink.append(events).await?;
    Ok((
        // 202: the log accepted it; the projections follow asynchronously.
        StatusCode::ACCEPTED,
        Json(IngestResponse {
            accepted,
            last_checkpoint: result.last_checkpoint,
        }),
    ))
}
