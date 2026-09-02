//! The SSE and WebSocket channels.
//!
//! Every frame carries its checkpoint as the SSE `id:`, so a browser resumes
//! through `Last-Event-ID` with no application code on either side. See
//! ADR_0004.

use std::convert::Infallible;
use std::time::Duration;

use axum::Router;
use axum::extract::{Path, Query, State, WebSocketUpgrade, ws};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::response::sse::{KeepAlive, Sse};
use axum::routing::get;
use futures::stream::{Stream, StreamExt};
use serde::Deserialize;
use utoipa::OpenApi;

use aiwatcher_core::Checkpoint;
use aiwatcher_core::ports::LiveEvent;

use crate::error::ApiResult;
use crate::state::AppState;
use crate::stream::{LiveFrame, Scope, as_sse, catch_up, live_tail};

/// The `Last-Event-ID` header a browser resends after an SSE drop.
const LAST_EVENT_ID: &str = "last-event-id";

/// This module's operations, as the contract they satisfy.
#[derive(OpenApi)]
#[openapi(paths(stream_events, stream_run, live_websocket,))]
struct Api;

/// The operations this module serves. Composed by [`crate::openapi`].
#[must_use]
pub fn openapi() -> utoipa::openapi::OpenApi {
    Api::openapi()
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/runs/{run_id}/stream", get(stream_run))
        .route("/api/v1/events/stream", get(stream_events))
        .route("/api/v1/live", get(live_websocket))
}

// ── Live ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
pub struct StreamQuery {
    /// Resume point. Usually unnecessary for SSE — the browser sends
    /// `Last-Event-ID` on its own — but explicit here for non-browser clients.
    pub from: Option<String>,
}

/// Server-sent events for the whole system: a catch-up marker, then live.
///
/// This is the panel's Observability transport. It is deliberately separate
/// from the WebSocket below: the panel only receives here, so EventSource can
/// own reconnects and `Last-Event-ID` resume without client-side machinery.
#[utoipa::path(
    get,
    path = "/api/v1/events/stream",
    params(StreamQuery),
    responses((status = 200, description = "text/event-stream of LiveFrame")),
    tag = "live",
)]
async fn stream_events(
    State(state): State<AppState>,
    Query(query): Query<StreamQuery>,
    headers: HeaderMap,
) -> ApiResult<Sse<impl Stream<Item = Result<axum::response::sse::Event, Infallible>>>> {
    let from = resume_point(&headers, query.from.as_deref())?;
    let scope = Scope::Everything;
    let (history, boundary) = catch_up(&state, from.as_ref(), &scope).await?;
    let tail = live_tail(&state.live, boundary, scope);
    let frames = futures::stream::iter(history).chain(tail);

    Ok(Sse::new(as_sse(frames)).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    ))
}

/// Server-sent events for one run: history, a catch-up marker, then live.
#[utoipa::path(
    get,
    path = "/api/v1/runs/{run_id}/stream",
    params(("run_id" = String, Path, description = "The run to follow"), StreamQuery),
    responses((status = 200, description = "text/event-stream of LiveFrame")),
    tag = "live",
)]
async fn stream_run(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
    Query(query): Query<StreamQuery>,
    headers: HeaderMap,
) -> ApiResult<Sse<impl Stream<Item = Result<axum::response::sse::Event, Infallible>>>> {
    let from = resume_point(&headers, query.from.as_deref())?;
    let scope = Scope::Run(run_id);
    let (history, boundary) = catch_up(&state, from.as_ref(), &scope).await?;
    let tail = live_tail(&state.live, boundary, scope);
    let frames = futures::stream::iter(history).chain(tail);

    Ok(Sse::new(as_sse(frames)).keep_alive(
        // Without this, an idle run's connection is dropped by any proxy in
        // between and the client reconnect-storms.
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    ))
}

/// The resume point, preferring the browser's `Last-Event-ID` over the query
/// string — the header is what the browser resends automatically, so it is the
/// more current of the two.
pub(crate) fn resume_point(
    headers: &HeaderMap,
    from: Option<&str>,
) -> ApiResult<Option<Checkpoint>> {
    let raw = headers
        .get(LAST_EVENT_ID)
        .and_then(|value| value.to_str().ok())
        .or(from);
    match raw {
        None => Ok(None),
        Some("") => Ok(None),
        Some(value) => Ok(Some(Checkpoint::parse(value)?)),
    }
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
pub struct LiveQuery {
    /// Follow one run only. Omit to follow everything.
    pub run_id: Option<String>,
    pub from: Option<String>,
}

/// A WebSocket for the whole system, or one run.
///
/// Prefer SSE where the traffic is one-way — it reconnects on its own and
/// survives proxies better. Use this when the panel needs to send *up* as well:
/// cancelling a run, approving a tool call, submitting human feedback.
#[utoipa::path(
    get,
    path = "/api/v1/live",
    params(LiveQuery),
    responses((status = 101, description = "WebSocket upgrade")),
    tag = "live",
)]
async fn live_websocket(
    State(state): State<AppState>,
    Query(query): Query<LiveQuery>,
    upgrade: WebSocketUpgrade,
) -> ApiResult<Response> {
    let from = match query.from.as_deref() {
        None | Some("") => None,
        Some(value) => Some(Checkpoint::parse(value)?),
    };
    Ok(upgrade.on_upgrade(move |socket| drive_socket(socket, state, from, query.run_id)))
}

async fn drive_socket(
    mut socket: ws::WebSocket,
    state: AppState,
    from: Option<Checkpoint>,
    run_id: Option<String>,
) {
    let scope = run_id.map_or(Scope::Everything, Scope::Run);
    let (history, boundary) = match catch_up(&state, from.as_ref(), &scope).await {
        Ok(result) => result,
        Err(error) => {
            tracing::warn!(%error, "websocket catch-up failed");
            let _ = socket.send(ws::Message::Close(None)).await;
            return;
        }
    };

    for frame in history {
        if send_frame(&mut socket, &frame).await.is_err() {
            return;
        }
    }

    let tail = live_tail(&state.live, boundary, scope);
    futures::pin_mut!(tail);
    loop {
        tokio::select! {
            frame = tail.next() => {
                let Some(frame) = frame else { break };
                if send_frame(&mut socket, &frame).await.is_err() {
                    return;
                }
            }
            // Read the client half so pings are answered and a close is
            // noticed promptly. Inbound control messages (cancel, approve)
            // would be handled here.
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(ws::Message::Close(_))) | None => break,
                    Some(Err(error)) => {
                        tracing::debug!(%error, "websocket closed");
                        break;
                    }
                    Some(Ok(_)) => {}
                }
            }
        }
    }
    let _ = socket.send(ws::Message::Close(None)).await;
}

async fn send_frame(socket: &mut ws::WebSocket, frame: &LiveFrame) -> Result<(), ()> {
    let Ok(payload) = serde_json::to_string(frame) else {
        tracing::error!("dropping an unserialisable live frame");
        return Ok(());
    };
    socket
        .send(ws::Message::Text(payload.into()))
        .await
        .map_err(|error| {
            tracing::debug!(%error, "websocket send failed");
        })
}

/// Re-exported so the OpenAPI document and the tests can name the payload the
/// live endpoints emit.
pub type StreamedEvent = LiveEvent;
