//! Running an evaluation, rather than recording one somebody else ran.
//!
//! A scoring run measures answers this deployment already holds against a card
//! somebody declared, and publishes the result as durable evidence. It calls no
//! model of the application under test, so what it costs is a fold and what it
//! proves is the measurement.
//!
//! Four steps, each idempotent. A recording is staged, which is what gives it a
//! digest nobody chose. A declaration names that digest with the card, the
//! cohort and the variant, is addressed by its own content, and answers with
//! the manifest it will publish and the approval that admits it — so an
//! operator admits the pair from what the server derived rather than from what
//! they wrote out. Starting it is then an ordinary managed execution whose plan
//! carries the declaration's address, and repeating a start lands on the run
//! already going. An independent repetition is a different declaration,
//! because it is a different measurement.
//!
//! A card may ask a judge. Then a calibration set is taken first — the
//! judgements people made of a published result's cases — and the declaration
//! names it with the judge's profile, model and settings; the run is claimed in
//! the work role, where the judge's address and credential are.

use aiwatcher_auth::Role;
use aiwatcher_core::ArtifactRef;
use aiwatcher_evaluation::{
    CalibrationRequest, CalibrationVersion, CohortRequest, DeclaredRun, DerivedCohort,
    RecordedCatalog, ScoringRun, ScoringRunView,
};
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
/// The same for a run that asks a judge, which waits on a model once per
/// judged case and once per calibration item.
const JUDGED_TIMEOUT_SECONDS: u64 = 3600;
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
#[openapi(paths(
    stage_recording,
    declare_scoring_run,
    get_scoring_run,
    start_scoring_run,
    take_calibration,
    get_calibration,
    derive_cohort,
    get_derived_cohort,
    get_scorer_catalog
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
            "/api/v1/evaluation-recordings/{name}",
            put(stage_recording).layer(axum::extract::DefaultBodyLimit::max(100 * 1024 * 1024)),
        )
        .route("/api/v1/evaluation-runs", post(declare_scoring_run))
        .route("/api/v1/evaluation-runs/{id}", get(get_scoring_run))
        .route(
            "/api/v1/evaluation-runs/{id}/start",
            post(start_scoring_run),
        )
        .route("/api/v1/evaluation-calibrations", post(take_calibration))
        .route(
            "/api/v1/evaluation-calibrations/{version}",
            get(get_calibration),
        )
        .route("/api/v1/evaluation-cohorts", post(derive_cohort))
        .route("/api/v1/evaluation-scorers", get(get_scorer_catalog))
        .route(
            "/api/v1/evaluation-cohorts/{cases}",
            get(get_derived_cohort),
        )
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

/// Declare a measurement.
///
/// Writes nothing that runs. What comes back is the declaration with the
/// manifest a result of it publishes and the approval that admits that pair,
/// because an operator has to admit it before it may publish and the metrics
/// in that manifest are derived from the card — a second copy written out by
/// hand is a second answer to what this run measures. Idempotent by content.
///
/// The card is resolved here rather than at score time, so a version nobody
/// published is a refusal now instead of a run that fails in a minute.
#[utoipa::path(post, path = "/api/v1/evaluation-runs", request_body = ScoringRun,
    responses((status = 200, body = ScoringRunView), (status = 400, body = crate::error::ErrorBody),
    (status = 403, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody),
    (status = 501, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn declare_scoring_run(
    State(state): State<AppState>,
    caller: Caller,
    Json(run): Json<ScoringRun>,
) -> ApiResult<Json<ScoringRunView>> {
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
    view(evaluations, &declared.id).await.map(Json)
}

/// What a run measures, what it publishes, and whether it may yet.
#[utoipa::path(get, path = "/api/v1/evaluation-runs/{id}",
    params(("id" = String, Path, description = "The declaration address")),
    responses((status = 200, body = ScoringRunView), (status = 404, body = crate::error::ErrorBody),
    (status = 501, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn get_scoring_run(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<ScoringRunView>> {
    caller.require(Role::Viewer)?;
    view(registry(&state)?, &id).await.map(Json)
}

/// Start the declared measurement.
///
/// Refused while nothing admits the pair, naming the approval that would. A
/// run started without one could only fail when it tried to publish, and that
/// failed run would then be what every later start of the same declaration
/// lands on. Once admitted, repeating this reaches the run already going.
#[utoipa::path(post, path = "/api/v1/evaluation-runs/{id}/start",
    params(("id" = String, Path, description = "The declaration address")),
    responses((status = 202, body = ScoringAccepted), (status = 403, body = crate::error::ErrorBody),
    (status = 404, body = crate::error::ErrorBody),
    (status = 422, body = crate::error::ErrorBody, description = "The run asks a judge profile this deployment does not have, or more questions at once than it allows"),
    (status = 409, body = crate::error::ErrorBody, description = "`pair_not_admitted`: no operator has admitted this pair yet; the message names the approval"),
    (status = 501, body = crate::error::ErrorBody), (status = 503, body = crate::error::ErrorBody)),
    tag = "evaluation")]
async fn start_scoring_run(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<(StatusCode, Json<ScoringAccepted>)> {
    let requester = caller.require(Role::Editor)?.log_subject().to_owned();
    let evaluations = registry(&state)?;
    let viewed = view(evaluations, &id).await?;
    // The gate's own refusal: not yet names the approval, and withdrawn is the
    // same 403 a producer's publication of that pair gets.
    evaluations.admission(&viewed.manifest).await?;
    // Before a run exists rather than after: a judged run on a deployment with
    // no judge, or with another profile, is one nothing would ever claim — and
    // a run whose card asks a scorer service, on a deployment with none.
    let external = asks_a_scorer_service(&viewed.manifest);
    let asked = viewed.declaration.run.settings.concurrency;
    if external {
        let ceiling = state.scorer_concurrency.ok_or(ApiError::ScorersDisabled)?;
        if let Some(asked) = asked
            && usize::try_from(asked).unwrap_or(usize::MAX) > ceiling
        {
            return Err(ApiError::PlanRefused {
                summary: "this run asks more of the scorer service than this deployment allows"
                    .to_owned(),
                problems: vec![format!(
                    "declared to score {asked} cases at once, and AIWATCHER_SCORER_CONCURRENCY \
                     allows {ceiling}"
                )],
            });
        }
    }
    if let Some(judge) = &viewed.declaration.run.judge {
        let deployed = state
            .judge_provider
            .as_deref()
            .ok_or(ApiError::JudgeDisabled)?;
        let mut problems = Vec::new();
        if judge.provider != deployed {
            problems.push(format!(
                "declared for judge profile {}, and AIWATCHER_JUDGE_PROVIDER is {deployed}",
                judge.provider
            ));
        }
        // More at once than the operator allows is refused rather than quietly
        // lowered: a run that says eight and asks two is a setting that lied.
        if let Some(asked) = asked
            && usize::try_from(asked).unwrap_or(usize::MAX) > state.judge_concurrency
        {
            problems.push(format!(
                "declared to ask {asked} questions at once, and AIWATCHER_JUDGE_CONCURRENCY allows \
                 {}",
                state.judge_concurrency
            ));
        }
        if !problems.is_empty() {
            return Err(ApiError::PlanRefused {
                summary: "this run asks a judge this deployment does not have".to_owned(),
                problems,
            });
        }
    }
    let started = state
        .executions()
        .start(
            plan_for(&viewed.declaration, external),
            StartRun {
                // The declaration is the content address of the intention, so
                // starting one twice is one run by construction and no header
                // decides it. Measuring the same variant again is a second
                // repetition, which is a different declaration.
                identity: RunIdentity::Key(viewed.declaration.id.clone()),
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
            declaration: viewed.declaration.id,
            execution: started.handled.projection,
            created: !started.handled.duplicate,
        }),
    ))
}

/// Freeze what people judged of one result, as a judge's calibration set.
///
/// Only a person's judgement, and only under the rubric versions named: a
/// judgement made under other words answered another question. Addressed by
/// its content, so taking the same judgements twice is the same set — and a
/// person changing their mind later is a later set, never this one re-read.
#[utoipa::path(post, path = "/api/v1/evaluation-calibrations", request_body = CalibrationRequest,
    responses((status = 200, body = CalibrationVersion), (status = 400, body = crate::error::ErrorBody),
    (status = 403, body = crate::error::ErrorBody), (status = 501, body = crate::error::ErrorBody)),
    tag = "evaluation")]
async fn take_calibration(
    State(state): State<AppState>,
    caller: Caller,
    Json(request): Json<CalibrationRequest>,
) -> ApiResult<Json<CalibrationVersion>> {
    let requester = caller.require(Role::Editor)?.log_subject().to_owned();
    // People's judgements of conversation evidence are taken by an admin, who
    // may read the cases they are about; anybody else is refused as that.
    Ok(Json(
        registry(&state)?
            .clone()
            .with_content_access(caller.require(Role::Admin).is_ok())
            .take_calibration(&request, &requester, now())
            .await?,
    ))
}

/// One calibration set, at the address a declaration names it by.
#[utoipa::path(get, path = "/api/v1/evaluation-calibrations/{version}",
    params(("version" = String, Path, description = "The calibration set's content address")),
    responses((status = 200, body = CalibrationVersion), (status = 404, body = crate::error::ErrorBody),
    (status = 501, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn get_calibration(
    State(state): State<AppState>,
    caller: Caller,
    Path(version): Path<String>,
) -> ApiResult<Json<CalibrationVersion>> {
    caller.require(Role::Viewer)?;
    registry(&state)?
        .calibration(&version)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("calibration set {version}")))
}

/// Take a cohort from a dataset version this deployment owns.
///
/// The first `limit` cases of a version's split, as the owner holds them, with
/// the three files a cohort pins derived here — so a declaration names them
/// and nobody writes or stages them. Admitting the pair derives them again from
/// the owner. A conversation corpus's cases are content, so an admin takes a
/// cohort from one, as an admin reads its cases.
#[utoipa::path(post, path = "/api/v1/evaluation-cohorts", request_body = CohortRequest,
    responses((status = 200, body = DerivedCohort), (status = 400, body = crate::error::ErrorBody),
    (status = 403, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody),
    (status = 501, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn derive_cohort(
    State(state): State<AppState>,
    caller: Caller,
    Json(request): Json<CohortRequest>,
) -> ApiResult<Json<DerivedCohort>> {
    let requester = caller.require(Role::Editor)?.log_subject().to_owned();
    Ok(Json(
        registry(&state)?
            .clone()
            .with_content_access(caller.require(Role::Admin).is_ok())
            .derive_cohort(&request, &requester, now())
            .await?,
    ))
}

/// Where the cases under one digest were derived from, if this deployment
/// derived them.
#[utoipa::path(get, path = "/api/v1/evaluation-cohorts/{cases}",
    params(("cases" = String, Path, description = "The digest of a cohort's cases")),
    responses((status = 200, body = DerivedCohort), (status = 400, body = crate::error::ErrorBody),
    (status = 404, body = crate::error::ErrorBody), (status = 501, body = crate::error::ErrorBody)),
    tag = "evaluation")]
async fn get_derived_cohort(
    State(state): State<AppState>,
    caller: Caller,
    Path(cases): Path<String>,
) -> ApiResult<Json<DerivedCohort>> {
    caller.require(Role::Viewer)?;
    registry(&state)?
        .derived_cohort(&cases)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("a cohort derived with cases {cases}")))
}

/// What the scorer service says it measures, as the work role last recorded it.
///
/// The vocabulary a card's external metrics are named from: each adapter at the
/// release it runs, the model its graded metrics ask, and every metric with its
/// unit, direction, what it reads and the parameters it takes. A card is pinned
/// against this when it is published.
#[utoipa::path(get, path = "/api/v1/evaluation-scorers",
    responses((status = 200, body = RecordedCatalog), (status = 404, body = crate::error::ErrorBody),
    (status = 501, body = crate::error::ErrorBody)), tag = "evaluation")]
async fn get_scorer_catalog(
    State(state): State<AppState>,
    caller: Caller,
) -> ApiResult<Json<RecordedCatalog>> {
    caller.require(Role::Viewer)?;
    registry(&state)?
        .scorer_catalog()
        .await?
        .map(Json)
        .ok_or_else(|| {
            ApiError::NotFound(
                "a scorer catalog: no scorer service has described itself to this deployment \
                 (AIWATCHER_SCORER_URL, on the work role)"
                    .to_owned(),
            )
        })
}

async fn view(evaluations: &aiwatcher_evaluation::Registry, id: &str) -> ApiResult<ScoringRunView> {
    evaluations
        .scoring_run_view(id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("scoring run {id}")))
}

/// The one-step plan that measures one declaration.
///
/// Sealed here rather than compiled from a name, because there is no name: a
/// declaration is addressed by its content, and that address is both the plan's
/// revision and the only thing its step carries.
fn plan_for(declared: &DeclaredRun, external: bool) -> ExecutionPlan {
    let spec = ScoreEvaluationSpec {
        declaration: declared.id.clone(),
    };
    // Where it runs is decided by the most any metric needs: a scorer service
    // is a socket the work role holds, with a judge beside it when the card
    // asks one too.
    let (runtime, default_timeout) = if external {
        (
            RuntimeBinding::ExternalEvaluation(spec),
            JUDGED_TIMEOUT_SECONDS,
        )
    } else if declared.run.judge.is_some() {
        (
            RuntimeBinding::JudgeEvaluation(spec),
            JUDGED_TIMEOUT_SECONDS,
        )
    } else {
        (
            RuntimeBinding::ScoreEvaluation(spec),
            SCORING_TIMEOUT_SECONDS,
        )
    };
    // The declaration's own deadline when it chose one. Read from the
    // declaration rather than from the start request, so the plan — and so the
    // run a repeated start lands on — is a function of the declaration alone.
    let timeout_seconds = declared
        .run
        .settings
        .timeout_seconds
        .unwrap_or(default_timeout);
    ExecutionPlan::seal(
        DefinitionKind::Evaluation,
        declared.run.evaluation_id.clone(),
        DefinitionRevision(declared.id.clone()),
        vec![PlanStep {
            id: SCORING_STEP.to_owned(),
            runtime,
            inputs: Vec::new(),
            outputs: Vec::new(),
            retry: RetryPolicy::default(),
            timeout_seconds,
            cache: CachePolicy::Never,
        }],
        Vec::new(),
    )
}

/// Whether any metric this manifest publishes is measured by a scorer service.
fn asks_a_scorer_service(manifest: &aiwatcher_evaluation::EvaluationManifest) -> bool {
    manifest
        .context
        .metrics
        .iter()
        .any(|metric| metric.measured_by.is_some())
}

fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declared(settings: aiwatcher_evaluation::RunSettings, judged: bool) -> DeclaredRun {
        let manifest: aiwatcher_evaluation::EvaluationManifest = serde_json::from_str(
            include_str!("../../../contracts/fixtures/evaluation-v1/manifest.json"),
        )
        .expect("the contract fixture parses");
        let judge = judged.then(|| aiwatcher_evaluation::JudgeDeclaration {
            provider: "llamacpp".into(),
            model: aiwatcher_evaluation::VersionReference {
                name: "gemma".into(),
                version: "q4".into(),
            },
            settings: aiwatcher_evaluation::JudgeSettings::default(),
            calibration: aiwatcher_evaluation::VersionReference {
                name: "people".into(),
                version: "c".repeat(64),
            },
        });
        DeclaredRun {
            id: "d".repeat(64),
            run: ScoringRun {
                evaluation_id: "paced".into(),
                repetition_id: "measurement-1".into(),
                variant: manifest.variant,
                cohort: aiwatcher_evaluation::Cohort {
                    case_manifest: manifest.context.case_manifest,
                    case_count: 2,
                    split: manifest.context.split,
                    input_schema: manifest.context.input_schema,
                    expectations_schema: manifest.context.expectations_schema,
                },
                scorecard: aiwatcher_evaluation::VersionReference {
                    name: "card".into(),
                    version: "a".repeat(64),
                },
                answers: aiwatcher_evaluation::Answers::Archive(
                    aiwatcher_evaluation::ArchiveWord::Archive,
                ),
                judge,
                external_calibration: None,
                settings,
            },
            declared_by: "ada".into(),
            declared_at: 0,
        }
    }

    #[test]
    fn a_declared_deadline_is_the_steps_and_none_is_the_kinds_default() {
        let chosen = aiwatcher_evaluation::RunSettings {
            timeout_seconds: Some(600),
            concurrency: None,
        };
        assert_eq!(
            plan_for(&declared(chosen.clone(), false), false).steps[0].timeout_seconds,
            600
        );
        assert_eq!(
            plan_for(&declared(chosen, true), false).steps[0].timeout_seconds,
            600
        );
        assert_eq!(
            plan_for(&declared(Default::default(), false), false).steps[0].timeout_seconds,
            SCORING_TIMEOUT_SECONDS
        );
        assert_eq!(
            plan_for(&declared(Default::default(), true), false).steps[0].timeout_seconds,
            JUDGED_TIMEOUT_SECONDS
        );
        let external = plan_for(&declared(Default::default(), false), true);
        assert_eq!(
            external.steps[0].runtime.kind().as_str(),
            "external_evaluation"
        );
    }
}
