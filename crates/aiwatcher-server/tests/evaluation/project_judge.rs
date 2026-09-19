//! A judged project measurement, through the production resolver and a real
//! object store: the rubric, the card, the people's judgements, the set they
//! were frozen into, the settings a context pins and every reply the judge
//! gave all belong to one project. Only the model behind the judge profile is
//! the deployment's, and no project start path is opened by any of this.
use super::{
    fixture::Fixture,
    project_evidence::{source, stage},
    *,
};
use aiwatcher_execution::{ActivityExecutor, FailureClass};
use aiwatcher_iam::{OrganizationId, ProjectId, ProjectScope};
use aiwatcher_server::execution::judge::{Remembering, ask_all};
use aiwatcher_server::execution::scoring::ScoreExecutor;
use serde_json::{Value, json};
use std::sync::Mutex;

fn scope() -> ProjectScope {
    ProjectScope {
        organization: OrganizationId::new(),
        project: ProjectId::new(),
    }
}

/// A judge on this host: it calls an answer helpful when the question shows the
/// word, says something of its own beside the value, and counts what it was
/// put. No provider is reached and no person's data is sent anywhere.
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
        Ok(JudgeReply {
            content: json!({
                "value": question.contains("helpful"),
                "because": "the judge repeated the answer it was shown"
            })
            .to_string(),
            served: Served {
                model: Some("local-judge.gguf".into()),
                fingerprint: None,
            },
        })
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

fn judged_card(rubric: &str) -> Scorecard {
    Scorecard {
        name: "judged-quality".into(),
        description: String::new(),
        scorers: vec![ScorerSpec {
            metric: "helpful".into(),
            answer_path: "/text".into(),
            expected_path: String::new(),
            input_path: None,
            scorer: Scorer::Judge {
                rubric: VersionReference {
                    name: "helpful".into(),
                    version: rubric.into(),
                },
                pass_level: None,
            },
        }],
    }
}

/// The address the registry gives a frozen set, computed the way it computes
/// one. Checked against a set the registry itself took before it is relied on,
/// so this stays a restatement of that rule rather than a second one.
fn calibration_address(set: &CalibrationSet) -> String {
    fn sort(value: &mut Value) {
        match value {
            Value::Object(map) => {
                map.sort_keys();
                for child in map.values_mut() {
                    sort(child);
                }
            }
            Value::Array(items) => items.iter_mut().for_each(sort),
            _ => {}
        }
    }
    let mut value = serde_json::to_value((1u32, "evaluation.calibration", set)).unwrap();
    sort(&mut value);
    hex::encode(<sha2::Sha256 as sha2::Digest>::digest(
        serde_json::to_vec(&value).unwrap(),
    ))
}

fn said(case_id: &str, text: &str) -> Value {
    json!({"case_id": case_id, "answer": {"text": text}})
}

fn answers() -> Vec<u8> {
    serde_json::to_vec(&json!({"answers": [
        said("capital-pl", "a helpful reply"),
        said("two-plus-two", "no"),
        said("empty", "silence"),
    ]}))
    .unwrap()
}

/// Everything one judged run needs, published into `registry` alone.
struct Local {
    rubric: String,
    calibration: CalibrationVersion,
    run: ScoringRun,
}

/// The result people judged, in this project: the fixture's own cases, with
/// answers shaped the way the card reads them.
async fn people_judged(registry: &Registry, fixture: &Fixture, rubric: &str) -> CalibrationVersion {
    let mut people = fixture.request.clone();
    for (case, text) in people
        .cases
        .iter_mut()
        .zip(["a helpful answer", "a helpful-sounding dodge"])
    {
        case.actual = Some(json!({"text": text}));
    }
    // The producer's own evidence, admitted in this project from the bytes its
    // bundle holds: a calibration set is frozen from a readable result.
    publish(registry, people.clone(), "project-editor", 100)
        .await
        .unwrap();
    for (case, value) in people.cases.iter().take(2).zip([true, false]) {
        registry
            .assess(
                &AssessmentRequest {
                    target: AssessmentTarget::Case {
                        evaluation_id: people.manifest.origin.evaluation_id.clone(),
                        case_id: case.case_id.clone(),
                        repetition_id: case.repetition_id.clone(),
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
                evaluation_id: people.manifest.origin.evaluation_id.clone(),
                rubrics: vec![VersionReference {
                    name: "helpful".into(),
                    version: rubric.into(),
                }],
            },
            "project-editor",
            120,
        )
        .await
        .unwrap()
}

/// Publish every local dependency of a judged run and hand back the run.
async fn seed(registry: &Registry, fixture: &Fixture, bundles: &dyn ApprovalBundles) -> Local {
    // The producer evidence a calibration set is taken from is admitted from
    // this project's own staged bundle, never from the host directory.
    stage(fixture, bundles).await;
    let rubric = registry
        .publish_rubric(&helpful(), "project-editor", 1)
        .await
        .unwrap()
        .version;
    let calibration = people_judged(registry, fixture, &rubric).await;
    let card = registry
        .publish_scorecard(&judged_card(&rubric), "project-editor", 130)
        .await
        .unwrap();
    let cohort = registry
        .derive_cohort(
            &CohortRequest {
                dataset: fixture.request.manifest.variant.dataset.clone(),
                split: "test".into(),
                limit: None,
            },
            "project-editor",
            131,
        )
        .await
        .unwrap();
    let recording = registry
        .stage_recording("answers.json", answers())
        .await
        .unwrap();
    Local {
        run: ScoringRun {
            evaluation_id: "project-judged".into(),
            repetition_id: "measurement-1".into(),
            variant: fixture.request.manifest.variant.clone(),
            cohort: cohort.cohort,
            scorecard: VersionReference {
                name: card.scorecard.name.clone(),
                version: card.version,
            },
            answers: Answers::Recording(recording),
            judge: Some(JudgeDeclaration {
                provider: "llamacpp".into(),
                model: VersionReference {
                    name: "local-judge".into(),
                    version: "q4".into(),
                },
                settings: JudgeSettings::default(),
                calibration: VersionReference {
                    name: "people".into(),
                    version: calibration.version.clone(),
                },
            }),
            external_calibration: None,
            settings: Default::default(),
        },
        rubric,
        calibration,
    }
}

/// Stage the variant's pinned members into this project's bundle and admit the
/// pair, the way a project admin does over the scoped approval route.
async fn admit(
    registry: &Registry,
    fixture: &Fixture,
    bundles: &dyn ApprovalBundles,
    view: &ScoringRunView,
) {
    bundles
        .stage(
            &view.approval_id,
            "manifest.json",
            serde_json::to_vec(&view.manifest).unwrap(),
        )
        .await
        .unwrap();
    for name in ["responses.py", "generation.json", "workflow.json"] {
        bundles
            .stage(
                &view.approval_id,
                name,
                tokio::fs::read(fixture.root.join(name)).await.unwrap(),
            )
            .await
            .unwrap();
    }
    registry
        .approve(&view.manifest, "project-admin", 140)
        .await
        .unwrap();
}

/// Everything after admission: ask the judge through the remembering wrapper,
/// fold what it said, and publish. This is the scoring step's own sequence,
/// driven directly rather than through a reactor — no executor is registered
/// and no project start path exists.
async fn measure(
    registry: &Arc<Registry>,
    judge: &Arc<Scripted>,
    declared: &DeclaredRun,
    view: &ScoringRunView,
) -> Result<EvaluationReceipt> {
    let run = &declared.run;
    let card = registry
        .scorecard(&run.scorecard.name, Some(&run.scorecard.version))
        .await?
        .unwrap();
    let rubrics = registry.rubrics_for(&card.scorecard).await?;
    let taken = registry
        .calibration(&run.judge.as_ref().unwrap().calibration.version)
        .await?
        .unwrap()
        .calibration;
    let cohort = registry
        .cohort_cases(&view.manifest, &declared.declared_by)
        .await?;
    let recorded = registry.answers(run, &cohort.expected).await?;
    let spelled = registry.spelled_answers(run).await?;
    let calibrated = registry
        .calibrated(&taken, false, &declared.declared_by, 150)
        .await?;
    let asking = questions(
        run,
        &card.scorecard,
        &rubrics,
        &cohort,
        &recorded,
        &taken,
        &calibrated,
    );
    let remembering: Arc<dyn JudgeModel> = Arc::new(Remembering::new(
        Arc::clone(judge) as Arc<dyn JudgeModel>,
        Arc::clone(registry),
        declared.id.clone(),
    ));
    let said = ask_all(
        &remembering,
        asking
            .questions
            .iter()
            .map(|question| question.call.clone())
            .collect(),
        2,
        &aiwatcher_execution::StopSignal::new(),
    )
    .await
    .unwrap();
    let (judged, report) = replies(run, &card.scorecard, &rubrics, &taken, &asking, &said);
    let scored = score_spelled(
        &card.scorecard,
        &cohort.expected,
        &recorded,
        &run.repetition_id,
        &judged,
        &spelled,
    );
    registry
        .publish(
            PublishEvaluation {
                manifest: view.manifest.clone(),
                status: scored.status,
                cases: scored.cases,
                judge: report,
                external: None,
                traces: None,
            },
            &declared.declared_by,
            150,
        )
        .await
}

#[tokio::test]
async fn a_project_judged_run_reads_only_its_own_rubric_card_people_and_settings() {
    let fixture = Fixture::new("project-judge").await;
    let path = fixture.root.join("store");
    let store: Arc<dyn ObjectStore> = Arc::new(FileObjectStore::open(&path).await.unwrap());
    let owner = Arc::new(source(store.clone(), &fixture));
    let root = Registry::new(store.clone(), owner.clone(), Default::default()).unwrap();
    let a = scope();
    let b = ProjectScope {
        project: ProjectId::new(),
        ..a
    };
    let datasets = aiwatcher_datasets::Registry::new(store.clone(), "datasets");
    for project in [a, b] {
        datasets
            .for_project(project)
            .unwrap()
            .publish(fixture.rows.clone())
            .await
            .unwrap();
    }
    let bundles = owner.for_project(a).unwrap();
    let registry = Arc::new(root.for_project_evidence(a).unwrap());
    let local = seed(&registry, &fixture, bundles.as_ref()).await;
    assert!(!local.calibration.calibration.from_archive);

    // The neighbour holds none of it, and a declaration naming this project's
    // versions is refused there rather than resolved from an identical address.
    let next_door = root.for_project_evidence(b).unwrap();
    assert!(
        next_door
            .calibration(&local.calibration.version)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        next_door
            .rubric("helpful", Some(&local.rubric))
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        next_door
            .declare_scoring_run(&local.run, "outsider", 200)
            .await
            .is_err()
    );
    assert!(
        root.declare_scoring_run(&local.run, "instance-editor", 200)
            .await
            .is_err()
    );

    let declared = registry
        .declare_scoring_run(&local.run, "project-editor", 200)
        .await
        .unwrap();
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
    assert!(!judge.reads_archive);
    assert_eq!(judge.calibration_dataset.version, local.calibration.version);
    assert_eq!(
        view.manifest.context.metrics[0].direction,
        MetricDirection::Higher,
        "which way is better stays the rubric's, in a project as anywhere else"
    );
    assert!(!view.admitted, "declaring admits nothing");

    // The settings a context pins are kept in this project, under the digest it
    // pins — and nowhere else, so an identical judge next door is its own.
    let settings_key = format!(
        "evaluation-scopes/{}/{}/registry/evaluation-judges/settings/{}.json",
        a.organization.0, a.project.0, judge.configuration.digest
    );
    assert!(store.get(&settings_key).await.unwrap().is_some());
    assert!(
        store
            .get(&format!(
                "evaluation-judges/settings/{}.json",
                judge.configuration.digest
            ))
            .await
            .unwrap()
            .is_none(),
        "a project writes no judge settings into the instance's own store"
    );

    admit(&registry, &fixture, bundles.as_ref(), &view).await;
    assert!(registry.admits(&view.manifest).await.unwrap());
    assert!(
        !next_door.admits(&view.manifest).await.unwrap(),
        "an approval admits a pair in the project that made it"
    );

    // Another organization publishing the same bytes reaches the same
    // addresses and none of this project's records: content is content, and a
    // matching ID is not a way in.
    let c = ProjectScope {
        organization: OrganizationId::new(),
        project: ProjectId::new(),
    };
    datasets
        .for_project(c)
        .unwrap()
        .publish(fixture.rows.clone())
        .await
        .unwrap();
    let elsewhere = Arc::new(root.for_project_evidence(c).unwrap());
    let there = seed(&elsewhere, &fixture, owner.for_project(c).unwrap().as_ref()).await;
    assert_eq!(there.rubric, local.rubric);
    assert_eq!(there.calibration.version, local.calibration.version);
    assert_eq!(there.run, local.run);
    let declared_there = elsewhere
        .declare_scoring_run(&there.run, "another-author", 200)
        .await
        .unwrap();
    assert_eq!(declared_there.id, declared.id);
    assert_eq!(declared_there.declared_by, "another-author");
    assert!(
        !elsewhere.admits(&view.manifest).await.unwrap(),
        "an approval made here admits nothing over there"
    );

    // Admitting the pair is still not authority to execute it. The production
    // executor refuses a project-bound registry before it reads a declaration,
    // whether or not this process holds a judge — the runtime boundary A has
    // yet to replace with durable per-execution ownership.
    let model = Arc::new(Scripted::default());
    let (command, context) = attempt(&declared.id, &local.run.evaluation_id);
    let refused = ScoreExecutor::new(Arc::clone(&registry))
        .judged_by(Arc::clone(&model) as Arc<dyn JudgeModel>, 2)
        .execute(&command, &context)
        .await
        .unwrap_err();
    assert_eq!(refused.class, FailureClass::Policy);
    assert!(
        refused.message.contains("execution authority"),
        "{refused:?}"
    );
    assert!(model.asked.lock().unwrap().is_empty(), "nothing was asked");

    let receipt = measure(&registry, &model, &declared, &view).await.unwrap();
    let asked = model.asked.lock().unwrap().len();
    assert_eq!(asked, 5, "three cases and two calibration items");

    let evidence = registry
        .get(&local.run.evaluation_id, "reader", 160)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(evidence.receipt.version, receipt.version);
    assert!(!evidence.reproducible, "a model said these at the time");
    let report = evidence
        .judge
        .clone()
        .expect("the agreement rides beside it");
    assert_eq!(report.agreement[0].items, 2);
    assert_eq!(
        report.served,
        vec![ServedModel {
            model: Some("local-judge.gguf".into()),
            fingerprint: None,
            replies: 5,
        }]
    );
    // The result is this project's, and knowing its ID opens it nowhere else.
    assert!(
        next_door
            .get(&local.run.evaluation_id, "reader", 160)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        root.get(&local.run.evaluation_id, "reader", 160)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn kept_project_replies_answer_a_retry_without_asking_again_and_hold_no_words() {
    let fixture = Fixture::new("project-judge-replies").await;
    let path = fixture.root.join("store");
    let store: Arc<dyn ObjectStore> = Arc::new(FileObjectStore::open(&path).await.unwrap());
    let owner = Arc::new(source(store.clone(), &fixture));
    let root = Registry::new(store.clone(), owner.clone(), Default::default()).unwrap();
    let a = scope();
    let b = ProjectScope {
        project: ProjectId::new(),
        ..a
    };
    let datasets = aiwatcher_datasets::Registry::new(store.clone(), "datasets");
    datasets
        .for_project(a)
        .unwrap()
        .publish(fixture.rows.clone())
        .await
        .unwrap();
    let bundles = owner.for_project(a).unwrap();
    let registry = Arc::new(root.for_project_evidence(a).unwrap());
    let local = seed(&registry, &fixture, bundles.as_ref()).await;
    let declared = registry
        .declare_scoring_run(&local.run, "project-editor", 200)
        .await
        .unwrap();
    let view = registry
        .scoring_run_view(&declared.id)
        .await
        .unwrap()
        .unwrap();
    admit(&registry, &fixture, bundles.as_ref(), &view).await;

    let model = Arc::new(Scripted::default());
    let first = measure(&registry, &model, &declared, &view).await.unwrap();
    let asked = model.asked.lock().unwrap().len();
    assert_eq!(asked, 5);

    // The same attempt again: every reply comes from what was kept, so nothing
    // is asked twice, and the same bytes are published under the same ID.
    let again = measure(&registry, &model, &declared, &view).await.unwrap();
    assert_eq!(model.asked.lock().unwrap().len(), asked);
    assert_eq!(again.version, first.version);

    // A kept reply is what reading it finds and no word the model wrote.
    let kept = store
        .list(&format!(
            "evaluation-scopes/{}/{}/registry/evaluation-judges/replies/{}/",
            a.organization.0, a.project.0, declared.id
        ))
        .await
        .unwrap();
    assert_eq!(kept.len(), asked);
    for entry in &kept {
        let bytes = store.get(&entry.key).await.unwrap().unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(
            !text.contains("the judge repeated the answer it was shown"),
            "a kept reply holds no sentence the model wrote: {text}"
        );
        let reply: JudgeReply = serde_json::from_str(&text).unwrap();
        let value: Value = serde_json::from_str(&reply.content).unwrap();
        assert!(value.get("value").is_some() && value.as_object().unwrap().len() == 1);
    }
    // Nothing of it is kept where the instance or a neighbour would read it.
    assert!(
        store
            .list(&format!("evaluation-judges/replies/{}/", declared.id))
            .await
            .unwrap()
            .is_empty()
    );
    let next_door = root.for_project_evidence(b).unwrap();
    for entry in &kept {
        let key = entry
            .key
            .rsplit_once('/')
            .map(|(_, name)| name.to_owned())
            .unwrap();
        assert!(
            store
                .get(&format!(
                    "evaluation-scopes/{}/{}/registry/evaluation-judges/replies/{}/{key}",
                    b.organization.0, b.project.0, declared.id
                ))
                .await
                .unwrap()
                .is_none()
        );
    }
    assert!(
        next_door.scoring_run(&declared.id).await.unwrap().is_none(),
        "a reply cache is reached through a declaration, and that is this project's"
    );
    // A narrower registry opens neither the settings nor the replies.
    let authored = root.for_project_authored(a).unwrap();
    assert!(authored.scoring_run(&declared.id).await.is_err());
}

#[tokio::test]
async fn a_project_judge_is_refused_the_archive_a_neighbours_dependency_and_a_corrupted_pin() {
    let fixture = Fixture::new("project-judge-refusals").await;
    let store: Arc<dyn ObjectStore> = Arc::new(MemoryObjectStore::new());
    let owner = Arc::new(source(store.clone(), &fixture));
    let root = Registry::new(store.clone(), owner.clone(), Default::default()).unwrap();
    let a = scope();
    let datasets = aiwatcher_datasets::Registry::new(store.clone(), "datasets");
    datasets
        .for_project(a)
        .unwrap()
        .publish(fixture.rows.clone())
        .await
        .unwrap();
    let bundles = owner.for_project(a).unwrap();
    let registry = Arc::new(root.for_project_evidence(a).unwrap());
    let local = seed(&registry, &fixture, bundles.as_ref()).await;

    // A set frozen from conversation evidence would be sent to a provider, so
    // it is refused by name rather than admitted quietly.
    assert_eq!(
        calibration_address(&local.calibration.calibration),
        local.calibration.version,
        "the address this test writes under is the one the registry gives"
    );
    let mut from_archive = local.calibration.calibration.clone();
    from_archive.from_archive = true;
    let version = calibration_address(&from_archive);
    store
        .put(
            &format!(
                "evaluation-scopes/{}/{}/registry/evaluation-judges/calibrations/{version}.json",
                a.organization.0, a.project.0,
            ),
            serde_json::to_vec(&CalibrationVersion {
                version: version.clone(),
                calibration: from_archive,
                taken_by: "project-editor".into(),
                taken_at: 120,
            })
            .unwrap(),
        )
        .await
        .unwrap();
    let mut archived = local.run.clone();
    archived.judge.as_mut().unwrap().calibration.version = version;
    let refused = registry
        .declare_scoring_run(&archived, "project-editor", 200)
        .await
        .unwrap_err();
    assert!(
        refused.to_string().contains("conversation evidence"),
        "{refused}"
    );

    // A card whose rubric this project never published resolves nothing here.
    let mut foreign = local.run.clone();
    foreign.scorecard.version = "b".repeat(64);
    assert!(
        registry
            .declare_scoring_run(&foreign, "project-editor", 200)
            .await
            .is_err()
    );

    // A rewritten rubric is a new version, and nobody has judged anything under
    // it: a judge calibrated against nothing is refused rather than defaulted.
    let mut reworded = helpful();
    reworded.question = "Does the answer answer the question?".into();
    let second = registry
        .publish_rubric(&reworded, "project-editor", 201)
        .await
        .unwrap();
    let recalibrated_card = registry
        .publish_scorecard(&judged_card(&second.version), "project-editor", 202)
        .await
        .unwrap();
    let mut uncalibrated = local.run.clone();
    uncalibrated.scorecard = VersionReference {
        name: recalibrated_card.scorecard.name.clone(),
        version: recalibrated_card.version,
    };
    let refused = registry
        .declare_scoring_run(&uncalibrated, "project-editor", 203)
        .await
        .unwrap_err();
    assert!(
        refused.to_string().contains("no human judgement"),
        "{refused}"
    );

    let declared = registry
        .declare_scoring_run(&local.run, "project-editor", 200)
        .await
        .unwrap();
    let view = registry
        .scoring_run_view(&declared.id)
        .await
        .unwrap()
        .unwrap();
    admit(&registry, &fixture, bundles.as_ref(), &view).await;
    assert!(registry.admits(&view.manifest).await.unwrap());

    // Admission is more than an approval marker: the settings the context pins
    // are read again, so corruption after approval is not `admitted: true`.
    let settings_key = format!(
        "evaluation-scopes/{}/{}/registry/evaluation-judges/settings/{}.json",
        a.organization.0,
        a.project.0,
        view.manifest
            .context
            .judge
            .as_ref()
            .unwrap()
            .configuration
            .digest
    );
    let original = store.get(&settings_key).await.unwrap().unwrap();
    store.put(&settings_key, b"{}".to_vec()).await.unwrap();
    assert!(registry.admission(&view.manifest).await.is_err());
    store.delete(&settings_key).await.unwrap();
    assert!(registry.admission(&view.manifest).await.is_err());
    store.put(&settings_key, original).await.unwrap();
    assert!(registry.admits(&view.manifest).await.unwrap());

    // A judged context this deployment did not measure has no adapter at all.
    let mut producer = view.manifest.clone();
    producer.context.scorer = VersionReference {
        name: "somebody-elses-scorer".into(),
        version: "1".into(),
    };
    assert!(
        registry
            .approve(&producer, "project-admin", 210)
            .await
            .is_err()
    );
}
