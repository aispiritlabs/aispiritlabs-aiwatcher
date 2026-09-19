//! The SSE and WebSocket channels.
//!
//! Every frame carries its checkpoint as the SSE `id:`, so a browser resumes
//! through `Last-Event-ID` with no application code on either side. See
//! ADR_0004.

use std::convert::Infallible;
use std::time::Duration;

use axum::Router;
use axum::extract::{Path, Query, RawQuery, State, WebSocketUpgrade, ws};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::response::sse::{KeepAlive, Sse};
use axum::routing::get;
use futures::stream::{Stream, StreamExt};
use serde::Deserialize;
use utoipa::OpenApi;

use aiwatcher_core::Checkpoint;
use aiwatcher_core::ports::LiveEvent;
use url::form_urlencoded;

use crate::error::ApiResult;
use crate::run_scope::{RECHECK_EVERY, RunRead, Still, StillHeld};
use crate::state::AppState;
use crate::stream::{LiveFrame, Scope, Selection, Subscription, as_sse, catch_up, live_tail};

/// The `Last-Event-ID` header a browser resends after an SSE drop.
const LAST_EVENT_ID: &str = "last-event-id";

/// This module's operations, as the contract they satisfy.
#[derive(OpenApi)]
#[openapi(paths(stream_events, stream_run, live_websocket,))]
struct Api;

/// The operations this module serves, on both route families. Composed by
/// [`crate::openapi`].
#[must_use]
pub fn openapi() -> utoipa::openapi::OpenApi {
    crate::project_scope::openapi(Api::openapi())
}

/// One set of routes, served twice — see [`crate::runs::router`].
///
/// The scoped family is what makes ADR_0013 load-bearing rather than
/// historical: a browser can set a header on neither of these two transports,
/// so the grant is asked against the identity in the **session cookie**, which
/// `EventSource` and `WebSocket` attach with no application code. That is the
/// whole reason the session is a cookie this server signs.
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
        .route("/runs/{run_id}/stream", get(stream_run))
        .route("/events/stream", get(stream_events))
        .route("/live", get(live_websocket))
}

/// Named so the scoped family's own path parameters need no spelling out.
#[derive(Deserialize)]
struct RunPath {
    run_id: String,
}

// ── Live ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
pub struct StreamQuery {
    /// Resume point. Usually unnecessary for SSE — the browser sends
    /// `Last-Event-ID` on its own — but explicit here for non-browser clients.
    pub from: Option<String>,
}

/// What the system stream was narrowed to, as repeated query parameters:
/// `?agent=planner&agent=estimator`.
///
/// Repeated rather than comma-joined because an agent id, a workflow name and
/// a session id are all caller-chosen strings, and a separator this document
/// picked would be one somebody's identifier eventually contains.
///
/// **Read off the raw query string rather than through [`axum::extract::Query`].**
/// That extractor is `serde_urlencoded`, which has no sequence support: a
/// `Vec<String>` field does not collect `a=1&a=2`, it fails to deserialize at
/// all. So this parses the query itself, and keeps `IntoParams` only for the
/// contract — the panel's generated client is built from this document, and a
/// parameter that is documented but not extracted would be a filter the panel
/// sends and the server ignores.
///
/// Every dimension is optional and an absent one is not a filter, so the
/// unfiltered stream is still `GET /api/v1/events/stream` with nothing on it.
#[derive(Debug, Default, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SelectionQuery {
    /// Events produced by any of these agents.
    #[param(rename = "agent")]
    pub agents: Vec<String>,
    /// Events produced by any of these services — the explorer's `runtime`.
    #[param(rename = "runtime")]
    pub services: Vec<String>,
    /// Events belonging to any of these workflows.
    #[param(rename = "workflow")]
    pub workflows: Vec<String>,
    /// Events belonging to any of these sessions.
    #[param(rename = "session")]
    pub conversations: Vec<String>,
    /// Only these event types — `llm.completed`, `tool.failed`, and so on.
    #[param(rename = "event_type")]
    pub event_types: Vec<String>,
}

impl SelectionQuery {
    /// Collect the repeats out of a raw query string.
    ///
    /// Unknown parameters are ignored rather than refused, because this reads
    /// the *same* string [`StreamQuery`] does and `from` is in it.
    #[must_use]
    pub fn from_query(raw: Option<&str>) -> Self {
        let mut selection = Self::default();
        let Some(raw) = raw else { return selection };

        for (key, value) in form_urlencoded::parse(raw.as_bytes()) {
            if value.is_empty() {
                continue;
            }
            let field = match key.as_ref() {
                "agent" => &mut selection.agents,
                "runtime" => &mut selection.services,
                "workflow" => &mut selection.workflows,
                "session" => &mut selection.conversations,
                "event_type" => &mut selection.event_types,
                _ => continue,
            };
            field.push(value.into_owned());
        }

        selection
    }
}

impl From<SelectionQuery> for Selection {
    fn from(query: SelectionQuery) -> Self {
        Self {
            agents: query.agents,
            services: query.services,
            workflows: query.workflows,
            conversations: query.conversations,
            event_types: query.event_types,
        }
    }
}

/// Server-sent events for the whole system: a catch-up marker, then live.
///
/// This is the panel's Observability transport. It is deliberately separate
/// from the WebSocket below: the panel only receives here, so EventSource can
/// own reconnects and `Last-Event-ID` resume without client-side machinery.
#[utoipa::path(
    get,
    path = "/api/v1/events/stream",
    params(StreamQuery, SelectionQuery),
    responses((status = 200, description = "text/event-stream of LiveFrame")),
    tag = "live",
)]
async fn stream_events(
    State(state): State<AppState>,
    read: RunRead,
    Query(query): Query<StreamQuery>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> ApiResult<Sse<impl Stream<Item = Result<axum::response::sse::Event, Infallible>>>> {
    let from = resume_point(&headers, query.from.as_deref())?;
    let subscription = Subscription {
        project: read.scope,
        scope: Selection::from(SelectionQuery::from_query(raw.as_deref())).scope(),
    };
    let (history, boundary) = catch_up(&state, from.as_ref(), &subscription).await?;
    let tail = live_tail(&state.live, boundary, subscription);
    let frames = futures::stream::iter(history).chain(while_held(read.held, tail));

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
    read: RunRead,
    Path(RunPath { run_id }): Path<RunPath>,
    Query(query): Query<StreamQuery>,
    headers: HeaderMap,
) -> ApiResult<Sse<impl Stream<Item = Result<axum::response::sse::Event, Infallible>>>> {
    let from = resume_point(&headers, query.from.as_deref())?;
    // No existence check, and none is needed: every frame carries its own
    // project, so a run of another project's admits nothing on this side —
    // and a run that has not started yet still has a stream to open, which an
    // existence check would refuse.
    let subscription = Subscription {
        project: read.scope,
        scope: Scope::Run(run_id),
    };
    let (history, boundary) = catch_up(&state, from.as_ref(), &subscription).await?;
    let tail = live_tail(&state.live, boundary, subscription);
    let frames = futures::stream::iter(history).chain(while_held(read.held, tail));

    Ok(Sse::new(as_sse(frames)).keep_alive(
        // Without this, an idle run's connection is dropped by any proxy in
        // between and the client reconnect-storms.
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    ))
}

/// The frames, until the access that opened the stream is gone.
///
/// A list is one request and one decision; a stream is one request that lives
/// for hours, and until this the session cookie's TTL *was* its revocation
/// window (ADR_0013's own Consequences). So every [`RECHECK_EVERY`] the access
/// is asked again, and a definite refusal ends the stream with a
/// [`LiveFrame::Revoked`] rather than letting it run to the end of the session.
///
/// Only a *refusal* closes it. An IAM store that could not be reached is not an
/// answer — `Transient`, as `ProjectDispatcher` reads the same failure — so the
/// connection stays and the next tick asks again. What that costs is stated in
/// [`Still::Unknown`], and it is bounded by the other half: no *new* stream
/// opens while IAM is unreachable, because the extractor fails closed on the
/// same error.
///
/// On the global side there is nothing per-stream to ask, so the tick is a
/// no-op with no await in it. One code path either way — a second one would be
/// the path that gets it wrong.
fn while_held(
    held: StillHeld,
    frames: impl Stream<Item = LiveFrame> + Send + 'static,
) -> impl Stream<Item = LiveFrame> + Send + 'static {
    let ticks = tokio::time::interval_at(
        // Not `interval`, whose first tick fires immediately: the grant was
        // just read to open this stream, and asking again in the same
        // millisecond would double every connection's cost for nothing.
        tokio::time::Instant::now() + RECHECK_EVERY,
        RECHECK_EVERY,
    );
    futures::stream::unfold(
        (Box::pin(frames), held, ticks, false),
        |(mut frames, held, mut ticks, closed)| async move {
            if closed {
                return None;
            }
            loop {
                tokio::select! {
                    frame = frames.next() => {
                        return frame.map(|frame| (frame, (frames, held, ticks, false)));
                    }
                    _ = ticks.tick() => {
                        if held.ask(time::OffsetDateTime::now_utc().unix_timestamp()).await
                            == Still::Gone
                        {
                            tracing::info!("closing a live stream whose access is gone");
                            return Some((LiveFrame::Revoked, (frames, held, ticks, true)));
                        }
                    }
                }
            }
        },
    )
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
    read: RunRead,
    Query(query): Query<LiveQuery>,
    upgrade: WebSocketUpgrade,
) -> ApiResult<Response> {
    let from = match query.from.as_deref() {
        None | Some("") => None,
        Some(value) => Some(Checkpoint::parse(value)?),
    };
    let subscription = Subscription {
        project: read.scope,
        scope: query.run_id.map_or(Scope::Everything, Scope::Run),
    };
    Ok(
        upgrade
            .on_upgrade(move |socket| drive_socket(socket, state, from, subscription, read.held)),
    )
}

async fn drive_socket(
    mut socket: ws::WebSocket,
    state: AppState,
    from: Option<Checkpoint>,
    subscription: Subscription,
    held: StillHeld,
) {
    let (history, boundary) = match catch_up(&state, from.as_ref(), &subscription).await {
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

    let tail = live_tail(&state.live, boundary, subscription);
    futures::pin_mut!(tail);
    let mut ticks =
        tokio::time::interval_at(tokio::time::Instant::now() + RECHECK_EVERY, RECHECK_EVERY);
    loop {
        tokio::select! {
            frame = tail.next() => {
                let Some(frame) = frame else { break };
                if send_frame(&mut socket, &frame).await.is_err() {
                    return;
                }
            }
            // The same re-check the SSE half makes, for the same reason: a
            // socket left open overnight must not outlive the grant that
            // opened it. See `while_held`.
            _ = ticks.tick() => {
                if held.ask(time::OffsetDateTime::now_utc().unix_timestamp()).await
                    == Still::Gone
                {
                    tracing::info!("closing a live socket whose access is gone");
                    let _ = send_frame(&mut socket, &LiveFrame::Revoked).await;
                    break;
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

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use futures::StreamExt;

    use super::{StillHeld, while_held};
    use crate::stream::LiveFrame;

    /// A stream whose access has gone ends, and says so on the way out.
    ///
    /// The frame is the point. A connection that simply stops looks exactly
    /// like a connection where nothing is happening, and "nothing looks wrong"
    /// is the failure mode ADR_0004 exists to prevent — here it would be a
    /// person watching a view that quietly stopped being theirs.
    #[tokio::test(start_paused = true)]
    async fn a_stream_whose_access_is_gone_ends_with_a_frame_that_says_so() {
        // An expired session rather than a revoked grant, so the guard answers
        // without a store: which of the two ways access can end is
        // `StillHeld::ask`'s own test, and this one is about what the stream
        // does with the answer.
        let held = StillHeld::Grant {
            store: std::sync::Arc::new(aiwatcher_iam::memory::MemoryIamStore::default()),
            principal: Box::new(
                aiwatcher_iam::Principal::new("https://issuer.test", "member")
                    .expect("a principal"),
            ),
            scope: aiwatcher_iam::ProjectScope {
                organization: aiwatcher_iam::OrganizationId(uuid::Uuid::nil()),
                project: aiwatcher_iam::ProjectId(uuid::Uuid::nil()),
            },
            expires_at: Some(0),
        };
        // A tail that never ends on its own, which is what a live stream is.
        let tail = futures::stream::pending::<LiveFrame>();
        let frames = while_held(held, tail);
        futures::pin_mut!(frames);

        assert!(
            matches!(frames.next().await, Some(LiveFrame::Revoked)),
            "the tick closes it rather than the connection running to the end of the session"
        );
        assert!(
            frames.next().await.is_none(),
            "and nothing follows the close"
        );
    }

    /// On the global side there is nothing per-stream to re-ask, so the tick
    /// changes nothing and the frames are the frames.
    #[tokio::test(start_paused = true)]
    async fn an_instance_stream_is_not_closed_by_the_re_check() {
        let frames = while_held(
            StillHeld::Instance,
            futures::stream::iter(vec![LiveFrame::Caught {
                checkpoint: aiwatcher_core::Checkpoint::beginning(),
            }]),
        );
        futures::pin_mut!(frames);
        assert!(matches!(
            frames.next().await,
            Some(LiveFrame::Caught { .. })
        ));
        assert!(frames.next().await.is_none());
    }
}
