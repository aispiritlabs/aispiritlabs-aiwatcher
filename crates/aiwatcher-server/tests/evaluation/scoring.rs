//! Measuring answers somebody already has, against a card somebody declared.
use super::*;
use serde_json::{Value, json};

/// The cohort this run selects, and the answer each case was expected to give.
#[derive(Debug)]
struct Expectations(BTreeMap<String, Value>);

#[async_trait]
impl SourceAuthority for Expectations {
    async fn resolve(&self, _: &EvaluationManifest, _: &str) -> Result<SourceEvidence> {
        Ok(SourceEvidence {
            expected: self.0.clone(),
            ..Default::default()
        })
    }
}

fn cohort() -> BTreeMap<String, Value> {
    BTreeMap::from([
        ("capital-pl".to_owned(), json!("Warsaw")),
        ("empty-input".to_owned(), json!("")),
        ("two-plus-two".to_owned(), json!("four")),
    ])
}

fn store(expected: BTreeMap<String, Value>) -> Registry {
    Registry::new(
        Arc::new(MemoryObjectStore::new()),
        Arc::new(Expectations(expected)),
        RegistryConfig::default(),
    )
    .unwrap()
}

fn card() -> Scorecard {
    Scorecard {
        name: "answer-quality".into(),
        description: String::new(),
        scorers: vec![
            ScorerSpec {
                metric: "exact".into(),
                answer_path: "/text".into(),
                expected_path: String::new(),
                scorer: Scorer::ExactMatch {
                    ignore_case: false,
                    trim: true,
                },
            },
            ScorerSpec {
                metric: "leaked".into(),
                answer_path: "/text".into(),
                expected_path: String::new(),
                scorer: Scorer::Forbidden {
                    text: "ssn".into(),
                    ignore_case: true,
                },
            },
        ],
    }
}

fn said(case_id: &str, text: &str) -> RecordedAnswer {
    RecordedAnswer {
        case_id: case_id.into(),
        answer: json!({ "text": text }),
        trace_id: None,
        span_id: None,
    }
}

/// A declaration over the pinned pieces the manifest fixture already carries.
fn declaration(evaluation_id: &str, version: &str) -> ScoringRun {
    let manifest = request(evaluation_id, 3).manifest;
    ScoringRun {
        evaluation_id: evaluation_id.into(),
        repetition_id: manifest.origin.repetition_id.clone(),
        variant: manifest.variant.clone(),
        cohort: Cohort {
            case_manifest: manifest.context.case_manifest.clone(),
            case_count: 3,
            split: manifest.context.split.clone(),
            input_schema: manifest.context.input_schema.clone(),
            expectations_schema: manifest.context.expectations_schema.clone(),
        },
        scorecard: VersionReference {
            name: "answer-quality".into(),
            version: version.into(),
        },
        answers: manifest.context.case_manifest.clone(),
    }
}

async fn measured(
    registry: &Registry,
    evaluation_id: &str,
    answers: &[RecordedAnswer],
) -> Result<EvaluationReceipt> {
    let card = card();
    let version = registry
        .publish_scorecard(&card, "ada", 100)
        .await
        .unwrap()
        .version;
    let run = declaration(evaluation_id, &version);
    let declared = registry
        .declare_scoring_run(&run, "ada", 100)
        .await
        .unwrap();
    let scored = score(&card, &cohort(), answers, &declared.run.repetition_id);
    publish(
        registry,
        PublishEvaluation {
            manifest: declared.run.manifest(&card, None).unwrap(),
            status: scored.status,
            cases: scored.cases,
        },
        "editor",
        200,
    )
    .await
}

#[tokio::test]
async fn what_a_recording_was_measured_under_is_two_references_rather_than_one() {
    let registry = store(cohort());
    let receipt = measured(
        &registry,
        "scored-1",
        &[
            said("two-plus-two", "four"),
            said("capital-pl", "Kraków"),
            said("empty-input", ""),
        ],
    )
    .await
    .unwrap();

    let evidence = registry
        .get("scored-1", "reader", 300)
        .await
        .unwrap()
        .unwrap();
    let context = evidence.manifest.unwrap().context;
    assert_eq!(context.suite.name, "answer-quality", "what was declared");
    assert_eq!(
        context.scorer,
        scoring_engine(),
        "and the code that read it, which a rewritten scorer moves"
    );
    assert_eq!(context.metrics[0].direction, MetricDirection::Higher);
    assert_eq!(context.metrics[1].direction, MetricDirection::Lower);

    assert_eq!(evidence.status, Some(ResultStatus::Succeeded));
    let exact = evidence.metrics["exact"];
    assert!(
        (exact - 2.0 / 3.0).abs() < 1e-9,
        "two of three answers were the expected one: {exact}"
    );
    assert_eq!(evidence.metrics["leaked"], 0.0);
    assert_eq!(receipt.evaluation_id, "scored-1");
}

#[tokio::test]
async fn a_case_nobody_answered_is_unscored_rather_than_scored_zero() {
    let registry = store(cohort());
    measured(
        &registry,
        "scored-2",
        &[said("two-plus-two", "four"), said("capital-pl", "Warsaw")],
    )
    .await
    .unwrap();

    let evidence = registry
        .get("scored-2", "reader", 300)
        .await
        .unwrap()
        .unwrap();
    let counts = evidence.counts.unwrap();
    assert_eq!((counts.selected, counts.scored, counts.unscored), (3, 2, 1));
    assert_eq!(evidence.status, Some(ResultStatus::Partial));
    assert_eq!(
        evidence.metrics["exact"], 1.0,
        "the average is over what was measured, and the gap is a count beside it"
    );
}

#[tokio::test]
async fn a_case_one_scorer_could_not_read_carries_none_of_the_numbers() {
    let registry = store(cohort());
    let mut unreadable = said("two-plus-two", "four");
    unreadable.answer = json!({"answer": "four"});
    let receipt = measured(
        &registry,
        "scored-3",
        &[
            unreadable,
            said("capital-pl", "Warsaw"),
            said("empty-input", ""),
        ],
    )
    .await
    .unwrap();

    let page = registry
        .cases("scored-3", &receipt.version, None, Some(200), "reader", 300)
        .await
        .unwrap()
        .unwrap();
    let refused = page
        .cases
        .iter()
        .find(|case| case.measurement.case_id == "two-plus-two")
        .expect("the case it could not read is in the evidence");
    assert!(refused.measurement.metrics.is_empty());
    assert!(
        refused
            .measurement
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("/text"),
        "the reason names what the scorer went looking for: {:?}",
        refused.measurement.error
    );

    let evidence = registry
        .get("scored-3", "reader", 300)
        .await
        .unwrap()
        .unwrap();
    let counts = evidence.counts.unwrap();
    assert_eq!((counts.scored, counts.failed), (2, 1));
    assert_eq!(evidence.status, Some(ResultStatus::Partial));
    assert_eq!(
        evidence.metrics["exact"], 1.0,
        "a case that answered neither metric is counted in neither average"
    );
}

#[tokio::test]
async fn two_answers_to_one_case_are_two_measurements_rather_than_one() {
    let registry = store(cohort());
    measured(
        &registry,
        "scored-4",
        &[
            said("two-plus-two", "four"),
            said("two-plus-two", "4"),
            said("capital-pl", "Warsaw"),
            said("empty-input", ""),
        ],
    )
    .await
    .unwrap();

    let evidence = registry
        .get("scored-4", "reader", 300)
        .await
        .unwrap()
        .unwrap();
    let counts = evidence.counts.unwrap();
    assert_eq!(
        (counts.scored, counts.failed),
        (2, 1),
        "one publication is one repetition, so the twice-answered case is not scored"
    );
}

#[tokio::test]
async fn an_answer_the_cohort_does_not_select_is_not_part_of_this_measurement() {
    let registry = store(cohort());
    measured(
        &registry,
        "scored-5",
        &[
            said("two-plus-two", "four"),
            said("capital-pl", "Warsaw"),
            said("empty-input", ""),
            said("something-else-entirely", "four"),
        ],
    )
    .await
    .unwrap();

    let evidence = registry
        .get("scored-5", "reader", 300)
        .await
        .unwrap()
        .unwrap();
    let counts = evidence.counts.unwrap();
    assert_eq!((counts.selected, counts.scored), (3, 3));
    assert_eq!(evidence.status, Some(ResultStatus::Succeeded));
}

#[tokio::test]
async fn declaring_one_intention_twice_is_one_document_and_a_second_run_is_not() {
    let registry = store(cohort());
    let run = declaration("scored-6", "b".repeat(64).as_str());
    let first = registry
        .declare_scoring_run(&run, "ada", 100)
        .await
        .unwrap();
    let again = registry
        .declare_scoring_run(&run, "grace", 900)
        .await
        .unwrap();
    assert_eq!(first.id, again.id);
    assert_eq!(
        again.declared_by, "ada",
        "the declaration that is there is the one a started run reads"
    );

    let mut repeated = run.clone();
    repeated.repetition_id = "measurement-2".into();
    let other = registry
        .declare_scoring_run(&repeated, "ada", 100)
        .await
        .unwrap();
    assert_ne!(
        other.id, first.id,
        "an independent repetition is another measurement, not the same one"
    );

    assert_eq!(
        registry.scoring_run(&first.id).await.unwrap().unwrap().run,
        run
    );
}

/// A result this deployment measured, ready to publish, and the registry.
async fn engine_scored(registry: &Registry, evaluation_id: &str) -> PublishEvaluation {
    let card = card();
    let version = registry
        .publish_scorecard(&card, "ada", 100)
        .await
        .unwrap()
        .version;
    let run = declaration(evaluation_id, &version);
    let scored = score(
        &card,
        &cohort(),
        &[
            said("two-plus-two", "four"),
            said("capital-pl", "Warsaw"),
            said("empty-input", ""),
        ],
        &run.repetition_id,
    );
    PublishEvaluation {
        manifest: run.manifest(&card, None).unwrap(),
        status: scored.status,
        cases: scored.cases,
    }
}

#[tokio::test]
async fn a_direction_edited_after_the_card_declared_it_is_not_admitted_as_the_cards() {
    let registry = store(cohort());
    let mut request = engine_scored(&registry, "tampered").await;
    request.manifest.context.metrics[1].direction = MetricDirection::Higher;

    let refused = registry
        .approve(&request.manifest, "operator", 100)
        .await
        .expect_err("leaking a phrase more often is not an improvement");
    assert!(refused.to_string().contains("context.metrics"), "{refused}");
    let refused = registry
        .publish(request, "editor", 200)
        .await
        .expect_err("and publication asks the same question");
    assert!(refused.to_string().contains("context.metrics"), "{refused}");
}

#[tokio::test]
async fn a_scorer_version_this_binary_does_not_implement_is_refused_naming_the_one_it_does() {
    let registry = store(cohort());
    let mut request = engine_scored(&registry, "from-the-future").await;
    request.manifest.context.scorer.version = "2".into();

    let refused = registry
        .approve(&request.manifest, "operator", 100)
        .await
        .expect_err("nothing here measured with version 2");
    let said = refused.to_string();
    assert!(said.contains("context.scorer.version"), "{said}");
    assert!(said.contains(SCORING_VERSION), "{said}");
}

#[tokio::test]
async fn a_scoring_run_is_admitted_with_no_suite_or_scorer_file_in_its_bundle() {
    use aiwatcher_evaluation::ApprovalBundles;
    use aiwatcher_server::evaluation::LocalSource;

    let store: Arc<dyn ObjectStore> = Arc::new(MemoryObjectStore::new());
    let adapter = Arc::new(LocalSource::new(None).with_bundles(store.clone()));
    let registry = Registry::new(store, adapter.clone(), RegistryConfig::default()).unwrap();

    let card = Scorecard {
        name: "answer-quality".into(),
        description: String::new(),
        scorers: vec![ScorerSpec {
            metric: "exact".into(),
            answer_path: "/text".into(),
            expected_path: "/answer".into(),
            scorer: Scorer::ExactMatch {
                ignore_case: true,
                trim: true,
            },
        }],
    };
    let version = registry
        .publish_scorecard(&card, "ada", 100)
        .await
        .unwrap()
        .version;
    let run = declaration("scored-against-the-fixture", &version);
    let manifest = run.manifest(&card, None).unwrap();
    let prepared = Evaluation::prepare(manifest.clone()).unwrap();
    let approval =
        aiwatcher_evaluation::approval_id(prepared.variant_id(), prepared.context_id()).unwrap();

    // Every pinned file an operator holds — and no suite.json or scorer.py,
    // because this suite is a card in the registry and this scorer is the
    // binary.
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../contracts/fixtures/evaluation-v1");
    for name in [
        "cases.json",
        "expectations-schema.json",
        "generation.json",
        "input-schema.json",
        "responses.py",
        "workflow.json",
    ] {
        adapter
            .stage(&approval, name, std::fs::read(fixture.join(name)).unwrap())
            .await
            .unwrap();
    }
    adapter
        .stage(
            &approval,
            "manifest.json",
            serde_json::to_vec(&manifest).unwrap(),
        )
        .await
        .unwrap();
    registry.approve(&manifest, "operator", 100).await.unwrap();

    // The expectations are the fixture's own: `{"answer": "4"}` and friends.
    let expected = registry.cohort(&manifest, "ada").await.unwrap();
    let scored = score(
        &card,
        &expected,
        &[
            said("capital-pl", "warsaw"),
            said("two-plus-two", "four"),
            said("empty", ""),
        ],
        &run.repetition_id,
    );
    registry
        .publish(
            PublishEvaluation {
                manifest,
                status: scored.status,
                cases: scored.cases,
            },
            "ada",
            200,
        )
        .await
        .unwrap();

    let evidence = registry
        .get("scored-against-the-fixture", "reader", 300)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(evidence.state, EvidenceState::Complete);
    let exact = evidence.metrics["exact"];
    assert!(
        (exact - 2.0 / 3.0).abs() < 1e-9,
        "the fixture expects the digit, not the word: {exact}"
    );
}
