//! Evaluation reports: suites, scores, regressions.
//!
//! An `eval.*` event rides the same log and forms **no span**, and these
//! routes read a bounded projection of its own rather than the runs list.
//! See ADR_0010.

use crate::auth::Caller;
use aiwatcher_auth::Role;
use aiwatcher_evaluation::{
    Approval, ApprovalPage, CasePage, DurableEvaluation, DurablePage, EvaluationManifest,
    EvaluationReceipt, EvidenceState, PublishEvaluation, ResultStatus,
};
use axum::extract::{Path, Query, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use utoipa::OpenApi;

use aiwatcher_projector::{EvaluationDetail, EvaluationFilter, EvaluationPage, SuitePage};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// This module's operations, as the contract they satisfy.
#[derive(OpenApi)]
#[openapi(paths(
    list_evaluations,
    get_evaluation,
    list_evaluation_suites,
    publish_result,
    list_results,
    get_result,
    get_cases,
    forget_result,
    approve_source,
    list_approvals,
    withdraw_approval,
))]
struct Api;

/// The operations this module serves. Composed by [`crate::openapi`].
#[must_use]
pub fn openapi() -> utoipa::openapi::OpenApi {
    Api::openapi()
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/evaluation-results", get(list_results))
        .route(
            "/api/v1/evaluation-results",
            post(publish_result).layer(axum::extract::DefaultBodyLimit::max(100 * 1024 * 1024)),
        )
        .route(
            "/api/v1/evaluation-results/{evaluation_id}",
            get(get_result).delete(forget_result),
        )
        .route("/api/v1/evaluation-approvals", get(list_approvals))
        .route("/api/v1/evaluation-approvals", post(approve_source))
        .route(
            "/api/v1/evaluation-approvals/{approval_id}",
            delete(withdraw_approval),
        )
        .route(
            "/api/v1/evaluation-results/{evaluation_id}/cases",
            get(get_cases),
        )
        .route("/api/v1/evaluations", get(list_evaluations))
        .route("/api/v1/evaluations/{evaluation_id}", get(get_evaluation))
        .route("/api/v1/evaluation-suites", get(list_evaluation_suites))
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
struct DetailQuery {
    /// Explicit baseline; missing IDs return 404, never an automatic replacement.
    baseline_id: Option<String>,
}

// ── Evaluations ──────────────────────────────────────────────────────────────

/// Evaluation reports, newest first.
///
/// The other half of the loop the traces come from: a trace says what one run
/// did, an evaluation says whether the thing producing those runs is getting
/// better. They arrive on the same log and are folded apart — an `eval.*`
/// event produces no span and no row in the runs list. See
/// `aiwatcher_projector::evaluations`.
#[utoipa::path(
    get,
    path = "/api/v1/evaluations",
    params(EvaluationFilter),
    responses((status = 200, body = EvaluationPage)),
    tag = "evaluation",
)]
async fn list_evaluations(
    State(state): State<AppState>,
    Query(filter): Query<EvaluationFilter>,
) -> ApiResult<Json<EvaluationPage>> {
    let excluded = known_ids(&state).await?;
    Ok(Json(
        state
            .read_model
            .legacy_evaluations(&filter, &excluded)
            .await,
    ))
}

/// One evaluation: its parameters, its metrics, its cases, the document the
/// producer attached, and the previous evaluation of the same suite on the
/// same dataset.
#[utoipa::path(
    get,
    path = "/api/v1/evaluations/{evaluation_id}",
    params(("evaluation_id" = String, Path, description = "The evaluation to fetch"), DetailQuery),
    responses(
        (status = 200, body = EvaluationDetail),
        (status = 404, body = crate::error::ErrorBody),
    ),
    tag = "evaluation",
)]
async fn get_evaluation(
    State(state): State<AppState>,
    Path(evaluation_id): Path<String>,
    Query(query): Query<DetailQuery>,
    caller: Caller,
) -> ApiResult<Json<EvaluationDetail>> {
    caller.require(Role::Viewer)?;
    if let Some(registry) = &state.evaluations {
        let registry = registry
            .as_ref()
            .clone()
            .with_content_access(caller.require(Role::Admin).is_ok());
        if let Some(detail) = registry
            .get(&evaluation_id, &caller.identity().subject, now())
            .await?
        {
            if query.baseline_id.is_some() {
                return Err(ApiError::BadRequest(
                    "durable result comparisons require the B3 comparison contract".into(),
                ));
            }
            let page = registry
                .cases(
                    &evaluation_id,
                    &detail.receipt.version,
                    None,
                    None,
                    &caller.identity().subject,
                    now(),
                )
                .await?;
            return Ok(Json(legacy_detail(detail, page)?));
        }
        if let Some(baseline) = &query.baseline_id
            && registry
                .get(baseline, &caller.identity().subject, now())
                .await?
                .is_some()
        {
            return Err(ApiError::BadRequest(
                "a durable baseline cannot be compared through legacy telemetry".into(),
            ));
        }
    }
    state
        .read_model
        .legacy_evaluation(
            &evaluation_id,
            query.baseline_id.as_deref(),
            &known_ids(&state).await?,
        )
        .await
        .map(Json)
        .ok_or_else(|| {
            ApiError::NotFound(format!("evaluation {evaluation_id} or requested baseline"))
        })
}

/// Suites: the level above a report, and what MLflow calls an experiment.
///
/// A separate resource rather than `/evaluations/suites`, so an evaluation
/// that happens to be called `suites` is still reachable.
#[utoipa::path(
    get,
    path = "/api/v1/evaluation-suites",
    responses((status = 200, body = SuitePage)),
    tag = "evaluation",
)]
async fn list_evaluation_suites(State(state): State<AppState>) -> ApiResult<Json<SuitePage>> {
    Ok(Json(
        state
            .read_model
            .legacy_evaluation_suites(&known_ids(&state).await?)
            .await,
    ))
}

async fn known_ids(state: &AppState) -> ApiResult<std::collections::BTreeSet<String>> {
    match &state.evaluations {
        Some(registry) => Ok(registry.known_ids().await?),
        None => Ok(Default::default()),
    }
}

fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}
fn registry(state: &AppState) -> ApiResult<&aiwatcher_evaluation::Registry> {
    state
        .evaluations
        .as_deref()
        .ok_or(ApiError::EvaluationDisabled)
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
struct ResultQuery {
    cursor: Option<String>,
    limit: Option<usize>,
}
#[derive(serde::Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
struct CasesQuery {
    version: String,
    cursor: Option<String>,
    limit: Option<usize>,
}

/// Publish terminal evidence. Retry the identical body and logical ID after a
/// timeout; a different body conflicts. No result content enters the event log.
/// Conversation evidence requires Admin, including publication.
#[utoipa::path(post, path = "/api/v1/evaluation-results", request_body = PublishEvaluation,
    responses((status = 200, body = EvaluationReceipt), (status = 400, body = crate::error::ErrorBody),
    (status = 403, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody),
    (status = 410, body = crate::error::ErrorBody), (status = 503, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn publish_result(
    State(state): State<AppState>,
    caller: Caller,
    Json(request): Json<PublishEvaluation>,
) -> ApiResult<Json<EvaluationReceipt>> {
    caller.require(Role::Editor)?;
    Ok(Json(
        registry(&state)?
            .clone()
            .with_content_access(caller.require(Role::Admin).is_ok())
            .publish(request, &caller.identity().subject, now())
            .await?,
    ))
}

/// Durable discovery is independent of telemetry retention. Pages follow stable
/// storage IDs; clients must use the opaque cursor, not a timestamp assumption.
/// Conversation content requires Admin; other readers receive a forbidden state.
#[utoipa::path(get, path = "/api/v1/evaluation-results", params(ResultQuery), responses((status = 200, body = DurablePage)), tag = "evaluation")]
async fn list_results(
    State(state): State<AppState>,
    caller: Caller,
    Query(query): Query<ResultQuery>,
) -> ApiResult<Json<DurablePage>> {
    caller.require(Role::Viewer)?;
    Ok(Json(
        registry(&state)?
            .clone()
            .with_content_access(caller.require(Role::Admin).is_ok())
            .list(
                query.cursor.as_deref(),
                query.limit.unwrap_or(200),
                &caller.identity().subject,
                now(),
            )
            .await?,
    ))
}

#[utoipa::path(get, path = "/api/v1/evaluation-results/{evaluation_id}", params(("evaluation_id" = String, Path)),
    responses((status = 200, body = DurableEvaluation), (status = 404, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn get_result(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<DurableEvaluation>> {
    caller.require(Role::Viewer)?;
    registry(&state)?
        .clone()
        .with_content_access(caller.require(Role::Admin).is_ok())
        .get(&id, &caller.identity().subject, now())
        .await?
        .map(Json)
        .ok_or(ApiError::NotFound(id))
}

#[utoipa::path(get, path = "/api/v1/evaluation-results/{evaluation_id}/cases", params(("evaluation_id" = String, Path), CasesQuery),
    responses((status = 200, body = CasePage), (status = 400, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn get_cases(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Query(query): Query<CasesQuery>,
) -> ApiResult<Json<CasePage>> {
    caller.require(Role::Viewer)?;
    registry(&state)?
        .clone()
        .with_content_access(caller.require(Role::Admin).is_ok())
        .cases(
            &id,
            &query.version,
            query.cursor.as_deref(),
            query.limit,
            &caller.identity().subject,
            now(),
        )
        .await?
        .map(Json)
        .ok_or(ApiError::NotFound(id))
}

// ── Approvals ────────────────────────────────────────────────────────────────

/// Admit one pinned pair.
///
/// The operator act that authorises publication, separated from publishing so
/// that every later repetition — from a worker, a schedule or CI — needs
/// nothing on the server's host. `Admin`, not `Editor`: an ingest token is an
/// editor by construction, and a producer that can admit its own evidence is
/// not an approval. Idempotent for the pair; a bundle that changed underneath
/// an admitted pair conflicts rather than moving what earlier results mean.
#[utoipa::path(post, path = "/api/v1/evaluation-approvals", request_body = EvaluationManifest,
    responses((status = 200, body = Approval), (status = 400, body = crate::error::ErrorBody),
    (status = 403, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody),
    (status = 501, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn approve_source(
    State(state): State<AppState>,
    caller: Caller,
    Json(manifest): Json<EvaluationManifest>,
) -> ApiResult<Json<Approval>> {
    caller.require(Role::Admin)?;
    Ok(Json(
        registry(&state)?
            .clone()
            .with_content_access(true)
            .approve(&manifest, &caller.identity().subject, now())
            .await?,
    ))
}

/// Every pair this instance has admitted, withdrawn ones included: an approval
/// that vanished from the list would read as one nobody ever made.
#[utoipa::path(get, path = "/api/v1/evaluation-approvals",
    responses((status = 200, body = ApprovalPage), (status = 501, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn list_approvals(
    State(state): State<AppState>,
    caller: Caller,
) -> ApiResult<Json<ApprovalPage>> {
    caller.require(Role::Viewer)?;
    Ok(Json(ApprovalPage {
        approvals: registry(&state)?.approvals().await?,
    }))
}

/// Withdraw one. Every result measured under that pair stops being readable and
/// no new one can be published; nothing already published has its retention
/// moved, in either direction. Final for this approval ID.
#[utoipa::path(delete, path = "/api/v1/evaluation-approvals/{approval_id}",
    params(("approval_id" = String, Path)),
    responses((status = 200, body = Approval), (status = 403, body = crate::error::ErrorBody),
    (status = 404, body = crate::error::ErrorBody), (status = 501, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn withdraw_approval(
    State(state): State<AppState>,
    caller: Caller,
    Path(approval_id): Path<String>,
) -> ApiResult<Json<Approval>> {
    caller.require(Role::Admin)?;
    registry(&state)?
        .withdraw(&approval_id, &caller.identity().subject, now())
        .await?
        .map(Json)
        .ok_or(ApiError::NotFound(approval_id))
}

/// Forget one published result.
///
/// The third way evidence disappears, beside its approval being withdrawn and
/// its retention running out — and the only one that is about a single
/// measurement. It writes the same durable marker the retention sweep does, so
/// the ID stays permanently unreadable rather than falling back to telemetry.
#[utoipa::path(delete, path = "/api/v1/evaluation-results/{evaluation_id}",
    params(("evaluation_id" = String, Path)),
    responses((status = 204), (status = 403, body = crate::error::ErrorBody),
    (status = 404, body = crate::error::ErrorBody), (status = 501, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn forget_result(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<axum::http::StatusCode> {
    caller.require(Role::Admin)?;
    if registry(&state)?.forget(&id).await? {
        return Ok(axum::http::StatusCode::NO_CONTENT);
    }
    Err(ApiError::NotFound(id))
}

/// The old detail shape stays readable. It exposes only a bounded first page
/// and no quality recommendation; durable clients page the separate resource.
fn legacy_detail(detail: DurableEvaluation, page: Option<CasePage>) -> ApiResult<EvaluationDetail> {
    use aiwatcher_projector::evaluations::EvaluationContext;
    use aiwatcher_projector::{EvaluationCase, EvaluationStatus, EvaluationSummary};
    if !matches!(
        detail.state,
        EvidenceState::Complete | EvidenceState::Partial
    ) {
        return Err(aiwatcher_evaluation::EvaluationError::Unavailable(detail.state).into());
    }
    let manifest = detail
        .manifest
        .ok_or_else(|| ApiError::BadRequest("durable manifest missing".into()))?;
    let counts = detail
        .counts
        .ok_or_else(|| ApiError::BadRequest("durable counts missing".into()))?;
    let at = time::OffsetDateTime::from_unix_timestamp(detail.receipt.committed_at)
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let summary = EvaluationSummary {
        evaluation_id: detail.receipt.evaluation_id,
        suite: manifest.context.suite.name.clone(),
        dataset: Some(manifest.context.dataset.name.clone()),
        variant: Some(detail.receipt.variant_id),
        execution_id: manifest.origin.execution_id,
        step_id: manifest.origin.step_id,
        context: EvaluationContext {
            dataset_kind: Some(
                match manifest.context.dataset.kind {
                    aiwatcher_evaluation::DatasetKind::Curation => "curation",
                    aiwatcher_evaluation::DatasetKind::Annotations => "annotations",
                    aiwatcher_evaluation::DatasetKind::Conversations => "conversations",
                    aiwatcher_evaluation::DatasetKind::External => "external",
                }
                .into(),
            ),
            dataset_version: Some(manifest.context.dataset.version),
            suite_version: Some(manifest.context.suite.version),
            scorer_version: Some(manifest.context.scorer.version),
            split: Some(manifest.context.split),
        },
        status: if detail.status == Some(ResultStatus::Succeeded) {
            EvaluationStatus::Succeeded
        } else {
            EvaluationStatus::Failed
        },
        started_at: at,
        ended_at: Some(at),
        duration_ms: None,
        params: Default::default(),
        metrics: detail.metrics,
        cases_total: counts.selected as u64,
        // A successful scorer is not necessarily a passing quality score.
        cases_passed: 0,
        cases_failed: counts.failed as u64,
        pass_rate: None,
        runtime: "evaluation-registry".into(),
        error: None,
        report_bytes: 0,
        report_dropped: false,
        last_checkpoint: aiwatcher_core::Checkpoint::beginning(),
    };
    let cases: Vec<_> = page
        .into_iter()
        .flat_map(|page| page.cases)
        .map(|case| EvaluationCase {
            case_id: case.measurement.case_id,
            score: None,
            passed: None,
            duration_ms: None,
            reason: None,
            error: case.measurement.error,
        })
        .collect();
    Ok(EvaluationDetail {
        summary,
        cases_truncated: cases.len() < counts.selected,
        cases,
        report: None,
        comparison: None,
    })
}
