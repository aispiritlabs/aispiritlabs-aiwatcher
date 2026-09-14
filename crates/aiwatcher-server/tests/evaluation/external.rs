//! A card's metrics measured by a scorer framework, through a service that
//! runs it — with a stand-in for the service, so every one of these is a rule.
use super::*;
use aiwatcher_execution::{ActivityExecutor, FailureClass};
use aiwatcher_server::execution::scoring::ScoreExecutor;
use serde_json::{Value, json};
use std::sync::Mutex;

#[derive(Debug)]
struct Expectations(BTreeMap<String, Value>);

#[async_trait]
impl SourceAuthority for Expectations {
    async fn resolve(&self, _: &EvaluationManifest, _: &str) -> Result<SourceEvidence> {
        Ok(SourceEvidence {
            expected: self.0.clone(),
            inputs: BTreeMap::from([
                (
                    "capital-pl".to_owned(),
                    json!({"question": "Capital of Poland?"}),
                ),
                ("two-plus-two".to_owned(), json!({"question": "2+2?"})),
            ]),
            ..Default::default()
        })
    }
}

fn catalog(version: &str) -> ScorerCatalog {
    serde_json::from_value(json!({
        "contract": 1,
        "adapters": [
            {"name": "opik", "version": version, "metrics": [
                {"metric": "equals", "unit": "ratio", "direction": "higher", "aggregation": "rate",
                 "reads": ["answer", "expected"],
                 "parameters": {"case_sensitive": {"kind": "boolean"}}}
            ]},
            {"name": "deepeval", "version": "4.2.2",
             "model": {"name": "gemma-4-e2b", "version": "ud-q4-k-xl"},
             "metrics": [
                {"metric": "answer_relevancy", "unit": "score", "direction": "higher",
                 "aggregation": "mean", "reads": ["input", "answer"], "model_graded": true,
                 "range": [0.0, 1.0], "parameters": {"threshold": {"kind": "number"}}}
            ]}
        ]
    }))
    .unwrap()
}

/// A scorer service that answers from a table, counts what it was asked and
/// describes itself with whatever catalog the test hands it.
#[derive(Debug)]
struct Service {
    catalog: Mutex<ScorerCatalog>,
    asked: Mutex<Vec<ExternalCall>>,
}

#[async_trait]
impl ExternalScorers for Service {
    async fn catalog(&self) -> std::result::Result<ScorerCatalog, ScorerFailure> {
        Ok(self.catalog.lock().unwrap().clone())
    }

    async fn score(
        &self,
        call: &ExternalCall,
    ) -> std::result::Result<ExternalReply, ScorerFailure> {
        self.asked.lock().unwrap().push(call.clone());
        Ok(match (call.metric.as_str(), &call.case.answer) {
            ("equals", answer) => ExternalReply {
                value: Some(f64::from(u8::from(
                    Some(answer) == call.case.expected.as_ref(),
                ))),
                failed: None,
            },
            ("answer_relevancy", answer) if answer == &json!("Kraków") => ExternalReply {
                value: None,
                failed: Some("MetricError: the model gave no verdict".into()),
            },
            _ => ExternalReply {
                value: Some(0.75),
                failed: None,
            },
        })
    }
}

fn card() -> Scorecard {
    serde_json::from_value(json!({
        "name": "framework-metrics",
        "scorers": [
            {"metric": "equals", "answer_path": "/text", "expected_path": "/text",
             "scorer": {"kind": "external", "adapter": "opik", "metric": "equals",
                        "parameters": {"case_sensitive": true}}},
            {"metric": "relevancy", "answer_path": "/text", "input_path": "/question",
             "scorer": {"kind": "external", "adapter": "deepeval", "metric": "answer_relevancy",
                        "parameters": {"threshold": 0.7}}}
        ]
    }))
    .unwrap()
}

async fn declared(
    registry: &Registry,
    card: &Scorecard,
    answers: Value,
) -> (String, EvaluationManifest) {
    let version = registry
        .publish_scorecard(card, "ada", 100)
        .await
        .unwrap()
        .version;
    let recording = registry
        .stage_recording("answers.json", serde_json::to_vec(&answers).unwrap())
        .await
        .unwrap();
    let manifest: EvaluationManifest = serde_json::from_str(include_str!(
        "../../../../contracts/fixtures/evaluation-v1/manifest.json"
    ))
    .unwrap();
    let run = ScoringRun {
        evaluation_id: "framework-scored".into(),
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
            name: card.name.clone(),
            version,
        },
        answers: Answers::Recording(recording),
        judge: None,
        external_calibration: None,
        settings: RunSettings {
            timeout_seconds: None,
            concurrency: Some(1),
            asked_since_seconds: None,
        },
    };
    let declared = registry
        .declare_scoring_run(&run, "ada", 100)
        .await
        .unwrap();
    let view = registry
        .scoring_run_view(&declared.id)
        .await
        .unwrap()
        .unwrap();
    registry
        .approve(&view.manifest, "operator", 100)
        .await
        .unwrap();
    (declared.id, view.manifest)
}

fn registry() -> Arc<Registry> {
    Arc::new(
        Registry::new(
            Arc::new(MemoryObjectStore::new()),
            Arc::new(Expectations(BTreeMap::from([
                ("capital-pl".to_owned(), json!({"text": "Warsaw"})),
                ("two-plus-two".to_owned(), json!({"text": "four"})),
            ]))),
            RegistryConfig::default(),
        )
        .unwrap(),
    )
}

#[tokio::test]
async fn a_card_names_a_framework_metric_and_publication_pins_what_the_service_said_it_is() {
    let registry = registry();
    let refused = registry
        .publish_scorecard(&card(), "ada", 100)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        refused.contains("AIWATCHER_SCORER_URL"),
        "no catalog, so nothing can say which way is better: {refused}"
    );

    registry
        .record_scorer_catalog(&catalog("2.2.59"), "work-1", 99)
        .await
        .unwrap();
    let published = registry
        .publish_scorecard(&card(), "ada", 100)
        .await
        .unwrap();
    let Scorer::External { declared, .. } = &published.scorecard.scorers[1].scorer else {
        panic!("an external scorer");
    };
    let declared = declared
        .as_ref()
        .expect("publication pinned the declaration");
    assert_eq!(declared.version, "4.2.2");
    assert_eq!(declared.model.as_ref().unwrap().name, "gemma-4-e2b");

    let metrics = published.scorecard.metrics(&Rubrics::default()).unwrap();
    assert_eq!(metrics[0].aggregation, Aggregation::Rate);
    assert!(metrics[0].measured_by.as_ref().unwrap().model.is_none());
    assert_eq!(
        metrics[1].measured_by.as_ref().unwrap().adapter,
        VersionReference {
            name: "deepeval".into(),
            version: "4.2.2".into()
        }
    );

    // A card brought from somewhere else with another release pinned is held to
    // this deployment's catalog, and says both.
    let mut elsewhere = published.scorecard.clone();
    if let Scorer::External { declared, .. } = &mut elsewhere.scorers[0].scorer {
        declared.as_mut().unwrap().version = "1.0.0".into();
    }
    let refused = registry
        .publish_scorecard(&elsewhere, "ada", 101)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        refused.contains("1.0.0") && refused.contains("2.2.59"),
        "{refused}"
    );
}

#[tokio::test]
async fn a_run_asks_the_service_publishes_its_numbers_and_says_a_model_graded_one() {
    let registry = registry();
    registry
        .record_scorer_catalog(&catalog("2.2.59"), "work-1", 99)
        .await
        .unwrap();
    let (declaration, manifest) = declared(
        &registry,
        &card(),
        json!({"answers": [
            {"case_id": "two-plus-two", "answer": {"text": "four"}},
            {"case_id": "capital-pl", "answer": {"text": "Warsaw"}}
        ]}),
    )
    .await;
    let view = registry
        .scoring_run_view(&declaration)
        .await
        .unwrap()
        .unwrap();
    assert!(
        view.warnings
            .iter()
            .any(|warning| warning.contains("`relevancy` is graded by gemma-4-e2b")),
        "{:?}",
        view.warnings
    );

    let service = Arc::new(Service {
        catalog: Mutex::new(catalog("2.2.59")),
        asked: Mutex::new(Vec::new()),
    });
    let executor = ScoreExecutor::new(Arc::clone(&registry)).scored_by(service.clone(), 4);
    let (command, context) = external_attempt(&declaration);
    let reported = executor
        .execute(&command, &context)
        .await
        .expect("the service scores every case")
        .result
        .unwrap();
    assert_eq!(reported["scorer_questions"], 4, "{reported}");
    let asked = service.asked.lock().unwrap().clone();
    for call in asked
        .iter()
        .filter(|call| call.metric == "answer_relevancy")
    {
        let question = if call.case.answer == json!("four") {
            "2+2?"
        } else {
            "Capital of Poland?"
        };
        assert_eq!(
            call.case.input,
            Some(json!(question)),
            "shown the question where the card points"
        );
        assert!(call.case.expected.is_none(), "it reads no expectation");
    }
    for call in asked.iter().filter(|call| call.metric == "equals") {
        assert!(call.case.input.is_none(), "it reads no question");
    }

    let evidence = registry
        .get("framework-scored", "viewer", 200)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(evidence.metrics["equals"], 1.0);
    assert_eq!(evidence.metrics["relevancy"], 0.75);
    assert!(!evidence.reproducible, "a model graded one of its numbers");
    assert_eq!(evidence.manifest.unwrap().context, manifest.context);

    // A retry asks nothing it was already told.
    executor.execute(&command, &context).await.unwrap();
    assert_eq!(service.asked.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn a_service_running_another_release_than_the_card_pinned_fails_the_run_naming_both() {
    let registry = registry();
    registry
        .record_scorer_catalog(&catalog("2.2.59"), "work-1", 99)
        .await
        .unwrap();
    let (declaration, _) = declared(
        &registry,
        &card(),
        json!({"answers": [
            {"case_id": "two-plus-two", "answer": {"text": "four"}},
            {"case_id": "capital-pl", "answer": {"text": "Kraków"}}
        ]}),
    )
    .await;
    let service = Arc::new(Service {
        catalog: Mutex::new(catalog("2.3.0")),
        asked: Mutex::new(Vec::new()),
    });
    let (command, context) = external_attempt(&declaration);
    let refused = ScoreExecutor::new(Arc::clone(&registry))
        .scored_by(service.clone(), 4)
        .execute(&command, &context)
        .await
        .unwrap_err();
    assert_eq!(refused.class, FailureClass::UserCode);
    assert!(
        refused.message.contains("2.2.59") && refused.message.contains("2.3.0"),
        "{}",
        refused.message
    );
    assert!(
        service.asked.lock().unwrap().is_empty(),
        "nothing was asked"
    );

    // The same release: the case the model gave no verdict for fails with the
    // adapter's sentence, and never a zero.
    *service.catalog.lock().unwrap() = catalog("2.2.59");
    ScoreExecutor::new(Arc::clone(&registry))
        .scored_by(service, 4)
        .execute(&command, &context)
        .await
        .unwrap();
    let page = registry
        .get("framework-scored", "viewer", 300)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(page.status, Some(ResultStatus::Partial));
    let version = page.receipt.version.clone();
    let cases = registry
        .cases("framework-scored", &version, None, None, "viewer", 300)
        .await
        .unwrap()
        .unwrap();
    let failed = cases
        .cases
        .iter()
        .find(|case| case.measurement.case_id == "capital-pl")
        .unwrap();
    assert!(
        failed
            .measurement
            .error
            .as_deref()
            .unwrap()
            .contains("the scorer service: MetricError"),
        "{:?}",
        failed.measurement.error
    );
}

fn external_attempt(
    declaration: &str,
) -> (
    aiwatcher_execution::ActivityCommand,
    aiwatcher_execution::ActivityContext,
) {
    let (mut command, mut context) = attempt(declaration, "framework-scored");
    let spec = aiwatcher_execution::plan::ScoreEvaluationSpec {
        declaration: declaration.to_owned(),
    };
    command.step.runtime = aiwatcher_execution::RuntimeBinding::ExternalEvaluation(spec);
    context.timeout = std::time::Duration::from_secs(600);
    (command, context)
}
