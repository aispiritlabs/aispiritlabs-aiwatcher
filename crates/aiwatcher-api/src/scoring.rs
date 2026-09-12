//! Running an evaluation, rather than recording one somebody else ran.
//!
//! A scoring run measures answers this deployment already holds against a card
//! somebody declared, and publishes the result as durable evidence. It calls no
//! model of the application under test, so what it costs is a fold and what it
//! proves is the measurement.
//!
//! Three routes and one shape between them. A recording is staged first, which
//! is what gives it a digest nobody chose; a declaration names that digest
//! together with the card, the cohort and the variant, and is addressed by its
//! own content; and starting one is an ordinary managed execution whose plan
//! carries that address. Repeating a declaration therefore lands on the run
//! that is already going rather than beside it — an independent repetition is
//! a different declaration, because it is a different measurement.

use aiwatcher_auth::Role;
use aiwatcher_core::ArtifactRef;
use aiwatcher_evaluation::{DeclaredRun, ScoringRun};
use aiwatcher_execution::message::RunProjection;
use aiwatcher_execution::plan::{
    CachePolicy, DefinitionKind, DefinitionRevision, ExecutionPlan, PlanStep, RetryPolicy,
    RuntimeBinding, ScoreEvaluationSpec,
};
use aiwatcher_execution::{RunIdentity, StartRun};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use utoipa::OpenApi;

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// How long one scoring step may run before the reactor stops waiting.
///
/// Generous, because the work is bounded by the cohort rather than by anything
/// this process waits on: nothing here opens a socket to a model.
const SCORING_TIMEOUT_SECONDS: u64 = 900;
/// The one step such a plan has. Named rather than numbered, because it is
/// what the waterfall and a context lookup address it by.
const SCORING_STEP: &str = "score";

/// An accepted measurement, and the run that will make it.
///
/// The declaration rides back beside the execution because it is the join: a
/// run's projection names a plan and a definition, and what this one measures
/// is a document only this address opens.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct ScoringAccepted {
    /// The content address of what this run measures.
    pub declaration: String,
    /// The store's inline projection after the decision that accepted this.
    pub execution: RunProjection,
    /// True when this request started the run, false when the same declaration
    /// landed on one that was already going. Both are 202.
    pub created: bool,
}

/// This module's operations, as the contract they satisfy.
#[derive(OpenApi)]
#[openapi(paths(stage_recording, start_scoring_run, get_scoring_run))]
struct Api;

/// The operations this module serves. Composed by [`crate::openapi`].
#[must_use]
pub fn openapi() -> utoipa::openapi::OpenApi {
    Api::openapi()
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/evaluation-recordings/{name}",
            put(stage_recording).layer(axum::extract::DefaultBodyLimit::max(100 * 1024 * 1024)),
        )
        .route("/api/v1/evaluation-runs", post(start_scoring_run))
        .route("/api/v1/evaluation-runs/{id}", get(get_scoring_run))
}

fn registry(state: &AppState) -> ApiResult<&aiwatcher_evaluation::Registry> {
    state
        .evaluations
        .as_deref()
        .ok_or(ApiError::EvaluationDisabled)
}

/// Keep the answers a run will measure, and hand back the reference.
///
/// The digest is of the bytes that arrived and never of anything the caller
/// claimed, so a declaration naming it names these exact bytes — and a retry of
/// the run reads what the first attempt read. Staging the same recording twice
/// answers with the same reference.
#[utoipa::path(put, path = "/api/v1/evaluation-recordings/{name}",
    params(("name" = String, Path, description = "What a reader calls this recording")),
    request_body = Vec<u8>,
    responses((status = 200, body = ArtifactRef), (status = 400, body = crate::error::ErrorBody),
    (status = 403, body = crate::error::ErrorBody), (status = 501, body = crate::error::ErrorBody)),
    tag = "evaluation")]
async fn stage_recording(
    State(state): State<AppState>,
    caller: Caller,
    Path(name): Path<String>,
    body: axum::body::Bytes,
) -> ApiResult<Json<ArtifactRef>> {
    caller.require(Role::Editor)?;
    Ok(Json(
        registry(&state)?
            .stage_recording(&name, body.to_vec())
            .await?,
    ))
}

/// Declare a measurement and start it.
///
/// The declaration is written before the run, because the plan names its
/// digest: a run whose declaration was never stored would name a document
/// nothing can resolve. Both are idempotent by content, so sending this twice
/// is one document and one execution.
///
/// The card is resolved here rather than at score time, so a version nobody
/// published is a refusal now instead of a run that fails in a minute.
#[utoipa::path(post, path = "/api/v1/evaluation-runs", request_body = ScoringRun,
    responses((status = 202, body = ScoringAccepted), (status = 400, body = crate::error::ErrorBody),
    (status = 403, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody),
    (status = 409, body = crate::error::ErrorBody), (status = 501, body = crate::error::ErrorBody),
    (status = 503, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn start_scoring_run(
    State(state): State<AppState>,
    caller: Caller,
    Json(run): Json<ScoringRun>,
) -> ApiResult<(StatusCode, Json<ScoringAccepted>)> {
    let requester = caller.require(Role::Editor)?.log_subject().to_owned();
    let evaluations = registry(&state)?;
    run.validate()?;
    evaluations
        .scorecard(&run.scorecard.name, Some(&run.scorecard.version))
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "scorecard {} at version {}",
                run.scorecard.name, run.scorecard.version
            ))
        })?;
    let declared = evaluations
        .declare_scoring_run(&run, &requester, now())
        .await?;

    let started = state
        .executions()
        .start(
            plan_for(&declared),
            StartRun {
                // The declaration is the content address of the intention, so
                // starting one twice is one run by construction and no header
                // decides it. Measuring the same variant again is a second
                // repetition, which is a different declaration.
                identity: RunIdentity::Key(declared.id.clone()),
                parameters: Default::default(),
                requested_by: requester,
                decided_by: Default::default(),
                payloads: None,
            },
        )
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(ScoringAccepted {
            declaration: declared.id,
            execution: started.handled.projection,
            created: !started.handled.duplicate,
        }),
    ))
}

/// The one-step plan that measures one declaration.
///
/// Sealed here rather than compiled from a name, because there is no name: a
/// declaration is addressed by its content, and that address is both the plan's
/// revision and the only thing its step carries.
fn plan_for(declared: &DeclaredRun) -> ExecutionPlan {
    ExecutionPlan::seal(
        DefinitionKind::Evaluation,
        declared.run.evaluation_id.clone(),
        DefinitionRevision(declared.id.clone()),
        vec![PlanStep {
            id: SCORING_STEP.to_owned(),
            runtime: RuntimeBinding::ScoreEvaluation(ScoreEvaluationSpec {
                declaration: declared.id.clone(),
            }),
            inputs: Vec::new(),
            outputs: Vec::new(),
            retry: RetryPolicy::default(),
            timeout_seconds: SCORING_TIMEOUT_SECONDS,
            cache: CachePolicy::Never,
        }],
        Vec::new(),
    )
}

/// What a run measures, by the address the plan names it with.
#[utoipa::path(get, path = "/api/v1/evaluation-runs/{id}",
    params(("id" = String, Path, description = "The declaration address a start returned")),
    responses((status = 200, body = DeclaredRun), (status = 404, body = crate::error::ErrorBody),
    (status = 501, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn get_scoring_run(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<DeclaredRun>> {
    caller.require(Role::Viewer)?;
    registry(&state)?
        .scoring_run(&id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("scoring run {id}")))
}

fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}
