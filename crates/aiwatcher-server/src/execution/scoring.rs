//! Measuring answers somebody already recorded, where the ingress is.
//!
//! The second binding that runs in the `serve` role, and for the first one's
//! reason: it executes nothing. It reads a declaration, a card and a recording
//! this deployment already holds, folds them, and writes the result through the
//! registry the panel reads. No model of the application under test is called —
//! that is what `score_existing` means — so a new card measuring an unchanged
//! recording differs from the last one by the measurement alone.
//!
//! **The declaration is the whole input.** The plan carries its digest, so the
//! card, the cohort, the recording and the variant are pinned together and a
//! retry reads exactly what the first attempt read.
//!
//! **Publication still needs an admitted pair.** Nothing here approves
//! anything: a run whose variant and context nobody admitted is refused by the
//! registry, in the same words a producer's publication is.

use std::sync::Arc;

use aiwatcher_api::state::AppState;
use aiwatcher_evaluation::{
    EvaluationError, EvidenceState, PublishEvaluation, Registry as Evaluations, StepOrigin, score,
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
    registry.with(Arc::new(ScoreExecutor {
        evaluations: Arc::clone(evaluations),
    }))
}

#[derive(Debug)]
pub struct ScoreExecutor {
    evaluations: Arc<Evaluations>,
}

#[async_trait]
impl ActivityExecutor for ScoreExecutor {
    fn runtime(&self) -> RuntimeKind {
        RuntimeKind::ScoreEvaluation
    }

    async fn execute(
        &self,
        command: &ActivityCommand,
        _: &ActivityContext,
    ) -> Result<ActivityResult, ActivityError> {
        let RuntimeBinding::ScoreEvaluation(spec) = &command.step.runtime else {
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

        let manifest = run
            .manifest(
                &card.scorecard,
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
        let cohort = self
            .evaluations
            .cohort(&manifest, &subject)
            .await
            .map_err(refusal)?;
        let recorded = self
            .evaluations
            .recording(&run.answers)
            .await
            .map_err(refusal)?;

        let scored = score(
            &card.scorecard,
            &cohort,
            &recorded.answers,
            &run.repetition_id,
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
        let receipt = self
            .evaluations
            .publish(
                PublishEvaluation {
                    manifest,
                    status,
                    cases: scored.cases,
                },
                &subject,
                time::OffsetDateTime::now_utc().unix_timestamp(),
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
                "selected": cohort.len(),
                "scored": measured - failed,
                "failed": failed,
                "unscored": cohort.len() - measured,
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
            answers,
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
                    .manifest(&card(), None)
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
        let result = ScoreExecutor {
            evaluations: Arc::clone(&registry),
        }
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
        let refused = ScoreExecutor {
            evaluations: Arc::clone(&registry),
        }
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
        let refused = ScoreExecutor {
            evaluations: registry,
        }
        .execute(&command, &context)
        .await
        .expect_err("there is no declaration under that address");
        assert!(refused.to_string().contains("no scoring run is declared"));
    }
}
