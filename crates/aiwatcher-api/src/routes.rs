//! The routes.

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Path, Query, State, WebSocketUpgrade, ws};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::stream::{Stream, StreamExt};
use serde::{Deserialize, Serialize};

use aiwatcher_core::ports::LiveEvent;
use aiwatcher_core::{Checkpoint, EventEnvelope, RecordedEvent};
use aiwatcher_projector::{
    ConversationFilter, ConversationPage, DimensionFilter, DimensionKind, DimensionPage,
    EvaluationDetail, EvaluationFilter, EvaluationPage, MetricsFilter, MetricsSummary, RunDetail,
    RunFilter, RunPage, SpanFilter, SpanPage, SuitePage,
};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::stream::{LiveFrame, Scope, as_sse, catch_up, live_tail};

/// The `Last-Event-ID` header a browser resends after an SSE drop.
const LAST_EVENT_ID: &str = "last-event-id";

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/conversations", get(list_conversations))
        .route("/api/v1/dimensions/{kind}", get(list_dimension))
        .route("/api/v1/spans", get(list_spans))
        .route("/api/v1/evaluations", get(list_evaluations))
        .route("/api/v1/evaluations/{evaluation_id}", get(get_evaluation))
        .route("/api/v1/evaluation-suites", get(list_evaluation_suites))
        .route("/api/v1/metrics", get(get_metrics))
        .route("/api/v1/runs", get(list_runs))
        .route("/api/v1/runs/{run_id}", get(get_run))
        .route("/api/v1/runs/{run_id}/events", get(get_run_events))
        .route("/api/v1/runs/{run_id}/stream", get(stream_run))
        .route("/api/v1/live", get(live_websocket))
        .route("/api/v1/events", post(ingest))
        .route("/livez", get(livez))
        .route("/healthz", get(livez))
        .route("/readyz", get(readyz))
        // Its own module: the registry is a different store with a different
        // lifetime, and keeping its routes beside the log's would hide that.
        .merge(crate::prompts::router())
        // And its own module for a sharper reason: one of these routes asks
        // another system to run something. See `crate::workflows`.
        .merge(crate::workflows::router())
        .with_state(state)
}

// ── Conversations ────────────────────────────────────────────────────────────

/// Sessions, each grouping the runs it produced.
///
/// The level above a run, so the panel can start from "which session" and walk
/// down to a single LLM call without going back to a flat list and re-filtering.
#[utoipa::path(
    get,
    path = "/api/v1/conversations",
    params(ConversationFilter),
    responses((status = 200, body = ConversationPage)),
    tag = "runs",
)]
async fn list_conversations(
    State(state): State<AppState>,
    Query(filter): Query<ConversationFilter>,
) -> Json<ConversationPage> {
    Json(state.read_model.conversations(&filter).await)
}

// ── Metrics ──────────────────────────────────────────────────────────────────

/// Aggregates over the runs the projector still holds.
///
/// Served from the read model rather than from a metrics backend: the numbers
/// are a fold over data already in memory, so the page renders with no PromQL
/// and no dependency on VictoriaMetrics being reachable. The window is bounded
/// by retention — `window.runs_retained` against `window.retention_limit` tells
/// a caller whether it is looking at everything or at a truncated tail.
#[utoipa::path(
    get,
    path = "/api/v1/metrics",
    params(MetricsFilter),
    responses((status = 200, body = MetricsSummary)),
    tag = "metrics",
)]
async fn get_metrics(
    State(state): State<AppState>,
    Query(filter): Query<MetricsFilter>,
) -> Json<MetricsSummary> {
    Json(state.read_model.metrics(&filter).await)
}

// ── Reads ────────────────────────────────────────────────────────────────────

/// List runs, newest first.
#[utoipa::path(
    get,
    path = "/api/v1/runs",
    params(RunFilter),
    responses((status = 200, body = RunPage)),
    tag = "runs",
)]
async fn list_runs(
    State(state): State<AppState>,
    Query(filter): Query<RunFilter>,
) -> Json<RunPage> {
    Json(state.read_model.list(&filter).await)
}

/// One run with the spans finished so far.
#[utoipa::path(
    get,
    path = "/api/v1/runs/{run_id}",
    params(("run_id" = String, Path, description = "The run to fetch")),
    responses(
        (status = 200, body = RunDetail),
        (status = 404, body = crate::error::ErrorBody),
    ),
    tag = "runs",
)]
async fn get_run(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
) -> ApiResult<Json<RunDetail>> {
    state
        .read_model
        .run(&run_id)
        .await
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("run {run_id}")))
}

/// The raw event log for one run.
///
/// How much of a run's history one request returns.
///
/// A long run has tens of thousands of events; a panel shows a screenful. The
/// ceiling is what stops one request from pulling a whole run into memory on
/// both sides of the wire.
const EVENTS_PAGE_DEFAULT: usize = 200;
const EVENTS_PAGE_MAX: usize = 1_000;

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct EventQuery {
    /// Cursor: the `stream_position` of the last event already seen. Exclusive.
    pub after: Option<u64>,
    pub limit: Option<usize>,
    /// Case-insensitive substring over the event type, agent, span key and the
    /// serialised payload.
    ///
    /// Applied to the page that was read, not to the whole run: the cost of a
    /// search stays the cost of a page. A search that finds nothing on this
    /// page but has `has_more` set means "keep paging", which is what the
    /// panel does.
    pub q: Option<String>,
    /// Only events on one span.
    pub span_id: Option<String>,
    /// Only events of one type, e.g. `llm.completed`.
    pub event_type: Option<String>,
}

/// One page of a run's history, filtered.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct EventPage {
    pub events: Vec<RecordedEvent>,
    /// Pass as `after` to fetch the next page. Absent at the end of the run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<u64>,
    pub has_more: bool,
    /// Events read before filtering. With a `q`, the difference between this
    /// and `events.len()` is what the filter removed — without it the panel
    /// cannot tell "nothing matched" from "nothing left".
    pub scanned: usize,
}

/// Read from the durable log rather than the read model: this is the audit
/// view, and it must show what was actually recorded.
#[utoipa::path(
    get,
    path = "/api/v1/runs/{run_id}/events",
    params(("run_id" = String, Path, description = "The run to fetch"), EventQuery),
    responses(
        (status = 200, body = EventPage),
        (status = 503, body = crate::error::ErrorBody),
    ),
    tag = "runs",
)]
async fn get_run_events(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
    Query(query): Query<EventQuery>,
) -> ApiResult<Json<EventPage>> {
    let stream_name = aiwatcher_core::StreamName::for_run(&run_id);
    let limit = query
        .limit
        .unwrap_or(EVENTS_PAGE_DEFAULT)
        .clamp(1, EVENTS_PAGE_MAX);
    let page = state
        .source
        .read_stream_page(&stream_name, query.after, limit)
        .await?;

    let scanned = page.events.len();
    // The cursor comes from what was *read*, never from what survived the
    // filter: paging past a page where nothing matched has to still advance.
    let events: Vec<RecordedEvent> = page
        .events
        .into_iter()
        .filter(|event| event_matches(event, &query))
        .collect();

    Ok(Json(EventPage {
        events,
        next_cursor: page.next_cursor,
        has_more: page.has_more,
        scanned,
    }))
}

fn event_matches(event: &RecordedEvent, query: &EventQuery) -> bool {
    if query
        .span_id
        .as_ref()
        .is_some_and(|wanted| &event.metadata.span_id.to_hex() != wanted)
    {
        return false;
    }
    if query
        .event_type
        .as_ref()
        .is_some_and(|wanted| &event.event_type.to_string() != wanted)
    {
        return false;
    }
    let Some(needle) = &query.q else {
        return true;
    };
    let needle = needle.to_lowercase();
    let metadata = &event.metadata;
    [
        event.event_type.to_string(),
        metadata.span_key.clone(),
        metadata.agent_id.clone().unwrap_or_default(),
        metadata.workflow_id.clone().unwrap_or_default(),
        event.data.to_string(),
    ]
    .iter()
    .any(|field| field.to_lowercase().contains(&needle))
}

// ── Dimensions and spans ─────────────────────────────────────────────────────

/// One dimension's rows: the explorer's top level, whatever it is rooted on.
///
/// `session`, `agent`, `runtime`, `workflow`, `trace`, `model` and `tool` all
/// return the same row shape, so the panel's tree has one renderer rather than
/// one per pivot. See `aiwatcher_projector::dimensions`.
#[utoipa::path(
    get,
    path = "/api/v1/dimensions/{kind}",
    params(
        ("kind" = DimensionKind, Path, description = "What to group runs by"),
        DimensionFilter,
    ),
    responses(
        (status = 200, body = DimensionPage),
        (status = 400, body = crate::error::ErrorBody),
    ),
    tag = "runs",
)]
async fn list_dimension(
    State(state): State<AppState>,
    Path(kind): Path<DimensionKind>,
    Query(filter): Query<DimensionFilter>,
) -> ApiResult<Json<DimensionPage>> {
    Ok(Json(state.read_model.dimensions(kind, &filter).await))
}

/// Every retained span, flat and filterable.
///
/// The waterfall answers "show me this run". This answers "show me every tool
/// call slower than two seconds", which is the question asked when the run to
/// look at is not yet known.
#[utoipa::path(
    get,
    path = "/api/v1/spans",
    params(SpanFilter),
    responses((status = 200, body = SpanPage)),
    tag = "runs",
)]
async fn list_spans(
    State(state): State<AppState>,
    Query(filter): Query<SpanFilter>,
) -> ApiResult<Json<SpanPage>> {
    Ok(Json(state.read_model.spans(&filter).await))
}

// ── Evaluations ──────────────────────────────────────────────────────────────

/// Evaluation reports, newest first.
///
/// The other half of the loop the traces come from: a trace says what one run
/// did, an evaluation says whether the thing producing those runs is getting
/// better. They arrive on the same log and are folded apart — an `eval.*`
/// event produces no span and no row in the runs list. See
/// `aiwatcher_projector::evaluations`.
#[utoipa::path(
    get,
    path = "/api/v1/evaluations",
    params(EvaluationFilter),
    responses((status = 200, body = EvaluationPage)),
    tag = "evaluation",
)]
async fn list_evaluations(
    State(state): State<AppState>,
    Query(filter): Query<EvaluationFilter>,
) -> Json<EvaluationPage> {
    Json(state.read_model.evaluations(&filter).await)
}

/// One evaluation: its parameters, its metrics, its cases, the document the
/// producer attached, and the previous evaluation of the same suite on the
/// same dataset.
#[utoipa::path(
    get,
    path = "/api/v1/evaluations/{evaluation_id}",
    params(("evaluation_id" = String, Path, description = "The evaluation to fetch")),
    responses(
        (status = 200, body = EvaluationDetail),
        (status = 404, body = crate::error::ErrorBody),
    ),
    tag = "evaluation",
)]
async fn get_evaluation(
    State(state): State<AppState>,
    Path(evaluation_id): Path<String>,
) -> ApiResult<Json<EvaluationDetail>> {
    state
        .read_model
        .evaluation(&evaluation_id)
        .await
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("evaluation {evaluation_id}")))
}

/// Suites: the level above a report, and what MLflow calls an experiment.
///
/// A separate resource rather than `/evaluations/suites`, so an evaluation
/// that happens to be called `suites` is still reachable.
#[utoipa::path(
    get,
    path = "/api/v1/evaluation-suites",
    responses((status = 200, body = SuitePage)),
    tag = "evaluation",
)]
async fn list_evaluation_suites(State(state): State<AppState>) -> Json<SuitePage> {
    Json(state.read_model.evaluation_suites().await)
}

// ── Live ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
pub struct StreamQuery {
    /// Resume point. Usually unnecessary for SSE — the browser sends
    /// `Last-Event-ID` on its own — but explicit here for non-browser clients.
    pub from: Option<String>,
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
    Json(request): Json<IngestRequest>,
) -> ApiResult<(StatusCode, Json<IngestResponse>)> {
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
    let result = sink.append(request.events).await?;
    Ok((
        // 202: the log accepted it; the projections follow asynchronously.
        StatusCode::ACCEPTED,
        Json(IngestResponse {
            accepted,
            last_checkpoint: result.last_checkpoint,
        }),
    ))
}

// ── Health ───────────────────────────────────────────────────────────────────

/// The process is running.
#[utoipa::path(get, path = "/livez", responses((status = 200)), tag = "health")]
async fn livez() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

/// The process can serve traffic.
///
/// Distinct from liveness: a projector replaying a backlog is alive but not
/// ready, and restarting it would make the replay start over.
#[utoipa::path(
    get,
    path = "/readyz",
    responses((status = 200), (status = 503)),
    tag = "health",
)]
async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    if state.health.is_ready() {
        (StatusCode::OK, "ready")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "not ready")
    }
}

/// Re-exported so the OpenAPI document and the tests can name the payload the
/// live endpoints emit.
pub type StreamedEvent = LiveEvent;
