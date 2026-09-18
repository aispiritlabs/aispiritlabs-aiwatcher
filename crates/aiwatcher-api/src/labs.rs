//! A workshop's labs: the authored brief, and the measurement it pins
//! (ADR_0034).
//!
//! Almost nothing here is new. A lab's tests are an evaluation scorecard at a
//! version and a cohort derived from a dataset version; the work handed in is
//! a recording or answers a worker generated; the mark is a published
//! evaluation result. All of those have routes already, and this module adds
//! none of them.
//!
//! Three shapes are worth noticing:
//!
//! * **Publishing is a `POST` to the collection.** The caller does not choose
//!   the version id — it is a digest over the document — so there is no
//!   address to `PUT` to until the document exists.
//! * **A lab's pins are checked where they live.** A publish that names a card
//!   or a cohort resolves both in *this project's* evaluation registry and is
//!   refused when either is missing, rather than storing a lab that reads fine
//!   and measures nothing.
//! * **`GET /labs/{name}/measurement` answers the context id, and nothing else
//!   computes one.** It is the join from a lab to its marks —
//!   `/evaluation-results?context_id=…` — and a digest over a canonicalised
//!   document is exactly the thing a second implementation in a browser would
//!   get subtly wrong. The precedent is `POST
//!   /api/v1/evaluation-approvals/address`, which exists for that reason.

use axum::extract::{Path, Query};
use axum::routing::{get, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use utoipa::OpenApi;

use aiwatcher_labs::{
    LabFilter, LabHead, LabMeasurement, LabName, LabPage, LabPublished, LabTests, LabVersion,
    PublishLab,
};

use crate::error::{ApiError, ApiResult};
use crate::lab_scope::{LabRead, LabWrite, Labs};
use crate::state::AppState;

/// This module's operations, as the contract they satisfy.
#[derive(OpenApi)]
#[openapi(paths(
    list_labs,
    publish_lab,
    get_lab,
    get_lab_version,
    set_lab_label,
    get_lab_measurement,
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
        .route("/labs", get(list_labs).post(publish_lab))
        .route("/labs/{name}", get(get_lab))
        .route("/labs/{name}/versions/{version_id}", get(get_lab_version))
        .route("/labs/{name}/labels/{label}", put(set_lab_label))
        .route("/labs/{name}/measurement", get(get_lab_measurement))
}

#[derive(Deserialize)]
struct LabPath {
    name: String,
}
#[derive(Deserialize)]
struct VersionPath {
    name: String,
    version_id: String,
}
#[derive(Deserialize)]
struct LabelPath {
    name: String,
    label: String,
}

#[derive(Deserialize, utoipa::IntoParams)]
struct ListQuery {
    /// Page by name: the last name on the previous page.
    after: Option<String>,
    limit: Option<usize>,
}

#[derive(Deserialize, utoipa::IntoParams)]
struct AtVersion {
    /// Which version to read. Absent is what `published` points at, or the
    /// newest when nothing is labelled.
    version: Option<String>,
    /// Which label to follow instead. Ignored when `version` is given.
    label: Option<String>,
}

fn name_of(raw: &str) -> ApiResult<LabName> {
    LabName::parse(raw).map_err(|error| ApiError::BadRequest(error.to_string()))
}

/// One lab, with enough to render its page in one request.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[schema(as = LabDetail)]
pub struct LabDetail {
    pub head: LabHead,
    /// The version `published` points at, or the newest when no label has been
    /// moved. Carried with its brief so the page paints without a second round
    /// trip — the brief is the thing being looked at.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<LabVersion>,
}

/// What a lab measures, or why nothing here can say.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[schema(as = LabMeasurementView)]
pub struct LabMeasurementView {
    pub name: LabName,
    /// The version this answer is about.
    pub version_id: String,
    /// The measurement, when the lab pins one and both halves resolve.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measurement: Option<LabMeasurement>,
    /// Why there is none, in the server's own sentence. A lab that pins no
    /// tests yet is the ordinary state of one being written, so this is a
    /// fact about the lab rather than a failure — which is why it is a 200
    /// carrying a reason rather than a 404 a reader has to interpret.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
}

/// Every lab in this workshop, by position and then by name.
#[utoipa::path(
    get,
    path = "/api/v1/labs",
    params(ListQuery),
    responses(
        (status = 200, body = LabPage),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "labs",
)]
async fn list_labs(
    LabRead(labs): LabRead,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<LabPage>> {
    let filter = LabFilter {
        after: query.after,
        limit: query.limit,
    };
    Ok(Json(labs.registry.list(&filter).await?))
}

/// Publish a version of a lab.
///
/// Idempotent on the document: `created` is `false` when this exact lab was
/// already stored, and the version that comes back is the one that was there.
///
/// A lab that pins tests is refused unless both halves resolve in this
/// project's evaluation registry — a stored lab that measures nothing is a
/// slot that reads as working and answers nothing when somebody submits.
#[utoipa::path(
    post,
    path = "/api/v1/labs",
    request_body = aiwatcher_labs::PublishLab,
    responses(
        (status = 201, body = LabPublished, description = "A new version was stored"),
        (status = 200, body = LabPublished, description = "This document was already stored"),
        (status = 400, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody, description = "Its tests name something this project cannot measure"),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "labs",
)]
async fn publish_lab(
    write: LabWrite,
    caller: crate::Caller,
    Json(request): Json<PublishLab>,
) -> ApiResult<(axum::http::StatusCode, Json<LabPublished>)> {
    let labs = write.authorize().await?;
    if let Some(tests) = &request.lab.tests {
        // Resolved, never believed: the refusal has to arrive while somebody
        // is writing the lab rather than when a participant submits to it.
        measure(&labs, tests).await?;
    }
    let author = Some(caller.identity().log_subject().to_owned());
    let published = labs.registry.publish(request, author).await?;
    let status = if published.created {
        axum::http::StatusCode::CREATED
    } else {
        axum::http::StatusCode::OK
    };
    Ok((status, Json(published)))
}

/// One lab: its versions, its labels, and the brief it is at now.
#[utoipa::path(
    get,
    path = "/api/v1/labs/{name}",
    params(("name" = String, Path, description = "The lab to fetch"), AtVersion),
    responses(
        (status = 200, body = LabDetail),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "labs",
)]
async fn get_lab(
    LabRead(labs): LabRead,
    Path(LabPath { name }): Path<LabPath>,
    Query(at): Query<AtVersion>,
) -> ApiResult<Json<LabDetail>> {
    let name = name_of(&name)?;
    let head = labs
        .registry
        .head(&name)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("lab {name}")))?;
    // A version wins over a label, a label over the head's own choice, and a
    // label pointing nowhere is an absent `current` rather than a 404 on the
    // lab: the head is what was asked for and it is right here.
    let version_id = match (&at.version, at.label.as_deref()) {
        (Some(version), _) => Some(version.as_str()),
        (None, Some(label)) => head.labels.get(label).map(String::as_str),
        (None, None) => head.current(),
    };
    let current = match version_id {
        Some(version) => labs.registry.version(&name, version).await?,
        None => None,
    };
    Ok(Json(LabDetail { head, current }))
}

/// One version, with its brief.
#[utoipa::path(
    get,
    path = "/api/v1/labs/{name}/versions/{version_id}",
    params(
        ("name" = String, Path, description = "The lab"),
        ("version_id" = String, Path, description = "The version's digest"),
    ),
    responses(
        (status = 200, body = LabVersion),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "labs",
)]
async fn get_lab_version(
    LabRead(labs): LabRead,
    Path(VersionPath { name, version_id }): Path<VersionPath>,
) -> ApiResult<Json<LabVersion>> {
    let name = name_of(&name)?;
    labs.registry
        .version(&name, &version_id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("lab {name} version {version_id}")))
}

/// Move a label onto a version that is already stored.
///
/// Its own route because writing a lesson and setting it are different
/// decisions — `POST /labs` with a `label` is the shorthand for doing both at
/// once, which is what a first publish wants.
#[utoipa::path(
    put,
    path = "/api/v1/labs/{name}/labels/{label}",
    params(
        ("name" = String, Path, description = "The lab"),
        ("label" = String, Path, description = "The label to move, e.g. `published`"),
    ),
    request_body = LabelBody,
    responses(
        (status = 200, body = LabHead),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "labs",
)]
async fn set_lab_label(
    write: LabWrite,
    Path(LabelPath { name, label }): Path<LabelPath>,
    Json(body): Json<LabelBody>,
) -> ApiResult<Json<LabHead>> {
    let labs = write.authorize().await?;
    let name = name_of(&name)?;
    Ok(Json(
        labs.registry
            .set_label(&name, &label, &body.version_id)
            .await?,
    ))
}

/// Which version a label points at.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[schema(as = LabLabelBody)]
#[serde(deny_unknown_fields)]
pub struct LabelBody {
    pub version_id: String,
}

/// The context every submission to this lab publishes under.
///
/// `context_id` is what `GET /api/v1/evaluation-results?context_id=…` is
/// narrowed by, so this is the whole join from a lab to its marks. It is
/// answerable before anybody has submitted, which is the point.
#[utoipa::path(
    get,
    path = "/api/v1/labs/{name}/measurement",
    params(("name" = String, Path, description = "The lab"), AtVersion),
    responses(
        (status = 200, body = LabMeasurementView),
        (status = 404, body = crate::error::ErrorBody),
        (status = 501, body = crate::error::ErrorBody),
    ),
    tag = "labs",
)]
async fn get_lab_measurement(
    LabRead(labs): LabRead,
    Path(LabPath { name }): Path<LabPath>,
    Query(at): Query<AtVersion>,
) -> ApiResult<Json<LabMeasurementView>> {
    let name = name_of(&name)?;
    let version = match &at.version {
        Some(version) => labs
            .registry
            .version(&name, version)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("lab {name} version {version}")))?,
        None => labs.registry.resolve(&name, at.label.as_deref()).await?,
    };
    let Some(tests) = &version.lab.tests else {
        return Ok(Json(LabMeasurementView {
            name,
            version_id: version.version_id,
            measurement: None,
            unavailable: Some("this lab pins no tests yet".into()),
        }));
    };
    let (measurement, unavailable) = match measure(&labs, tests).await {
        Ok(measurement) => (Some(measurement), None),
        Err(error) => (None, Some(error.to_string())),
    };
    Ok(Json(LabMeasurementView {
        name,
        version_id: version.version_id,
        measurement,
        unavailable,
    }))
}

/// Resolve a lab's pins and fold them into the context its results share.
///
/// The rubrics handed in are empty on purpose rather than read: a lab refuses
/// a card whose metrics are judged or calibrated, so nothing left needs one,
/// and a round trip per publish to resolve rubrics nothing reads would be a
/// read for its own sake.
async fn measure(labs: &Labs, tests: &LabTests) -> ApiResult<LabMeasurement> {
    let registry = labs
        .evaluations
        .as_ref()
        .ok_or(ApiError::EvaluationDisabled)?;
    let card = registry
        .scorecard(&tests.scorecard.name, Some(&tests.scorecard.version))
        .await?
        .ok_or_else(|| {
            aiwatcher_labs::LabError::Unpinned(format!(
                "no scorecard `{}` at version {} is published in this project",
                tests.scorecard.name, tests.scorecard.version
            ))
        })?;
    let derived = registry
        .derived_cohort(&tests.cases)
        .await?
        .ok_or_else(|| {
            aiwatcher_labs::LabError::Unpinned(format!(
                "no cohort was derived in this project under {} — derive one with POST \
                 /api/v1/evaluation-cohorts",
                tests.cases
            ))
        })?;
    Ok(tests.measurement(
        &derived.request.dataset,
        &derived.cohort,
        &card.scorecard,
        &aiwatcher_evaluation::Rubrics::default(),
    )?)
}
