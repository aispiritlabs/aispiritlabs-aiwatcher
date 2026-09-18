//! Declarations only: no project executor or server-measured approval is enabled.
use super::{fixture::Fixture, project_evidence::source, *};
use aiwatcher_iam::{OrganizationId, ProjectId, ProjectScope};
use serde_json::json;

fn scope() -> ProjectScope {
    ProjectScope {
        organization: OrganizationId::new(),
        project: ProjectId::new(),
    }
}
fn root(store: Arc<dyn ObjectStore>, fixture: &Fixture) -> Registry {
    Registry::new(
        store.clone(),
        Arc::new(source(store, fixture)),
        Default::default(),
    )
    .unwrap()
}
fn card() -> Scorecard {
    serde_json::from_value(json!({"name":"quality", "scorers":[{
        "metric":"exact", "answer_path":"", "expected_path":"", "scorer":{
            "kind":"exact_match", "ignore_case":false, "trim":false
        }
    }]}))
    .unwrap()
}
fn recording() -> Vec<u8> {
    b"{\"answers\":[{\"case_id\":\"capital-pl\",\"answer\":1.00000000000000001}]}".to_vec()
}
fn request(fixture: &Fixture) -> CohortRequest {
    CohortRequest {
        dataset: fixture.request.manifest.variant.dataset.clone(),
        split: "test".into(),
        limit: None,
    }
}
pub(super) async fn seed(registry: &Registry, fixture: &Fixture) -> ScoringRun {
    let cohort = registry
        .derive_cohort(&request(fixture), "person", 1)
        .await
        .unwrap();
    let version = registry
        .publish_scorecard(&card(), "person", 1)
        .await
        .unwrap();
    let answers = registry
        .stage_recording("answers", recording())
        .await
        .unwrap();
    ScoringRun {
        evaluation_id: "project-measurement".into(),
        repetition_id: "first".into(),
        variant: fixture.request.manifest.variant.clone(),
        cohort: cohort.cohort,
        scorecard: VersionReference {
            name: version.scorecard.name,
            version: version.version,
        },
        answers: Answers::Recording(answers),
        judge: None,
        external_calibration: None,
        settings: Default::default(),
    }
}

#[tokio::test]
async fn project_declarations_require_local_pins_and_survive_reopen_without_admitting_execution() {
    let fixture = Fixture::new("project-declarations").await;
    let path = fixture.root.join("store");
    let store = Arc::new(FileObjectStore::open(&path).await.unwrap());
    let legacy = root(store.clone(), &fixture);
    let a = scope();
    let b = ProjectScope {
        project: ProjectId::new(),
        ..a
    };
    let c = ProjectScope {
        organization: OrganizationId::new(),
        ..a
    };
    let datasets = aiwatcher_datasets::Registry::new(store.clone(), "datasets");
    datasets
        .for_project(a)
        .unwrap()
        .publish(fixture.rows.clone())
        .await
        .unwrap();
    let registry = legacy.for_project_evidence(a).unwrap();
    let run = seed(&registry, &fixture).await;
    let declared = registry
        .declare_scoring_run(&run, "first-author", 10)
        .await
        .unwrap();
    let again = registry
        .declare_scoring_run(&run, "other-author", 20)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&declared).unwrap(),
        serde_json::to_value(&again).unwrap()
    );
    assert_eq!(declared.id, run.id().unwrap());
    assert!(legacy.scoring_run(&declared.id).await.unwrap().is_none());
    for other in [b, c] {
        let other_registry = legacy.for_project_evidence(other).unwrap();
        assert!(
            other_registry
                .scoring_run(&declared.id)
                .await
                .unwrap()
                .is_none()
        );
        // A card in the other project is not this project's card.
        assert!(
            other_registry
                .declare_scoring_run(&run, "person", 1)
                .await
                .is_err()
        );
        other_registry
            .publish_scorecard(&card(), "person", 1)
            .await
            .unwrap();
        assert!(
            other_registry
                .declare_scoring_run(&run, "person", 1)
                .await
                .is_err()
        );
        other_registry
            .stage_recording("answers", recording())
            .await
            .unwrap();
        assert!(
            other_registry
                .declare_scoring_run(&run, "person", 1)
                .await
                .is_err()
        );
        // The derivation itself cannot reach the identical dataset next door.
        assert!(
            other_registry
                .derive_cohort(&request(&fixture), "person", 1)
                .await
                .is_err()
        );
        datasets
            .for_project(other)
            .unwrap()
            .publish(fixture.rows.clone())
            .await
            .unwrap();
        assert!(
            other_registry
                .declare_scoring_run(&run, "person", 1)
                .await
                .is_err()
        );
        other_registry
            .derive_cohort(&request(&fixture), "person", 1)
            .await
            .unwrap();
        let independent = other_registry
            .declare_scoring_run(&run, "local-author", 30)
            .await
            .unwrap();
        assert_eq!(independent.id, declared.id);
        assert_eq!(independent.declared_by, "local-author");
    }
    let reopened = root(
        Arc::new(FileObjectStore::open(&path).await.unwrap()),
        &fixture,
    )
    .for_project_evidence(a)
    .unwrap();
    let view = reopened
        .scoring_run_view(&declared.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(view.declaration.run, run);
    assert!(!view.admitted);
    assert_eq!(view.cohort.unwrap().cohort, run.cohort);
    assert!(reopened.approve(&view.manifest, "admin", 40).await.is_err());
    assert!(
        reopened
            .cohort_cases(&view.manifest, "person")
            .await
            .is_err()
    );
    assert!(reopened.for_project_evidence(b).is_err());
    assert!(
        legacy
            .for_project_authored(a)
            .unwrap()
            .scoring_run(&declared.id)
            .await
            .is_err()
    );
    assert!(
        legacy
            .for_project_cohorts(a)
            .unwrap()
            .declare_scoring_run(&run, "person", 1)
            .await
            .is_err()
    );
    for invalid in ["../secret", "..\\secret", "", "run"] {
        assert!(reopened.scoring_run(invalid).await.is_err());
    }
    // The first immutable writer remains authoritative, but corrupt content is
    // not an idempotent success, even if its embedded ID was left unchanged.
    let key = format!(
        "evaluation-scopes/{}/{}/registry/evaluation-runs/{}.json",
        a.organization.0, a.project.0, declared.id
    );
    let original = store.get(&key).await.unwrap().unwrap();
    let mut corrupt = declared.clone();
    corrupt.run.repetition_id = "substituted".into();
    store
        .put(&key, serde_json::to_vec(&corrupt).unwrap())
        .await
        .unwrap();
    assert!(matches!(
        reopened.scoring_run(&declared.id).await,
        Err(EvaluationError::Unavailable(EvidenceState::CorruptArtifact))
    ));
    assert!(
        reopened
            .declare_scoring_run(&run, "person", 50)
            .await
            .is_err()
    );
    store.put(&key, original).await.unwrap();
    assert_eq!(
        reopened
            .scoring_run(&declared.id)
            .await
            .unwrap()
            .unwrap()
            .run,
        run
    );
}

#[tokio::test]
async fn project_declarations_recheck_native_bytes_and_refuse_unscoped_answer_authorities_before_writing()
 {
    let fixture = Fixture::new("project-declaration-refusals").await;
    let store = Arc::new(MemoryObjectStore::new());
    let legacy = root(store.clone(), &fixture);
    let a = scope();
    let datasets = aiwatcher_datasets::Registry::new(store.clone(), "datasets");
    datasets.publish(fixture.rows.clone()).await.unwrap();
    datasets
        .for_project(a)
        .unwrap()
        .publish(fixture.rows.clone())
        .await
        .unwrap();
    let registry = legacy.for_project_evidence(a).unwrap();
    let run = seed(&registry, &fixture).await;
    let prefix = format!(
        "evaluation-scopes/{}/{}/registry/evaluation-runs/",
        a.organization.0, a.project.0
    );
    let mut generated = run.clone();
    generated.answers = Answers::Generated(Generated {
        generated_by: Generation {
            task: "task@1".into(),
            queue: "workers".into(),
            params: Default::default(),
        },
    });
    assert!(
        registry
            .declare_scoring_run(&generated, "person", 1)
            .await
            .is_err()
    );
    let mut external = run.clone();
    external.variant.dataset.kind = DatasetKind::External;
    assert!(
        registry
            .declare_scoring_run(&external, "person", 1)
            .await
            .is_err()
    );
    let mut changed = run.clone();
    changed.cohort.case_count += 1;
    assert!(
        registry
            .declare_scoring_run(&changed, "person", 1)
            .await
            .is_err()
    );
    let mut archived = run.clone();
    archived.answers = Answers::Archive(ArchiveWord::Archive);
    assert!(
        registry
            .declare_scoring_run(&archived, "person", 1)
            .await
            .is_err()
    );
    let rubric = registry
        .publish_rubric(
            &serde_json::from_value(json!({"name":"helpful", "question":"Does it help?",
                "scale":{"kind":"flag"}, "direction":"higher"}))
            .unwrap(),
            "person",
            1,
        )
        .await
        .unwrap();
    let judged_card = registry
        .publish_scorecard(
            &serde_json::from_value(json!({"name":"judged", "scorers":[{
                "metric":"helpful", "scorer":{"kind":"judge", "rubric":{
                    "name":"helpful", "version":rubric.version
                }}
            }]}))
            .unwrap(),
            "person",
            1,
        )
        .await
        .unwrap();
    let mut judged = run.clone();
    judged.scorecard = VersionReference {
        name: "judged".into(),
        version: judged_card.version,
    };
    judged.judge = Some(JudgeDeclaration {
        provider: "llamacpp".into(),
        model: VersionReference {
            name: "judge".into(),
            version: "v1".into(),
        },
        settings: Default::default(),
        calibration: VersionReference {
            name: "people".into(),
            version: "a".repeat(64),
        },
    });
    assert!(
        matches!(registry.declare_scoring_run(&judged, "person", 1).await,
        Err(EvaluationError::Invalid { field, .. }) if field == "run")
    );
    // Even the judge settings must not be persisted by a refused declaration.
    assert!(
        store
            .list(&format!(
                "evaluation-scopes/{}/{}/registry/evaluation-judges/",
                a.organization.0, a.project.0
            ))
            .await
            .unwrap()
            .is_empty()
    );
    assert!(store.list(&prefix).await.unwrap().is_empty());
    let declared = registry
        .declare_scoring_run(&run, "person", 1)
        .await
        .unwrap();
    let Answers::Recording(recording_ref) = &run.answers else {
        panic!("recording")
    };
    let key = format!(
        "evaluation-scopes/{}/{}/registry/evaluation-recordings/{}.json",
        a.organization.0, a.project.0, recording_ref.digest
    );
    legacy
        .stage_recording("answers", recording())
        .await
        .unwrap();
    store.put(&key, b"{\"answers\":[]}".to_vec()).await.unwrap();
    assert!(
        registry
            .declare_scoring_run(&run, "person", 2)
            .await
            .is_err()
    );
    assert!(registry.scoring_run_view(&declared.id).await.is_err());
    store.delete(&key).await.unwrap();
    assert!(
        registry
            .declare_scoring_run(&run, "person", 2)
            .await
            .is_err()
    );
    registry
        .stage_recording("answers", recording())
        .await
        .unwrap();
    let dataset_prefix = format!(
        "datasets/scopes/{}/{}/registry/",
        a.organization.0, a.project.0
    );
    let key = store
        .list(&dataset_prefix)
        .await
        .unwrap()
        .into_iter()
        .find(|entry| {
            entry
                .key
                .ends_with(&format!("/{}.json", run.variant.dataset.version))
        })
        .unwrap()
        .key;
    store.put(&key, b"{}".to_vec()).await.unwrap();
    assert!(
        registry
            .declare_scoring_run(&run, "person", 2)
            .await
            .is_err()
    );
    store.delete(&key).await.unwrap();
    assert!(registry.scoring_run_view(&declared.id).await.is_err());
    assert!(
        registry
            .declare_scoring_run(&run, "person", 2)
            .await
            .is_err()
    );
    // Metadata is still readable by its address; that does not admit a run.
    assert_eq!(
        registry
            .scoring_run(&declared.id)
            .await
            .unwrap()
            .unwrap()
            .run,
        run
    );
}
