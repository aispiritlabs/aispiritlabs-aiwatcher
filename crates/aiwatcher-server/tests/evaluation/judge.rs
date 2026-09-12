//! A judge is admitted under its own rule: pinned settings, a calibration set
//! of people's judgements, its agreement beside the result, and a mark that a
//! model said so at the time.
use super::*;
use aiwatcher_execution::{ActivityExecutor, FailureClass};
use aiwatcher_server::execution::scoring::ScoreExecutor;
use serde_json::{Value, json};
use std::sync::Mutex;

/// Publications and judgements happen now, because the executor reads them now
/// and a result past its retention is retired by the read that finds it.
fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

/// A judge that calls an answer helpful when it says "helpful", declines to
/// answer when it says "mumble", and remembers every question it was put.
#[derive(Debug, Default)]
struct Scripted {
    asked: Mutex<Vec<String>>,
}

#[async_trait]
impl JudgeModel for Scripted {
    fn provider(&self) -> &str {
        "llamacpp"
    }

    async fn ask(&self, call: &JudgeCall) -> std::result::Result<JudgeReply, JudgeFailure> {
        let question = call.messages[1].content.clone();
        self.asked.lock().unwrap().push(question.clone());
        let content = if question.contains("mumble") {
            "I would rather not say".to_owned()
        } else {
            json!({"value": question.contains("helpful")}).to_string()
        };
        Ok(JudgeReply { content })
    }
}

fn helpful() -> Rubric {
    Rubric {
        name: "helpful".into(),
        question: "Does the answer help the person who asked?".into(),
        guidance: String::new(),
        scale: Scale::Flag,
        direction: MetricDirection::Higher,
    }
}

fn card(rubric: &str) -> Scorecard {
    Scorecard {
        name: "judged-quality".into(),
        description: String::new(),
        scorers: vec![ScorerSpec {
            metric: "helpful".into(),
            answer_path: "/text".into(),
            expected_path: String::new(),
            scorer: Scorer::Judge {
                rubric: VersionReference {
                    name: "helpful".into(),
                    version: rubric.into(),
                },
            },
        }],
    }
}

fn said(case_id: &str, text: &str) -> Value {
    json!({"case_id": case_id, "answer": {"text": text}})
}

/// A result people already judged: two answers, one helpful and one not, and
/// the people's verdict on each.
async fn calibrated(registry: &Registry, rubric: &str) -> CalibrationVersion {
    let mut people = request("people-judged", 2);
    people.manifest.variant.experiment_id = "earlier".into();
    people.cases[0].actual = Some(json!({"text": "a helpful answer"}));
    people.cases[1].actual = Some(json!({"text": "a helpful-sounding dodge"}));
    publish(registry, people.clone(), "editor", now())
        .await
        .unwrap();
    for (case, value) in [("case-00000", true), ("case-00001", false)] {
        registry
            .assess(
                &AssessmentRequest {
                    target: AssessmentTarget::Case {
                        evaluation_id: "people-judged".into(),
                        case_id: case.into(),
                        repetition_id: people.manifest.origin.repetition_id.clone(),
                    },
                    rubric: "helpful".into(),
                    rubric_version: Some(rubric.into()),
                    value: AssessmentValue::Flag { value },
                    source: AssessmentSource::Human,
                    author: None,
                    rationale: String::new(),
                },
                "grace",
                110,
            )
            .await
            .unwrap();
    }
    registry
        .take_calibration(
            &CalibrationRequest {
                name: "people".into(),
                evaluation_id: "people-judged".into(),
                rubrics: vec![VersionReference {
                    name: "helpful".into(),
                    version: rubric.into(),
                }],
            },
            "ada",
            120,
        )
        .await
        .unwrap()
}

async fn declared(
    registry: &Registry,
    calibration: &CalibrationVersion,
    rubric: &str,
) -> DeclaredRun {
    let template = request("judged-run", 3).manifest;
    let recording = registry
        .stage_recording(
            "answers.json",
            serde_json::to_vec(&json!({"answers": [
                said("case-00000", "a helpful reply"),
                said("case-00001", "no"),
                said("case-00002", "mumble"),
            ]}))
            .unwrap(),
        )
        .await
        .unwrap();
    let version = registry
        .publish_scorecard(&card(rubric), "ada", now())
        .await
        .unwrap()
        .version;
    let run = ScoringRun {
        evaluation_id: "judged-run".into(),
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
            name: "judged-quality".into(),
            version,
        },
        answers: Answers::Recording(recording),
        judge: Some(JudgeDeclaration {
            provider: "llamacpp".into(),
            model: VersionReference {
                name: "gemma-4-e2b".into(),
                version: "ud-q4-k-xl".into(),
            },
            settings: JudgeSettings::default(),
            calibration: VersionReference {
                name: "people".into(),
                version: calibration.version.clone(),
            },
        }),
    };
    registry
        .declare_scoring_run(&run, "ada", now())
        .await
        .unwrap()
}

#[tokio::test]
async fn a_judged_run_publishes_what_the_model_said_beside_how_far_it_agreed_with_people() {
    let registry = Arc::new(registry(
        Arc::new(MemoryObjectStore::new()),
        Arc::new(Source::default()),
    ));
    let rubric = registry
        .publish_rubric(&helpful(), "ada", now())
        .await
        .unwrap()
        .version;
    let calibration = calibrated(&registry, &rubric).await;
    assert_eq!(calibration.calibration.items.len(), 2);
    assert_eq!(calibration.calibration.result.name, "people-judged");

    let declared = declared(&registry, &calibration, &rubric).await;
    let view = registry
        .scoring_run_view(&declared.id)
        .await
        .unwrap()
        .unwrap();
    let judge = view
        .manifest
        .context
        .judge
        .clone()
        .expect("a judged context");
    assert_eq!(judge.calibration_dataset.kind, DatasetKind::Assessments);
    assert_eq!(judge.calibration_dataset.version, calibration.version);
    assert_eq!(
        view.manifest.context.metrics[0].direction,
        MetricDirection::Higher,
        "which way is better is the rubric's declaration, not the card author's"
    );
    registry
        .approve(&view.manifest, "operator", now())
        .await
        .expect("admitted under the judge's rule, with no bundle file for a model");

    let model = Arc::new(Scripted::default());
    let executor = ScoreExecutor::new(Arc::clone(&registry)).judged_by(model.clone(), 2);
    let (command, attempt) = attempt(&declared.id, "judged-run");
    let reported = executor
        .execute(&command, &attempt)
        .await
        .expect("the run scores")
        .result
        .expect("a report");
    assert_eq!(
        reported["judge_questions"], 5,
        "three cases and two calibration items"
    );
    assert!(
        model
            .asked
            .lock()
            .unwrap()
            .iter()
            .any(|asked| asked.contains("a helpful-sounding dodge")),
        "the judge is put the answer the people judged"
    );

    let evidence = registry
        .get("judged-run", "reader", now())
        .await
        .unwrap()
        .unwrap();
    assert!(!evidence.reproducible, "a model said these at the time");
    assert_eq!(evidence.metrics["helpful"], 0.5, "one of the two it read");
    let counts = evidence.counts.clone().unwrap();
    assert_eq!((counts.scored, counts.failed), (2, 1));
    let report = evidence
        .judge
        .clone()
        .expect("the agreement rides beside the numbers");
    let helpful = &report.agreement[0];
    assert_eq!((helpful.items, helpful.answered), (2, 2));
    assert_eq!(
        helpful.agreement, 0.5,
        "it called the dodge helpful and the person did not"
    );
    let page = registry
        .cases(
            "judged-run",
            &evidence.receipt.version,
            None,
            None,
            "reader",
            now(),
        )
        .await
        .unwrap()
        .unwrap();
    let declined = page
        .cases
        .iter()
        .find(|case| case.measurement.case_id == "case-00002")
        .unwrap();
    let reason = declined.measurement.error.clone().unwrap();
    assert!(reason.contains("helpful"), "{reason}");
    assert!(
        !reason.contains("rather not"),
        "a reason never quotes what the judge said: {reason}"
    );
}

#[tokio::test]
async fn a_judge_calibrated_against_nobody_or_against_its_own_run_is_refused() {
    let registry = registry(
        Arc::new(MemoryObjectStore::new()),
        Arc::new(Source::default()),
    );
    let rubric = registry
        .publish_rubric(&helpful(), "ada", now())
        .await
        .unwrap()
        .version;
    let people = request("nobody-judged", 2);
    publish(&registry, people, "editor", now()).await.unwrap();
    let refused = registry
        .take_calibration(
            &CalibrationRequest {
                name: "nobody".into(),
                evaluation_id: "nobody-judged".into(),
                rubrics: vec![VersionReference {
                    name: "helpful".into(),
                    version: rubric.clone(),
                }],
            },
            "ada",
            120,
        )
        .await
        .unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("refused rather than defaulted"),
        "{refused}"
    );

    let calibration = calibrated(&registry, &rubric).await;
    let mut run = declared(&registry, &calibration, &rubric).await.run;
    run.evaluation_id = "people-judged".into();
    let own = registry
        .declare_scoring_run(&run, "ada", now())
        .await
        .unwrap_err();
    assert!(own.to_string().contains("own result"), "{own}");

    run.evaluation_id = "judged-again".into();
    run.judge = None;
    let unasked = registry
        .declare_scoring_run(&run, "ada", now())
        .await
        .unwrap_err();
    assert!(unasked.to_string().contains("asks a judge"), "{unasked}");

    let unpublished = registry
        .publish_scorecard(&card(&"f".repeat(64)), "ada", now())
        .await
        .unwrap_err();
    assert!(
        unpublished.to_string().contains("no published version"),
        "a judge naming a rubric nobody wrote is refused when the card is published: {unpublished}"
    );
}

#[tokio::test]
async fn a_run_declared_for_another_judge_profile_asks_nothing() {
    #[derive(Debug, Default)]
    struct Elsewhere;
    #[async_trait]
    impl JudgeModel for Elsewhere {
        fn provider(&self) -> &str {
            "openai"
        }
        async fn ask(&self, _: &JudgeCall) -> std::result::Result<JudgeReply, JudgeFailure> {
            panic!("a run declared for another profile must not reach this judge")
        }
    }

    let registry = Arc::new(registry(
        Arc::new(MemoryObjectStore::new()),
        Arc::new(Source::default()),
    ));
    let rubric = registry
        .publish_rubric(&helpful(), "ada", now())
        .await
        .unwrap()
        .version;
    let calibration = calibrated(&registry, &rubric).await;
    let declared = declared(&registry, &calibration, &rubric).await;
    let view = registry
        .scoring_run_view(&declared.id)
        .await
        .unwrap()
        .unwrap();
    registry
        .approve(&view.manifest, "operator", now())
        .await
        .unwrap();

    let (command, attempt) = attempt(&declared.id, "judged-run");
    let refused = ScoreExecutor::new(Arc::clone(&registry))
        .judged_by(Arc::new(Elsewhere), 1)
        .execute(&command, &attempt)
        .await
        .unwrap_err();
    assert_eq!(refused.class, FailureClass::UserCode);
    assert!(
        refused.message.contains("AIWATCHER_JUDGE_PROVIDER"),
        "{}",
        refused.message
    );

    let unjudged = ScoreExecutor::new(Arc::clone(&registry))
        .execute(&command, &attempt)
        .await
        .unwrap_err();
    assert!(
        unjudged.message.contains("holds none"),
        "{}",
        unjudged.message
    );
}
