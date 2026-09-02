//! The annotation surface: projects, images, drawings, reviews and exports.
//!
//! Its own module for the same reason the prompt registry has one — a different
//! store with a different lifetime — and for one more. These are the only
//! routes that accept *bytes*, and the only ones whose 4xx carries a list
//! rather than a sentence: a drawing can be wrong in nine ways at once, and a
//! labeller who has to discover them one round trip at a time stops using the
//! tool. See ADR_0017.
//!
//! Names may contain slashes (`floor-plans/dom-projekt`), so every identifier
//! travels as a query parameter rather than a path segment — the same choice
//! `/api/v1/dataset-rows` made, for the same reason.

use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;

use aiwatcher_annotations::{
    AnnotationProject, BuiltExport, ExportManifest, ExportPage, ExportRequest, ImageDetail,
    ImageFilter, ImageHead, ImagePage, ImportReport, ImportRequest, ProjectPage, ProjectSummary,
    RegisterImageRequest, Registry, ReviewRequest, ReviewState, SaveProjectRequest,
    SaveRevisionRequest, SavedRevision, SourcePage, SourceUsage, Split, StoredBlob,
};

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use utoipa::OpenApi;

/// Matches [`aiwatcher_annotations`]'s own blob cap. axum's default body limit
/// is two megabytes, which is under a 300 dpi catalogue plan, so this route
/// raises it — and only this route.
const MAX_UPLOAD_BYTES: usize = 16 * 1024 * 1024;

/// This module's operations, as the contract they satisfy.
///
/// Derived beside the router rather than listed in the root document, so
/// adding a route and forgetting the contract is a change to one file rather
/// than a change to two files that has to be noticed in the second.
#[derive(OpenApi)]
#[openapi(paths(
    list_projects,
    get_project,
    save_project,
    list_images,
    register_image,
    get_image,
    save_revision,
    review_image,
    list_exports,
    build_export,
    get_export,
    get_export_coco,
    upload_blob,
    get_blob,
    list_sources,
    import_images,
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
            "/api/v1/annotation-projects",
            get(list_projects).post(save_project),
        )
        .route("/api/v1/annotation-project", get(get_project))
        .route(
            "/api/v1/annotation-images",
            get(list_images).post(register_image),
        )
        .route("/api/v1/annotation-image", get(get_image))
        .route("/api/v1/annotation-imports", post(import_images))
        .route("/api/v1/annotation-revisions", post(save_revision))
        .route("/api/v1/annotation-reviews", post(review_image))
        .route(
            "/api/v1/annotation-exports",
            get(list_exports).post(build_export),
        )
        .route("/api/v1/annotation-export", get(get_export))
        .route("/api/v1/annotation-export/coco", get(get_export_coco))
        .route(
            "/api/v1/annotation-blobs",
            post(upload_blob).layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES)),
        )
        .route("/api/v1/annotation-blobs/{image_id}", get(get_blob))
        .route("/api/v1/annotation-sources", get(list_sources))
}

fn registry(state: &AppState) -> ApiResult<&Arc<Registry>> {
    state
        .annotations
        .as_ref()
        .ok_or(ApiError::AnnotationRegistryDisabled)
}

/// Every write here is an editor's. Reading a corpus is one job and changing
/// what a model will be trained on is another.
fn may_author(caller: &Caller) -> ApiResult<()> {
    caller.require(aiwatcher_auth::Role::Editor).map(|_| ())
}

/// Who a revision or a review is attributed to.
///
/// Provenance, not authorisation — the role check above is what decides
/// whether the write happens. An instance running `AIWATCHER_AUTH_MODE=none`
/// records `anonymous`, which is honest about a deployment where nobody signs
/// in.
fn author(caller: &Caller) -> String {
    let identity = caller.identity();
    if identity.subject.is_empty() {
        "anonymous".to_owned()
    } else {
        identity.subject.clone()
    }
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct ProjectQuery {
    pub name: String,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct ImagesQuery {
    pub project: String,
    pub review: Option<ReviewState>,
    pub split: Option<Split>,
    pub group_id: Option<String>,
    pub search: Option<String>,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct ImageQuery {
    pub project: String,
    pub image_id: String,
    /// Which revision's shapes to return. Omitted means the accepted one, then
    /// the newest — what a labeller reopening an image expects to see.
    pub revision: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct ExportQuery {
    pub project: String,
    pub export: String,
    /// One side of the split, for a trainer that wants its own file per split.
    pub split: Option<Split>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct SourcesQuery {
    /// Free text over name, summary, licence, labels and notes.
    pub q: Option<String>,
    /// The filter that matters: what a commercial model may be trained on.
    pub usage: Option<SourceUsage>,
    /// A label kind the corpus has to carry — `doors`, `scale`, `graph`.
    pub label: Option<String>,
}

// ── Projects ─────────────────────────────────────────────────────────────────

/// Every annotation project.
#[utoipa::path(
    get,
    path = "/api/v1/annotation-projects",
    responses(
        (status = 200, body = ProjectPage),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "annotations",
)]
async fn list_projects(State(state): State<AppState>) -> ApiResult<Json<ProjectPage>> {
    Ok(Json(registry(&state)?.projects().await?))
}

/// One project, with the counts that answer "is there enough data yet".
#[utoipa::path(
    get,
    path = "/api/v1/annotation-project",
    params(ProjectQuery),
    responses(
        (status = 200, body = ProjectSummary),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "annotations",
)]
async fn get_project(
    State(state): State<AppState>,
    Query(query): Query<ProjectQuery>,
) -> ApiResult<Json<ProjectSummary>> {
    Ok(Json(registry(&state)?.project_summary(&query.name).await?))
}

/// Create a project, or replace its description, split policy and label schema.
///
/// The schema version is re-derived from the classes. An unchanged class list
/// keeps its version and every accepted revision stays valid; a changed one is
/// a new version, and the next export names every revision drawn under the old
/// one as excluded rather than quietly relabelling them.
#[utoipa::path(
    post,
    path = "/api/v1/annotation-projects",
    request_body = SaveProjectRequest,
    responses(
        (status = 200, body = AnnotationProject),
        (status = 400, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "annotations",
)]
async fn save_project(
    State(state): State<AppState>,
    caller: Caller,
    Json(request): Json<SaveProjectRequest>,
) -> ApiResult<Json<AnnotationProject>> {
    may_author(&caller)?;
    Ok(Json(registry(&state)?.save_project(request).await?))
}

// ── Images ───────────────────────────────────────────────────────────────────

/// The images in a project, newest registration first.
#[utoipa::path(
    get,
    path = "/api/v1/annotation-images",
    params(ImagesQuery),
    responses(
        (status = 200, body = ImagePage),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "annotations",
)]
async fn list_images(
    State(state): State<AppState>,
    Query(query): Query<ImagesQuery>,
) -> ApiResult<Json<ImagePage>> {
    let filter = ImageFilter {
        review: query.review,
        split: query.split,
        group_id: query.group_id,
        search: query.search,
    };
    Ok(Json(
        registry(&state)?
            .images(
                &query.project,
                &filter,
                query.offset.unwrap_or(0),
                query.limit.unwrap_or(50),
            )
            .await?,
    ))
}

/// Register an image into a project, or correct what is known about it.
///
/// Re-registering keeps the revisions and the review state: a plan re-submitted
/// with a corrected `group_id` is a correction, and resetting its labels would
/// punish the correction.
#[utoipa::path(
    post,
    path = "/api/v1/annotation-images",
    request_body = RegisterImageRequest,
    responses(
        (status = 200, body = ImageHead),
        (status = 400, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "annotations",
)]
async fn register_image(
    State(state): State<AppState>,
    caller: Caller,
    Json(request): Json<RegisterImageRequest>,
) -> ApiResult<Json<ImageHead>> {
    may_author(&caller)?;
    Ok(Json(registry(&state)?.register_image(request).await?))
}

/// Register many images at once, from rows a Flow PHP pipeline produced.
///
/// The write half of dataset discovery. A hub search says a corpus exists; a
/// Flow query maps its file listing onto this schema; this registers the
/// result into a project somebody can draw on.
///
/// Two things about the response are worth reading rather than skipping. One
/// bad row does not fail the batch — `outcomes` names every refusal — and
/// `warnings` carries the states that *succeed* and still leave the corpus
/// worth less than it looks: unknown rights, and a group key that gave every
/// image its own family.
///
/// Always `dry_run` first from a UI. Six hundred images with the split key
/// mapped from a filename is not something to discover after the fact.
#[utoipa::path(
    post,
    path = "/api/v1/annotation-imports",
    request_body = ImportRequest,
    responses(
        (status = 200, body = ImportReport),
        (status = 400, body = crate::error::ErrorBody, description = "The asserted rights contradict what a human recorded about this corpus"),
        (status = 404, body = crate::error::ErrorBody),
        (status = 413, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "annotations",
)]
async fn import_images(
    State(state): State<AppState>,
    caller: Caller,
    Json(mut request): Json<ImportRequest>,
) -> ApiResult<Json<ImportReport>> {
    may_author(&caller)?;
    let registry = registry(&state)?;
    let (fetched, problems) = hydrate(&state, registry, &mut request).await;
    let mut report = registry.import_images(request).await?;
    report.fetched = fetched;
    report.warnings.extend(problems);
    Ok(Json(report))
}

/// Download the bytes for rows that name a hub asset and carry no content
/// address, and store them here.
///
/// This is the composition the import route exists to make, and it lives in
/// the API layer because it is the only place that holds both halves: the
/// registry writes an object store and reaches nothing, and `Hubs` reaches a
/// hub and writes nothing.
///
/// It runs for a dry run too. A row with no `image_id` is refused by
/// `images::check`, so a preview that skipped the download would reject every
/// row and teach the reader nothing about the batch — and blobs are addressed
/// by their content, so a dry run followed by an import stores each picture
/// once.
///
/// Three things bound it, and none of them is a flag:
///
/// * a row that already carries an `image_id` is left alone, which is every
///   batch whose pipeline did its own fetching;
/// * only a hub's own asset host is fetched — `Hubs::fetch` refuses the rest,
///   because "download this address for me" from inside a cluster is a
///   request-forgery primitive;
/// * a failure is a warning naming the row, never a failed batch. A hub asset
///   URL expires within hours of being listed, so the interesting case is a
///   preview from yesterday, and the reader needs to be told that rather than
///   handed a 502.
async fn hydrate(
    state: &AppState,
    registry: &Registry,
    request: &mut ImportRequest,
) -> (usize, Vec<String>) {
    let Some(hubs) = state.hubs.as_ref() else {
        return (0, Vec::new());
    };

    let mut fetched = 0;
    let mut problems = Vec::new();
    for row in &mut request.rows {
        if row.image_id.is_some() {
            continue;
        }
        let outcome = match hubs.fetch(&row.uri).await {
            // A URI this instance cannot fetch is not a problem yet: it may be
            // a perfectly good address somebody else already stored, and the
            // registry says so in its own words if it is not.
            Err(_) if !row.uri.starts_with("https://") => continue,
            Err(error) => Err(error),
            Ok((bytes, content_type)) => registry
                .put_blob(bytes, &content_type)
                .await
                .map_err(|error| error.to_string()),
        };

        match outcome {
            Ok(stored) => {
                fetched += 1;
                row.metadata
                    .insert("import.hub_uri".to_owned(), row.uri.clone().into());
                row.uri = stored.uri;
                row.image_id = Some(stored.image_id);
            }
            Err(error) => problems.push(format!("{}: {error}", row.uri)),
        }
    }
    (fetched, problems)
}

/// One image, its revision history, one revision's shapes, and the side of the
/// split its family is on.
#[utoipa::path(
    get,
    path = "/api/v1/annotation-image",
    params(ImageQuery),
    responses(
        (status = 200, body = ImageDetail),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "annotations",
)]
async fn get_image(
    State(state): State<AppState>,
    Query(query): Query<ImageQuery>,
) -> ApiResult<Json<ImageDetail>> {
    Ok(Json(
        registry(&state)?
            .image(&query.project, &query.image_id, query.revision.as_deref())
            .await?,
    ))
}

// ── Drawings ─────────────────────────────────────────────────────────────────

/// Save a drawing.
///
/// A revision that does not validate against the project's schema is not
/// stored at all, and the 422 carries every problem rather than the first.
/// Storing an invalid revision and marking it so would move the decision about
/// whether a half-drawn door is a training target to whatever reads it next.
#[utoipa::path(
    post,
    path = "/api/v1/annotation-revisions",
    request_body = SaveRevisionRequest,
    responses(
        (status = 201, body = SavedRevision, description = "A new revision was stored"),
        (status = 200, body = SavedRevision, description = "These exact shapes already existed"),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 413, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody, description = "The drawing was refused; `details` holds every problem"),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "annotations",
)]
async fn save_revision(
    State(state): State<AppState>,
    caller: Caller,
    Json(request): Json<SaveRevisionRequest>,
) -> ApiResult<(StatusCode, Json<SavedRevision>)> {
    may_author(&caller)?;
    let saved = registry(&state)?
        .save_revision(request, &author(&caller))
        .await?;
    let status = if saved.created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(saved)))
}

/// Move an image's review state, and pin the revision an export will read.
#[utoipa::path(
    post,
    path = "/api/v1/annotation-reviews",
    request_body = ReviewRequest,
    responses(
        (status = 200, body = ImageHead),
        (status = 400, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "annotations",
)]
async fn review_image(
    State(state): State<AppState>,
    caller: Caller,
    Json(request): Json<ReviewRequest>,
) -> ApiResult<Json<ImageHead>> {
    may_author(&caller)?;
    Ok(Json(
        registry(&state)?.review(request, &author(&caller)).await?,
    ))
}

// ── Exports ──────────────────────────────────────────────────────────────────

/// Every export built from a project, newest first.
#[utoipa::path(
    get,
    path = "/api/v1/annotation-exports",
    params(ProjectQuery),
    responses(
        (status = 200, body = ExportPage),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "annotations",
)]
async fn list_exports(
    State(state): State<AppState>,
    Query(query): Query<ProjectQuery>,
) -> ApiResult<Json<ExportPage>> {
    Ok(Json(registry(&state)?.exports(&query.name).await?))
}

/// Freeze the project as it stands into an immutable, content-addressed
/// manifest.
///
/// The reference a training run records is `project@export`. Two exports of an
/// unchanged project are one export, which is why re-running this nightly is
/// cheap and why a manifest is a comparison boundary rather than a timestamp.
#[utoipa::path(
    post,
    path = "/api/v1/annotation-exports",
    request_body = ExportRequest,
    responses(
        (status = 201, body = BuiltExport, description = "A new export was stored"),
        (status = 200, body = BuiltExport, description = "This exact export already existed"),
        (status = 400, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "annotations",
)]
async fn build_export(
    State(state): State<AppState>,
    caller: Caller,
    Json(request): Json<ExportRequest>,
) -> ApiResult<(StatusCode, Json<BuiltExport>)> {
    may_author(&caller)?;
    let built = registry(&state)?.export(request).await?;
    let status = if built.created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(built)))
}

/// One immutable manifest: its samples, its splits, and every exclusion with
/// its reason.
#[utoipa::path(
    get,
    path = "/api/v1/annotation-export",
    params(ExportQuery),
    responses(
        (status = 200, body = ExportManifest),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "annotations",
)]
async fn get_export(
    State(state): State<AppState>,
    Query(query): Query<ExportQuery>,
) -> ApiResult<Json<ExportManifest>> {
    Ok(Json(
        registry(&state)?
            .export_manifest(&query.project, &query.export)
            .await?,
    ))
}

/// The same export as a COCO document.
///
/// Generated rather than stored: COCO is a derived view of the vector
/// annotations, and storing it would be a second copy of the truth that can
/// disagree with the first. It reads one revision object per sample, which is
/// paid once per training run rather than once per epoch.
#[utoipa::path(
    get,
    path = "/api/v1/annotation-export/coco",
    params(ExportQuery),
    responses(
        (status = 200, description = "A COCO document", body = serde_json::Value),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "annotations",
)]
async fn get_export_coco(
    State(state): State<AppState>,
    Query(query): Query<ExportQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    Ok(Json(
        registry(&state)?
            .coco(&query.project, &query.export, query.split)
            .await?,
    ))
}

// ── Bytes ────────────────────────────────────────────────────────────────────

/// Store an image under the digest of its own bytes.
///
/// The digest is computed here. A content address supplied by the caller is
/// not a content address, and trusting one would let two different images
/// occupy the same key.
#[utoipa::path(
    post,
    path = "/api/v1/annotation-blobs",
    request_body(content = Vec<u8>, content_type = "application/octet-stream"),
    responses(
        (status = 201, body = StoredBlob, description = "The bytes were stored"),
        (status = 200, body = StoredBlob, description = "These exact bytes were already stored"),
        (status = 400, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 413, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "annotations",
)]
async fn upload_blob(
    State(state): State<AppState>,
    caller: Caller,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> ApiResult<(StatusCode, Json<StoredBlob>)> {
    may_author(&caller)?;
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let stored = registry(&state)?
        .put_blob(body.to_vec(), content_type)
        .await?;
    let status = if stored.created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(stored)))
}

/// The bytes of an uploaded image.
///
/// Immutable by construction — the key *is* the digest — so it is cached for a
/// year. An annotation canvas re-fetching a 2 MB plan on every pan would be the
/// slowest part of the tool.
#[utoipa::path(
    get,
    path = "/api/v1/annotation-blobs/{image_id}",
    params(("image_id" = String, Path, description = "The image's SHA-256")),
    responses(
        (status = 200, description = "The image bytes"),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "annotations",
)]
async fn get_blob(
    State(state): State<AppState>,
    Path(image_id): Path<String>,
) -> ApiResult<Response> {
    let (body, content_type) = registry(&state)?.blob(&image_id).await?;
    let mut response = body.into_response();
    let headers = response.headers_mut();
    if let Ok(value) = content_type.parse() {
        headers.insert(header::CONTENT_TYPE, value);
    }
    headers.insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("private, max-age=31536000, immutable"),
    );
    Ok(response)
}

// ── Where data comes from ────────────────────────────────────────────────────

/// The public floor-plan corpora, and where to look for more.
///
/// A dated table this build ships, not a search against Hugging Face or
/// Roboflow. Those mirrors restate licences wrongly often enough that a live
/// answer would be worse than no answer, because it would arrive looking
/// authoritative. Every row links its original.
#[utoipa::path(
    get,
    path = "/api/v1/annotation-sources",
    params(SourcesQuery),
    responses((status = 200, body = SourcePage)),
    tag = "annotations",
)]
async fn list_sources(
    State(state): State<AppState>,
    Query(query): Query<SourcesQuery>,
) -> Json<SourcePage> {
    Json(
        state
            .sources
            .search(query.q.as_deref(), query.usage, query.label.as_deref()),
    )
}
