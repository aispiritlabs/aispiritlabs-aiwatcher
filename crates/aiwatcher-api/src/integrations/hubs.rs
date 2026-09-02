//! Searching public dataset hubs, and what the answers are not.
//!
//! Its own module because these are the only read routes in this crate that
//! leave the building. Everything else here answers from the log, the read
//! model or the object store; these two ask Kaggle and Hugging Face, and a
//! reader of the router should be able to see that at a glance.
//!
//! The guardrail lives in [`aiwatcher_annotations::integrations::hubs`] rather than here, so
//! that a second caller cannot route around it. What this layer adds is the
//! 501: a hub nobody configured is not an empty result, and saying so with the
//! variable name is the same choice `RegistryDisabled` makes.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};

use aiwatcher_annotations::integrations::hubs::{HubQuery, HubSearchPage, HubStatus, Hubs};
use serde::Serialize;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use utoipa::OpenApi;

/// This module's operations, as the contract they satisfy.
///
/// Derived beside the router rather than listed in the root document, so
/// adding a route and forgetting the contract is a change to one file rather
/// than a change to two files that has to be noticed in the second.
#[derive(OpenApi)]
#[openapi(paths(list_hubs, search_hubs,))]
struct Api;

/// The operations this module serves. Composed by [`crate::openapi`].
#[must_use]
pub fn openapi() -> utoipa::openapi::OpenApi {
    Api::openapi()
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/dataset-hubs", get(list_hubs))
        .route("/api/v1/dataset-hubs/search", get(search_hubs))
}

/// What this instance can search, before anybody types a query.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct HubsPage {
    pub hubs: Vec<HubStatus>,
    /// The same sentence every search response carries. Repeated here so a
    /// panel that renders the hub list before the first search has it.
    pub notice: String,
}

fn hubs(state: &AppState) -> ApiResult<&Arc<Hubs>> {
    state.hubs.as_ref().ok_or(ApiError::HubsDisabled)
}

/// Which hubs are configured, and the variable to set for each that is not.
#[utoipa::path(
    get,
    path = "/api/v1/dataset-hubs",
    responses(
        (status = 200, body = HubsPage),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "datasets",
)]
async fn list_hubs(State(state): State<AppState>) -> ApiResult<Json<HubsPage>> {
    let configured = hubs(&state)?;
    Ok(Json(HubsPage {
        hubs: configured.status(),
        notice: aiwatcher_annotations::integrations::hubs::NOTICE.to_owned(),
    }))
}

/// Search the configured hubs.
///
/// Always 200 when at least one hub is configured, even if every one of them
/// failed: the per-hub status carries the failure. A search where Kaggle is
/// rate-limited and Hugging Face answered is a partial result, and returning a
/// 502 for it would throw away the half that worked.
#[utoipa::path(
    get,
    path = "/api/v1/dataset-hubs/search",
    params(HubQuery),
    responses(
        (status = 200, body = HubSearchPage),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "datasets",
)]
async fn search_hubs(
    State(state): State<AppState>,
    Query(query): Query<HubQuery>,
) -> ApiResult<Json<HubSearchPage>> {
    Ok(Json(hubs(&state)?.search(&query).await))
}
