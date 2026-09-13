//! Feedback into regression cases, through people (FTI C4).
//!
//! A proposal names where something was noticed and the dataset the case would
//! join; people write the expected answer, approve it, and publish the approved
//! cases as a new version of that curation dataset. The review's rules are
//! `aiwatcher_evaluation::review`'s; what this module adds is the one step that
//! spans two registries — reading the dataset's current version and writing
//! the next — in the order that survives a crash between them: the version
//! first, then the proposals marked with it.

use aiwatcher_auth::Role;
use aiwatcher_datasets::{PublishDatasetRequest, PublishedDataset};
use aiwatcher_evaluation::{
    AssessmentTargetQuery, CaseProposal, ReviewAction, ReviewItem, ReviewPage, ReviewState,
    TargetReviews,
};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use utoipa::OpenApi;

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// This module's operations, as the contract they satisfy.
#[derive(OpenApi)]
#[openapi(paths(
    list_reviews,
    reviews_of_target,
    propose_case,
    review_case,
    publish_reviews
))]
struct Api;

/// The operations this module serves. Composed by [`crate::openapi`].
#[must_use]
pub fn openapi() -> utoipa::openapi::OpenApi {
    Api::openapi()
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/evaluation-reviews",
            get(list_reviews).post(propose_case),
        )
        .route("/api/v1/evaluation-reviews/publish", post(publish_reviews))
        .route(
            "/api/v1/evaluation-reviews/of-target",
            get(reviews_of_target),
        )
        .route("/api/v1/evaluation-reviews/{id}/actions", post(review_case))
}

fn evaluations(state: &AppState) -> ApiResult<&aiwatcher_evaluation::Registry> {
    state
        .evaluations
        .as_deref()
        .ok_or(ApiError::EvaluationDisabled)
}

fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

#[derive(Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
struct DatasetQuery {
    /// The curation dataset the cases join.
    dataset: String,
}

/// Every proposal for one dataset, oldest first, at its current revision.
#[utoipa::path(get, path = "/api/v1/evaluation-reviews", params(DatasetQuery),
    responses((status = 200, body = ReviewPage), (status = 501, body = crate::error::ErrorBody)),
    tag = "evaluation")]
async fn list_reviews(
    State(state): State<AppState>,
    caller: Caller,
    Query(query): Query<DatasetQuery>,
) -> ApiResult<Json<ReviewPage>> {
    caller.require(Role::Viewer)?;
    Ok(Json(evaluations(&state)?.reviews(&query.dataset).await?))
}

/// Every proposal seen on one target — a trace, a span, a session or a case
/// of a result — whichever dataset each would join, at its current revision.
/// What a case's judgements read to say the case is already under review.
#[utoipa::path(get, path = "/api/v1/evaluation-reviews/of-target", params(AssessmentTargetQuery),
    responses((status = 200, body = TargetReviews), (status = 400, body = crate::error::ErrorBody),
    (status = 501, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn reviews_of_target(
    State(state): State<AppState>,
    caller: Caller,
    Query(query): Query<AssessmentTargetQuery>,
) -> ApiResult<Json<TargetReviews>> {
    caller.require(Role::Viewer)?;
    Ok(Json(
        evaluations(&state)?.reviews_of(&query.target()?).await?,
    ))
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ProposedCase {
    pub review: ReviewItem,
    /// False when this landed on a review already under way for that target.
    pub created: bool,
}

/// Propose a case. Proposing what was noticed on the same target for the same
/// dataset again answers the review already under way, 200 rather than 201.
///
/// Without a question, a case target is read at `at` — the position a
/// comparison row carries — for what the cohort asked and what the variant
/// answered, and the proposal is `measured`. A trace holds no words to read.
#[utoipa::path(post, path = "/api/v1/evaluation-reviews", request_body = CaseProposal,
    responses((status = 201, body = ProposedCase), (status = 200, body = ProposedCase),
    (status = 400, body = crate::error::ErrorBody), (status = 501, body = crate::error::ErrorBody)),
    tag = "evaluation")]
async fn propose_case(
    State(state): State<AppState>,
    caller: Caller,
    Json(proposal): Json<CaseProposal>,
) -> ApiResult<(StatusCode, Json<ProposedCase>)> {
    let subject = caller.require(Role::Editor)?.log_subject().to_owned();
    let (review, created) = evaluations(&state)?
        .propose_case(&proposal, &subject, now())
        .await?;
    Ok((
        if created {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        },
        Json(ProposedCase { review, created }),
    ))
}

/// Write an expected answer, approve or reject. Approving words somebody using
/// the application said — `observed` content — takes the admin role.
#[utoipa::path(post, path = "/api/v1/evaluation-reviews/{id}/actions",
    params(("id" = String, Path), DatasetQuery), request_body = ReviewAction,
    responses((status = 200, body = ReviewItem), (status = 400, body = crate::error::ErrorBody),
    (status = 403, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody),
    (status = 501, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn review_case(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Query(query): Query<DatasetQuery>,
    Json(action): Json<ReviewAction>,
) -> ApiResult<Json<ReviewItem>> {
    let subject = caller.require(Role::Editor)?.log_subject().to_owned();
    evaluations(&state)?
        .review_case(
            &query.dataset,
            &id,
            &action,
            &subject,
            caller.require(Role::Admin).is_ok(),
            now(),
        )
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("review {id} of {}", query.dataset)))
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PublishedReviews {
    pub dataset: PublishedDataset,
    /// The proposals that version holds, marked with it.
    pub published: Vec<ReviewItem>,
}

/// Publish every approved case as a new version of its dataset: the current
/// version's rows, then one row per case, `review-…` by its ID.
///
/// A dataset whose rows are not cases — `case_id`, `input`, `expected` — is
/// refused rather than given rows of a second shape. Publishing the same
/// approved set again lands on the same version.
#[utoipa::path(post, path = "/api/v1/evaluation-reviews/publish", params(DatasetQuery),
    responses((status = 200, body = PublishedReviews), (status = 422, body = crate::error::ErrorBody),
    (status = 501, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn publish_reviews(
    State(state): State<AppState>,
    caller: Caller,
    Query(query): Query<DatasetQuery>,
) -> ApiResult<Json<PublishedReviews>> {
    let subject = caller.require(Role::Editor)?.log_subject().to_owned();
    let evaluations = evaluations(&state)?;
    let datasets = state
        .datasets
        .as_ref()
        .ok_or(ApiError::DatasetRegistryDisabled)?;
    let approved: Vec<ReviewItem> = evaluations
        .reviews(&query.dataset)
        .await?
        .items
        .into_iter()
        .filter(|item| item.state == ReviewState::Approved)
        .collect();
    if approved.is_empty() {
        return Err(refused(format!(
            "{} has no approved case to publish",
            query.dataset
        )));
    }
    let current = datasets
        .datasets()
        .await?
        .datasets
        .into_iter()
        .find(|dataset| dataset.name == query.dataset);
    let (description, prior, mut items, mut columns) = match &current {
        Some(head) => {
            let version = datasets
                .verified_version(&head.name, &head.latest.version)
                .await?;
            (
                version.description,
                Some(version.summary.version),
                version.items,
                version.summary.columns,
            )
        }
        None => (String::new(), None, Vec::new(), Vec::new()),
    };
    let shape = ["case_id", "input", "expected"];
    if !items.is_empty()
        && !shape
            .iter()
            .all(|column| columns.iter().any(|c| c == column))
    {
        return Err(refused(format!(
            "{}'s rows are not cases — case_id, input and expected — so reviewed cases are not \
             rows of it",
            query.dataset
        )));
    }
    for column in shape {
        if !columns.iter().any(|c| c == column) {
            columns.push(column.to_owned());
        }
    }
    let held: std::collections::BTreeSet<String> = items
        .iter()
        .filter_map(|row| row.get("case_id")?.as_str().map(str::to_owned))
        .collect();
    for item in &approved {
        if held.contains(&item.case_id()) {
            continue;
        }
        items.push(BTreeMap::from([
            ("case_id".to_owned(), Value::String(item.case_id())),
            ("input".to_owned(), json!({ "question": item.question })),
            (
                "expected".to_owned(),
                json!({ "answer": item.expected.clone().unwrap_or_default() }),
            ),
        ]));
    }
    let published = datasets
        .publish(PublishDatasetRequest {
            name: query.dataset.clone(),
            description,
            recipe: None,
            pipeline: format!(
                "// Not a query: {} and {} cases people reviewed and approved in aiwatcher.",
                prior.map_or_else(
                    || "no earlier version".to_owned(),
                    |prior| format!("{}@{prior}", query.dataset)
                ),
                approved.len()
            ),
            engine: Default::default(),
            columns,
            items,
            source: "evaluation-reviews".to_owned(),
            window_seconds: None,
            produced_by: Some(format!("evaluation-reviews/{}", query.dataset)),
            execution_id: None,
        })
        .await?;
    let version = published.dataset.latest.version.clone();
    let marked = evaluations
        .reviews_published(&approved, &version, &subject, now())
        .await?;
    Ok(Json(PublishedReviews {
        dataset: published,
        published: marked,
    }))
}

fn refused(summary: String) -> ApiError {
    ApiError::PlanRefused {
        summary,
        problems: Vec::new(),
    }
}
