//! Saved Flow curation recipes and the versioned dataset artifacts they produce.
//!
//! The PHP service executes a transformation; these routes persist its exact
//! script and output behind the same editor permission as prompt authoring.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};

use aiwatcher_datasets::{
    DatasetPage, DatasetRowsPage, PublishDatasetRequest, PublishedDataset, RecipePage, Registry,
    SaveRecipeRequest, SavedRecipe,
};
use serde::Deserialize;

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use utoipa::OpenApi;

/// This module's operations, as the contract they satisfy.
///
/// Derived here rather than listed in the root document, so a route added to
/// `router` below and forgotten here is a change to one file rather than a
/// change to two files that has to be noticed in the second.
#[derive(OpenApi)]
#[openapi(paths(
    list_datasets,
    get_dataset_rows,
    publish_dataset,
    list_recipes,
    save_recipe,
))]
struct Api;

/// The operations this module serves. Composed by [`crate::openapi`].
#[must_use]
pub fn openapi() -> utoipa::openapi::OpenApi {
    Api::openapi()
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/datasets", get(list_datasets).post(publish_dataset))
        .route("/api/v1/dataset-rows", get(get_dataset_rows))
        .route("/api/v1/curations", get(list_recipes).post(save_recipe))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct DatasetRowsQuery {
    pub name: String,
    pub version: Option<String>,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
    pub search: Option<String>,
}

fn registry(state: &AppState) -> ApiResult<&Arc<Registry>> {
    state
        .datasets
        .as_ref()
        .ok_or(ApiError::DatasetRegistryDisabled)
}

fn may_author(caller: &Caller) -> ApiResult<()> {
    caller.require(aiwatcher_auth::Role::Editor).map(|_| ())
}

/// Every saved dataset, newest execution first.
#[utoipa::path(
    get,
    path = "/api/v1/datasets",
    responses(
        (status = 200, body = DatasetPage),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "datasets",
)]
async fn list_datasets(State(state): State<AppState>) -> ApiResult<Json<DatasetPage>> {
    Ok(Json(registry(&state)?.datasets().await?))
}

/// One immutable dataset version, returned in small slices for an interactive viewer.
#[utoipa::path(
    get,
    path = "/api/v1/dataset-rows",
    params(DatasetRowsQuery),
    responses(
        (status = 200, body = DatasetRowsPage),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "datasets",
)]
async fn get_dataset_rows(
    State(state): State<AppState>,
    Query(query): Query<DatasetRowsQuery>,
) -> ApiResult<Json<DatasetRowsPage>> {
    Ok(Json(
        registry(&state)?
            .rows(
                &query.name,
                query.version.as_deref(),
                query.offset.unwrap_or(0),
                query.limit.unwrap_or(50),
                query.search.as_deref(),
            )
            .await?,
    ))
}

/// Persist one completed Flow PHP execution as an immutable dataset version.
#[utoipa::path(
    post,
    path = "/api/v1/datasets",
    request_body = PublishDatasetRequest,
    responses(
        (status = 201, body = PublishedDataset, description = "A new version was stored"),
        (status = 200, body = PublishedDataset, description = "This exact version already existed"),
        (status = 400, body = crate::error::ErrorBody),
        (status = 413, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "datasets",
)]
async fn publish_dataset(
    State(state): State<AppState>,
    caller: Caller,
    Json(request): Json<PublishDatasetRequest>,
) -> ApiResult<(StatusCode, Json<PublishedDataset>)> {
    may_author(&caller)?;
    let published = registry(&state)?.publish(request).await?;
    let status = if published.created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(published)))
}

/// Every saved Flow PHP recipe, newest save first.
#[utoipa::path(
    get,
    path = "/api/v1/curations",
    responses(
        (status = 200, body = RecipePage),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "data-curation",
)]
async fn list_recipes(State(state): State<AppState>) -> ApiResult<Json<RecipePage>> {
    Ok(Json(registry(&state)?.recipes().await?))
}

/// Save a content-addressed revision of a Flow PHP recipe.
#[utoipa::path(
    post,
    path = "/api/v1/curations",
    request_body = SaveRecipeRequest,
    responses(
        (status = 201, body = SavedRecipe, description = "A new revision was stored"),
        (status = 200, body = SavedRecipe, description = "This exact revision already existed"),
        (status = 400, body = crate::error::ErrorBody),
        (status = 413, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "data-curation",
)]
async fn save_recipe(
    State(state): State<AppState>,
    caller: Caller,
    Json(request): Json<SaveRecipeRequest>,
) -> ApiResult<(StatusCode, Json<SavedRecipe>)> {
    may_author(&caller)?;
    let saved = registry(&state)?.save_recipe(request).await?;
    let status = if saved.created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(saved)))
}
