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

use std::sync::Arc;

use aiwatcher_api::state::AppState;
use aiwatcher_evaluation::{
    DatasetKind, EvaluationError, EvidenceState, JudgeFailure, JudgeModel, Judged,
    PublishEvaluation, Registry as Evaluations, StepOrigin, questions, replies, score_with,
};
use aiwatcher_execution::{
    ActivityCommand, ActivityContext, ActivityError, ActivityExecutor, ActivityResult,
    ExecutorRegistry, FailureClass, RuntimeBinding, RuntimeKind,
};
use async_trait::async_trait;
use serde_json::json;

/// The scoring executor, if this deployment has an evaluation registry.
///
/// No registry means no executor, which means a `score_evaluation` attempt is
/// never claimed here — the same shape as the publisher's missing dataset
/// registry, and the same consequence: the work waits rather than failing.
#[must_use]
pub fn executors(state: &AppState) -> ExecutorRegistry {
    let registry = ExecutorRegistry::new();
    let Some(evaluations) = state.evaluations.as_ref() else {
        return registry;
    };
    tracing::info!("the serve role scores recorded answers");
    registry.with(Arc::new(ScoreExecutor::new(Arc::clone(evaluations))))
}

/// The judging executor, if this deployment has a registry and a judge.
///
/// Either missing registers nothing, so a `judge_evaluation` attempt waits for
/// a process that holds both rather than failing in one that holds neither.
#[must_use]
pub fn judged(state: &AppState, config: &crate::config::Config) -> ExecutorRegistry {
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
    registry.with(Arc::new(
        ScoreExecutor::new(Arc::clone(evaluations))
            .judged_by(Arc::new(judge), config.judge_concurrency),
    ))
}

#[derive(Debug)]
pub struct ScoreExecutor {
    evaluations: Arc<Evaluations>,
    judge: Option<(Arc<dyn JudgeModel>, usize)>,
}

impl ScoreExecutor {
    #[must_use]
    pub const fn new(evaluations: Arc<Evaluations>) -> Self {
        Self {
            evaluations,
            judge: None,
        }
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
        if self.judge.is_some() {
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
        let (RuntimeBinding::ScoreEvaluation(spec) | RuntimeBinding::JudgeEvaluation(spec)) =
            &command.step.runtime
        else {
            return Err(ActivityError::user_code("this step does not score"));
        };
        let declared = self
            .evaluations
            .scoring_run(&spec.declaration)
            .await
            .map_err(refusal)?
            .ok_or_else(|| {
                ActivityError::user_code(format!(
                    "no scoring run is declared under {}",
                    spec.declaration
                ))
            })?;
        let run = &declared.run;
        let card = self
            .evaluations
            .scorecard(&run.scorecard.name, Some(&run.scorecard.version))
            .await
            .map_err(refusal)?
            .ok_or_else(|| {
                ActivityError::user_code(format!(
                    "{} has no version {}",
                    run.scorecard.name, run.scorecard.version
                ))
            })?;

        let rubrics = self
            .evaluations
            .rubrics_for(&card.scorecard)
            .await
            .map_err(refusal)?;
        let taken = match &run.judge {
            Some(declared) => Some(
                self.evaluations
                    .calibration(&declared.calibration.version)
                    .await
                    .map_err(refusal)?
                    .ok_or_else(|| {
                        ActivityError::user_code("the calibration set this run names is gone")
                    })?,
            ),
            None => None,
        };
        let manifest = run
            .manifest(
                &card.scorecard,
                &rubrics,
                taken.as_ref().map(|taken| &taken.calibration),
                Some(&StepOrigin {
                    execution_id: command.key.execution_id.to_string(),
                    step_id: Some(command.key.step_id.clone()),
                }),
            )
            .map_err(refusal)?;
        // Who declared it, rather than who pressed start: the declaration is
        // what the source's rights are resolved against, and repeating one is
        // how two people reach the same run at all.
        let subject = declared.declared_by.clone();
        // Asked before anything is read rather than left to publication: for
        // a conversation cohort, the admission is the only authority there is
        // to read it under.
        self.evaluations
            .admission(&manifest)
            .await
            .map_err(refusal)?;
        let reads_archive = manifest
            .context
            .judge
            .as_ref()
            .is_some_and(|judge| judge.reads_archive);
        // The same authority for a calibration set taken from conversation
        // evidence: the admitted context says its judge reads the archive.
        let evaluations = self.evaluations.as_ref().clone().with_content_access(
            manifest.context.dataset.kind == DatasetKind::Conversations || reads_archive,
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
        let answers = evaluations
            .answers(run, &cohort.expected)
            .await
            .map_err(refusal)?;
        let now = time::OffsetDateTime::now_utc().unix_timestamp();

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
                    *concurrency,
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
        // seconds after a cancel: the reactor's grace is for exactly this.
        context.stop.check()?;
        let scored = score_with(
            &card.scorecard,
            &cohort.expected,
            &answers,
            &run.repetition_id,
            &judged,
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
                "judge": report,
            })),
            diagnostics: None,
            awaiting: None,
            ..ActivityResult::default()
        })
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
