//! Saved Flow curation recipes and the versioned dataset artifacts they produce.
//!
//! The PHP service executes a transformation; these routes persist its exact
//! script and output behind an instance editor role (legacy) or a current
//! project editor grant (scoped).

use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};

use aiwatcher_datasets::{
    DatasetPage, DatasetRowsPage, PipelinePage, PublishDatasetRequest, PublishedDataset,
    RecipePage, SavePipelineRequest, SaveRecipeRequest, SavedPipeline, SavedRecipe,
};
use serde::Deserialize;

use crate::dataset_scope::{DatasetRead, DatasetWrite};
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
    publish_dataset_sample,
    list_recipes,
    save_recipe,
    list_pipelines,
    get_pipeline_revision,
    save_pipeline,
    search_block_library,
    save_block_template,
))]
struct Api;

/// The operations this module serves. Composed by [`crate::openapi`].
#[must_use]
pub fn openapi() -> utoipa::openapi::OpenApi {
    let mut api = Api::openapi();
    // Both route families execute these same handlers/extractors. Derive the
    // scoped contract from the legacy operations so their bodies cannot drift.
    for (path, mut item) in api.paths.paths.clone() {
        for (operation, write) in [(&mut item.get, false), (&mut item.post, true)] {
            let Some(operation) = operation else { continue };
            operation.operation_id = operation
                .operation_id
                .take()
                .map(|id| format!("project_{id}"));
            let parameters = operation.parameters.get_or_insert_with(Vec::new);
            for name in ["organization", "project"] {
                parameters.push(
                    utoipa::openapi::path::ParameterBuilder::new()
                        .name(name)
                        .parameter_in(utoipa::openapi::path::ParameterIn::Path)
                        .required(utoipa::openapi::Required::True)
                        .schema(Some(
                            utoipa::openapi::ObjectBuilder::new()
                                .schema_type(utoipa::openapi::Type::String)
                                .format(Some(utoipa::openapi::SchemaFormat::KnownFormat(
                                    utoipa::openapi::KnownFormat::Uuid,
                                ))),
                        ))
                        .build(),
                );
            }
            if write {
                parameters.push(
                    utoipa::openapi::path::ParameterBuilder::new()
                        .name("X-AIWatcher-IAM")
                        .parameter_in(utoipa::openapi::path::ParameterIn::Header)
                        .required(utoipa::openapi::Required::True)
                        .description(Some("Required value: 1"))
                        .schema(Some(
                            utoipa::openapi::ObjectBuilder::new()
                                .schema_type(utoipa::openapi::Type::String),
                        ))
                        .build(),
                );
            }
            for status in ["401", "403", "404", "503"] {
                operation
                    .responses
                    .responses
                    .entry(status.into())
                    .or_insert_with(|| {
                        utoipa::openapi::ResponseBuilder::new()
                            .description("Current project authorization failed or is unavailable")
                            .build()
                            .into()
                    });
            }
        }
        api.paths.paths.insert(
            path.replacen(
                "/api/v1",
                "/api/v1/orgs/{organization}/projects/{project}",
                1,
            ),
            item,
        );
    }
    api
}

pub fn router() -> Router<AppState> {
    Router::new().nest("/api/v1", resource_router()).nest(
        "/api/v1/orgs/{organization}/projects/{project}",
        resource_router()
            .layer(axum::Extension(crate::dataset_scope::ScopedRoute))
            .layer(tower_http::set_header::SetResponseHeaderLayer::overriding(
                axum::http::header::CACHE_CONTROL,
                axum::http::HeaderValue::from_static("no-store"),
            )),
    )
}

fn resource_router() -> Router<AppState> {
    Router::new()
        .route("/datasets", get(list_datasets).post(publish_dataset))
        .route("/dataset-rows", get(get_dataset_rows))
        .route(
            "/dataset-samples",
            axum::routing::post(publish_dataset_sample),
        )
        .route("/curations", get(list_recipes).post(save_recipe))
        .route(
            "/curation-library",
            get(search_block_library).post(save_block_template),
        )
        .route(
            "/curation-pipelines",
            get(list_pipelines).post(save_pipeline),
        )
        .route(
            "/curation-pipelines/{name}/revisions/{revision}",
            get(get_pipeline_revision),
        )
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

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct LibraryQuery {
    pub search: Option<String>,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

/// Search reusable block solutions in the selected registry.
#[utoipa::path(
    get, path = "/api/v1/curation-library", params(LibraryQuery),
    responses(
        (status = 200, body = aiwatcher_datasets::BlockTemplatePage),
        (status = 400, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ), tag = "datasets",
)]
async fn search_block_library(
    DatasetRead(registry): DatasetRead,
    Query(query): Query<LibraryQuery>,
) -> ApiResult<Json<aiwatcher_datasets::BlockTemplatePage>> {
    Ok(Json(
        registry
            .block_templates(
                query.search.as_deref().unwrap_or_default(),
                query.offset.unwrap_or(0),
                query.limit.unwrap_or(24),
            )
            .await?,
    ))
}

/// Publish a reusable solution through the same registry operation as the seed.
#[utoipa::path(
    post, path = "/api/v1/curation-library",
    request_body = aiwatcher_datasets::SaveBlockTemplateRequest,
    responses(
        (status = 200, body = aiwatcher_datasets::BlockTemplate),
        (status = 400, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ), tag = "datasets",
)]
async fn save_block_template(
    registry: DatasetWrite,
    Json(request): Json<aiwatcher_datasets::SaveBlockTemplateRequest>,
) -> ApiResult<Json<aiwatcher_datasets::BlockTemplate>> {
    let registry = registry.authorize().await?;
    Ok(Json(registry.save_block_template(request).await?))
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
async fn list_datasets(DatasetRead(registry): DatasetRead) -> ApiResult<Json<DatasetPage>> {
    Ok(Json(registry.datasets().await?))
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
    DatasetRead(registry): DatasetRead,
    Query(query): Query<DatasetRowsQuery>,
) -> ApiResult<Json<DatasetRowsPage>> {
    Ok(Json(
        registry
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

/// Persist exact execution output as an immutable version, optionally labelled as a sample.
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
    registry: DatasetWrite,
    Json(request): Json<PublishDatasetRequest>,
) -> ApiResult<(StatusCode, Json<PublishedDataset>)> {
    let registry = registry.authorize().await?;
    let published = registry.publish(request).await?;
    let status = if published.created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(published)))
}

/// Publish explicitly labelled limited output. Sample metadata is required.
/// A separate route prevents older servers from ignoring a new sample field.
#[utoipa::path(
    post,
    path = "/api/v1/dataset-samples",
    request_body = PublishDatasetRequest,
    responses(
        (status = 201, body = PublishedDataset),
        (status = 200, body = PublishedDataset),
        (status = 400, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 413, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "datasets",
)]
async fn publish_dataset_sample(
    registry: DatasetWrite,
    Json(request): Json<PublishDatasetRequest>,
) -> ApiResult<(StatusCode, Json<PublishedDataset>)> {
    if request.sample.is_none() {
        return Err(aiwatcher_datasets::RegistryError::Invalid(
            "sample metadata is required".into(),
        )
        .into());
    }
    publish_dataset(registry, Json(request)).await
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
async fn list_recipes(DatasetRead(registry): DatasetRead) -> ApiResult<Json<RecipePage>> {
    Ok(Json(registry.recipes().await?))
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
    registry: DatasetWrite,
    Json(request): Json<SaveRecipeRequest>,
) -> ApiResult<(StatusCode, Json<SavedRecipe>)> {
    let registry = registry.authorize().await?;
    let saved = registry.save_recipe(request).await?;
    let status = if saved.created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(saved)))
}

/// Every saved curation pipeline, newest save first.
#[utoipa::path(
    get,
    path = "/api/v1/curation-pipelines",
    responses(
        (status = 200, body = PipelinePage),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "data-curation",
)]
async fn list_pipelines(DatasetRead(registry): DatasetRead) -> ApiResult<Json<PipelinePage>> {
    Ok(Json(registry.pipelines().await?))
}

#[derive(Deserialize)]
struct PipelineRevisionPath {
    name: String,
    revision: String,
}

/// Read an exact saved pipeline, including its layout and notebook pins.
/// A missing revision never falls back to the current head.
#[utoipa::path(
    get,
    path = "/api/v1/curation-pipelines/{name}/revisions/{revision}",
    params(("name" = String, Path), ("revision" = String, Path)),
    responses(
        (status = 200, body = aiwatcher_datasets::CurationPipeline),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "data-curation",
)]
async fn get_pipeline_revision(
    DatasetRead(registry): DatasetRead,
    Path(PipelineRevisionPath { name, revision }): Path<PipelineRevisionPath>,
) -> ApiResult<Json<aiwatcher_datasets::CurationPipeline>> {
    let pipeline = registry
        .pipeline(&name, Some(&revision))
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("pipeline {name}@{revision}")))?;
    Ok(Json(pipeline))
}

/// Save a content-addressed revision of a block pipeline.
///
/// The blocks are checked as a chain here and nowhere else — the canvas draws
/// what it is told and implements no rules of its own, exactly as the
/// annotation canvas does not re-implement the shape validator. A refusal is a
/// 422 whose `details` carry every problem at once.
#[utoipa::path(
    post,
    path = "/api/v1/curation-pipelines",
    request_body = SavePipelineRequest,
    responses(
        (status = 201, body = SavedPipeline, description = "A new revision was stored"),
        (status = 200, body = SavedPipeline, description = "This exact revision already existed"),
        (status = 400, body = crate::error::ErrorBody),
        (status = 413, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody, description = "The blocks do not form a runnable chain"),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "data-curation",
)]
async fn save_pipeline(
    registry: DatasetWrite,
    Json(request): Json<SavePipelineRequest>,
) -> ApiResult<(StatusCode, Json<SavedPipeline>)> {
    let registry = registry.authorize().await?;
    let saved = registry.save_pipeline(request).await?;
    let status = if saved.created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(saved)))
}
