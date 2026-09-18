//! Full producer manifests resolved through the actual project-bound owners.
use super::fixture::Fixture;
use super::*;
use aiwatcher_iam::{OrganizationId, ProjectId, ProjectScope};
use aiwatcher_server::evaluation::LocalSource;
use serde_json::json;
use sha2::{Digest, Sha256};

fn scope() -> ProjectScope {
    ProjectScope {
        organization: OrganizationId::new(),
        project: ProjectId::new(),
    }
}

pub(super) async fn stage(fixture: &Fixture, source: &dyn ApprovalBundles) -> String {
    let prepared = Evaluation::prepare(fixture.request.manifest.clone()).unwrap();
    let id = approval_id(prepared.variant_id(), prepared.context_id()).unwrap();
    for entry in std::fs::read_dir(&fixture.root).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            source
                .stage(
                    &id,
                    entry.file_name().to_str().unwrap(),
                    tokio::fs::read(entry.path()).await.unwrap(),
                )
                .await
                .unwrap();
        }
    }
    id
}

pub(super) fn source(store: Arc<dyn ObjectStore>, fixture: &Fixture) -> LocalSource {
    LocalSource::new(Some(fixture.root.to_str().unwrap().into()))
        .with_bundles(store.clone())
        .with_curation(Arc::new(aiwatcher_datasets::Registry::new(
            store.clone(),
            "datasets",
        )))
        .with_prompts(Arc::new(aiwatcher_prompts::Registry::new(
            store.clone(),
            Default::default(),
        )))
        .with_training(Arc::new(aiwatcher_training::Registry::new(
            store, "training",
        )))
}

#[tokio::test]
async fn project_evidence_reopens_and_never_falls_back_to_global_or_neighbouring_sources() {
    let fixture = Fixture::new("project-evidence").await;
    let dir = fixture.root.join("object-store");
    let store = Arc::new(FileObjectStore::open(&dir).await.unwrap());
    let root = source(store.clone(), &fixture);
    let a = scope();
    let b = ProjectScope {
        project: ProjectId::new(),
        ..a
    };
    let c = ProjectScope {
        organization: OrganizationId::new(),
        ..a
    };
    let manifest = &fixture.request.manifest;
    let datasets = aiwatcher_datasets::Registry::new(store.clone(), "datasets");
    datasets.publish(fixture.rows.clone()).await.unwrap();
    stage(&fixture, &root).await;
    assert_eq!(
        root.resolve(manifest, "viewer")
            .await
            .unwrap()
            .expected
            .len(),
        3
    );
    let ar = root.for_project_evidence(a).unwrap();
    assert!(ar.resolve(manifest, "viewer").await.is_err());
    let ab = root.for_project(a).unwrap();
    let id = stage(&fixture, ab.as_ref()).await;
    // An identical global version does not satisfy a project pin.
    assert!(ar.resolve(manifest, "viewer").await.is_err());
    let published = datasets
        .for_project(a)
        .unwrap()
        .publish(fixture.rows.clone())
        .await
        .unwrap();
    assert_eq!(
        published.dataset.latest.version,
        manifest.context.dataset.version
    );
    let expected = ar.resolve(manifest, "viewer").await.unwrap().expected;
    for other in [b, c] {
        assert!(
            root.for_project_evidence(other)
                .unwrap()
                .resolve(manifest, "viewer")
                .await
                .is_err()
        );
        // Even identical staged bytes need an independently present local owner.
        stage(&fixture, root.for_project(other).unwrap().as_ref()).await;
        assert!(
            root.for_project_evidence(other)
                .unwrap()
                .resolve(manifest, "viewer")
                .await
                .is_err()
        );
    }
    let reopened = source(
        Arc::new(FileObjectStore::open(&dir).await.unwrap()),
        &fixture,
    )
    .for_project_evidence(a)
    .unwrap();
    assert_eq!(
        reopened.resolve(manifest, "viewer").await.unwrap().expected,
        expected
    );
    assert!(ar.for_project_evidence(b).is_err());
    assert!(ar.for_project_cohorts(b).is_err());
    assert!(ar.for_project_evidence(a).is_ok());
    // Narrowing to cohort derivation cannot restore full evidence access.
    assert!(
        ar.for_project_cohorts(a)
            .unwrap()
            .for_project_evidence(a)
            .is_err()
    );
    let prefix = format!(
        "datasets/scopes/{}/{}/registry/",
        a.organization.0, a.project.0
    );
    let key = store
        .list(&prefix)
        .await
        .unwrap()
        .into_iter()
        .find(|entry| {
            entry
                .key
                .ends_with(&format!("/{}.json", manifest.context.dataset.version))
        })
        .unwrap()
        .key;
    let bytes = store.get(&key).await.unwrap().unwrap();
    store.put(&key, b"{}".to_vec()).await.unwrap();
    assert!(reopened.resolve(manifest, "viewer").await.is_err());
    store.delete(&key).await.unwrap();
    assert!(reopened.resolve(manifest, "viewer").await.is_err());
    store.put(&key, bytes).await.unwrap();
    assert_eq!(
        reopened.resolve(manifest, "viewer").await.unwrap().expected,
        expected
    );
    ab.stage(&id, "scorer.py", b"changed".to_vec())
        .await
        .unwrap();
    assert!(reopened.resolve(manifest, "viewer").await.is_err());
    ab.discard(&id).await.unwrap();
    // Host and global bundles still exist; neither is a fallback.
    assert!(reopened.resolve(manifest, "viewer").await.is_err());
    assert!(root.resolve(manifest, "viewer").await.is_ok());
}

#[tokio::test]
async fn project_evidence_requires_its_own_prompt_version_and_rechecks_its_bytes() {
    let mut fixture = Fixture::new("project-evidence-prompt").await;
    let prompts = aiwatcher_prompts::Registry::new(fixture.store.clone(), Default::default());
    let request: aiwatcher_prompts::PublishRequest = serde_json::from_value(json!({
        "name":"private-prompt", "text":"Answer {{ question }}."
    }))
    .unwrap();
    let version = prompts.publish(request.clone()).await.unwrap().version;
    fixture.request.manifest.variant.prompt = Some(VersionReference {
        name: version.name.to_string(),
        version: version.version_id.to_string(),
    });
    fixture.approve(&fixture.request.manifest).await;
    let a = scope();
    fixture
        .datasets
        .for_project(a)
        .unwrap()
        .publish(fixture.rows.clone())
        .await
        .unwrap();
    let root = source(fixture.store.clone(), &fixture);
    stage(&fixture, root.for_project(a).unwrap().as_ref()).await;
    let bound = root.for_project_evidence(a).unwrap();
    let manifest = &fixture.request.manifest;
    assert!(bound.resolve(manifest, "viewer").await.is_err());
    prompts
        .for_project(a)
        .unwrap()
        .publish(request)
        .await
        .unwrap();
    assert!(bound.resolve(manifest, "viewer").await.is_ok());
    let prefix = format!(
        "prompts/scopes/{}/{}/registry/",
        a.organization.0, a.project.0
    );
    let key = fixture
        .store
        .list(&prefix)
        .await
        .unwrap()
        .into_iter()
        .find(|entry| {
            entry
                .key
                .ends_with(&format!("/{}.json", version.version_id))
        })
        .unwrap()
        .key;
    fixture.store.put(&key, b"{}".to_vec()).await.unwrap();
    assert!(bound.resolve(manifest, "viewer").await.is_err());
    fixture.store.delete(&key).await.unwrap();
    assert!(bound.resolve(manifest, "viewer").await.is_err());
    // An already-bound native owner cannot be rebound by wrapping it.
    let wrong = root.with_prompts(Arc::new(prompts.for_project(scope()).unwrap()));
    assert!(wrong.for_project_evidence(a).is_err());
}

#[tokio::test]
async fn project_evidence_requires_its_own_registered_model_and_verified_artifacts() {
    let mut fixture = Fixture::new("project-evidence-model").await;
    let training = aiwatcher_training::Registry::new(fixture.store.clone(), "training");
    let start: aiwatcher_training::StartRunRequest = serde_json::from_value(json!({
        "run_id":"private-run", "model":"private-model", "dataset":"train@abc"
    }))
    .unwrap();
    let weights = b"[0.25,0.75]";
    let package = json!({"runtime":"weights", "entry_point":"weights", "artifacts":[{
        "name":"weights", "uri":"https://unreachable.invalid/never-fetch",
        "digest":hex::encode(Sha256::digest(weights)), "size_bytes":weights.len(), "kind":"model"
    }]});
    let register: aiwatcher_training::RegisterModelRequest = serde_json::from_value(json!({
        "name":"private-model", "run_id":"private-run", "checkpoint_uri":"never-fetch", "package":package
    })).unwrap();
    training.start(start.clone()).await.unwrap();
    let version = training
        .register_model(register.clone())
        .await
        .unwrap()
        .version;
    fixture.request.manifest.variant.model = Some(VersionReference {
        name: version.name,
        version: version.version,
    });
    fixture.approve(&fixture.request.manifest).await;
    let a = scope();
    fixture
        .datasets
        .for_project(a)
        .unwrap()
        .publish(fixture.rows.clone())
        .await
        .unwrap();
    let root = source(fixture.store.clone(), &fixture);
    let bundle = root.for_project(a).unwrap();
    let id = stage(&fixture, bundle.as_ref()).await;
    bundle
        .stage(
            &id,
            "model-package.json",
            serde_json::to_vec(&package).unwrap(),
        )
        .await
        .unwrap();
    bundle
        .stage(&id, "model-artifacts/weights", weights.to_vec())
        .await
        .unwrap();
    let bound = root.for_project_evidence(a).unwrap();
    let manifest = &fixture.request.manifest;
    assert!(bound.resolve(manifest, "viewer").await.is_err());
    let local = training.for_project(a).unwrap();
    local.start(start).await.unwrap();
    let local_version = local.register_model(register).await.unwrap().version;
    assert_eq!(
        local_version.version,
        manifest.variant.model.as_ref().unwrap().version
    );
    assert!(bound.resolve(manifest, "viewer").await.is_ok());
    bundle
        .stage(&id, "model-artifacts/weights", b"different".to_vec())
        .await
        .unwrap();
    assert!(bound.resolve(manifest, "viewer").await.is_err());
}

#[tokio::test]
async fn external_project_evidence_checks_every_staged_pin_without_native_owners() {
    let mut fixture = Fixture::new("project-evidence-external").await;
    fixture.request = request("project-external", 3);
    fixture.approve(&fixture.request.manifest).await;
    let root = LocalSource::new(None).with_bundles(fixture.store.clone());
    let a = scope();
    let bundle = root.for_project(a).unwrap();
    let id = stage(&fixture, bundle.as_ref()).await;
    let bound = root.for_project_evidence(a).unwrap();
    let manifest = &fixture.request.manifest;
    assert_eq!(
        bound
            .resolve(manifest, "viewer")
            .await
            .unwrap()
            .expected
            .len(),
        3
    );
    for name in [
        manifest.variant.code.name.as_str(),
        manifest.variant.generation_config.name.as_str(),
        manifest.context.case_manifest.name.as_str(),
        manifest.context.input_schema.name.as_str(),
        manifest.context.expectations_schema.name.as_str(),
        "suite.json",
        "scorer.py",
        "workflow.json",
    ] {
        bundle.stage(&id, name, b"changed".to_vec()).await.unwrap();
        assert!(bound.resolve(manifest, "viewer").await.is_err(), "{name}");
        stage(&fixture, bundle.as_ref()).await;
    }
    assert!(root.resolve(manifest, "viewer").await.is_err());
}

#[tokio::test]
async fn project_evidence_denies_unsupported_capabilities_before_reading_any_bytes() {
    assert!(Source::default().for_project_evidence(scope()).is_err());
    let fixture = Fixture::new("project-evidence-denied").await;
    let root = source(fixture.store.clone(), &fixture);
    let bound = root.for_project_evidence(scope()).unwrap();
    for kind in [DatasetKind::Conversations, DatasetKind::Assessments] {
        let mut manifest = fixture.request.manifest.clone();
        manifest.context.dataset.kind = kind;
        manifest.variant.dataset.kind = kind;
        assert!(matches!(
            bound.resolve(&manifest, "admin").await,
            Err(EvaluationError::Unavailable(EvidenceState::Forbidden))
        ));
    }
    let mut manifest = fixture.request.manifest.clone();
    manifest.context.scorer.name = "aiwatcher.scoring".into();
    assert!(manifest.context.scored_here());
    assert!(matches!(
        bound.resolve(&manifest, "admin").await,
        Err(EvaluationError::Unavailable(EvidenceState::Forbidden))
    ));
    assert!(
        LocalSource::new(Some(fixture.root.to_str().unwrap().into()))
            .for_project_evidence(scope())
            .is_err()
    );
}
