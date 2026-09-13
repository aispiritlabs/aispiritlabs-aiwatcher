//! A run whose answers a worker generates: the cases handed out without their
//! expectations, and the rows the worker wrote scored like a recording.
use super::*;
use aiwatcher_execution::{ActivityExecutor, FailureClass};
use aiwatcher_server::execution::artifacts::Artifacts;
use aiwatcher_server::execution::scoring::{CasesExecutor, ScoreExecutor};
use serde_json::json;

fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

fn card() -> Scorecard {
    serde_json::from_value(json!({
        "name": "generated-quality",
        "scorers": [
            {"metric": "exact", "answer_path": "/text", "expected_path": "/answer",
             "scorer": {"kind": "exact_match", "trim": true}}
        ]
    }))
    .unwrap()
}

async fn declared(registry: &Registry) -> DeclaredRun {
    let version = registry
        .publish_scorecard(&card(), "ada", now())
        .await
        .unwrap()
        .version;
    let template = request("generated-run", 3).manifest;
    let run = ScoringRun {
        evaluation_id: "generated-run".into(),
        repetition_id: template.origin.repetition_id.clone(),
        variant: template.variant.clone(),
        cohort: Cohort {
            case_manifest: template.context.case_manifest.clone(),
            case_count: 3,
            split: template.context.split.clone(),
            input_schema: template.context.input_schema.clone(),
            expectations_schema: template.context.expectations_schema.clone(),
        },
        scorecard: VersionReference {
            name: "generated-quality".into(),
            version,
        },
        answers: Answers::Generated(Generated {
            generated_by: Generation {
                task: "support-bot.answer@3".into(),
                queue: "evaluation".into(),
                params: serde_json::Map::new(),
            },
        }),
        judge: None,
        external_calibration: None,
        settings: Default::default(),
    };
    let declared = registry
        .declare_scoring_run(&run, "ada", now())
        .await
        .unwrap();
    let view = registry
        .scoring_run_view(&declared.id)
        .await
        .unwrap()
        .unwrap();
    registry
        .approve(&view.manifest, "operator", now())
        .await
        .unwrap();
    declared
}

fn step(
    declaration: &str,
    id: &str,
    runtime: aiwatcher_execution::RuntimeBinding,
    inputs: Vec<aiwatcher_core::ArtifactRef>,
) -> (
    aiwatcher_execution::ActivityCommand,
    aiwatcher_execution::ActivityContext,
) {
    let (mut command, mut context) = attempt(declaration, "generated-run");
    command.step.id = id.to_owned();
    command.step.runtime = runtime;
    command.key = aiwatcher_execution::AttemptKey::new(
        aiwatcher_execution::ExecutionId::new("exec-generated-run"),
        id,
        1,
    );
    command.inputs = inputs;
    context.context_id = format!("exec-generated-run/{id}/1");
    (command, context)
}

#[tokio::test]
async fn a_worker_is_handed_each_cases_input_and_its_answers_are_scored_like_a_recording() {
    let registry = Arc::new(registry(
        Arc::new(MemoryObjectStore::new()),
        Arc::new(Source::default()),
    ));
    let artifacts = Artifacts::new(Arc::new(MemoryObjectStore::new()));
    let declared = declared(&registry).await;
    let spec = || aiwatcher_execution::plan::ScoreEvaluationSpec {
        declaration: declared.id.clone(),
    };

    // The cases, as the worker will read them: what each asked, and never
    // what it was expected to answer.
    let (command, context) = step(
        &declared.id,
        "cases",
        aiwatcher_execution::RuntimeBinding::EvaluationCases(spec()),
        Vec::new(),
    );
    let handed = CasesExecutor::new(Arc::clone(&registry), artifacts.clone())
        .execute(&command, &context)
        .await
        .expect("the cases are read under the admitted pair");
    let rows = artifacts.read_rows(&handed.outputs[0]).await.unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["case_id"], "case-00000");
    assert_eq!(rows[0]["input"], json!({"question": "question 0"}));
    assert!(
        rows.iter()
            .all(|row| row.keys().all(|key| key == "case_id" || key == "input")),
        "nothing a case expected reaches the generator: {rows:?}"
    );

    // What the worker wrote back — one answer per case, one of them wrong.
    let answers = artifacts
        .put_rows(
            "answers",
            &vec![
                BTreeMap::from([
                    ("case_id".to_owned(), json!("case-00000")),
                    ("answer".to_owned(), json!({"text": ""})),
                    (
                        "trace_id".to_owned(),
                        json!("0af7651916cd43dd8448eb211c80319c"),
                    ),
                ]),
                BTreeMap::from([
                    ("case_id".to_owned(), json!("case-00001")),
                    ("answer".to_owned(), json!({"text": "  "})),
                ]),
                BTreeMap::from([
                    ("case_id".to_owned(), json!("case-00002")),
                    ("answer".to_owned(), json!({"text": "not it"})),
                ]),
            ],
        )
        .await
        .unwrap();
    let (command, context) = step(
        &declared.id,
        "score",
        aiwatcher_execution::RuntimeBinding::ScoreEvaluation(spec()),
        vec![answers],
    );
    let reported = ScoreExecutor::new(Arc::clone(&registry))
        .reading_from(artifacts.clone())
        .execute(&command, &context)
        .await
        .expect("the generated answers score")
        .result
        .unwrap();
    assert_eq!(reported["scored"], 3, "{reported}");

    let evidence = registry
        .get("generated-run", "reader", now())
        .await
        .unwrap()
        .unwrap();
    assert!((evidence.metrics["exact"] - 2.0 / 3.0).abs() < 1e-9);
    let page = registry
        .cases(
            "generated-run",
            &evidence.receipt.version,
            None,
            None,
            "reader",
            now(),
        )
        .await
        .unwrap()
        .unwrap();
    let traced = page
        .cases
        .iter()
        .find(|case| case.measurement.case_id == "case-00000")
        .unwrap();
    assert_eq!(
        traced.measurement.trace_id.as_deref(),
        Some("0af7651916cd43dd8448eb211c80319c"),
        "from the result to the trace the worker's answer was made in"
    );
}

#[tokio::test]
async fn a_row_the_worker_wrote_that_is_not_an_answer_fails_the_step_naming_it() {
    let registry = Arc::new(registry(
        Arc::new(MemoryObjectStore::new()),
        Arc::new(Source::default()),
    ));
    let artifacts = Artifacts::new(Arc::new(MemoryObjectStore::new()));
    let declared = declared(&registry).await;
    let answers = artifacts
        .put_rows(
            "answers",
            &vec![BTreeMap::from([
                ("case".to_owned(), json!("case-00000")),
                ("text".to_owned(), json!("Warsaw")),
            ])],
        )
        .await
        .unwrap();
    let (command, context) = step(
        &declared.id,
        "score",
        aiwatcher_execution::RuntimeBinding::ScoreEvaluation(
            aiwatcher_execution::plan::ScoreEvaluationSpec {
                declaration: declared.id.clone(),
            },
        ),
        vec![answers],
    );
    let refused = ScoreExecutor::new(Arc::clone(&registry))
        .reading_from(artifacts)
        .execute(&command, &context)
        .await
        .unwrap_err();
    assert_eq!(refused.class, FailureClass::UserCode);
    assert!(refused.message.contains("row 0"), "{}", refused.message);
    assert!(
        registry
            .get("generated-run", "reader", now())
            .await
            .unwrap()
            .is_none(),
        "nothing was published"
    );
}
