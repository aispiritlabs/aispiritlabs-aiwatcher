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

/// The variant pins the prompt alone: what a trace shows of a workflow or a
/// model is the business of the tests that pin one.
async fn declared(registry: &Registry) -> DeclaredRun {
    declared_with(registry, |variant| variant.workflow = None).await
}

async fn declared_with(
    registry: &Registry,
    pins: impl FnOnce(&mut aiwatcher_evaluation::VariantManifest),
) -> DeclaredRun {
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
    pins(&mut variant);
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

/// Events of one run folded as the serve role folds them: into the read model
/// and, through the assembler, into its spans — each published under
/// `publisher`, as the ingest route records the credential it checked.
async fn folded_run(
    read_model: &aiwatcher_projector::ReadModel,
    run_id: &str,
    publisher: &str,
    workflow: Option<&str>,
    events: Vec<(aiwatcher_core::EventType, serde_json::Value)>,
) {
    use aiwatcher_core::{EventEnvelope, Sdk, Source as Producer};
    let at = time::OffsetDateTime::now_utc();
    let mut assembler = aiwatcher_trace::SpanAssembler::default();
    for (position, (event_type, data)) in events.into_iter().enumerate() {
        let mut envelope = EventEnvelope::new(
            event_type,
            run_id,
            at,
            Producer::new("support-bot", Sdk::Python),
        )
        .with_data(data);
        envelope.workflow_id = workflow.map(ToOwned::to_owned);
        envelope.published_by = Some(publisher.to_owned());
        let recorded = envelope.record(position as u64 + 1, position as u64 + 1, at, None);
        read_model.apply(&recorded).await;
        read_model
            .record_spans(&assembler.ingest(&recorded).spans)
            .await;
    }
}

/// The measurement's execution on the log, as its engine declares it, a
/// minute before the application's runs: where calls asked elsewhere during it
/// are looked for from.
async fn measurement_started(read_model: &aiwatcher_projector::ReadModel) {
    use aiwatcher_core::{EventEnvelope, EventType, Sdk, Source as Producer};
    let at = time::OffsetDateTime::now_utc() - time::Duration::minutes(1);
    let mut envelope = EventEnvelope::new(
        EventType::WorkflowDeclared,
        "exec-generated-run",
        at,
        Producer::new("aiwatcher", Sdk::Rust),
    )
    .with_data(json!({"nodes": ["cases", "generate", "traces", "score"]}));
    envelope.workflow_id = Some("scoring".to_owned());
    envelope.workflow_run_id = Some("exec-generated-run".to_owned());
    read_model.apply(&envelope.record(1, 1, at, None)).await;
}

/// A bundle that holds one workflow declaration and nothing else.
#[derive(Debug)]
struct Declaration(Vec<u8>);

#[async_trait::async_trait]
impl aiwatcher_evaluation::ApprovalBundles for Declaration {
    async fn stage(
        &self,
        _: &str,
        name: &str,
        bytes: Vec<u8>,
    ) -> aiwatcher_evaluation::Result<aiwatcher_evaluation::StagedFile> {
        Ok(aiwatcher_evaluation::StagedFile {
            name: name.to_owned(),
            size_bytes: bytes.len() as u64,
        })
    }
    async fn staged(
        &self,
        _: &str,
    ) -> aiwatcher_evaluation::Result<Vec<aiwatcher_evaluation::StagedFile>> {
        Ok(Vec::new())
    }
    async fn discard(&self, _: &str) -> aiwatcher_evaluation::Result<usize> {
        Ok(0)
    }
    async fn member(&self, _: &str, name: &str) -> aiwatcher_evaluation::Result<Option<Vec<u8>>> {
        Ok((name == "workflow.json").then(|| self.0.clone()))
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
        json!({"answers": 3, "named": 2, "seen": 1, "on_prompt": 1, "witnessed_prompt": 0,
               "witnessed_answer": 0, "witnessed_input": 0, "witnessed_exchange": 0,
               "asked_since_seconds": 0})
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
    assert_eq!(
        seen.measurement.usage.as_ref().map(|usage| usage
            .models
            .iter()
            .map(|model| (model.model.as_str(), model.calls))
            .collect::<Vec<_>>()),
        Some(vec![("support-model", 1)]),
        "and its usage says which model its run called, which is what prices it"
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

#[tokio::test]
async fn a_serving_host_witnesses_the_model_and_a_run_off_the_pinned_workflow_is_refused() {
    use aiwatcher_core::EventType;
    use sha2::Digest;
    let registry = Arc::new(registry(
        Arc::new(MemoryObjectStore::new()),
        Arc::new(Source::default()),
    ));
    let artifacts = Artifacts::new(Arc::new(MemoryObjectStore::new()));
    let declaration = br#"{"nodes": ["retrieve", "answer"], "edges": [["retrieve", "answer"]]}"#;
    let declared = declared_with(&registry, |variant| {
        variant.model = Some(VersionReference {
            name: "support-model".into(),
            version: "v7".into(),
        });
        variant.workflow = Some(VersionReference {
            name: "support-app".into(),
            version: hex::encode(sha2::Sha256::digest(declaration)),
        });
    })
    .await;
    let read_model = Arc::new(aiwatcher_projector::ReadModel::default());
    let call = |node: &str| {
        json!({"call_id": format!("c-{node}"), "model": "support-model", "model_version": "v7",
               "prompt_name": "support-bot", "prompt_version": PROMPT,
               "response_model": "support-model-q4"})
    };
    let application = |run_id: &'static str, nodes: &'static [&'static str]| {
        let mut events = vec![
            (
                EventType::RunStarted,
                json!({"evaluation_id": "generated-run"}),
            ),
            (
                EventType::WorkflowDeclared,
                serde_json::from_slice(declaration).unwrap(),
            ),
        ];
        for node in nodes {
            events.push((
                EventType::StepStarted,
                json!({"node": node, "call_id": format!("s-{node}")}),
            ));
            events.push((EventType::LlmStarted, call(node)));
            events.push((EventType::LlmCompleted, call(node)));
            events.push((
                EventType::StepCompleted,
                json!({"node": node, "call_id": format!("s-{node}")}),
            ));
        }
        events.push((EventType::RunCompleted, json!({})));
        (run_id, events)
    };
    for (run_id, events) in [
        application("run-served", &["retrieve", "answer"]),
        application("run-self-served", &["retrieve", "answer"]),
    ] {
        folded_run(&read_model, run_id, "worker", Some("support-app"), events).await;
    }
    // The serving host's run, under its own credential, naming the call it
    // served — with its keyed digests of the answer it sent back and the
    // question it was asked — and one the worker published for itself, which
    // is its own word.
    use aiwatcher_core::witness::{Said, digest, key_for};
    let key = key_for("serving-secret");
    let served_call = json!({"call_id": "serve-1", "model": "support-model", "model_version": "v7",
        "prompt_name": "support-bot", "prompt_version": PROMPT,
        "prompt_verified": true, "prompt_exact": true,
        "replied_digests": [digest(&key, Said::Replied, r#"{"text":""}"#)],
        "asked_digests": [digest(&key, Said::Asked, "question 0")],
        "rendered_digests": [digest(&key, Said::Replied, "question 0")]});
    folded_run(
        &read_model,
        "serve-1",
        "serving",
        None,
        vec![
            (
                EventType::RunStarted,
                json!({"caller_run_id": "run-served"}),
            ),
            (EventType::LlmStarted, served_call.clone()),
            (EventType::LlmCompleted, served_call),
            (EventType::RunCompleted, json!({})),
        ],
    )
    .await;
    let self_call = json!({"call_id": "serve-2", "model": "support-model", "model_version": "v7"});
    folded_run(
        &read_model,
        "serve-2",
        "worker",
        None,
        vec![
            (
                EventType::RunStarted,
                json!({"caller_run_id": "run-self-served"}),
            ),
            (EventType::LlmStarted, self_call.clone()),
            (EventType::LlmCompleted, self_call),
            (EventType::RunCompleted, json!({})),
        ],
    )
    .await;

    measurement_started(&read_model).await;

    let spec = aiwatcher_execution::plan::ScoreEvaluationSpec {
        declaration: declared.id.clone(),
    };
    let answers = answers_in(
        &artifacts,
        &[
            ("case-00000", Some("run-served")),
            ("case-00001", Some("run-self-served")),
        ],
    )
    .await;
    let (command, context) = step(
        &declared.id,
        "traces",
        aiwatcher_execution::RuntimeBinding::EvaluationTraces(spec.clone()),
        vec![answers],
    );
    let traced = TracesExecutor::new(
        Arc::clone(&registry),
        artifacts.clone(),
        Arc::clone(&read_model),
        std::time::Duration::ZERO,
    )
    .reading_bundles_from(Arc::new(Declaration(declaration.to_vec())))
    .witnessed_by(aiwatcher_evaluation::Witnesses::default().keyed([("serving".to_owned(), key)]))
    .execute(&command, &context)
    .await
    .expect("nothing contradicts the pins");
    assert_eq!(
        traced.result.as_ref().unwrap()["traces"],
        json!({"answers": 2, "named": 2, "seen": 2, "on_prompt": 2, "on_model": 2,
               "on_workflow": 2, "witnessed_model": 1, "witnessed_prompt": 1,
               "witnessed_answer": 1, "witnessed_input": 1, "witnessed_exchange": 1,
               "asked_since_seconds": 0, "self_witnessed": 1, "witnesses": ["serving"],
               "served": [{"model": "support-model-q4", "answers": 2}]}),
        "the serving run the worker's own credential published is no witness"
    );

    // The same question on the pinned prompt, relayed by the witness for no run
    // of this measurement: a reply the application could have seen first.
    let peeked = json!({"call_id": "peek-1", "model": "support-model", "model_version": "v7",
        "prompt_name": "support-bot", "prompt_version": PROMPT,
        "prompt_verified": true, "prompt_exact": true,
        "asked_digests": [digest(&key, Said::Asked, "question 0")]});
    folded_run(
        &read_model,
        "peek-1",
        "serving",
        None,
        vec![
            (EventType::RunStarted, json!({})),
            (EventType::LlmStarted, peeked.clone()),
            (EventType::LlmCompleted, peeked),
            (EventType::RunCompleted, json!({})),
        ],
    )
    .await;
    let again = TracesExecutor::new(
        Arc::clone(&registry),
        artifacts.clone(),
        Arc::clone(&read_model),
        std::time::Duration::ZERO,
    )
    .reading_bundles_from(Arc::new(Declaration(declaration.to_vec())))
    .witnessed_by(aiwatcher_evaluation::Witnesses::default().keyed([("serving".to_owned(), key)]))
    .execute(&command, &context)
    .await
    .expect("nothing contradicts the pins");
    let traces = &again.result.as_ref().unwrap()["traces"];
    assert_eq!(
        (&traces["witnessed_exchange"], &traces["asked_elsewhere"]),
        (&json!(0), &json!(1)),
        "a case asked elsewhere while the measurement ran is no exchange: {traces}"
    );

    // A run that stepped through a node the pinned declaration does not have.
    let (run_id, mut events) = application("run-off-the-graph", &["answer"]);
    events.insert(
        2,
        (
            EventType::StepStarted,
            json!({"node": "improvise", "call_id": "s-improvise"}),
        ),
    );
    folded_run(&read_model, run_id, "worker", Some("support-app"), events).await;
    let answers = answers_in(&artifacts, &[("case-00000", Some("run-off-the-graph"))]).await;
    let (command, context) = step(
        &declared.id,
        "traces",
        aiwatcher_execution::RuntimeBinding::EvaluationTraces(spec),
        vec![answers],
    );
    let refused = TracesExecutor::new(
        Arc::clone(&registry),
        artifacts,
        read_model,
        std::time::Duration::ZERO,
    )
    .reading_bundles_from(Arc::new(Declaration(declaration.to_vec())))
    .execute(&command, &context)
    .await
    .unwrap_err();
    assert_eq!(refused.class, FailureClass::UserCode);
    assert!(refused.message.contains("improvise"), "{}", refused.message);
}
