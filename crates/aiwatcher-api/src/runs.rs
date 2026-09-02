//! Runs, and every way of slicing them.
//!
//! One fold answers `session | agent | runtime | workflow | trace | model |
//! tool` with one row shape, and every list here is a cursor page — nothing
//! loads a whole run. See ADR_0007.

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use utoipa::OpenApi;

use aiwatcher_core::RecordedEvent;
use aiwatcher_projector::{
    ConversationFilter, ConversationPage, DimensionFilter, DimensionKind, DimensionPage, RunDetail,
    RunFilter, RunPage, SpanFilter, SpanPage,
};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// This module's operations, as the contract they satisfy.
#[derive(OpenApi)]
#[openapi(paths(
    list_conversations,
    list_runs,
    get_run,
    get_run_events,
    list_dimension,
    list_spans,
))]
struct Api;

/// The operations this module serves. Composed by [`crate::openapi`].
#[must_use]
pub fn openapi() -> utoipa::openapi::OpenApi {
    Api::openapi()
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/conversations", get(list_conversations))
        .route("/api/v1/dimensions/{kind}", get(list_dimension))
        .route("/api/v1/spans", get(list_spans))
        .route("/api/v1/runs", get(list_runs))
        .route("/api/v1/runs/{run_id}", get(get_run))
        .route("/api/v1/runs/{run_id}/events", get(get_run_events))
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
