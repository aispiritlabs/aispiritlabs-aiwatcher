//! A run whose answers a worker generates: the cases handed out without their
//! expectations, and the rows the worker wrote scored like a recording.
use super::*;
use aiwatcher_execution::{ActivityExecutor, FailureClass};
use aiwatcher_server::execution::artifacts::Artifacts;
use aiwatcher_server::execution::scoring::{CasesExecutor, ScoreExecutor, TracesExecutor};
use serde_json::json;

/// The prompt version the declared variant pins.
const PROMPT: &str = "5b0c1e9a8f7d6c5b4a3928171605f4e3d2c1b0a99887766554433221100ffeed";

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
    let mut variant = template.variant.clone();
    variant.prompt = Some(VersionReference {
        name: "support-bot".into(),
        version: PROMPT.into(),
    });
    let run = ScoringRun {
        evaluation_id: "generated-run".into(),
        repetition_id: template.origin.repetition_id.clone(),
        variant,
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

/// The row a task writes to say what it generated with: what the variant pins,
/// or `code` in its place.
async fn generated_with(
    artifacts: &Artifacts,
    declared: &DeclaredRun,
    code: Option<&str>,
) -> aiwatcher_core::ArtifactRef {
    let variant = &declared.run.variant;
    artifacts
        .put_rows(
            "generated_with",
            &vec![BTreeMap::from([
                (
                    "code".to_owned(),
                    json!(code.unwrap_or(&variant.code.digest)),
                ),
                (
                    "generation_config".to_owned(),
                    json!(variant.generation_config.digest),
                ),
            ])],
        )
        .await
        .unwrap()
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
        vec![answers, generated_with(&artifacts, &declared, None).await],
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
        vec![answers, generated_with(&artifacts, &declared, None).await],
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

#[tokio::test]
async fn answers_a_task_generated_with_other_code_than_the_variant_pins_are_never_scored() {
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
                ("case_id".to_owned(), json!("case-00000")),
                ("answer".to_owned(), json!({"text": ""})),
            ])],
        )
        .await
        .unwrap();
    let spec = aiwatcher_execution::plan::ScoreEvaluationSpec {
        declaration: declared.id.clone(),
    };
    let score = |inputs| {
        step(
            &declared.id,
            "score",
            aiwatcher_execution::RuntimeBinding::ScoreEvaluation(spec.clone()),
            inputs,
        )
    };
    let executor = ScoreExecutor::new(Arc::clone(&registry)).reading_from(artifacts.clone());

    // A worker built from another commit: its answers are not the variant's.
    let older = "0".repeat(64);
    let (command, context) = score(vec![
        answers.clone(),
        generated_with(&artifacts, &declared, Some(&older)).await,
    ]);
    let refused = executor.execute(&command, &context).await.unwrap_err();
    assert_eq!(refused.class, FailureClass::UserCode);
    assert!(
        refused.message.contains(&older)
            && refused.message.contains(&declared.run.variant.code.digest),
        "names both: {}",
        refused.message
    );

    // A task that never said what it generated with.
    let (command, context) = score(vec![answers]);
    let silent = executor.execute(&command, &context).await.unwrap_err();
    assert_eq!(silent.class, FailureClass::UserCode);
    assert!(
        silent.message.contains("wrote no `generated_with`"),
        "{}",
        silent.message
    );
    assert!(
        registry
            .get("generated-run", "reader", now())
            .await
            .unwrap()
            .is_none(),
        "nothing was published"
    );
}

/// An application's run on this deployment's log: the variant and result it
/// names, and one model call on `prompt`.
async fn application_run(
    read_model: &aiwatcher_projector::ReadModel,
    run_id: &str,
    variant_id: &str,
    prompt: &str,
) {
    use aiwatcher_core::{EventEnvelope, EventType, Sdk, Source as Producer};
    let at = time::OffsetDateTime::now_utc();
    let mut assembler = aiwatcher_trace::SpanAssembler::default();
    let call = json!({"call_id": "c1", "model": "support-model", "prompt_name": "support-bot",
                      "prompt_version": prompt});
    for (position, (event_type, data)) in [
        (
            EventType::RunStarted,
            json!({"evaluation_id": "generated-run"}),
        ),
        (EventType::LlmStarted, call.clone()),
        (EventType::LlmCompleted, call),
        (EventType::RunCompleted, json!({})),
    ]
    .into_iter()
    .enumerate()
    {
        let mut envelope = EventEnvelope::new(
            event_type,
            run_id,
            at,
            Producer::new("support-bot", Sdk::Python),
        )
        .with_data(data);
        envelope.variant_id = Some(variant_id.to_owned());
        let recorded = envelope.record(position as u64 + 1, position as u64 + 1, at, None);
        read_model.apply(&recorded).await;
        read_model
            .record_spans(&assembler.ingest(&recorded).spans)
            .await;
    }
}

async fn answers_in(
    artifacts: &Artifacts,
    runs: &[(&str, Option<&str>)],
) -> aiwatcher_core::ArtifactRef {
    artifacts
        .put_rows(
            "answers",
            &runs
                .iter()
                .map(|(case_id, run_id)| {
                    let mut row = BTreeMap::from([
                        ("case_id".to_owned(), json!(case_id)),
                        ("answer".to_owned(), json!({"text": ""})),
                    ]);
                    if let Some(run_id) = run_id {
                        row.insert("run_id".to_owned(), json!(run_id));
                    }
                    row
                })
                .collect(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn the_traces_of_generated_answers_say_how_many_ran_on_the_pinned_prompt_and_the_result_carries_it()
 {
    let registry = Arc::new(registry(
        Arc::new(MemoryObjectStore::new()),
        Arc::new(Source::default()),
    ));
    let artifacts = Artifacts::new(Arc::new(MemoryObjectStore::new()));
    let declared = declared(&registry).await;
    let view = registry
        .scoring_run_view(&declared.id)
        .await
        .unwrap()
        .unwrap();
    let read_model = Arc::new(aiwatcher_projector::ReadModel::default());
    application_run(&read_model, "run-on-the-pin", &view.variant_id, PROMPT).await;
    let spec = aiwatcher_execution::plan::ScoreEvaluationSpec {
        declaration: declared.id.clone(),
    };

    // One answer made on the pinned prompt, one whose run never reached the
    // log, one that names no run at all.
    let answers = answers_in(
        &artifacts,
        &[
            ("case-00000", Some("run-on-the-pin")),
            ("case-00001", Some("run-never-sent")),
            ("case-00002", None),
        ],
    )
    .await;
    let (command, context) = step(
        &declared.id,
        "traces",
        aiwatcher_execution::RuntimeBinding::EvaluationTraces(spec.clone()),
        vec![answers.clone()],
    );
    let traced = TracesExecutor::new(
        Arc::clone(&registry),
        artifacts.clone(),
        Arc::clone(&read_model),
        std::time::Duration::ZERO,
    )
    .execute(&command, &context)
    .await
    .expect("nothing the traces show contradicts the pins");
    assert_eq!(
        traced.result.as_ref().unwrap()["traces"],
        json!({"answers": 3, "named": 2, "seen": 1, "on_prompt": 1})
    );

    let (command, context) = step(
        &declared.id,
        "score",
        aiwatcher_execution::RuntimeBinding::ScoreEvaluation(spec),
        vec![
            answers,
            generated_with(&artifacts, &declared, None).await,
            traced.outputs[0].clone(),
        ],
    );
    ScoreExecutor::new(Arc::clone(&registry))
        .reading_from(artifacts)
        .execute(&command, &context)
        .await
        .expect("the answers score");
    let evidence = registry
        .get("generated-run", "reader", now())
        .await
        .unwrap()
        .unwrap();
    let traces = evidence
        .traces
        .clone()
        .expect("a generated result says what its traces showed");
    assert_eq!(
        (traces.answers, traces.seen, traces.on_prompt),
        (3, 1, Some(1))
    );
    assert!(!traces.complete());
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
    let seen = page
        .cases
        .iter()
        .find(|case| case.measurement.case_id == "case-00000")
        .unwrap();
    assert_eq!(
        seen.measurement.trace_id,
        Some(aiwatcher_core::TraceId::derive("run-on-the-pin").to_hex()),
        "the case leads to the trace its run was seen in, which the answer never named"
    );
}

#[tokio::test]
async fn answers_whose_traces_show_another_prompt_version_are_never_scored() {
    let registry = Arc::new(registry(
        Arc::new(MemoryObjectStore::new()),
        Arc::new(Source::default()),
    ));
    let artifacts = Artifacts::new(Arc::new(MemoryObjectStore::new()));
    let declared = declared(&registry).await;
    let view = registry
        .scoring_run_view(&declared.id)
        .await
        .unwrap()
        .unwrap();
    let read_model = Arc::new(aiwatcher_projector::ReadModel::default());
    let other = "f".repeat(64);
    application_run(&read_model, "run-off-the-pin", &view.variant_id, &other).await;
    let answers = answers_in(&artifacts, &[("case-00000", Some("run-off-the-pin"))]).await;
    let (command, context) = step(
        &declared.id,
        "traces",
        aiwatcher_execution::RuntimeBinding::EvaluationTraces(
            aiwatcher_execution::plan::ScoreEvaluationSpec {
                declaration: declared.id.clone(),
            },
        ),
        vec![answers],
    );

    let refused = TracesExecutor::new(
        Arc::clone(&registry),
        artifacts,
        read_model,
        std::time::Duration::ZERO,
    )
    .execute(&command, &context)
    .await
    .unwrap_err();

    assert_eq!(refused.class, FailureClass::UserCode);
    assert!(
        refused.message.contains(&other) && refused.message.contains(PROMPT),
        "names both versions: {}",
        refused.message
    );
    assert!(
        registry
            .get("generated-run", "reader", now())
            .await
            .unwrap()
            .is_none(),
        "nothing was published"
    );
}
