//! The same vertical for a card's framework metrics: what the scorer service
//! runs is the deployment's one record, and everything the card, the run and
//! the replies say about it is one project's. No service address or credential
//! reaches a project or a plan, and no start path is opened here either.
use super::{
    fixture::Fixture,
    project_evidence::{source, stage},
    *,
};
use aiwatcher_execution::{ActivityExecutor, FailureClass};
use aiwatcher_iam::{OrganizationId, ProjectId, ProjectScope};
use aiwatcher_server::execution::scorers::{Remembering, score_all};
use aiwatcher_server::execution::scoring::ScoreExecutor;
use serde_json::json;
use std::sync::Mutex;

fn scope() -> ProjectScope {
    ProjectScope {
        organization: OrganizationId::new(),
        project: ProjectId::new(),
    }
}

/// What the deployment's scorer service says it runs. Names and releases only:
/// where it lives and what it is called with stay in the work role's config.
fn catalog(release: &str) -> ScorerCatalog {
    serde_json::from_value(json!({
        "contract": 1,
        "adapters": [{"name": "opik", "version": release, "metrics": [
            {"metric": "equals", "unit": "ratio", "direction": "higher", "aggregation": "rate",
             "reads": ["answer", "expected"],
             "parameters": {"case_sensitive": {"kind": "boolean"}}}
        ]}]
    }))
    .unwrap()
}

fn framework_card() -> Scorecard {
    serde_json::from_value(json!({
        "name": "framework-quality",
        "scorers": [{"metric": "equals", "answer_path": "/text", "expected_path": "/answer",
            "scorer": {"kind": "external", "adapter": "opik", "metric": "equals",
                       "parameters": {"case_sensitive": true}}}]
    }))
    .unwrap()
}

/// A scorer service on this host: it answers from the case it is given and
/// counts what it was asked. No framework is installed and nothing is sent out.
#[derive(Debug)]
struct Service {
    catalog: Mutex<ScorerCatalog>,
    asked: Mutex<Vec<ExternalCall>>,
}

impl Service {
    fn new(release: &str) -> Self {
        Self {
            catalog: Mutex::new(catalog(release)),
            asked: Mutex::new(Vec::new()),
        }
    }
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
        Ok(ExternalReply {
            value: Some(f64::from(u8::from(
                Some(&call.case.answer) == call.case.expected.as_ref(),
            ))),
            failed: None,
        })
    }
}

fn answers() -> Vec<u8> {
    serde_json::to_vec(&json!({"answers": [
        {"case_id": "capital-pl", "answer": {"text": "Warsaw"}},
        {"case_id": "two-plus-two", "answer": {"text": "five"}},
        {"case_id": "empty", "answer": {"text": ""}},
    ]}))
    .unwrap()
}

/// Every local dependency of one externally scored run.
async fn seed(
    registry: &Registry,
    fixture: &Fixture,
    bundles: &dyn ApprovalBundles,
) -> (ScoringRun, ScorecardVersion) {
    stage(fixture, bundles).await;
    let card = registry
        .publish_scorecard(&framework_card(), "project-editor", 100)
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
            101,
        )
        .await
        .unwrap();
    let recording = registry
        .stage_recording("answers.json", answers())
        .await
        .unwrap();
    (
        ScoringRun {
            evaluation_id: "project-framework-scored".into(),
            repetition_id: "measurement-1".into(),
            variant: fixture.request.manifest.variant.clone(),
            cohort: cohort.cohort,
            scorecard: VersionReference {
                name: card.scorecard.name.clone(),
                version: card.version.clone(),
            },
            answers: Answers::Recording(recording),
            judge: None,
            external_calibration: None,
            settings: Default::default(),
        },
        card,
    )
}

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

/// The scoring step's own sequence for a card that asks a service, driven
/// directly: no executor is registered and no project start route exists.
async fn measure(
    registry: &Arc<Registry>,
    service: &Arc<Service>,
    declared: &DeclaredRun,
    view: &ScoringRunView,
) -> Result<EvaluationReceipt> {
    let run = &declared.run;
    let card = registry
        .scorecard(&run.scorecard.name, Some(&run.scorecard.version))
        .await?
        .unwrap();
    let rubrics = registry.rubrics_for(&card.scorecard).await?;
    let cohort = registry
        .cohort_cases(&view.manifest, &declared.declared_by)
        .await?;
    let recorded = registry.answers(run, &cohort.expected).await?;
    let spelled = registry.spelled_answers(run).await?;
    let asking = external_questions(&card.scorecard, &cohort, &recorded, None);
    let remembering: Arc<dyn ExternalScorers> = Arc::new(Remembering::new(
        Arc::clone(service) as Arc<dyn ExternalScorers>,
        Arc::clone(registry),
        declared.id.clone(),
    ));
    let replies = score_all(
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
    let (scored_elsewhere, report) =
        external_replies(run, &card.scorecard, &rubrics, None, &asking, &replies);
    let scored = score_spelled(
        &card.scorecard,
        &cohort.expected,
        &recorded,
        &run.repetition_id,
        &scored_elsewhere,
        &spelled,
    );
    registry
        .publish(
            PublishEvaluation {
                manifest: view.manifest.clone(),
                status: scored.status,
                cases: scored.cases,
                judge: None,
                external: report,
                traces: None,
            },
            &declared.declared_by,
            150,
        )
        .await
}

#[tokio::test]
async fn a_project_card_pins_the_deployment_catalog_and_keeps_its_replies_in_the_project() {
    let fixture = Fixture::new("project-external").await;
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
    let registry = Arc::new(root.for_project_evidence(a).unwrap());
    let bundles = owner.for_project(a).unwrap();

    // Nothing has described itself yet, so a card naming a framework metric is
    // refused rather than published against an author's own word.
    assert!(
        registry
            .publish_scorecard(&framework_card(), "project-editor", 99)
            .await
            .is_err()
    );
    // And a project may not supply that description itself.
    assert!(
        registry
            .record_scorer_catalog(&catalog("2.2.59"), "project-editor", 99)
            .await
            .is_err()
    );
    root.record_scorer_catalog(&catalog("2.2.59"), "work-1", 99)
        .await
        .unwrap();

    let (run, published) = seed(&registry, &fixture, bundles.as_ref()).await;
    let declared = published.scorecard.scorers[0]
        .scorer
        .external()
        .unwrap()
        .declared
        .expect("the catalog's description, pinned into the version");
    assert_eq!(declared.version, "2.2.59");
    assert_eq!(declared.unit, "ratio");
    assert_eq!(declared.direction, MetricDirection::Higher);
    assert_eq!(declared.aggregation, Aggregation::Rate);
    assert!(declared.model.is_none());

    let run_declared = registry
        .declare_scoring_run(&run, "project-editor", 200)
        .await
        .unwrap();
    let view = registry
        .scoring_run_view(&run_declared.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        view.manifest.context.metrics[0]
            .measured_by
            .as_ref()
            .expect("a framework measured it")
            .adapter
            .version,
        "2.2.59",
        "what measured this metric is named where the number is read"
    );
    admit(&registry, &fixture, bundles.as_ref(), &view).await;

    let service = Arc::new(Service::new("2.2.59"));
    // The production executor still refuses a project-bound registry before it
    // reads a declaration, service or no service: admission is not authority.
    let (command, context) = attempt(&run_declared.id, &run.evaluation_id);
    let refused = ScoreExecutor::new(Arc::clone(&registry))
        .scored_by(Arc::clone(&service) as Arc<dyn ExternalScorers>, 2)
        .execute(&command, &context)
        .await
        .unwrap_err();
    assert_eq!(refused.class, FailureClass::Policy);
    assert!(
        refused.message.contains("execution authority"),
        "{refused:?}"
    );
    assert!(service.asked.lock().unwrap().is_empty());

    measure(&registry, &service, &run_declared, &view)
        .await
        .unwrap();
    let asked = service.asked.lock().unwrap().len();
    assert_eq!(asked, 3, "one question per case");

    let evidence = registry
        .get(&run.evaluation_id, "reader", 160)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        evidence.metrics["equals"],
        2.0 / 3.0,
        "two of three answers match"
    );
    // The result and its replies are this project's; an identical ID opens
    // neither next door nor on the instance.
    let next_door = root.for_project_evidence(b).unwrap();
    assert!(
        next_door
            .get(&run.evaluation_id, "reader", 160)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        root.get(&run.evaluation_id, "reader", 160)
            .await
            .unwrap()
            .is_none()
    );
    let kept = store
        .list(&format!(
            "evaluation-scopes/{}/{}/registry/evaluation-scorers/replies/{}/",
            a.organization.0, a.project.0, run_declared.id
        ))
        .await
        .unwrap();
    assert_eq!(kept.len(), asked);
    for entry in &kept {
        let text = String::from_utf8(store.get(&entry.key).await.unwrap().unwrap()).unwrap();
        let reply: KeptScore = serde_json::from_str(&text).unwrap();
        assert!(reply.value.is_some());
        assert!(
            !text.contains("Warsaw") && !text.contains("five"),
            "a kept score is the number, never the case it was about: {text}"
        );
    }
    assert!(
        store
            .list(&format!("evaluation-scorers/replies/{}/", run_declared.id))
            .await
            .unwrap()
            .is_empty(),
        "a project keeps no reply where the instance would read it"
    );

    // The same attempt again asks nothing and lands on the same bytes.
    let again = measure(&registry, &service, &run_declared, &view)
        .await
        .unwrap();
    assert_eq!(service.asked.lock().unwrap().len(), asked);
    assert_eq!(again.version, evidence.receipt.version);

    // A later catalog changes no card already published: an upgrade is
    // publishing the card again, and this version keeps what it pinned.
    root.record_scorer_catalog(&catalog("3.0.0"), "work-1", 300)
        .await
        .unwrap();
    let reopened_store: Arc<dyn ObjectStore> =
        Arc::new(FileObjectStore::open(&path).await.unwrap());
    let reopened = Registry::new(
        reopened_store.clone(),
        Arc::new(source(reopened_store, &fixture)),
        Default::default(),
    )
    .unwrap()
    .for_project_evidence(a)
    .unwrap();
    assert_eq!(
        reopened
            .scorecard(&run.scorecard.name, Some(&run.scorecard.version))
            .await
            .unwrap()
            .unwrap()
            .scorecard,
        published.scorecard
    );
    assert!(reopened.admits(&view.manifest).await.unwrap());
    let next = reopened
        .publish_scorecard(&framework_card(), "project-editor", 301)
        .await
        .unwrap();
    assert_ne!(
        next.version, published.version,
        "a new release is a new version"
    );
}

#[tokio::test]
async fn a_project_framework_metric_is_refused_an_unknown_name_and_a_neighbours_card() {
    let fixture = Fixture::new("project-external-refusals").await;
    let store: Arc<dyn ObjectStore> = Arc::new(MemoryObjectStore::new());
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
    root.record_scorer_catalog(&catalog("2.2.59"), "work-1", 99)
        .await
        .unwrap();
    let registry = Arc::new(root.for_project_evidence(a).unwrap());
    let bundles = owner.for_project(a).unwrap();
    let (run, published) = seed(&registry, &fixture, bundles.as_ref()).await;

    // A metric no adapter in this deployment's catalog implements.
    let unknown: Scorecard = serde_json::from_value(json!({
        "name": "framework-quality", "scorers": [{"metric": "equals", "answer_path": "/text",
            "scorer": {"kind": "external", "adapter": "opik", "metric": "hallucination",
                       "parameters": {}}}]
    }))
    .unwrap();
    assert!(
        registry
            .publish_scorecard(&unknown, "project-editor", 100)
            .await
            .is_err()
    );

    // The card belongs to this project: a run naming its version resolves it
    // nowhere else, even where the same deployment catalog is in front of both.
    let next_door = root.for_project_evidence(b).unwrap();
    assert!(
        next_door
            .scorecard(&run.scorecard.name, Some(&run.scorecard.version))
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        next_door
            .declare_scoring_run(&run, "outsider", 200)
            .await
            .is_err()
    );
    // Republished next door the card has the same address — and the run is
    // still refused until that project holds the rest of what it names.
    let there = next_door
        .publish_scorecard(&published.scorecard, "outsider", 200)
        .await
        .unwrap();
    assert_eq!(there.version, published.version);
    assert!(
        next_door
            .declare_scoring_run(&run, "outsider", 200)
            .await
            .is_err()
    );

    let declared = registry
        .declare_scoring_run(&run, "project-editor", 200)
        .await
        .unwrap();
    let view = registry
        .scoring_run_view(&declared.id)
        .await
        .unwrap()
        .unwrap();
    assert!(!view.admitted);
    // An approval made here admits this project's pair and nobody else's.
    admit(&registry, &fixture, bundles.as_ref(), &view).await;
    assert!(registry.admits(&view.manifest).await.unwrap());
    assert!(!next_door.admits(&view.manifest).await.unwrap());

    // A service now running another release measures something else under this
    // card's name, so the pin the card carries is what a step is held to —
    // named on both sides rather than silently accepted.
    let live = Service::new("3.0.0").catalog().await.unwrap();
    let external = published.scorecard.scorers[0].scorer.external().unwrap();
    let refused = resolve_external(
        &live,
        "scorecard.scorers[0].scorer",
        external.adapter,
        external.metric,
        external.parameters,
        external.declared,
    )
    .unwrap_err();
    assert!(
        refused.to_string().contains("2.2.59") && refused.to_string().contains("3.0.0"),
        "{refused}"
    );

    // A card asking a service that also calibrates against people needs the
    // frozen set here, and refuses one frozen from conversation evidence.
    let rubric = registry
        .publish_rubric(
            &serde_json::from_value(json!({"name": "helpful", "question": "Does it help?",
                "scale": {"kind": "flag"}, "direction": "higher"}))
            .unwrap(),
            "project-editor",
            205,
        )
        .await
        .unwrap();
    let calibrated: Scorecard = serde_json::from_value(json!({
        "name": "calibrated-quality", "scorers": [{"metric": "equals", "answer_path": "/text",
            "expected_path": "/answer",
            "scorer": {"kind": "external", "adapter": "opik", "metric": "equals",
                       "parameters": {"case_sensitive": true},
                       "calibration": {"rubric": {"name": "helpful", "version": rubric.version},
                                       "pass_at": 1.0}}}]
    }))
    .unwrap();
    let held = registry
        .publish_scorecard(&calibrated, "project-editor", 210)
        .await
        .unwrap();
    let mut run = run;
    run.scorecard = VersionReference {
        name: held.scorecard.name.clone(),
        version: held.version,
    };
    run.external_calibration = Some(VersionReference {
        name: "people".into(),
        version: "b".repeat(64),
    });
    assert!(
        matches!(registry.declare_scoring_run(&run, "project-editor", 220).await,
        Err(EvaluationError::Invalid { field, .. }) if field == "run.external_calibration"),
        "the set a run holds a metric against is this project's, by name and address"
    );
}
