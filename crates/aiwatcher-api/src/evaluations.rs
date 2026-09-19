//! Evaluation reports: suites, scores, regressions.
//!
//! An `eval.*` event rides the same log and forms **no span**, and these
//! routes read a bounded projection of its own rather than the runs list.
//! See ADR_0010.

use crate::auth::Caller;
use crate::evidence_scope::{EvidenceAdmin, EvidenceRead, EvidenceWrite};
use aiwatcher_auth::Role;
use aiwatcher_evaluation::{
    Approval, ApprovalLine, ApprovalLinePage, ApprovalPage, CaseDiffPage, CaseFilter, CasePage,
    DiffQuery, DurableEvaluation, DurablePage, EvaluationManifest, EvaluationReceipt,
    EvidenceComparison, EvidenceState, GateDecision, GatePolicy, PublishEvaluation, ResultStatus,
};
use axum::extract::{Path, Query, State};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use utoipa::OpenApi;

use aiwatcher_projector::{EvaluationDetail, EvaluationFilter, EvaluationPage, SuitePage};

use crate::error::{ApiError, ApiResult};
use crate::run_scope::RunRead;
use crate::state::AppState;

/// This module's operations, as the contract they satisfy.
#[derive(OpenApi)]
#[openapi(paths(
    address_approval,
    gate_result,
    admit_line,
    list_lines,
    withdraw_line,
    stage_variant_artifact,
))]
struct Api;

#[derive(OpenApi)]
#[openapi(paths(
    publish_result,
    list_results,
    get_result,
    get_cases,
    compare_results,
    compare_cases,
    forget_result,
    approve_source,
    list_approvals,
    withdraw_approval
))]
struct EvidenceApi;

/// The legacy telemetry fold's operations, which have a scoped twin because
/// the fold behind them carries the project in the row.
#[derive(OpenApi)]
#[openapi(paths(list_evaluations, get_evaluation, list_evaluation_suites))]
struct FoldApi;

#[derive(serde::Deserialize)]
struct ResultPath {
    evaluation_id: String,
}
#[derive(serde::Deserialize)]
struct ApprovalPath {
    approval_id: String,
}

/// The operations this module serves. Composed by [`crate::openapi`].
#[must_use]
pub fn openapi() -> utoipa::openapi::OpenApi {
    let mut api = Api::openapi();
    api.merge(crate::project_scope::openapi(EvidenceApi::openapi()));
    api.merge(crate::project_scope::openapi(FoldApi::openapi()));
    api.merge(crate::evaluation_bundles::openapi());
    api
}

pub fn router() -> Router<AppState> {
    Router::new()
        .nest(
            "/api/v1/orgs/{organization}/projects/{project}",
            evidence_router()
                .layer(axum::Extension(crate::project_scope::ScopedRoute))
                .layer(tower_http::set_header::SetResponseHeaderLayer::overriding(
                    axum::http::header::CACHE_CONTROL,
                    axum::http::HeaderValue::from_static("no-store"),
                )),
        )
        .nest("/api/v1", evidence_router())
        .nest("/api/v1", fold_router())
        .nest(
            "/api/v1/orgs/{organization}/projects/{project}",
            fold_router()
                .layer(axum::Extension(crate::project_scope::ScopedRoute))
                .layer(tower_http::set_header::SetResponseHeaderLayer::overriding(
                    axum::http::header::CACHE_CONTROL,
                    axum::http::HeaderValue::from_static("no-store"),
                )),
        )
        .route(
            "/api/v1/evaluation-results/{evaluation_id}/gate",
            post(gate_result),
        )
        .route(
            "/api/v1/evaluation-approval-lines",
            get(list_lines).post(admit_line),
        )
        .route(
            "/api/v1/evaluation-approval-lines/{line_id}",
            delete(withdraw_line),
        )
        .route(
            "/api/v1/evaluation-variant-artifacts/{name}",
            put(stage_variant_artifact)
                .layer(axum::extract::DefaultBodyLimit::max(100 * 1024 * 1024)),
        )
        .route(
            "/api/v1/evaluation-approvals/address",
            post(address_approval),
        )
        .merge(crate::evaluation_bundles::router())
}

/// The legacy telemetry fold, served twice.
///
/// The fold carries the project in the row (IAM-02 E2's remainder) and the
/// durable registry these bridge to resolves per project already, so both
/// halves of each answer come from one side — [`RunRead`] resolves the fold's
/// and `RunRead::iam_scope` the registry's, off the same grant.
fn fold_router() -> Router<AppState> {
    Router::new()
        .route("/evaluations", get(list_evaluations))
        .route("/evaluations/{evaluation_id}", get(get_evaluation))
        .route("/evaluation-suites", get(list_evaluation_suites))
}

/// Named so the scoped family's own path parameters need no spelling out.
#[derive(serde::Deserialize)]
struct EvaluationPath {
    evaluation_id: String,
}

fn evidence_router() -> Router<AppState> {
    Router::new()
        .route(
            "/evaluation-results",
            get(list_results)
                .post(publish_result)
                .layer(axum::extract::DefaultBodyLimit::max(100 * 1024 * 1024)),
        )
        .route(
            "/evaluation-results/{evaluation_id}",
            get(get_result).delete(forget_result),
        )
        .route("/evaluation-results/{evaluation_id}/cases", get(get_cases))
        .route(
            "/evaluation-results/{evaluation_id}/comparison",
            get(compare_results),
        )
        .route(
            "/evaluation-results/{evaluation_id}/comparison/cases",
            get(compare_cases),
        )
        .route(
            "/evaluation-approvals",
            get(list_approvals).post(approve_source),
        )
        .route(
            "/evaluation-approvals/{approval_id}",
            delete(withdraw_approval),
        )
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
    read: RunRead,
    Query(filter): Query<EvaluationFilter>,
) -> ApiResult<Json<EvaluationPage>> {
    let excluded = known_ids(&state, &read).await?;
    Ok(Json(
        state
            .read_model
            .legacy_evaluations(read.scope, &filter, &excluded)
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
    read: RunRead,
    Path(EvaluationPath { evaluation_id }): Path<EvaluationPath>,
    Query(query): Query<DetailQuery>,
    caller: Caller,
) -> ApiResult<Json<EvaluationDetail>> {
    caller.require(Role::Viewer)?;
    if let Some(registry) = evidence_for(&state, &read, caller.require(Role::Admin).is_ok())? {
        if let Some(detail) = registry
            .get(&evaluation_id, &caller.identity().subject, now())
            .await?
        {
            if query.baseline_id.is_some() {
                return Err(ApiError::BadRequest(
                    "compare published evidence with GET /api/v1/evaluation-results/{id}/comparison"
                        .into(),
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
            return Ok(Json(legacy_detail(detail, page, read.iam_scope())?));
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
            read.scope,
            &evaluation_id,
            query.baseline_id.as_deref(),
            &known_ids(&state, &read).await?,
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
async fn list_evaluation_suites(
    State(state): State<AppState>,
    read: RunRead,
) -> ApiResult<Json<SuitePage>> {
    let excluded = known_ids(&state, &read).await?;
    Ok(Json(
        state
            .read_model
            .legacy_evaluation_suites(read.scope, &excluded)
            .await,
    ))
}

/// The published-evidence registry on the side this read is answering on.
///
/// `None` where this deployment has no registry at all, which is a working
/// state for these three routes: the fold answers on its own, and the bridge
/// below has nothing to bridge to.
fn evidence_for(
    state: &AppState,
    read: &RunRead,
    content_access: bool,
) -> ApiResult<Option<aiwatcher_evaluation::Registry>> {
    let Some(root) = &state.evaluations else {
        return Ok(None);
    };
    match read.iam_scope() {
        // A project's evidence is read through the grant, which is
        // `for_project_evidence`'s own rule rather than a second one here: an
        // instance role does not become a project role (ADR_0033).
        Some(scope) => Ok(Some(root.for_project_evidence(scope)?)),
        None => Ok(Some(
            root.as_ref().clone().with_content_access(content_access),
        )),
    }
}

/// Which ids the durable registry on this side already owns, so the fold does
/// not answer for them twice.
///
/// Asked without content access: which ids exist is not the content, and a
/// list that shrank for a viewer would show them a report the registry owns as
/// though the fold still did.
async fn known_ids(
    state: &AppState,
    read: &RunRead,
) -> ApiResult<std::collections::BTreeSet<String>> {
    match evidence_for(state, read, false)? {
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
    /// Published within this many seconds. The catalogue has a published
    /// order, so a period is a bound on it rather than a filter over a scan.
    window_seconds: Option<i64>,
    /// Only results measured under this pinned context. Every row that comes
    /// back is a legitimate baseline for every other, which is what makes this
    /// the candidate list for a comparison rather than a convenience filter.
    context_id: Option<String>,
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
struct ComparisonQuery {
    /// Explicit. A baseline nobody chose is a baseline nobody checked.
    baseline: String,
}
#[derive(serde::Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
struct CaseComparisonQuery {
    /// Explicit. A baseline nobody chose is a baseline nobody checked.
    baseline: String,
    /// Both sides' offsets and both versions. A page of a narrowed diff ends
    /// at the rows asked for or at the cases it was allowed to walk, so this
    /// promises another case rather than another match.
    cursor: Option<String>,
    limit: Option<usize>,
    /// The question being asked, not the verdict a case was given: `worse` is
    /// what a release gate reads and includes the cases that lost something
    /// and gained something else.
    only: Option<CaseFilter>,
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
///
/// A pair nobody has admitted yet is a 409 `pair_not_admitted` naming the
/// approval, as starting a scoring run is; a withdrawn pair stays a 403.
#[utoipa::path(post, path = "/api/v1/evaluation-results", request_body = PublishEvaluation,
    responses((status = 200, body = EvaluationReceipt), (status = 400, body = crate::error::ErrorBody),
    (status = 403, body = crate::error::ErrorBody, description = "`evidence_forbidden`: the pair was withdrawn, its bundle changed, or the caller may not read the source; `measured_here`: evidence aiwatcher scores is published by its run"),
    (status = 409, body = crate::error::ErrorBody, description = "`pair_not_admitted` names the approval that would admit the pair; `evaluation_conflict` is a different body under this ID"),
    (status = 410, body = crate::error::ErrorBody), (status = 503, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn publish_result(
    evidence: EvidenceWrite,
    caller: Caller,
    Json(request): Json<PublishEvaluation>,
) -> ApiResult<Json<EvaluationReceipt>> {
    let registry = evidence.authorize().await?;
    if request.manifest.context.scored_here() {
        return Err(ApiError::MeasuredHere);
    }
    Ok(Json(
        registry
            .publish(request, &caller.identity().subject, now())
            .await?,
    ))
}

/// Durable discovery is independent of telemetry retention. Newest first, and
/// clients page with the opaque cursor rather than a timestamp assumption.
/// Conversation content requires Admin; other readers receive a forbidden state.
#[utoipa::path(get, path = "/api/v1/evaluation-results", params(ResultQuery), responses((status = 200, body = DurablePage)), tag = "evaluation")]
async fn list_results(
    EvidenceRead(registry): EvidenceRead,
    caller: Caller,
    Query(query): Query<ResultQuery>,
) -> ApiResult<Json<DurablePage>> {
    Ok(Json(
        registry
            .list(
                query.cursor.as_deref(),
                query.limit.unwrap_or(200),
                query.window_seconds,
                query.context_id.as_deref(),
                &caller.identity().subject,
                now(),
            )
            .await?,
    ))
}

#[utoipa::path(get, path = "/api/v1/evaluation-results/{evaluation_id}", params(("evaluation_id" = String, Path)),
    responses((status = 200, body = DurableEvaluation), (status = 404, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn get_result(
    EvidenceRead(registry): EvidenceRead,
    caller: Caller,
    Path(ResultPath { evaluation_id: id }): Path<ResultPath>,
) -> ApiResult<Json<DurableEvaluation>> {
    registry
        .get(&id, &caller.identity().subject, now())
        .await?
        .map(Json)
        .ok_or(ApiError::NotFound(id))
}

/// One published result against another named one.
///
/// Two headers, not two results: the metrics a comparison subtracts are in the
/// metadata each side was published with, so this costs what two summaries
/// cost however many cases are behind them. Which two may be subtracted is the
/// server's answer — `comparability` with its reasons, the same three words the
/// folded half uses — and a delta is withheld rather than shown wrong. Case
/// level regressions are not here: they are a full read of both sides.
#[utoipa::path(get, path = "/api/v1/evaluation-results/{evaluation_id}/comparison",
    params(("evaluation_id" = String, Path), ComparisonQuery),
    responses((status = 200, body = EvidenceComparison), (status = 404, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn compare_results(
    EvidenceRead(registry): EvidenceRead,
    caller: Caller,
    Path(ResultPath { evaluation_id: id }): Path<ResultPath>,
    Query(query): Query<ComparisonQuery>,
) -> ApiResult<Json<EvidenceComparison>> {
    registry
        .compare(&id, &query.baseline, &caller.identity().subject, now())
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("{id} or baseline {}", query.baseline)))
}

/// Which cases moved between one published result and another named one.
///
/// The comparison beside this one reads two headers; this reads both results,
/// so it pages, it is narrowed here rather than in the browser, and one
/// request walks a bounded number of cases. The verdict rides along, so a pair
/// this server will not subtract is refused in the same sentence the header
/// comparison uses rather than as an empty list — and an unverified pair keeps
/// its rows, because the cases that failed are what somebody opens this for.
#[utoipa::path(get, path = "/api/v1/evaluation-results/{evaluation_id}/comparison/cases",
    params(("evaluation_id" = String, Path), CaseComparisonQuery),
    responses((status = 200, body = CaseDiffPage), (status = 400, body = crate::error::ErrorBody),
    (status = 404, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn compare_cases(
    EvidenceRead(registry): EvidenceRead,
    caller: Caller,
    Path(ResultPath { evaluation_id: id }): Path<ResultPath>,
    Query(query): Query<CaseComparisonQuery>,
) -> ApiResult<Json<CaseDiffPage>> {
    registry
        .compare_cases(
            &id,
            &query.baseline,
            DiffQuery {
                cursor: query.cursor.as_deref(),
                limit: query.limit,
                only: query.only,
            },
            &caller.identity().subject,
            now(),
        )
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("{id} or baseline {}", query.baseline)))
}

#[utoipa::path(get, path = "/api/v1/evaluation-results/{evaluation_id}/cases", params(("evaluation_id" = String, Path), CasesQuery),
    responses((status = 200, body = CasePage), (status = 400, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn get_cases(
    EvidenceRead(registry): EvidenceRead,
    caller: Caller,
    Path(ResultPath { evaluation_id: id }): Path<ResultPath>,
    Query(query): Query<CasesQuery>,
) -> ApiResult<Json<CasePage>> {
    registry
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

// ── The gate ─────────────────────────────────────────────────────────────────

#[derive(serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
struct GateRequest {
    /// The result the candidate is held to.
    baseline: String,
    #[serde(default)]
    policy: GatePolicy,
}

/// Whether a result may ship against a baseline, under a policy.
///
/// A read: nothing is written, so a pipeline asks as often as it likes and the
/// panel asks the same thing. `pass`, `regression`, `incomplete` or `error`,
/// with every reason in words — a metric worse than its tolerance, a critical
/// case lost whatever the average did, a scorer that failed, a pair that does
/// not compare.
#[utoipa::path(post, path = "/api/v1/evaluation-results/{evaluation_id}/gate",
    params(("evaluation_id" = String, Path)), request_body = GateRequest,
    responses((status = 200, body = GateDecision), (status = 400, body = crate::error::ErrorBody),
    (status = 404, body = crate::error::ErrorBody), (status = 501, body = crate::error::ErrorBody)),
    tag = "evaluation")]
async fn gate_result(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(request): Json<GateRequest>,
) -> ApiResult<Json<GateDecision>> {
    caller.require(Role::Viewer)?;
    registry(&state)?
        .clone()
        .with_content_access(caller.require(Role::Admin).is_ok())
        .gate(
            &id,
            &request.baseline,
            &request.policy,
            &caller.identity().subject,
            now(),
        )
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("{id} or baseline {}", request.baseline)))
}

// ── Lines ────────────────────────────────────────────────────────────────────

/// Admit every variant of one experiment measured in one context.
///
/// Sent any declaration of the line; its context and its variant's
/// `experiment_id` are what is admitted. `Admin`, for a pair's reason. A
/// scoring run of a variant the line covers is then admitted when it starts,
/// from the bytes its pipeline staged, and the approval names the line.
#[utoipa::path(post, path = "/api/v1/evaluation-approval-lines", request_body = EvaluationManifest,
    responses((status = 200, body = ApprovalLine), (status = 400, body = crate::error::ErrorBody),
    (status = 403, body = crate::error::ErrorBody), (status = 501, body = crate::error::ErrorBody)),
    tag = "evaluation")]
async fn admit_line(
    State(state): State<AppState>,
    caller: Caller,
    Json(manifest): Json<EvaluationManifest>,
) -> ApiResult<Json<ApprovalLine>> {
    caller.require(Role::Admin)?;
    Ok(Json(
        registry(&state)?
            .clone()
            .with_content_access(true)
            .admit_line(&manifest, &caller.identity().subject, now())
            .await?,
    ))
}

/// Every line, withdrawn ones included.
#[utoipa::path(get, path = "/api/v1/evaluation-approval-lines",
    responses((status = 200, body = ApprovalLinePage), (status = 501, body = crate::error::ErrorBody)),
    tag = "evaluation")]
async fn list_lines(
    State(state): State<AppState>,
    caller: Caller,
) -> ApiResult<Json<ApprovalLinePage>> {
    caller.require(Role::Viewer)?;
    Ok(Json(ApprovalLinePage {
        lines: registry(&state)?.lines().await?,
    }))
}

/// Stop a line admitting further variants. What it admitted stays admitted,
/// each withdrawable through its own approval.
#[utoipa::path(delete, path = "/api/v1/evaluation-approval-lines/{line_id}",
    params(("line_id" = String, Path)),
    responses((status = 200, body = ApprovalLine), (status = 403, body = crate::error::ErrorBody),
    (status = 404, body = crate::error::ErrorBody), (status = 501, body = crate::error::ErrorBody)),
    tag = "evaluation")]
async fn withdraw_line(
    State(state): State<AppState>,
    caller: Caller,
    Path(line_id): Path<String>,
) -> ApiResult<Json<ApprovalLine>> {
    caller.require(Role::Admin)?;
    registry(&state)?
        .withdraw_line(&line_id, &caller.identity().subject, now())
        .await?
        .map(Json)
        .ok_or(ApiError::NotFound(line_id))
}

/// Keep one file a variant pins, by its content.
///
/// `Editor`: the bytes are addressed by the digest computed here, so sending
/// them admits nothing — a line reads them when a run of that variant starts,
/// and a pin names them or nothing.
#[utoipa::path(put, path = "/api/v1/evaluation-variant-artifacts/{name}",
    params(("name" = String, Path, description = "The name the variant pins it under")),
    request_body = Vec<u8>,
    responses((status = 200, body = aiwatcher_core::ArtifactRef), (status = 400, body = crate::error::ErrorBody),
    (status = 501, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn stage_variant_artifact(
    State(state): State<AppState>,
    caller: Caller,
    Path(name): Path<String>,
    body: axum::body::Bytes,
) -> ApiResult<Json<aiwatcher_core::ArtifactRef>> {
    caller.require(Role::Editor)?;
    Ok(Json(
        registry(&state)?
            .stage_variant_artifact(&name, body.to_vec())
            .await?,
    ))
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
    (status = 403, body = crate::error::ErrorBody),
    (status = 409, body = crate::error::ErrorBody, description = "`admitted_other_bytes`: the pair is admitted already, over other bundle bytes; the message names the approval"),
    (status = 501, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn approve_source(
    EvidenceAdmin(evidence): EvidenceAdmin,
    caller: Caller,
    Json(manifest): Json<EvaluationManifest>,
) -> ApiResult<Json<Approval>> {
    Ok(Json(
        evidence
            .authorize()
            .await?
            .approve(&manifest, &caller.identity().subject, now())
            .await?,
    ))
}

/// Every pair this instance has admitted, withdrawn ones included: an approval
/// that vanished from the list would read as one nobody ever made.
#[utoipa::path(get, path = "/api/v1/evaluation-approvals",
    responses((status = 200, body = ApprovalPage), (status = 501, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn list_approvals(EvidenceRead(registry): EvidenceRead) -> ApiResult<Json<ApprovalPage>> {
    Ok(Json(ApprovalPage {
        approvals: registry.approvals().await?,
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
    EvidenceAdmin(evidence): EvidenceAdmin,
    caller: Caller,
    Path(ApprovalPath { approval_id }): Path<ApprovalPath>,
) -> ApiResult<Json<Approval>> {
    evidence
        .authorize()
        .await?
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
    EvidenceAdmin(evidence): EvidenceAdmin,
    Path(ResultPath { evaluation_id: id }): Path<ResultPath>,
) -> ApiResult<axum::http::StatusCode> {
    if evidence.authorize().await?.forget(&id).await? {
        return Ok(axum::http::StatusCode::NO_CONTENT);
    }
    Err(ApiError::NotFound(id))
}

/// The old detail shape stays readable. It exposes only a bounded first page
/// and no quality recommendation; durable clients page the separate resource.
fn legacy_detail(
    detail: DurableEvaluation,
    page: Option<CasePage>,
    project: Option<aiwatcher_iam::ProjectScope>,
) -> ApiResult<EvaluationDetail> {
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
        // The registry a caller reached this through is already the one their
        // grant admitted, so the row says which side it came from rather than
        // deciding anything: `project_scope` is what answers whose it is.
        project: project.map(crate::project_scope::on_the_log),
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
                    aiwatcher_evaluation::DatasetKind::Assessments => "assessments",
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

/// The address one declaration's bundle is staged under, and the pair it names.
///
/// A caller cannot work this out: it is a digest over the canonicalised
/// declaration, and a second implementation of that in another language is a
/// second answer to what a pair *is*. Pure — it reads no store and admits
/// nothing — so it is the one route here below `admin`, and a declaration it
/// refuses is a 400 saying what is wrong with it rather than a digest of
/// something invalid.
#[utoipa::path(post, path = "/api/v1/evaluation-approvals/address",
    request_body = EvaluationManifest,
    responses((status = 200, body = ApprovalAddress), (status = 400, body = crate::error::ErrorBody)),
    tag = "evaluation")]
async fn address_approval(
    caller: Caller,
    Json(manifest): Json<EvaluationManifest>,
) -> ApiResult<Json<ApprovalAddress>> {
    caller.require(Role::Viewer)?;
    let prepared = aiwatcher_evaluation::Evaluation::prepare(manifest)?;
    Ok(Json(ApprovalAddress {
        approval_id: aiwatcher_evaluation::approval_id(
            prepared.variant_id(),
            prepared.context_id(),
        )?,
        variant_id: prepared.variant_id().into(),
        context_id: prepared.context_id().into(),
    }))
}

/// Where a declaration's bundle belongs, and the two IDs it is derived from.
#[derive(serde::Serialize, utoipa::ToSchema)]
struct ApprovalAddress {
    approval_id: String,
    variant_id: String,
    context_id: String,
}
