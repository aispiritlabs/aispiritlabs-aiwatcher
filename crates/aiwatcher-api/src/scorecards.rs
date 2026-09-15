//! What an evaluation measures, as a resource a run can name.
//!
//! The suite list beside this one is an aggregate of reports — a name a
//! producer sent, learnt after the fact — so nothing in it can be run again. A
//! scorecard is the declaration: which scorers a case goes through, what each
//! one writes, and which end of it is better. It is authored, so it is
//! versioned by its content and a run names the concrete version.

use aiwatcher_evaluation::{
    Scorecard, ScorecardDiff, ScorecardPage, ScorecardVersion, ScorecardVersions,
};
use axum::extract::{Path, Query};
use axum::routing::get;
use axum::{Json, Router};
use utoipa::OpenApi;

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::evaluation_scope::{EvaluationRead, EvaluationWrite};
use crate::state::AppState;

/// This module's operations, as the contract they satisfy.
#[derive(OpenApi)]
#[openapi(paths(
    publish_scorecard,
    list_scorecards,
    get_scorecard,
    list_scorecard_versions,
    diff_scorecard
))]
struct Api;

/// The operations this module serves. Composed by [`crate::openapi`].
#[must_use]
pub fn openapi() -> utoipa::openapi::OpenApi {
    crate::project_scope::openapi(Api::openapi())
}

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
        .route(
            "/evaluation-scorecards",
            get(list_scorecards).post(publish_scorecard),
        )
        .route("/evaluation-scorecards/{name}", get(get_scorecard))
        .route(
            "/evaluation-scorecards/{name}/versions",
            get(list_scorecard_versions),
        )
        .route("/evaluation-scorecards/{name}/diff", get(diff_scorecard))
}

#[derive(serde::Deserialize)]
struct NamedPath {
    name: String,
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
    write: EvaluationWrite,
    caller: Caller,
    Json(scorecard): Json<Scorecard>,
) -> ApiResult<Json<ScorecardVersion>> {
    let registry = write.authorize().await?;
    let identity = &caller.0;
    Ok(Json(
        registry
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
    EvaluationRead(registry): EvaluationRead,
) -> ApiResult<Json<ScorecardPage>> {
    Ok(Json(ScorecardPage {
        scorecards: registry.scorecards().await?,
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
    EvaluationRead(registry): EvaluationRead,
    Path(NamedPath { name }): Path<NamedPath>,
    Query(query): Query<VersionQuery>,
) -> ApiResult<Json<ScorecardVersion>> {
    registry
        .scorecard(&name, query.version.as_deref())
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("scorecard {name}")))
}

/// Every version of one card, newest first — each whole, so a form can start
/// from any of them and a reader can pick two to compare.
#[utoipa::path(get, path = "/api/v1/evaluation-scorecards/{name}/versions",
    params(("name" = String, Path)),
    responses((status = 200, body = ScorecardVersions), (status = 404, body = crate::error::ErrorBody),
    (status = 501, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn list_scorecard_versions(
    EvaluationRead(registry): EvaluationRead,
    Path(NamedPath { name }): Path<NamedPath>,
) -> ApiResult<Json<ScorecardVersions>> {
    registry
        .scorecard_versions(&name)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("scorecard {name}")))
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
struct DiffQuery {
    /// The version read as before.
    from: String,
    /// The version read as after.
    to: String,
}

/// What changed from one version of a card to another: the metrics added,
/// removed and changed, each changed field of a scorer by JSON pointer, and
/// what each metric was derived to be on both sides — the part a version does
/// not hold.
#[utoipa::path(get, path = "/api/v1/evaluation-scorecards/{name}/diff",
    params(("name" = String, Path), DiffQuery),
    responses((status = 200, body = ScorecardDiff), (status = 404, body = crate::error::ErrorBody),
    (status = 501, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn diff_scorecard(
    EvaluationRead(registry): EvaluationRead,
    Path(NamedPath { name }): Path<NamedPath>,
    Query(query): Query<DiffQuery>,
) -> ApiResult<Json<ScorecardDiff>> {
    registry
        .scorecard_diff(&name, &query.from, &query.to)
        .await?
        .map(Json)
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "scorecard {name} at {} and {}",
                query.from, query.to
            ))
        })
}
