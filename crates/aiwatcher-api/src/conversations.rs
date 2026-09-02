//! The governed conversation archive: writing turns, reviewing them, erasing
//! them, and the exports built from them.
//!
//! **This is the one area where a role decides whether content is returned at
//! all, and it is the reason `admin` now guards more than the rerun.** Reading
//! a turn's words, or an export's rows, needs `admin`; everything else about a
//! turn — its role, its ordering, its policy, its findings, its review state —
//! is `viewer`, and writing one is `editor` so that a producer holding an
//! ingest token can record but never read back. That split is the whole point
//! of the head/content separation in `aiwatcher_conversations::archive`: a
//! review queue, an audit and an exclusion report are all answerable without
//! anybody decrypting anything.
//!
//! Ids travel in query parameters rather than in the path, the way
//! `/api/v1/dataset-rows` already does. A `conversation_id` comes from a
//! producer that never heard of this API and holds slashes routinely, and a
//! path segment that can contain one makes every route below it ambiguous.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use utoipa::OpenApi;

use aiwatcher_conversations::{
    ArchivePolicy, ConversationPage, ErasureReport, ExportJob, ExportJobPage, ExportPage,
    ExportRequest, ExportRowsPage, FindingKind, RecordTurnRequest, RecordedTurn, Registry,
    ReviewRequest, ReviewState, Role, TurnContent, TurnFilter, TurnPage,
};

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// This module's operations, as the contract they satisfy.
///
/// Derived beside the router rather than listed in the root document, so
/// adding a route and forgetting the contract is a change to one file rather
/// than a change to two files that has to be noticed in the second.
#[derive(OpenApi)]
#[openapi(paths(
    conversation_policy,
    list_conversation_archive,
    record_conversation_turns,
    list_conversation_turns,
    conversation_turn_content,
    review_conversation_turn,
    erase_conversation_content,
    list_conversation_exports,
    create_conversation_export,
    get_conversation_export,
    cancel_conversation_export,
    list_conversation_datasets,
    get_conversation_dataset_rows,
))]
struct Api;

/// The operations this module serves. Composed by [`crate::openapi`].
#[must_use]
pub fn openapi() -> utoipa::openapi::OpenApi {
    Api::openapi()
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/conversation-policy", get(conversation_policy))
        .route(
            "/api/v1/conversation-archive",
            get(list_conversation_archive),
        )
        .route(
            "/api/v1/conversation-turns",
            get(list_conversation_turns).post(record_conversation_turns),
        )
        .route(
            "/api/v1/conversation-turn-content",
            get(conversation_turn_content),
        )
        .route(
            "/api/v1/conversation-turn-reviews",
            post(review_conversation_turn),
        )
        .route(
            "/api/v1/conversation-erasures",
            post(erase_conversation_content),
        )
        .route(
            "/api/v1/conversation-exports",
            get(list_conversation_exports).post(create_conversation_export),
        )
        .route(
            "/api/v1/conversation-exports/{job_id}",
            get(get_conversation_export),
        )
        .route(
            "/api/v1/conversation-exports/{job_id}/cancel",
            post(cancel_conversation_export),
        )
        .route(
            "/api/v1/conversation-datasets",
            get(list_conversation_datasets),
        )
        .route(
            "/api/v1/conversation-dataset-rows",
            get(get_conversation_dataset_rows),
        )
}

fn archive(state: &AppState) -> ApiResult<&Arc<Registry>> {
    state
        .conversations
        .as_ref()
        .ok_or(ApiError::ConversationArchiveDisabled)
}

/// Writing a turn, and deciding about one.
fn may_author(caller: &Caller) -> ApiResult<()> {
    caller.require(aiwatcher_auth::Role::Editor).map(|_| ())
}

/// Reading what somebody said, and erasing it.
///
/// The strongest role this system has, on the two operations that most deserve
/// it: content leaves the encryption boundary, and an erasure is not
/// reversible. An ingest token is capped at `editor` precisely so a leaked
/// producer credential cannot do either — see the guardrail in CLAUDE.md.
fn may_read_content(caller: &Caller) -> ApiResult<&aiwatcher_auth::Identity> {
    caller.require(aiwatcher_auth::Role::Admin)
}

// ── Policy ───────────────────────────────────────────────────────────────────

/// What this deployment demands of a producer, and which keys it can open.
///
/// Answered before anything is sent, on purpose: a producer that discovers the
/// consent requirement from a 422 has already put a megabyte of content on the
/// wire, and the panel needs it to render the archive's state rather than an
/// empty list that looks like "nothing has happened yet".
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ArchiveConfig {
    #[serde(flatten)]
    pub policy: ArchivePolicy,
    /// Key ids this deployment can decrypt with, newest first. Never the keys.
    /// Present so an operator can see that a rotation reached this process
    /// before they retire the old key and make everything sealed under it
    /// unreadable.
    pub key_ids: Vec<String>,
}

/// What this instance demands before it will hold conversation content.
#[utoipa::path(
    get,
    path = "/api/v1/conversation-policy",
    responses(
        (status = 200, body = ArchiveConfig),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "conversations",
)]
async fn conversation_policy(State(state): State<AppState>) -> ApiResult<Json<ArchiveConfig>> {
    let archive = archive(&state)?;
    Ok(Json(ArchiveConfig {
        policy: archive.policy(),
        key_ids: archive.key_ids(),
    }))
}

// ── Turns ────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RecordTurnsBody {
    /// One exchange, as a producer flushes it. Not a transaction — see
    /// `Registry::record_batch`.
    pub turns: Vec<RecordTurnRequest>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RecordedTurns {
    pub turns: Vec<RecordedTurn>,
}

/// Record conversation content, with the consent and retention that permit it.
#[utoipa::path(
    post,
    path = "/api/v1/conversation-turns",
    request_body = RecordTurnsBody,
    responses(
        (status = 201, body = RecordedTurns),
        (status = 400, body = crate::error::ErrorBody),
        (status = 413, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody, description = "Refused: every policy problem at once"),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "conversations",
)]
async fn record_conversation_turns(
    State(state): State<AppState>,
    caller: Caller,
    Json(body): Json<RecordTurnsBody>,
) -> ApiResult<(StatusCode, Json<RecordedTurns>)> {
    may_author(&caller)?;
    let turns = archive(&state)?.record_batch(body.turns).await?;
    Ok((StatusCode::CREATED, Json(RecordedTurns { turns })))
}

/// Every conversation the archive holds, newest activity first.
#[utoipa::path(
    get,
    path = "/api/v1/conversation-archive",
    responses(
        (status = 200, body = ConversationPage),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "conversations",
)]
async fn list_conversation_archive(
    State(state): State<AppState>,
) -> ApiResult<Json<ConversationPage>> {
    Ok(Json(archive(&state)?.conversations().await?))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct TurnsQuery {
    pub conversation_id: String,
    /// Narrow to one review state — what the review queue asks for.
    pub review: Option<ReviewState>,
    /// Narrow to turns carrying a finding of this kind.
    pub finding: Option<FindingKind>,
    pub role: Option<Role>,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

/// One conversation's turns. Heads only: nothing here is decrypted.
#[utoipa::path(
    get,
    path = "/api/v1/conversation-turns",
    params(TurnsQuery),
    responses(
        (status = 200, body = TurnPage),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "conversations",
)]
async fn list_conversation_turns(
    State(state): State<AppState>,
    Query(query): Query<TurnsQuery>,
) -> ApiResult<Json<TurnPage>> {
    let filter = TurnFilter {
        review: query.review,
        finding: query.finding,
        role: query.role,
    };
    Ok(Json(
        archive(&state)?
            .turns(
                &query.conversation_id,
                &filter,
                query.offset.unwrap_or(0),
                query.limit.unwrap_or(50),
            )
            .await?,
    ))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct ContentQuery {
    pub conversation_id: String,
    pub turn_id: String,
}

/// What was actually said. The one route that leaves the encryption boundary.
///
/// A turn whose content has been erased answers 410 rather than 404: "it was
/// here and it is gone" is the answer an auditor came for, and a 404 would make
/// a completed erasure indistinguishable from a turn that never existed.
#[utoipa::path(
    get,
    path = "/api/v1/conversation-turn-content",
    params(ContentQuery),
    responses(
        (status = 200, body = TurnContent),
        (status = 403, body = crate::error::ErrorBody, description = "Reading content needs the admin role"),
        (status = 404, body = crate::error::ErrorBody),
        (status = 410, body = crate::error::ErrorBody, description = "Erased: the head remains, the words do not"),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "conversations",
)]
async fn conversation_turn_content(
    State(state): State<AppState>,
    caller: Caller,
    Query(query): Query<ContentQuery>,
) -> ApiResult<Json<TurnContent>> {
    let identity = may_read_content(&caller)?;
    let content = archive(&state)?
        .content(&query.conversation_id, &query.turn_id)
        .await?;
    // The one audit line this crate writes into the ordinary log. It names who
    // and what, and — the point — never any of it.
    tracing::info!(
        subject = %identity.subject,
        conversation_id = %query.conversation_id,
        turn_id = %query.turn_id,
        "conversation content was read"
    );
    Ok(Json(content))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewTurnBody {
    pub conversation_id: String,
    pub turn_id: String,
    pub review: ReviewRequest,
}

/// Approve or reject one turn, attributed to whoever is asking.
#[utoipa::path(
    post,
    path = "/api/v1/conversation-turn-reviews",
    request_body = ReviewTurnBody,
    responses(
        (status = 200, body = aiwatcher_conversations::ArchivedTurn),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "conversations",
)]
async fn review_conversation_turn(
    State(state): State<AppState>,
    caller: Caller,
    Json(body): Json<ReviewTurnBody>,
) -> ApiResult<Json<aiwatcher_conversations::ArchivedTurn>> {
    may_author(&caller)?;
    // The reviewer is the caller, never the request. A client that could name
    // its own reviewer could file somebody else's approval.
    let reviewer = caller.identity().subject.clone();
    Ok(Json(
        archive(&state)?
            .review(
                &body.conversation_id,
                &body.turn_id,
                &reviewer,
                &body.review,
            )
            .await?,
    ))
}

// ── Erasure ──────────────────────────────────────────────────────────────────

/// An erasure request: a subject, or a whole conversation.
///
/// A POST rather than a DELETE, because it is a request somebody files with a
/// body and gets a report back — and because the subject it names is personal
/// data that has no business in a URL. Exactly one of the two fields.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ErasureBody {
    /// The consent subject, as recorded on the turns. What a person's request
    /// actually names.
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub conversation_id: Option<String>,
}

/// Erase conversation content, wherever it has reached.
///
/// Two steps, and the second is the one that is easy to forget: the archive's
/// content is removed, and then every published corpus that read one of these
/// conversations has its rows deleted too. Erasing the archive and leaving the
/// corpus would be an erasure in name only.
///
/// What remains everywhere is the record: heads, digests, review decisions and
/// export manifests. That is what lets an auditor still be told what was there.
#[utoipa::path(
    post,
    path = "/api/v1/conversation-erasures",
    request_body = ErasureBody,
    responses(
        (status = 200, body = ErasureReport),
        (status = 400, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody, description = "Erasing needs the admin role"),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "conversations",
)]
async fn erase_conversation_content(
    State(state): State<AppState>,
    caller: Caller,
    Json(body): Json<ErasureBody>,
) -> ApiResult<Json<ErasureReport>> {
    let identity = may_read_content(&caller)?;
    let by = identity.subject.clone();
    let archive = archive(&state)?;
    let report = match (body.subject, body.conversation_id) {
        (Some(subject), None) => archive.erase_subject(&subject, &by).await?,
        (None, Some(conversation_id)) => archive.erase_conversation(&conversation_id, &by).await?,
        _ => {
            return Err(ApiError::BadRequest(
                "name exactly one of subject or conversation_id".to_owned(),
            ));
        }
    };
    Ok(Json(report))
}

// ── Exports ──────────────────────────────────────────────────────────────────

/// Queue an export of the archive into an immutable corpus.
///
/// 202, not 201: what comes back is a job, and the corpus it will produce does
/// not exist yet. A 201 would be this API claiming a dataset that a worker has
/// not built.
#[utoipa::path(
    post,
    path = "/api/v1/conversation-exports",
    request_body = ExportRequest,
    responses(
        (status = 202, body = ExportJob, description = "Queued, or the job this request already started"),
        (status = 400, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody, description = "The selection matched no conversation"),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "conversations",
)]
async fn create_conversation_export(
    State(state): State<AppState>,
    caller: Caller,
    Json(request): Json<ExportRequest>,
) -> ApiResult<(StatusCode, Json<ExportJob>)> {
    may_author(&caller)?;
    let job = archive(&state)?
        .create_export(request, &caller.identity().subject)
        .await?;
    // Nudge the worker rather than waiting for its next tick, so a queued
    // export starts now instead of in fifteen seconds.
    state.notify_export_worker();
    Ok((StatusCode::ACCEPTED, Json(job)))
}

/// Every export job, newest first.
#[utoipa::path(
    get,
    path = "/api/v1/conversation-exports",
    responses(
        (status = 200, body = ExportJobPage),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "conversations",
)]
async fn list_conversation_exports(
    State(state): State<AppState>,
) -> ApiResult<Json<ExportJobPage>> {
    Ok(Json(archive(&state)?.export_jobs().await?))
}

/// One job: where it is, what it has counted, and what it left out.
#[utoipa::path(
    get,
    path = "/api/v1/conversation-exports/{job_id}",
    params(("job_id" = String, Path,)),
    responses(
        (status = 200, body = ExportJob),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "conversations",
)]
async fn get_conversation_export(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
) -> ApiResult<Json<ExportJob>> {
    Ok(Json(archive(&state)?.export_job(&job_id).await?))
}

/// Stop a job. What it has written stays written; there is no version.
#[utoipa::path(
    post,
    path = "/api/v1/conversation-exports/{job_id}/cancel",
    params(("job_id" = String, Path,)),
    responses(
        (status = 200, body = ExportJob),
        (status = 404, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody, description = "It already finished"),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "conversations",
)]
async fn cancel_conversation_export(
    State(state): State<AppState>,
    caller: Caller,
    Path(job_id): Path<String>,
) -> ApiResult<Json<ExportJob>> {
    may_author(&caller)?;
    Ok(Json(archive(&state)?.cancel_export(&job_id).await?))
}

/// Every immutable corpus this archive has produced.
#[utoipa::path(
    get,
    path = "/api/v1/conversation-datasets",
    responses(
        (status = 200, body = ExportPage),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "conversations",
)]
async fn list_conversation_datasets(State(state): State<AppState>) -> ApiResult<Json<ExportPage>> {
    Ok(Json(archive(&state)?.exports().await?))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct DatasetRowsQuery {
    pub name: String,
    pub version: String,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

/// One page of a corpus. Content, so `admin` — the same gate as reading a turn.
///
/// A corpus an erasure has withdrawn answers 410, exactly as an erased turn
/// does: its manifest, counts and digests survive so a training run naming the
/// reference still resolves to something that can say what happened to it, and
/// only the rows are gone.
#[utoipa::path(
    get,
    path = "/api/v1/conversation-dataset-rows",
    params(DatasetRowsQuery),
    responses(
        (status = 200, body = ExportRowsPage),
        (status = 400, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody, description = "Reading rows needs the admin role"),
        (status = 404, body = crate::error::ErrorBody),
        (status = 410, body = crate::error::ErrorBody, description = "Withdrawn: an erasure took this corpus' rows"),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "conversations",
)]
async fn get_conversation_dataset_rows(
    State(state): State<AppState>,
    caller: Caller,
    Query(query): Query<DatasetRowsQuery>,
) -> ApiResult<Json<ExportRowsPage>> {
    let identity = may_read_content(&caller)?;
    let page = archive(&state)?
        .export_rows(
            &query.name,
            &query.version,
            query.offset.unwrap_or(0),
            query.limit.unwrap_or(50),
        )
        .await?;
    tracing::info!(
        subject = %identity.subject,
        name = %query.name,
        version = %query.version,
        rows = page.rows.len(),
        "conversation corpus rows were read"
    );
    Ok(Json(page))
}
