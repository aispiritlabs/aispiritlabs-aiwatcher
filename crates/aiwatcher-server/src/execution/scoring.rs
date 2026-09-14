//! Measuring answers somebody already recorded, where the ingress is.
//!
//! The second binding in the `serve` role, for the first one's reason: it
//! executes nothing. It reads a declaration, a card and the answers this
//! deployment holds, folds them, and publishes through the registry the panel
//! reads — calling no model of the application under test, so a new card over
//! unchanged answers differs from the last result by the measurement alone.
//! The plan carries the declaration's digest, so a retry reads exactly what the
//! first attempt read.
//!
//! **The approval is the only authority.** Nothing here admits anything, and a
//! conversation cohort's cases are content a request reads only for an admin.
//! Nobody's session is here to ask, and asking whoever pressed start would make
//! an editor's click a way to read it — so the gate is asked first, and only a
//! pair an admin admitted is read with content access.
//!
//! **A card that asks a judge is the same step in the other role.** Asking a
//! model is a socket and a credential, so that kind is `judge_evaluation`,
//! claimed where `AIWATCHER_JUDGE_URL` is. Every question is put before the
//! fold runs, so the fold still opens nothing.
//!
//! **So is a card that asks a scorer service** (`external_evaluation`, where
//! `AIWATCHER_SCORER_URL` is), held to the release and model the card pinned.

use std::sync::Arc;

use aiwatcher_api::state::AppState;
use aiwatcher_evaluation::{
    DatasetKind, EvaluationError, EvidenceState, ExternalScorers, GENERATED_ANSWERS,
    GENERATED_WITH, GENERATION_TRACES, GeneratedWith, GenerationTrace, JudgeFailure, JudgeModel,
    Judged, PublishEvaluation, RecordedAnswer, Registry as Evaluations, ScorerFailure, StepOrigin,
    StepSeen, TracedAnswer, TracedCall, TracedRun, VariantManifest, external_questions,
    external_replies, questions, replies, score_spelled, trace_answers,
};
use aiwatcher_execution::{
    ActivityCommand, ActivityContext, ActivityError, ActivityExecutor, ActivityResult,
    ExecutorRegistry, FailureClass, RuntimeBinding, RuntimeKind,
};
use async_trait::async_trait;
use serde_json::json;

use super::artifacts::Artifacts;

/// The scoring executor, if this deployment has an evaluation registry.
///
/// No registry means no executor, which means a `score_evaluation` attempt is
/// never claimed here — the same shape as the publisher's missing dataset
/// registry, and the same consequence: the work waits rather than failing.
#[must_use]
pub fn executors(state: &AppState, artifacts: Option<&Artifacts>) -> ExecutorRegistry {
    let registry = ExecutorRegistry::new();
    let Some(evaluations) = state.evaluations.as_ref() else {
        return registry;
    };
    tracing::info!("the serve role scores recorded answers");
    let mut scoring = ScoreExecutor::new(Arc::clone(evaluations));
    if let Some(artifacts) = artifacts {
        scoring = scoring.reading_from(artifacts.clone());
    }
    let registry = registry.with(Arc::new(scoring));
    // A run whose answers a worker generates starts by handing it the cases,
    // which needs somewhere to put them, and reads the traces of what came
    // back off this role's fold of the log. No object store, no such steps
    // here — they wait for a process that has one.
    match artifacts {
        Some(artifacts) => registry
            .with(Arc::new(CasesExecutor {
                evaluations: Arc::clone(evaluations),
                artifacts: artifacts.clone(),
            }))
            .with(Arc::new(TracesExecutor {
                evaluations: Arc::clone(evaluations),
                artifacts: artifacts.clone(),
                read_model: Arc::clone(&state.read_model),
                bundles: state.evaluation_bundles.clone(),
                witnesses: state.witnesses.clone(),
                prompts: state.prompts.clone(),
                wait: TELEMETRY_WAIT,
            })),
        None => registry,
    }
}

/// The judging executor, if this deployment has a registry and a judge.
///
/// Either missing registers nothing, so a `judge_evaluation` attempt waits for
/// a process that holds both rather than failing in one that holds neither.
#[must_use]
pub fn judged(
    state: &AppState,
    config: &crate::config::Config,
    artifacts: Option<&Artifacts>,
) -> ExecutorRegistry {
    let registry = ExecutorRegistry::new();
    let Some(evaluations) = state.evaluations.as_ref() else {
        return registry;
    };
    let judge = match super::judge::OpenAiJudge::from_config(config) {
        Ok(Some(judge)) => judge,
        Ok(None) => return registry,
        Err(error) => {
            tracing::error!(%error, "the judge client did not build; no judged run is claimed here");
            return registry;
        }
    };
    tracing::info!(provider = judge.provider(), "the work role asks a judge");
    let mut executor = ScoreExecutor::new(Arc::clone(evaluations))
        .judged_by(Arc::new(judge), config.judge_concurrency);
    if let Some(artifacts) = artifacts {
        executor = executor.reading_from(artifacts.clone());
    }
    registry.with(Arc::new(executor))
}

/// The executor for runs whose card asks a scorer service, if this deployment
/// has a registry and a service — asking a judge too, when it has one.
#[must_use]
pub fn external(
    state: &AppState,
    config: &crate::config::Config,
    scorers: Option<&Arc<dyn ExternalScorers>>,
    artifacts: Option<&Artifacts>,
) -> ExecutorRegistry {
    let registry = ExecutorRegistry::new();
    let (Some(evaluations), Some(scorers)) = (state.evaluations.as_ref(), scorers) else {
        return registry;
    };
    let mut executor = ScoreExecutor::new(Arc::clone(evaluations))
        .scored_by(Arc::clone(scorers), config.scorer_concurrency);
    if let Some(artifacts) = artifacts {
        executor = executor.reading_from(artifacts.clone());
    }
    match super::judge::OpenAiJudge::from_config(config) {
        Ok(Some(judge)) => {
            executor = executor.judged_by(Arc::new(judge), config.judge_concurrency);
        }
        Ok(None) => {}
        Err(error) => {
            tracing::error!(%error, "the judge client did not build; a card asking one fails here");
        }
    }
    tracing::info!("the work role asks a scorer service");
    registry.with(Arc::new(executor))
}

#[derive(Debug)]
pub struct ScoreExecutor {
    evaluations: Arc<Evaluations>,
    judge: Option<(Arc<dyn JudgeModel>, usize)>,
    scorers: Option<(Arc<dyn ExternalScorers>, usize)>,
    /// Where a generated run's answers are read from: the rows the generation
    /// step's worker wrote, which reach this step as its input.
    artifacts: Option<Artifacts>,
}

impl ScoreExecutor {
    #[must_use]
    pub const fn new(evaluations: Arc<Evaluations>) -> Self {
        Self {
            evaluations,
            judge: None,
            scorers: None,
            artifacts: None,
        }
    }

    /// The same executor, reading a generated run's answers from this store.
    #[must_use]
    pub fn reading_from(mut self, artifacts: Artifacts) -> Self {
        self.artifacts = Some(artifacts);
        self
    }

    /// The answers a worker generated, as the rows the step before this wrote —
    /// read once what the task generated them with agrees with the variant.
    ///
    /// Read from this attempt's own inputs, which the run pinned when the
    /// generation step completed: a retry of this step reads the same answers,
    /// and never asks the application again.
    async fn generated(
        &self,
        command: &ActivityCommand,
        variant: &VariantManifest,
    ) -> Result<
        (
            Vec<RecordedAnswer>,
            Option<GenerationTrace>,
            std::collections::BTreeMap<String, String>,
        ),
        ActivityError,
    > {
        let Some(artifacts) = &self.artifacts else {
            return Err(ActivityError::user_code(
                "this run's answers were generated, and this process holds no object store to \
                 read them from",
            ));
        };
        let input = |name: &str| {
            command
                .inputs
                .iter()
                .find(|input| input.name == name)
                .ok_or_else(|| {
                    ActivityError::user_code(format!(
                        "this run's answers were generated, and the generation step wrote no \
                         `{name}`"
                    ))
                })
        };
        let with = artifacts
            .read_rows(input(GENERATED_WITH)?)
            .await?
            .into_iter()
            .map(|row| {
                serde_json::from_value::<GeneratedWith>(serde_json::Value::Object(
                    row.into_iter().collect(),
                ))
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                ActivityError::user_code(format!(
                    "the generation step's `{GENERATED_WITH}` is not what it generated with \
                     ({{\"code\", \"generation_config\", \"response_schema\"?, \"tools\"?}}): \
                     {error}"
                ))
            })?;
        let [with] = with.as_slice() else {
            return Err(ActivityError::user_code(format!(
                "the generation step's `{GENERATED_WITH}` holds {} rows, and says what it \
                 generated with in one",
                with.len()
            )));
        };
        let disagreements = with.disagreements(variant);
        if !disagreements.is_empty() {
            return Err(ActivityError::user_code(format!(
                "the task did not generate with what the variant pins, so its answers are not \
                 the variant's — {}",
                disagreements.join("; ")
            )));
        }
        let mut answers = read_answers(artifacts, input(GENERATED_ANSWERS)?).await?;
        let spelled = spelled_answers(artifacts, input(GENERATED_ANSWERS)?).await?;
        // What the traces step found, when the plan has one: a run started
        // before it was part of the template has none, and says nothing.
        let traces = match command
            .inputs
            .iter()
            .find(|input| input.name == GENERATION_TRACES)
        {
            Some(traced) => {
                let rows = artifacts
                    .read_rows(traced)
                    .await?
                    .into_iter()
                    .map(|row| {
                        serde_json::from_value::<TracedAnswer>(serde_json::Value::Object(
                            row.into_iter().collect(),
                        ))
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| {
                        ActivityError::user_code(format!(
                            "the traces step's `{GENERATION_TRACES}` is not what the traces \
                             showed: {error}"
                        ))
                    })?;
                // A case leads to the trace its run was seen in, when the
                // application did not name one itself.
                for answer in &mut answers {
                    let Some(row) = rows.iter().find(|row| row.case_id == answer.case_id) else {
                        continue;
                    };
                    if answer.trace_id.is_none() {
                        answer.trace_id.clone_from(&row.trace_id);
                    }
                    // What the run's calls reported, by model, where the task
                    // said nothing about which models it called.
                    if !row.models.is_empty() {
                        let usage = answer.usage.get_or_insert_with(Default::default);
                        if usage.models.is_empty() {
                            usage.models.clone_from(&row.models);
                        }
                    }
                }
                Some(GenerationTrace::of(&rows))
            }
            None => None,
        };
        Ok((answers, traces, spelled))
    }

    /// The same executor, putting cases to this scorer service this many at a
    /// time — and so performing `external_evaluation`.
    #[must_use]
    pub fn scored_by(mut self, scorers: Arc<dyn ExternalScorers>, concurrency: usize) -> Self {
        self.scorers = Some((scorers, concurrency));
        self
    }

    /// The same executor, asking this judge this many questions at a time —
    /// and so performing `judge_evaluation` rather than `score_evaluation`.
    #[must_use]
    pub fn judged_by(mut self, judge: Arc<dyn JudgeModel>, concurrency: usize) -> Self {
        self.judge = Some((judge, concurrency));
        self
    }
}

#[async_trait]
impl ActivityExecutor for ScoreExecutor {
    fn runtime(&self) -> RuntimeKind {
        if self.scorers.is_some() {
            RuntimeKind::ExternalEvaluation
        } else if self.judge.is_some() {
            RuntimeKind::JudgeEvaluation
        } else {
            RuntimeKind::ScoreEvaluation
        }
    }

    async fn execute(
        &self,
        command: &ActivityCommand,
        context: &ActivityContext,
    ) -> Result<ActivityResult, ActivityError> {
        let (RuntimeBinding::ScoreEvaluation(spec)
        | RuntimeBinding::JudgeEvaluation(spec)
        | RuntimeBinding::ExternalEvaluation(spec)) = &command.step.runtime
        else {
            return Err(ActivityError::user_code("this step does not score"));
        };
        let Prepared {
            declared,
            card,
            rubrics,
            taken,
            external_taken,
            manifest,
            subject,
        } = prepare(&self.evaluations, &spec.declaration, command).await?;
        let run = &declared.run;
        let reads_archive = manifest
            .context
            .judge
            .as_ref()
            .is_some_and(|judge| judge.reads_archive);
        // The same authority for a calibration set taken from conversation
        // evidence: the admitted context says its judge, or its scorer
        // service, reads the archive.
        let external_reads_archive = manifest
            .context
            .external_calibration
            .as_ref()
            .is_some_and(|pin| pin.reads_archive);
        let evaluations = self.evaluations.as_ref().clone().with_content_access(
            manifest.context.dataset.kind == DatasetKind::Conversations
                || reads_archive
                || external_reads_archive,
        );
        if reads_archive {
            tracing::warn!(
                evaluation = %run.evaluation_id,
                provider = manifest.context.judge.as_ref().map_or("", |judge| judge.provider.as_str()),
                "a judge is being sent words from the conversation archive, under an admitted pair \
                 whose context says so"
            );
        }
        // Between the pieces of work rather than inside them: a stop that
        // arrives while the cohort is read is honoured before anybody is asked
        // anything about it.
        context.stop.check()?;
        let cohort = evaluations
            .cohort_cases(&manifest, &subject)
            .await
            .map_err(refusal)?;
        let (answers, traces, spelled) = match run.answers.generation() {
            Some(_) => self.generated(command, &run.variant).await?,
            None => (
                evaluations
                    .answers(run, &cohort.expected)
                    .await
                    .map_err(refusal)?,
                None,
                evaluations.spelled_answers(run).await.map_err(refusal)?,
            ),
        };
        let now = time::OffsetDateTime::now_utc().unix_timestamp();

        // A scorer service's numbers, before the judge's: both are handed to
        // the fold in one map, and neither asks anything of the other.
        let (scored_elsewhere, external_report, external_asked) =
            if card.scorecard.asks_a_scorer_service() {
                let Some((scorers, ceiling)) = &self.scorers else {
                    return Err(ActivityError::user_code(
                        "this card asks a scorer service, and this process holds none",
                    ));
                };
                context.stop.check()?;
                // Held to the card before anything is asked: a service now running
                // another release, or grading with another model, would measure
                // something else under this card's name.
                let live = scorers.catalog().await.map_err(scorer_failure)?;
                for (index, spec) in card.scorecard.scorers.iter().enumerate() {
                    if let Some(external) = spec.scorer.external() {
                        aiwatcher_evaluation::resolve_external(
                            &live,
                            &format!("scorecard.scorers[{index}].scorer"),
                            external.adapter,
                            external.metric,
                            external.parameters,
                            external.declared,
                        )
                        .map_err(|error| ActivityError::user_code(error.to_string()))?;
                    }
                }
                let calibrated = match &external_taken {
                    Some(taken) => Some(
                        evaluations
                            .calibrated(
                                &taken.calibration,
                                card.scorecard.scorers.iter().any(|spec| {
                                    spec.scorer
                                        .external()
                                        .is_some_and(|external| external.calibration.is_some())
                                        && spec.input_path.is_some()
                                }),
                                &subject,
                                now,
                            )
                            .await
                            .map_err(refusal)?,
                    ),
                    None => None,
                };
                let asking = external_questions(
                    &card.scorecard,
                    &cohort,
                    &answers,
                    external_taken
                        .as_ref()
                        .zip(calibrated.as_ref())
                        .map(|(taken, calibrated)| (&taken.calibration, calibrated)),
                );
                let remembering: Arc<dyn ExternalScorers> =
                    Arc::new(super::scorers::Remembering::new(
                        Arc::clone(scorers),
                        Arc::clone(&self.evaluations),
                        spec.declaration.clone(),
                    ));
                let replies = super::scorers::score_all(
                    &remembering,
                    asking
                        .questions
                        .iter()
                        .map(|question| question.call.clone())
                        .collect(),
                    paced(run.settings.concurrency, *ceiling),
                    &context.stop,
                )
                .await
                .map_err(|failure| match context.stop.requested() {
                    Some(reason) => reason.as_error(),
                    None => scorer_failure(failure),
                })?;
                let (scored, report) = external_replies(
                    run,
                    &card.scorecard,
                    &rubrics,
                    external_taken.as_ref().map(|taken| &taken.calibration),
                    &asking,
                    &replies,
                );
                (scored, report, asking.questions.len())
            } else {
                (Judged::new(), None, 0)
            };

        let (judged, report, asked) = match (&run.judge, &self.judge) {
            (None, _) => (Judged::new(), None, 0),
            (Some(_), None) => {
                return Err(ActivityError::user_code(
                    "this card asks a judge, and this process holds none",
                ));
            }
            (Some(declared), Some((judge, concurrency))) => {
                if declared.provider != judge.provider() {
                    return Err(ActivityError::user_code(format!(
                        "this run was declared for judge profile {} and this deployment's is {} \
                         (AIWATCHER_JUDGE_PROVIDER)",
                        declared.provider,
                        judge.provider()
                    )));
                }
                let Some(taken) = &taken else {
                    return Err(ActivityError::user_code(
                        "the calibration set this run names is gone",
                    ));
                };
                let shows_inputs = card
                    .scorecard
                    .scorers
                    .iter()
                    .any(|spec| spec.input_path.is_some());
                let calibrated = evaluations
                    .calibrated(&taken.calibration, shows_inputs, &subject, now)
                    .await
                    .map_err(refusal)?;
                let asking = questions(
                    run,
                    &card.scorecard,
                    &rubrics,
                    &cohort,
                    &answers,
                    &taken.calibration,
                    &calibrated,
                );
                // Kept per question under this declaration, so an attempt
                // after this one asks only what nobody answered yet.
                let remembering: Arc<dyn JudgeModel> = Arc::new(super::judge::Remembering::new(
                    Arc::clone(judge),
                    Arc::clone(&self.evaluations),
                    spec.declaration.clone(),
                ));
                let said = super::judge::ask_all(
                    &remembering,
                    asking
                        .questions
                        .iter()
                        .map(|question| question.call.clone())
                        .collect(),
                    paced(run.settings.concurrency, *concurrency),
                    &context.stop,
                )
                .await
                .map_err(|failure| {
                    // A stop is reported as the stop, never as the outage the
                    // dropped questions would otherwise read as.
                    if let Some(reason) = context.stop.requested() {
                        return reason.as_error();
                    }
                    match failure {
                        JudgeFailure::Unavailable(_) => {
                            ActivityError::transient(failure.to_string())
                        }
                        JudgeFailure::Refused(_) => ActivityError::user_code(failure.to_string()),
                    }
                })?;
                let (judged, report) = replies(
                    run,
                    &card.scorecard,
                    &rubrics,
                    &taken.calibration,
                    &asking,
                    &said,
                );
                (judged, report, asking.questions.len())
            }
        };

        // The last look. Past here the result is being published, and a
        // publication stopped halfway is worth less than one finished a few
        // seconds after a cancel, so the reactor waits for it however late.
        let _committing = context.stop.committing()?;
        let mut judged = judged;
        judged.extend(scored_elsewhere);
        let scored = score_spelled(
            &card.scorecard,
            &cohort.expected,
            &answers,
            &run.repetition_id,
            &judged,
            &spelled,
        );
        let (status, measured, failed) = (
            scored.status,
            scored.cases.len(),
            scored
                .cases
                .iter()
                .filter(|case| case.error.is_some())
                .count(),
        );
        let receipt = evaluations
            .publish(
                PublishEvaluation {
                    manifest,
                    status,
                    cases: scored.cases,
                    judge: report.clone(),
                    external: external_report.clone(),
                    traces: traces.clone(),
                },
                &subject,
                now,
            )
            .await
            .map_err(refusal)?;

        Ok(ActivityResult {
            outputs: Vec::new(),
            result: Some(json!({
                "evaluation_id": receipt.evaluation_id,
                "version": receipt.version,
                "variant_id": receipt.variant_id,
                "context_id": receipt.context_id,
                "status": status,
                "selected": cohort.expected.len(),
                "scored": measured - failed,
                "failed": failed,
                "unscored": cohort.expected.len() - measured,
                "judge_questions": asked,
                "scorer_questions": external_asked,
                "judge": report,
                "external": external_report,
                "traces": traces,
            })),
            diagnostics: None,
            awaiting: None,
            ..ActivityResult::default()
        })
    }
}

/// The first step of a run whose answers a worker generates: each case's input,
/// as rows the worker reads.
///
/// Only the inputs. What a case expected stays with the cohort's owner and the
/// step that scores, because a generator that could read the expectations
/// could answer by copying them — and nothing in the numbers would say so.
#[derive(Debug)]
pub struct CasesExecutor {
    evaluations: Arc<Evaluations>,
    artifacts: Artifacts,
}

impl CasesExecutor {
    #[must_use]
    pub const fn new(evaluations: Arc<Evaluations>, artifacts: Artifacts) -> Self {
        Self {
            evaluations,
            artifacts,
        }
    }
}

#[async_trait]
impl ActivityExecutor for CasesExecutor {
    fn runtime(&self) -> RuntimeKind {
        RuntimeKind::EvaluationCases
    }

    async fn execute(
        &self,
        command: &ActivityCommand,
        context: &ActivityContext,
    ) -> Result<ActivityResult, ActivityError> {
        let RuntimeBinding::EvaluationCases(spec) = &command.step.runtime else {
            return Err(ActivityError::user_code("this step reads no cohort"));
        };
        let Prepared {
            declared,
            manifest,
            subject,
            ..
        } = prepare(&self.evaluations, &spec.declaration, command).await?;
        if declared.run.answers.generation().is_none() {
            return Err(ActivityError::user_code(
                "this run scores answers it already has, so nothing is generated for its cases",
            ));
        }
        context.stop.check()?;
        let cohort = self
            .evaluations
            .cohort_cases(&manifest, &subject)
            .await
            .map_err(refusal)?;
        // In the cohort's own order, which is the owner's — so the rows a
        // worker is handed, and the answers it writes back, read the way the
        // cohort does.
        let rows: Vec<std::collections::BTreeMap<String, serde_json::Value>> = cohort
            .expected
            .keys()
            .map(|case_id| {
                let mut row = std::collections::BTreeMap::new();
                row.insert("case_id".to_owned(), json!(case_id));
                if let Some(input) = cohort.inputs.get(case_id) {
                    row.insert("input".to_owned(), input.clone());
                }
                row
            })
            .collect();
        let artifact = self
            .artifacts
            .put_rows(aiwatcher_evaluation::COHORT_INPUTS, &rows)
            .await?;
        Ok(ActivityResult {
            outputs: vec![artifact],
            result: Some(json!({
                "cases": rows.len(),
                "with_input": cohort.inputs.len(),
            })),
            diagnostics: None,
            awaiting: None,
            // The cohort is pinned, but a result is only ever read by the run
            // that produced it: the generation after it is never cached.
            cacheable: false,
        })
    }
}

/// The answers a generation step wrote, one row each.
async fn read_answers(
    artifacts: &Artifacts,
    written: &aiwatcher_core::ArtifactRef,
) -> Result<Vec<RecordedAnswer>, ActivityError> {
    artifacts
        .read_rows(written)
        .await?
        .into_iter()
        .enumerate()
        .map(|(index, row)| {
            serde_json::from_value::<RecordedAnswer>(serde_json::Value::Object(
                row.into_iter().collect(),
            ))
            .map_err(|error| {
                ActivityError::user_code(format!(
                    "the generation step's row {index} is not an answer ({{\"case_id\", \
                     \"answer\", \"run_id\"?, \"trace_id\"?, \"span_id\"?, \"usage\"?}}): {error}"
                ))
            })
        })
        .collect()
}

/// Each answer's JSON as the generation step wrote it, by its case: an integer
/// wider than 64 bits, or a decimal longer than a double keeps, is still every
/// digit it was sent with here, where a parsed answer holds the double nearest
/// it.
async fn spelled_answers(
    artifacts: &Artifacts,
    written: &aiwatcher_core::ArtifactRef,
) -> Result<std::collections::BTreeMap<String, String>, ActivityError> {
    #[derive(serde::Deserialize)]
    struct Spelled {
        case_id: String,
        answer: Box<serde_json::value::RawValue>,
    }
    let bytes = artifacts.read_bytes(written).await?;
    Ok(serde_json::from_slice::<Vec<Spelled>>(&bytes)
        .map(|rows| {
            rows.into_iter()
                .map(|row| (row.case_id, row.answer.get().to_owned()))
                .collect()
        })
        .unwrap_or_default())
}

/// How long the traces step waits for the application's telemetry to reach
/// the log: the SDK flushes every second, and the fold follows the log closely.
const TELEMETRY_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// The step between a generation and its scoring: the run each answer names,
/// read off this deployment's fold of the log and held to the variant's prompt
/// and model.
///
/// In the serve role, because the fold is there and nowhere else. It waits a
/// bounded while for runs still arriving — telemetry is sent in batches — and
/// then reports what it saw: a run the log never received is unseen and
/// counted, while a run whose calls contradict the pins fails the step with
/// every contradiction named, and nothing is published.
#[derive(Debug)]
pub struct TracesExecutor {
    evaluations: Arc<Evaluations>,
    artifacts: Artifacts,
    read_model: Arc<aiwatcher_projector::ReadModel>,
    /// Where the pair's bundle is read: the declaration of a pinned workflow,
    /// which a run's own declaration is compared with.
    bundles: Option<Arc<dyn aiwatcher_evaluation::ApprovalBundles>>,
    /// The credentials whose runs may witness an answer — empty, any other
    /// than the answer's own — and the key each one's digests are made under.
    witnesses: aiwatcher_evaluation::Witnesses,
    /// Where a pinned judging prompt's text is read: the order of its
    /// placeholders is what an order a judge was shown is held to.
    prompts: Option<Arc<aiwatcher_prompts::Registry>>,
    wait: std::time::Duration,
}

impl TracesExecutor {
    #[must_use]
    pub fn new(
        evaluations: Arc<Evaluations>,
        artifacts: Artifacts,
        read_model: Arc<aiwatcher_projector::ReadModel>,
        wait: std::time::Duration,
    ) -> Self {
        Self {
            evaluations,
            artifacts,
            read_model,
            bundles: None,
            witnesses: aiwatcher_evaluation::Witnesses::default(),
            prompts: None,
            wait,
        }
    }

    /// Where a pinned judging prompt's text is read from.
    #[must_use]
    pub fn reading_prompts_from(mut self, prompts: Arc<aiwatcher_prompts::Registry>) -> Self {
        self.prompts = Some(prompts);
        self
    }

    /// The text of the judging prompt version a pinned order is held to;
    /// `None` where this process holds no registry, or it holds no such version.
    async fn judging_template(
        &self,
        name: &str,
        version: &str,
    ) -> Result<Option<String>, ActivityError> {
        let (Some(prompts), Ok(name), Ok(version)) = (
            &self.prompts,
            aiwatcher_core::prompts::PromptName::parse(name),
            aiwatcher_core::prompts::PromptVersionId::parse(version),
        ) else {
            return Ok(None);
        };
        match prompts.verified_version(&name, &version).await {
            Ok(found) => Ok(found.map(|found| found.text)),
            Err(error) if error.is_retryable() => Err(ActivityError::transient(error.to_string())),
            Err(error) => Err(ActivityError::user_code(format!(
                "the judging prompt the variant pins could not be read: {error}"
            ))),
        }
    }

    /// Only these credentials' runs witness an answer, each with its key.
    #[must_use]
    pub fn witnessed_by(mut self, witnesses: aiwatcher_evaluation::Witnesses) -> Self {
        self.witnesses = witnesses;
        self
    }

    /// Where the declaration of a pinned workflow is read from.
    #[must_use]
    pub fn reading_bundles_from(
        mut self,
        bundles: Arc<dyn aiwatcher_evaluation::ApprovalBundles>,
    ) -> Self {
        self.bundles = Some(bundles);
        self
    }

    /// The runs named, as the fold holds them once each has ended and every
    /// model call it started has a span — with the calls runs published by a
    /// serving host say they served for each.
    async fn finished(
        &self,
        named: &std::collections::BTreeSet<&str>,
    ) -> std::collections::BTreeMap<String, TracedRun> {
        let mut serving = self.read_model.serving(named).await;
        let mut runs = std::collections::BTreeMap::new();
        for run_id in named {
            let Some(detail) = self.read_model.run(run_id).await else {
                continue;
            };
            let calls = traced_calls(&detail);
            if detail.summary.status == aiwatcher_projector::RunStatus::Running
                || (calls.len() as u64) < detail.summary.llm_calls
            {
                continue;
            }
            let servers = serving.remove(*run_id).unwrap_or_default();
            // A serving host's run still open is a witness not yet finished
            // saying what it served; the answer's run waits with it.
            if servers.iter().any(|server| {
                server.summary.status == aiwatcher_projector::RunStatus::Running
                    || (traced_calls(server).len() as u64) < server.summary.llm_calls
            }) {
                continue;
            }
            let served_for_it = servers.iter().flat_map(traced_calls).collect();
            let tools_served_for_it = servers.iter().flat_map(traced_tools).collect();
            let summary = detail.summary;
            runs.insert(
                (*run_id).to_owned(),
                TracedRun {
                    trace_id: Some(summary.trace_id.to_hex()),
                    variant_id: summary.variant_id,
                    evaluation_id: summary.evaluation_id,
                    calls,
                    published_by: summary.published_by,
                    workflow: summary.workflow,
                    workflow_topology: summary.workflow_topology,
                    nodes_run: summary.nodes_run,
                    node_steps: summary
                        .node_steps
                        .into_iter()
                        .map(|step| match step {
                            aiwatcher_projector::NodeStep::Started(node) => StepSeen::Started(node),
                            aiwatcher_projector::NodeStep::Completed(node) => {
                                StepSeen::Completed(node)
                            }
                            aiwatcher_projector::NodeStep::Failed(node) => StepSeen::Failed(node),
                        })
                        .collect(),
                    node_steps_dropped: summary.node_steps_dropped,
                    served_for_it,
                    tools_served_for_it,
                },
            );
        }
        runs
    }

    /// A file the variant pins, read as JSON from the pair's bundle under its
    /// digest; `None` where the bundle holds none, or it is not JSON.
    async fn pinned_json(
        &self,
        approval_id: &str,
        pinned: &aiwatcher_core::ArtifactRef,
    ) -> Result<Option<serde_json::Value>, ActivityError> {
        let Some(bundles) = &self.bundles else {
            return Ok(None);
        };
        let Some(bytes) = bundles
            .member(approval_id, &pinned.name)
            .await
            .map_err(refusal)?
        else {
            return Ok(None);
        };
        if hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&bytes)) != pinned.digest {
            return Err(ActivityError::user_code(format!(
                "the {} this pair's bundle holds is not the file the variant pins",
                pinned.name
            )));
        }
        Ok(serde_json::from_slice(&bytes).ok())
    }

    /// The shape of the workflow the variant pins, from the declaration the
    /// pair's bundle holds under its digest; `None` where that names no node.
    async fn pinned_shape(
        &self,
        approval_id: &str,
        pin: &aiwatcher_evaluation::VersionReference,
    ) -> Result<Option<aiwatcher_core::topology::Topology>, ActivityError> {
        let Some(bundles) = &self.bundles else {
            return Ok(None);
        };
        let Some(bytes) = bundles
            .member(approval_id, "workflow.json")
            .await
            .map_err(refusal)?
        else {
            return Ok(None);
        };
        if hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&bytes)) != pin.version {
            return Err(ActivityError::user_code(format!(
                "the workflow.json this pair's bundle holds is not the declaration {} @ {} pins",
                pin.name, pin.version
            )));
        }
        Ok(serde_json::from_slice(&bytes)
            .ok()
            .and_then(|declaration| aiwatcher_core::topology::Topology::read(&declaration)))
    }
}

/// A whole-number attribute of a span, or nought.
fn number(span: &aiwatcher_core::ports::CompletedSpan, key: &str) -> i64 {
    span.attributes
        .iter()
        .find_map(|(name, value)| match value {
            aiwatcher_core::ports::AttrValue::Int(number) if name == key => Some(*number),
            _ => None,
        })
        .unwrap_or(0)
}

/// A yes-or-no attribute of a span, where it has one.
fn flag(span: &aiwatcher_core::ports::CompletedSpan, key: &str) -> Option<bool> {
    span.attributes
        .iter()
        .find_map(|(name, value)| match value {
            aiwatcher_core::ports::AttrValue::Bool(flag) if name == key => Some(*flag),
            _ => None,
        })
}

/// A list-of-text attribute of a span, or none.
fn list(span: &aiwatcher_core::ports::CompletedSpan, key: &str) -> Vec<String> {
    span.attributes
        .iter()
        .find_map(|(name, value)| match value {
            aiwatcher_core::ports::AttrValue::StrList(items) if name == key => Some(items.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

/// A run's model calls, as their spans say.
fn traced_calls(detail: &aiwatcher_projector::RunDetail) -> Vec<TracedCall> {
    use aiwatcher_core::attrs::{aiwatcher as own, genai};
    let text = |span: &aiwatcher_core::ports::CompletedSpan, key: &str| {
        span.attributes
            .iter()
            .find_map(|(name, value)| match value {
                aiwatcher_core::ports::AttrValue::Str(text) if name == key => Some(text.clone()),
                _ => None,
            })
    };
    detail
        .spans
        .iter()
        .filter(|span| text(span, genai::OPERATION_NAME).as_deref() == Some(genai::operation::CHAT))
        .map(|span| TracedCall {
            model: text(span, genai::REQUEST_MODEL),
            model_version: text(span, own::model::VERSION),
            prompt_name: text(span, own::prompt::NAME),
            prompt_version: text(span, own::prompt::VERSION_ID),
            served_model: text(span, genai::RESPONSE_MODEL),
            prompt_verified: flag(span, own::prompt::VERIFIED),
            prompt_exact: flag(span, own::prompt::EXACT),
            published_by: text(span, own::source::PUBLISHED_BY),
            input_tokens: number(span, genai::USAGE_INPUT_TOKENS),
            output_tokens: number(span, genai::USAGE_OUTPUT_TOKENS),
            cached_tokens: number(span, "gen_ai.usage.cached_tokens"),
            asked: list(span, own::witness::ASKED),
            replied: list(span, own::witness::REPLIED),
            rendered: list(span, own::witness::RENDERED),
            placed: list(span, own::witness::PLACED)
                .iter()
                .filter_map(|pair| pair.split_once(':'))
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .collect(),
            derived: list(span, own::witness::DERIVED)
                .iter()
                .filter_map(|pair| pair.split_once(':'))
                .map(|(value, source)| (value.to_owned(), source.to_owned()))
                .collect(),
            taken: list(span, own::witness::TAKEN),
            taking: text(span, own::witness::TAKING),
            took_nothing: flag(span, own::witness::TOOK_NOTHING) == Some(true),
            started_ms: i64::try_from(span.start.unix_timestamp_nanos() / 1_000_000).ok(),
        })
        .collect()
}

/// The tool calls a run's spans say a witness relayed.
fn traced_tools(detail: &aiwatcher_projector::RunDetail) -> Vec<aiwatcher_evaluation::TracedTool> {
    use aiwatcher_core::attrs::{aiwatcher as own, genai};
    let text = |span: &aiwatcher_core::ports::CompletedSpan, key: &str| {
        span.attributes
            .iter()
            .find_map(|(name, value)| match value {
                aiwatcher_core::ports::AttrValue::Str(text) if name == key => Some(text.clone()),
                _ => None,
            })
    };
    detail
        .spans
        .iter()
        .filter(|span| {
            text(span, genai::OPERATION_NAME).as_deref() == Some(genai::operation::EXECUTE_TOOL)
        })
        .map(|span| aiwatcher_evaluation::TracedTool {
            name: text(span, genai::TOOL_NAME),
            published_by: text(span, own::source::PUBLISHED_BY),
            arguments: list(span, own::witness::ARGUMENTS),
            returned: list(span, own::witness::RETURNED),
        })
        .collect()
}

#[async_trait]
impl ActivityExecutor for TracesExecutor {
    fn runtime(&self) -> RuntimeKind {
        RuntimeKind::EvaluationTraces
    }

    async fn execute(
        &self,
        command: &ActivityCommand,
        context: &ActivityContext,
    ) -> Result<ActivityResult, ActivityError> {
        let RuntimeBinding::EvaluationTraces(spec) = &command.step.runtime else {
            return Err(ActivityError::user_code("this step reads no traces"));
        };
        let Prepared {
            declared,
            manifest,
            subject,
            ..
        } = prepare(&self.evaluations, &spec.declaration, command).await?;
        // What a witness's digests of a request are tested for: each case's
        // input, read under the pair's admission as the cases step read it.
        let variant = &declared.run.variant;
        let witnesses = if variant.model.is_some() || variant.prompt.is_some() {
            let cohort = self
                .evaluations
                .cohort_cases(&manifest, &subject)
                .await
                .map_err(refusal)?;
            self.witnesses.clone().asked(cohort.inputs)
        } else {
            self.witnesses.clone()
        };
        let prepared = aiwatcher_evaluation::Evaluation::prepare(manifest)
            .map_err(|error| ActivityError::user_code(error.to_string()))?;
        let written = command
            .inputs
            .iter()
            .find(|input| input.name == GENERATED_ANSWERS)
            .ok_or_else(|| {
                ActivityError::user_code(format!(
                    "the traces step reads the generation's `{GENERATED_ANSWERS}`, and none was \
                     bound"
                ))
            })?;
        let answers = read_answers(&self.artifacts, written).await?;
        let witnesses = witnesses.spelled(spelled_answers(&self.artifacts, written).await?);
        let named: std::collections::BTreeSet<&str> = answers
            .iter()
            .filter_map(|answer| answer.run_id.as_deref())
            .collect();
        let deadline = tokio::time::Instant::now() + self.wait;
        let runs = loop {
            context.stop.check()?;
            let runs = self.finished(&named).await;
            if runs.len() == named.len() || tokio::time::Instant::now() >= deadline {
                break runs;
            }
            tokio::select! {
                reason = context.stop.stopped() => return Err(reason.as_error()),
                () = tokio::time::sleep(std::time::Duration::from_millis(250)) => {}
            }
        };
        let approval =
            aiwatcher_evaluation::approval_id(prepared.variant_id(), prepared.context_id())
                .map_err(refusal)?;
        let shape = match &declared.run.variant.workflow {
            Some(pin) => self.pinned_shape(&approval, pin).await?,
            None => None,
        };
        // A bound several edges share counts a cycle's rounds, so a declaration
        // putting one on anything else pins a count nothing could keep.
        if let (Some(pin), Some(problems)) = (
            &declared.run.variant.workflow,
            shape
                .as_ref()
                .map(aiwatcher_core::topology::Topology::misbounded)
                .filter(|problems| !problems.is_empty()),
        ) {
            return Err(ActivityError::user_code(format!(
                "the declaration of {} the variant pins bounds what is no cycle's way back — {}",
                pin.name,
                problems.join("; ")
            )));
        }
        // What the variant's generation config pins about making an answer out
        // of replies — taking one out, joining several, choosing among them —
        // and the shape an answer made of several replies has.
        let witnesses = if variant.prompt.is_some() {
            let config = self
                .pinned_json(&approval, &variant.generation_config)
                .await?;
            let shaped = match &variant.response_schema {
                Some(schema) => self.pinned_json(&approval, schema).await?,
                None => None,
            };
            let mut witnesses = witnesses.pinned(config.as_ref(), shaped);
            // A judge pinned to an order is held to where its prompt's text
            // places each candidate.
            if let Some((name, version)) = witnesses
                .judge_ordered_on()
                .map(|(name, version)| (name.to_owned(), version.to_owned()))
                && let Some(template) = self.judging_template(&name, &version).await?
            {
                witnesses = witnesses.judged_with(&template);
            }
            // Every call a witness relayed since the measurement started, in any
            // run: a case asked elsewhere is a question the application could
            // have chosen the run it answered in by.
            match self
                .read_model
                .workflow_execution(&command.key.execution_id.to_string())
                .await
            {
                Some(execution) => witnesses.asked_elsewhere(
                    self.read_model
                        .asked_since(execution.summary.started_at)
                        .await
                        .iter()
                        .flat_map(|detail| {
                            let caller = detail.summary.caller_run_id.clone();
                            traced_calls(detail).into_iter().map(move |call| {
                                aiwatcher_evaluation::CallElsewhere {
                                    caller_run_id: caller.clone(),
                                    call,
                                }
                            })
                        })
                        .collect(),
                ),
                None => witnesses.elsewhere_unread(),
            }
        } else {
            witnesses
        };
        let mut rows = trace_answers(
            &declared.run.variant,
            prepared.variant_id(),
            &declared.run.evaluation_id,
            &answers,
            &runs,
            shape.as_ref(),
            &witnesses,
        )
        .map_err(|contradictions| {
            ActivityError::user_code(format!(
                "the traces of these answers contradict what the variant pins, so they are not \
                 the variant's — {}",
                contradictions.join("; ")
            ))
        })?;
        // Of the runs not on the log, which a client counted and lost and which
        // no client opened for this result.
        let counted: Vec<aiwatcher_evaluation::RunsCounted> = self
            .read_model
            .measured_runs(&declared.run.evaluation_id)
            .await
            .into_iter()
            .map(|count| aiwatcher_evaluation::RunsCounted {
                client: count.client,
                attempt: count.attempt,
                opened: count.opened,
                arrived: count.arrived,
            })
            .collect();
        let mut started = std::collections::BTreeSet::new();
        for row in rows.iter().filter(|row| !row.seen) {
            if let Some(run_id) = row.run_id.as_deref()
                && self.read_model.run(run_id).await.is_some()
            {
                started.insert(run_id.to_owned());
            }
        }
        aiwatcher_evaluation::lost_or_unknown(&mut rows, &counted, |run_id| {
            started.contains(run_id)
        });
        let trace = GenerationTrace::of(&rows);
        let table: Vec<std::collections::BTreeMap<String, serde_json::Value>> = rows
            .iter()
            .map(|row| {
                serde_json::to_value(row)
                    .ok()
                    .and_then(|value| value.as_object().cloned())
                    .map(|object| object.into_iter().collect())
                    .unwrap_or_default()
            })
            .collect();
        let artifact = self.artifacts.put_rows(GENERATION_TRACES, &table).await?;
        Ok(ActivityResult {
            outputs: vec![artifact],
            result: Some(json!({ "traces": trace, "shortfall": trace.shortfall() })),
            diagnostics: None,
            awaiting: None,
            // What the log holds moves, so what it showed is read again.
            cacheable: false,
        })
    }
}

/// What every step of a scoring run reads before it does its own part.
struct Prepared {
    declared: aiwatcher_evaluation::DeclaredRun,
    card: aiwatcher_evaluation::ScorecardVersion,
    rubrics: aiwatcher_evaluation::Rubrics,
    taken: Option<aiwatcher_evaluation::CalibrationVersion>,
    external_taken: Option<aiwatcher_evaluation::CalibrationVersion>,
    manifest: aiwatcher_evaluation::EvaluationManifest,
    /// Who declared it, rather than who pressed start: the declaration is what
    /// the source's rights are resolved against, and repeating one is how two
    /// people reach the same run at all.
    subject: String,
}

/// The declaration, its card and its calibration sets, the manifest the run
/// publishes — and the pair's admission, asked before anything is read rather
/// than left to publication: for a conversation cohort, the admission is the
/// only authority there is to read it under.
async fn prepare(
    evaluations: &Evaluations,
    declaration: &str,
    command: &ActivityCommand,
) -> Result<Prepared, ActivityError> {
    let declared = evaluations
        .scoring_run(declaration)
        .await
        .map_err(refusal)?
        .ok_or_else(|| {
            ActivityError::user_code(format!("no scoring run is declared under {declaration}"))
        })?;
    let run = &declared.run;
    let card = evaluations
        .scorecard(&run.scorecard.name, Some(&run.scorecard.version))
        .await
        .map_err(refusal)?
        .ok_or_else(|| {
            ActivityError::user_code(format!(
                "{} has no version {}",
                run.scorecard.name, run.scorecard.version
            ))
        })?;
    let rubrics = evaluations
        .rubrics_for(&card.scorecard)
        .await
        .map_err(refusal)?;
    let calibration = |named: Option<&aiwatcher_evaluation::VersionReference>,
                       gone: &'static str| {
        let version = named.map(|named| named.version.clone());
        async move {
            match version {
                Some(version) => evaluations
                    .calibration(&version)
                    .await
                    .map_err(refusal)?
                    .map(Some)
                    .ok_or_else(|| ActivityError::user_code(gone)),
                None => Ok(None),
            }
        }
    };
    let taken = calibration(
        run.judge.as_ref().map(|judge| &judge.calibration),
        "the calibration set this run names is gone",
    )
    .await?;
    let external_taken = calibration(
        run.external_calibration.as_ref(),
        "the calibration set this run names for its framework metrics is gone",
    )
    .await?;
    let manifest = run
        .manifest(
            &card.scorecard,
            &rubrics,
            taken.as_ref().map(|taken| &taken.calibration),
            external_taken.as_ref().map(|taken| &taken.calibration),
            Some(&StepOrigin {
                execution_id: command.key.execution_id.to_string(),
                step_id: Some(command.key.step_id.clone()),
            }),
        )
        .map_err(refusal)?;
    evaluations.admission(&manifest).await.map_err(refusal)?;
    let subject = declared.declared_by.clone();
    Ok(Prepared {
        declared,
        card,
        rubrics,
        taken,
        external_taken,
        manifest,
        subject,
    })
}

/// The declaration's pace, never past the deployment's: the start refused a run
/// asking for more, and a declaration read back here is held to the same line.
fn paced(declared: Option<u32>, ceiling: usize) -> usize {
    declared
        .and_then(|asked| usize::try_from(asked).ok())
        .map_or(ceiling, |asked| asked.min(ceiling))
}

/// A scorer service's failure, as the class that decides whether to retry.
fn scorer_failure(failure: ScorerFailure) -> ActivityError {
    match failure {
        ScorerFailure::Unavailable(_) => ActivityError::transient(failure.to_string()),
        ScorerFailure::Refused(_) => ActivityError::user_code(failure.to_string()),
    }
}

/// A registry refusal, as the class that decides whether to retry.
///
/// The same split the API renders as a status: an object store that is down is
/// worth coming back for, and evidence whose source has been deleted or whose
/// pair was withdrawn says the same thing however many times it is asked.
fn refusal(error: EvaluationError) -> ActivityError {
    match error {
        EvaluationError::Storage(port) if port.is_retryable() => {
            ActivityError::transient(port.to_string())
        }
        EvaluationError::Storage(port) => {
            ActivityError::new(FailureClass::Infrastructure, port.to_string())
        }
        // Bytes that are gone or do not hash to what was pinned: the run and
        // the declaration were both fine, so this is the store's problem and
        // worth one more look.
        EvaluationError::Unavailable(
            state @ (EvidenceState::MissingArtifact | EvidenceState::CorruptArtifact),
        ) => ActivityError::new(
            FailureClass::Infrastructure,
            format!("the evidence this run reads is {state:?}"),
        ),
        other => ActivityError::user_code(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use aiwatcher_core::MessageId;
    use aiwatcher_evaluation::{
        Cohort, EvaluationManifest, MetricDirection, RegistryConfig, ResultStatus, Scorecard,
        Scorer, ScorerSpec, ScoringRun, SourceAuthority, SourceEvidence, VersionReference,
    };
    use aiwatcher_execution::plan::{
        CachePolicy, DefinitionKind, DefinitionRevision, ExecutionPlan, PlanStep, RetryPolicy,
    };
    use aiwatcher_execution::{AttemptKey, ExecutionId};
    use aiwatcher_prompts::adapters::memory::MemoryObjectStore;
    use serde_json::{Value, json};

    use super::*;

    /// The cohort a declaration selects, resolved the way a deployment's own
    /// adapter would.
    #[derive(Debug)]
    struct Expectations(BTreeMap<String, Value>);

    #[async_trait]
    impl SourceAuthority for Expectations {
        async fn resolve(
            &self,
            _: &EvaluationManifest,
            _: &str,
        ) -> Result<SourceEvidence, EvaluationError> {
            Ok(SourceEvidence {
                expected: self.0.clone(),
                ..Default::default()
            })
        }
    }

    fn cohort() -> BTreeMap<String, Value> {
        BTreeMap::from([
            ("capital-pl".to_owned(), json!("Warsaw")),
            ("two-plus-two".to_owned(), json!("four")),
        ])
    }

    fn card() -> Scorecard {
        Scorecard {
            name: "answer-quality".into(),
            description: String::new(),
            scorers: vec![ScorerSpec {
                metric: "exact".into(),
                answer_path: "/text".into(),
                expected_path: String::new(),
                input_path: None,
                scorer: Scorer::ExactMatch {
                    ignore_case: false,
                    trim: true,
                },
            }],
        }
    }

    fn declaration(version: &str, answers: aiwatcher_core::ArtifactRef) -> ScoringRun {
        let manifest: EvaluationManifest = serde_json::from_str(include_str!(
            "../../../../contracts/fixtures/evaluation-v1/manifest.json"
        ))
        .expect("the contract fixture parses");
        ScoringRun {
            evaluation_id: "scored-by-a-run".into(),
            repetition_id: "measurement-1".into(),
            variant: manifest.variant.clone(),
            cohort: Cohort {
                case_manifest: manifest.context.case_manifest.clone(),
                case_count: 2,
                split: manifest.context.split.clone(),
                input_schema: manifest.context.input_schema.clone(),
                expectations_schema: manifest.context.expectations_schema.clone(),
            },
            scorecard: VersionReference {
                name: "answer-quality".into(),
                version: version.into(),
            },
            answers: aiwatcher_evaluation::Answers::Recording(answers),
            judge: None,
            external_calibration: None,
            settings: Default::default(),
        }
    }

    fn attempt(declaration: &str) -> (ActivityCommand, ActivityContext) {
        let step = PlanStep {
            id: "score".to_owned(),
            runtime: RuntimeBinding::ScoreEvaluation(
                aiwatcher_execution::plan::ScoreEvaluationSpec {
                    declaration: declaration.to_owned(),
                },
            ),
            inputs: Vec::new(),
            outputs: Vec::new(),
            retry: RetryPolicy::default(),
            timeout_seconds: 60,
            cache: CachePolicy::Never,
        };
        let plan = ExecutionPlan::seal(
            DefinitionKind::Evaluation,
            "scored-by-a-run".to_owned(),
            DefinitionRevision(declaration.to_owned()),
            vec![step.clone()],
            Vec::new(),
        );
        let key = AttemptKey::new(ExecutionId::new("exec-1".to_owned()), "score", 1);
        (
            ActivityCommand {
                key,
                command_id: MessageId::new("dispatch-1".to_owned()),
                step,
                inputs: Vec::new(),
                parameters: BTreeMap::new(),
                answers: Vec::new(),
            },
            ActivityContext {
                owner: "serve".to_owned(),
                timeout: std::time::Duration::from_secs(60),
                context_id: "exec-1/score/1".to_owned(),
                plan: Arc::new(plan),
                stop: aiwatcher_execution::StopSignal::new(),
            },
        )
    }

    async fn evaluations(expected: BTreeMap<String, Value>) -> Arc<Evaluations> {
        Arc::new(
            Evaluations::new(
                Arc::new(MemoryObjectStore::new()),
                Arc::new(Expectations(expected)),
                RegistryConfig::default(),
            )
            .expect("a registry"),
        )
    }

    /// Stage, declare and admit — everything a start does before the reactor.
    async fn ready(registry: &Evaluations, answers: Value) -> String {
        let version = registry
            .publish_scorecard(&card(), "ada", 100)
            .await
            .expect("a card publishes")
            .version;
        let recording = registry
            .stage_recording(
                "answers.json",
                serde_json::to_vec(&answers).expect("answers encode"),
            )
            .await
            .expect("a recording stages");
        let declared = registry
            .declare_scoring_run(&declaration(&version, recording), "ada", 100)
            .await
            .expect("a run declares");
        registry
            .approve(
                &declared
                    .run
                    .manifest(
                        &card(),
                        &aiwatcher_evaluation::Rubrics::default(),
                        None,
                        None,
                        None,
                    )
                    .expect("a manifest derives"),
                "operator",
                100,
            )
            .await
            .expect("an operator admits the pair");
        declared.id
    }

    #[tokio::test]
    async fn a_run_publishes_what_it_measured_and_names_the_execution_that_did_it() {
        let registry = evaluations(cohort()).await;
        let declaration = ready(
            &registry,
            json!({"answers": [
                {"case_id": "two-plus-two", "answer": {"text": "four"}, "trace_id": "t-1"},
                {"case_id": "capital-pl", "answer": {"text": "Kraków"}}
            ]}),
        )
        .await;

        let (command, context) = attempt(&declaration);
        let result = ScoreExecutor::new(Arc::clone(&registry))
            .execute(&command, &context)
            .await
            .expect("the step scores");
        let reported = result.result.expect("a scoring step reports what it did");
        assert_eq!(reported["status"], "succeeded");
        assert_eq!(reported["scored"], 2);

        let evidence = registry
            .get("scored-by-a-run", "reader", 200)
            .await
            .expect("the evidence reads")
            .expect("a result was published");
        let manifest = evidence.manifest.expect("evidence keeps its manifest");
        assert_eq!(
            manifest.origin.execution_id.as_deref(),
            Some("exec-1"),
            "evidence a run produced names the run"
        );
        assert_eq!(manifest.origin.step_id.as_deref(), Some("score"));
        assert_eq!(
            manifest.context.metrics[0].direction,
            MetricDirection::Higher
        );
        assert_eq!(evidence.status, Some(ResultStatus::Succeeded));
        assert_eq!(evidence.metrics["exact"], 0.5);
    }

    #[tokio::test]
    async fn a_run_asked_to_stop_publishes_nothing_and_says_it_was_stopped() {
        // The executor looks between its pieces, and the last look is before
        // anything is published: a cancelled measurement must not leave a
        // result behind that reads as the one somebody asked for.
        let registry = evaluations(cohort()).await;
        let declaration = ready(
            &registry,
            json!({"answers": [
                {"case_id": "two-plus-two", "answer": {"text": "four"}},
                {"case_id": "capital-pl", "answer": {"text": "Warsaw"}}
            ]}),
        )
        .await;

        let (command, context) = attempt(&declaration);
        context
            .stop
            .stop(aiwatcher_execution::StopReason::RunStopping);
        let stopped = ScoreExecutor::new(Arc::clone(&registry))
            .execute(&command, &context)
            .await
            .expect_err("a stopped run does not score");
        assert_eq!(stopped.class, FailureClass::Policy, "{stopped}");
        assert!(
            registry
                .get("scored-by-a-run", "reader", 200)
                .await
                .expect("the catalogue reads")
                .is_none(),
            "nothing was published"
        );
    }

    /// A run admits nothing. It measures, and then asks to publish.
    #[tokio::test]
    async fn a_pair_nobody_admitted_is_refused_at_the_same_gate_a_producer_meets() {
        let registry = evaluations(cohort()).await;
        let version = registry
            .publish_scorecard(&card(), "ada", 100)
            .await
            .expect("a card publishes")
            .version;
        let recording = registry
            .stage_recording(
                "answers.json",
                serde_json::to_vec(&json!({"answers": [
                    {"case_id": "two-plus-two", "answer": {"text": "four"}},
                    {"case_id": "capital-pl", "answer": {"text": "Warsaw"}}
                ]}))
                .expect("answers encode"),
            )
            .await
            .expect("a recording stages");
        let declared = registry
            .declare_scoring_run(&declaration(&version, recording), "ada", 100)
            .await
            .expect("a run declares");

        let (command, context) = attempt(&declared.id);
        let refused = ScoreExecutor::new(Arc::clone(&registry))
            .execute(&command, &context)
            .await
            .expect_err("nothing admitted this variant and context");
        assert_eq!(
            refused.class,
            FailureClass::UserCode,
            "an operator has not acted, and asking again will not make them"
        );
        assert!(
            registry
                .get("scored-by-a-run", "reader", 200)
                .await
                .expect("the catalogue reads")
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_run_whose_declaration_nobody_stored_says_so_rather_than_scoring_nothing() {
        let registry = evaluations(cohort()).await;
        let (command, context) = attempt(&"a".repeat(64));
        let refused = ScoreExecutor::new(registry)
            .execute(&command, &context)
            .await
            .expect_err("there is no declaration under that address");
        assert!(refused.to_string().contains("no scoring run is declared"));
    }
}
