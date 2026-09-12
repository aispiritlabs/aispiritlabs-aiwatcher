//! What an evaluation measures, as a resource a run can name.
//!
//! The suite list beside this one is an aggregate of reports — a name a
//! producer sent, learnt after the fact — so nothing in it can be run again. A
//! scorecard is the declaration: which scorers a case goes through, what each
//! one writes, and which end of it is better. It is authored, so it is
//! versioned by its content and a run names the concrete version.

use aiwatcher_auth::Role;
use aiwatcher_evaluation::{Scorecard, ScorecardPage, ScorecardVersion};
use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use utoipa::OpenApi;

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// This module's operations, as the contract they satisfy.
#[derive(OpenApi)]
#[openapi(paths(publish_scorecard, list_scorecards, get_scorecard))]
struct Api;

/// The operations this module serves. Composed by [`crate::openapi`].
#[must_use]
pub fn openapi() -> utoipa::openapi::OpenApi {
    Api::openapi()
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/evaluation-scorecards",
            get(list_scorecards).post(publish_scorecard),
        )
        .route("/api/v1/evaluation-scorecards/{name}", get(get_scorecard))
}

fn registry(state: &AppState) -> ApiResult<&aiwatcher_evaluation::Registry> {
    state
        .evaluations
        .as_deref()
        .ok_or(ApiError::EvaluationDisabled)
}

fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

/// Publish a scorecard. Idempotent by content: the same measurements answer
/// with the version that is already there, and the head moves to it either
/// way.
///
/// Nothing here is code. A scorer is a name this deployment implements and the
/// parameters that name takes, so publishing a card is not a way to run
/// something on the server's host.
#[utoipa::path(post, path = "/api/v1/evaluation-scorecards", request_body = Scorecard,
    responses((status = 200, body = ScorecardVersion), (status = 400, body = crate::error::ErrorBody),
    (status = 403, body = crate::error::ErrorBody), (status = 501, body = crate::error::ErrorBody)),
    tag = "evaluation")]
async fn publish_scorecard(
    State(state): State<AppState>,
    caller: Caller,
    Json(scorecard): Json<Scorecard>,
) -> ApiResult<Json<ScorecardVersion>> {
    let identity = caller.require(Role::Editor)?;
    Ok(Json(
        registry(&state)?
            .publish_scorecard(&scorecard, &identity.subject, now())
            .await?,
    ))
}

/// Every scorecard this instance holds, at its current version.
///
/// A head carries the metrics its version declares, so a form choosing what to
/// run says what each card would measure without opening it.
#[utoipa::path(get, path = "/api/v1/evaluation-scorecards",
    responses((status = 200, body = ScorecardPage), (status = 501, body = crate::error::ErrorBody)),
    tag = "evaluation")]
async fn list_scorecards(
    State(state): State<AppState>,
    caller: Caller,
) -> ApiResult<Json<ScorecardPage>> {
    caller.require(Role::Viewer)?;
    Ok(Json(ScorecardPage {
        scorecards: registry(&state)?.scorecards().await?,
    }))
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
struct VersionQuery {
    /// Absent means the current one. A run names a concrete version, so this
    /// is how a reader opens the card a result was actually measured under.
    version: Option<String>,
}

/// One scorecard, at the version asked for or at the head.
#[utoipa::path(get, path = "/api/v1/evaluation-scorecards/{name}",
    params(("name" = String, Path), VersionQuery),
    responses((status = 200, body = ScorecardVersion), (status = 404, body = crate::error::ErrorBody),
    (status = 501, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn get_scorecard(
    State(state): State<AppState>,
    caller: Caller,
    Path(name): Path<String>,
    Query(query): Query<VersionQuery>,
) -> ApiResult<Json<ScorecardVersion>> {
    caller.require(Role::Viewer)?;
    registry(&state)?
        .scorecard(&name, query.version.as_deref())
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("scorecard {name}")))
}
