use super::*;
use super::{
    fixture::Fixture,
    project_evidence::{source, stage},
};
use aiwatcher_iam::{OrganizationId, ProjectId, ProjectScope};
use aiwatcher_server::config::{BackendKind, Config, PromptStoreKind};
use serde_json::json;

fn scope() -> ProjectScope {
    ProjectScope {
        organization: OrganizationId::new(),
        project: ProjectId::new(),
    }
}
fn registry_for(store: Arc<dyn ObjectStore>, fixture: &Fixture) -> Registry {
    Registry::new(
        store.clone(),
        Arc::new(source(store, fixture)),
        Default::default(),
    )
    .unwrap()
}

#[tokio::test]
async fn project_results_preserve_receipts_and_bytes_on_reopen_with_local_approvals_and_erasure() {
    let fixture = Fixture::new("project-results").await;
    let path = fixture.root.join("store");
    let store = Arc::new(FileObjectStore::open(&path).await.unwrap());
    let root = registry_for(store.clone(), &fixture);
    let owner = source(store.clone(), &fixture);
    let a = scope();
    let b = ProjectScope {
        project: ProjectId::new(),
        ..a
    };
    let datasets = aiwatcher_datasets::Registry::new(store.clone(), "datasets");
    let ar = root.for_project_evidence(a).unwrap();
    let br = root.for_project_evidence(b).unwrap();
    assert!(
        root.for_project_authored(a)
            .unwrap()
            .for_project_evidence(a)
            .is_err()
    );
    assert!(ar.for_project_evidence(b).is_err());
    assert!(ar.project_evidence_scopes().await.is_err());
    for (scope, registry) in [(a, &ar), (b, &br)] {
        datasets
            .for_project(scope)
            .unwrap()
            .publish(fixture.rows.clone())
            .await
            .unwrap();
        stage(&fixture, owner.for_project(scope).unwrap().as_ref()).await;
        assert!(matches!(
            registry
                .publish(fixture.request.clone(), "editor", 100)
                .await,
            Err(EvaluationError::NotAdmitted(_))
        ));
    }
    let approval = ar
        .approve(&fixture.request.manifest, "admin", 100)
        .await
        .unwrap();
    let receipt = ar
        .publish(fixture.request.clone(), "editor", 100)
        .await
        .unwrap();
    assert!(br.approvals().await.unwrap().is_empty());
    assert!(root.approvals().await.unwrap().is_empty());
    assert!(
        br.get(&receipt.evaluation_id, "viewer", 101)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        root.get(&receipt.evaluation_id, "viewer", 101)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        br.approve(&fixture.request.manifest, "admin", 100)
            .await
            .unwrap(),
        approval
    );
    assert_eq!(
        br.publish(fixture.request.clone(), "editor", 100)
            .await
            .unwrap(),
        receipt
    );
    let reopened = registry_for(
        Arc::new(FileObjectStore::open(&path).await.unwrap()),
        &fixture,
    )
    .for_project_evidence(a)
    .unwrap();
    assert_eq!(
        reopened
            .publish(fixture.request.clone(), "editor", 101)
            .await
            .unwrap(),
        receipt
    );
    assert_eq!(
        reopened
            .cases(
                &receipt.evaluation_id,
                &receipt.version,
                None,
                Some(1),
                "viewer",
                101
            )
            .await
            .unwrap()
            .unwrap()
            .cases
            .len(),
        1
    );
    let prefix = |s: ProjectScope| {
        format!(
            "evaluation-scopes/{}/{}/registry/",
            s.organization.0, s.project.0
        )
    };
    for entry in store.list(&prefix(a)).await.unwrap() {
        let relative = entry.key.strip_prefix(&prefix(a)).unwrap().to_owned();
        assert_eq!(
            store.get(&entry.key).await.unwrap(),
            store
                .get(&format!("{}{relative}", prefix(b)))
                .await
                .unwrap()
        );
    }
    assert_eq!(root.project_evidence_scopes().await.unwrap().len(), 2);
    reopened
        .withdraw(&approval.record.approval_id, "admin", 102)
        .await
        .unwrap();
    assert_eq!(
        reopened
            .get(&receipt.evaluation_id, "viewer", 103)
            .await
            .unwrap()
            .unwrap()
            .state,
        EvidenceState::Forbidden
    );
    assert_eq!(
        br.get(&receipt.evaluation_id, "viewer", 103)
            .await
            .unwrap()
            .unwrap()
            .state,
        EvidenceState::Complete
    );
    assert!(reopened.forget(&receipt.evaluation_id).await.unwrap());
    assert_eq!(br.collect_orphans(104).await.unwrap().removed, 0);
    assert!(
        br.cases(
            &receipt.evaluation_id,
            &receipt.version,
            None,
            None,
            "viewer",
            104
        )
        .await
        .unwrap()
        .is_some()
    );
}

#[tokio::test]
async fn project_calibration_survives_reopen_without_widening_the_judge_store() {
    let fixture = Fixture::new("project-calibration").await;
    let path = fixture.root.join("store");
    let store = Arc::new(FileObjectStore::open(&path).await.unwrap());
    let root = registry_for(store.clone(), &fixture);
    let a = scope();
    let b = scope();
    let ar = root.for_project_evidence(a).unwrap();
    aiwatcher_datasets::Registry::new(store.clone(), "datasets")
        .for_project(a)
        .unwrap()
        .publish(fixture.rows.clone())
        .await
        .unwrap();
    stage(
        &fixture,
        source(store.clone(), &fixture)
            .for_project(a)
            .unwrap()
            .as_ref(),
    )
    .await;
    publish(&ar, fixture.request.clone(), "editor", 100)
        .await
        .unwrap();
    let rubric = ar.publish_rubric(&serde_json::from_value(json!({
        "name":"helpful", "question":"Does it help?", "scale":{"kind":"flag"}, "direction":"higher"
    })).unwrap(), "editor", 101).await.unwrap();
    let case = &fixture.request.cases[0];
    ar.assess(
        &serde_json::from_value(json!({
            "target":{"kind":"case","evaluation_id":fixture.request.manifest.origin.evaluation_id,
                      "case_id":case.case_id,"repetition_id":case.repetition_id},
            "rubric":"helpful","rubric_version":rubric.version,"value":{"type":"flag","value":true}
        }))
        .unwrap(),
        "person",
        102,
    )
    .await
    .unwrap();
    let request = serde_json::from_value(json!({
        "name":"people", "evaluation_id":fixture.request.manifest.origin.evaluation_id,
        "rubrics":[{"name":"helpful","version":rubric.version}]
    }))
    .unwrap();
    let taken = ar.take_calibration(&request, "editor", 103).await.unwrap();
    assert!(!taken.calibration.from_archive);
    assert!(root.calibration(&taken.version).await.unwrap().is_none());
    assert!(
        root.for_project_evidence(b)
            .unwrap()
            .calibration(&taken.version)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        root.for_project_authored(a)
            .unwrap()
            .calibration(&taken.version)
            .await
            .is_err()
    );
    // An authored registry opens neither a judge's replies nor a declaration,
    // and an evidence registry's replies are its project's: absent, never a
    // neighbour's or the instance's.
    let call = aiwatcher_evaluation::JudgeCall {
        model: "judge".into(),
        messages: vec![],
        temperature: 0.0,
        seed: None,
        max_tokens: 10,
        schema: json!({}),
    };
    assert!(
        root.for_project_authored(a)
            .unwrap()
            .remembered_reply("declared-run", &call)
            .await
            .is_err()
    );
    assert!(
        ar.remembered_reply("declared-run", &call)
            .await
            .unwrap()
            .is_none()
    );
    assert!(ar.scoring_run("run").await.is_err());
    let reopened = registry_for(
        Arc::new(FileObjectStore::open(&path).await.unwrap()),
        &fixture,
    )
    .for_project_evidence(a)
    .unwrap();
    assert_eq!(
        reopened.calibration(&taken.version).await.unwrap(),
        Some(taken.clone())
    );
    assert_eq!(
        reopened
            .take_calibration(&request, "other-editor", 104)
            .await
            .unwrap(),
        taken
    );
    let key = format!(
        "evaluation-scopes/{}/{}/registry/evaluation-judges/calibrations/{}.json",
        a.organization.0, a.project.0, taken.version
    );
    let bytes = store.get(&key).await.unwrap().unwrap();
    let mut corrupted: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    corrupted["calibration"]["items"][0]["author"] = json!("somebody-else");
    store
        .put(&key, serde_json::to_vec(&corrupted).unwrap())
        .await
        .unwrap();
    assert!(reopened.calibration(&taken.version).await.is_err());
    store.delete(&key).await.unwrap();
    assert!(
        reopened
            .calibration(&taken.version)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn retention_worker_discovers_project_evidence_and_records_scoped_sweeps() {
    let fixture = Fixture::new("project-retention").await;
    let store = fixture.store.clone();
    let root = Arc::new(registry_for(store.clone(), &fixture));
    let a = scope();
    let ar = root.for_project_evidence(a).unwrap();
    fixture
        .datasets
        .for_project(a)
        .unwrap()
        .publish(fixture.rows.clone())
        .await
        .unwrap();
    let owner = source(store.clone(), &fixture);
    stage(&fixture, owner.for_project(a).unwrap().as_ref()).await;
    let receipt = publish(&ar, fixture.request.clone(), "editor", 100)
        .await
        .unwrap();
    let mut runtime = aiwatcher_server::wiring::build(Config {
        bus: BackendKind::Memory,
        prompt_store: PromptStoreKind::Memory,
        data_dir: fixture.root.join("runtime").to_str().unwrap().into(),
        ..Config::default()
    })
    .await
    .unwrap();
    runtime.state.evaluations = Some(root.clone());
    let shutdown = tokio_util::sync::CancellationToken::new();
    let worker = aiwatcher_server::evaluation::spawn(&runtime.state, shutdown.clone()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if ar.retention().await.unwrap().is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    shutdown.cancel();
    worker.await.unwrap();
    assert_eq!(ar.retention().await.unwrap().unwrap().retired, 1);
    assert_eq!(root.retention().await.unwrap().unwrap().retired, 0);
    assert_eq!(
        ar.get(&receipt.evaluation_id, "viewer", 101)
            .await
            .unwrap()
            .unwrap()
            .state,
        EvidenceState::Expired
    );
    let prefix = format!(
        "evaluation-scopes/{}/{}/registry/evaluations/",
        a.organization.0, a.project.0
    );
    assert!(
        !store
            .list(&prefix)
            .await
            .unwrap()
            .iter()
            .any(|entry| entry.key.contains("/content/"))
    );
}
