//! Model identity, approved package metadata and actual bytes all gate evidence.
use super::fixture::Fixture;
use super::*;
use aiwatcher_training::{ModelPackage, Registry as Training};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

async fn pin(fixture: &mut Fixture, training: &Training) {
    let dir = fixture.root.join("model-artifacts");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let weights = b"[0.25,0.75]";
    let config = b"{\"threshold\":0.5}";
    let mut artifacts = Vec::new();
    for (name, bytes) in [
        ("weights", weights.as_slice()),
        ("config", config.as_slice()),
    ] {
        tokio::fs::write(dir.join(name), bytes).await.unwrap();
        artifacts.push(json!({"name": name, "uri": "https://unreachable.invalid/never-fetch",
            "digest": hex::encode(Sha256::digest(bytes)), "size_bytes": bytes.len(), "kind": "model"}));
    }
    let package: ModelPackage = serde_json::from_value(json!({
        "runtime": "weights", "entry_point": "weights", "artifacts": artifacts
    }))
    .unwrap();
    training
        .start(
            serde_json::from_value(json!({
                "run_id": "model-run", "model": "fti-model", "dataset": "train-data@abc"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let registered = training.register_model(serde_json::from_value(json!({
        "name": "fti-model", "run_id": "model-run", "checkpoint_uri": "https://unreachable.invalid/checkpoint",
        "package": package
    })).unwrap()).await.unwrap();
    assert!(registered.promotion_blocked.is_some());
    fixture.request.manifest.variant.model = Some(VersionReference {
        name: registered.version.name,
        version: registered.version.version,
    });
    fixture.request.manifest.variant.workflow = None;
    tokio::fs::write(
        fixture.root.join("model-package.json"),
        serde_json::to_vec(&package).unwrap(),
    )
    .await
    .unwrap();
    fixture.approve(&fixture.request.manifest).await;
}
fn registry(fixture: &Fixture, training: Arc<Training>) -> Registry {
    Registry::new(
        fixture.store.clone(),
        Arc::new(fixture.source().with_training(training)),
        RegistryConfig::default(),
    )
    .unwrap()
}
async fn version_key(store: &dyn ObjectStore, version: &str) -> String {
    store
        .list("training/")
        .await
        .unwrap()
        .into_iter()
        .find(|entry| entry.key.ends_with(&format!("/{version}.json")))
        .unwrap()
        .key
}

#[tokio::test]
async fn model_pins_survive_head_changes_and_reopen_then_retire_when_version_is_deleted() {
    let mut fixture = Fixture::new("model-lifecycle").await;
    let training = Arc::new(Training::new(fixture.store.clone(), "training"));
    pin(&mut fixture, &training).await;
    let first = registry(&fixture, training.clone());
    let receipt = first
        .publish(fixture.request.clone(), "editor", 100)
        .await
        .unwrap();
    let new = training
        .register_model(
            serde_json::from_value(json!({
                "name": "fti-model", "run_id": "model-run", "checkpoint_uri": "new-checkpoint",
                "metrics": {"test": {"accuracy": 0.5}}
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    training
        .set_label(
            "fti-model",
            serde_json::from_value(json!({"label": "production", "version": new.version.version}))
                .unwrap(),
        )
        .await
        .unwrap();
    let head = fixture
        .store
        .list("training/")
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.key.ends_with("/head.json"))
        .unwrap();
    fixture.store.delete(&head.key).await.unwrap();
    drop(first);
    drop(training);
    let reopened = registry(
        &fixture,
        Arc::new(Training::new(fixture.store.clone(), "training")),
    );
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
                None,
                "viewer",
                101
            )
            .await
            .unwrap()
            .unwrap()
            .cases
            .len(),
        3
    );
    let key = version_key(
        fixture.store.as_ref(),
        &fixture
            .request
            .manifest
            .variant
            .model
            .as_ref()
            .unwrap()
            .version,
    )
    .await;
    let bytes = fixture.store.get(&key).await.unwrap().unwrap();
    fixture.store.delete(&key).await.unwrap();
    assert_eq!(reopened.sweep("retention-worker", 102).await.unwrap(), 1);
    fixture.store.put(&key, bytes).await.unwrap();
    assert_eq!(
        reopened
            .get(&receipt.evaluation_id, "viewer", 103)
            .await
            .unwrap()
            .unwrap()
            .state,
        EvidenceState::DeletedSource
    );
    assert!(
        !fixture
            .store
            .list("evaluations/")
            .await
            .unwrap()
            .iter()
            .any(|e| e.key.contains("/content/"))
    );
}

#[tokio::test]
async fn every_artifact_and_full_package_are_checked_and_revocation_hides_evidence() {
    let mut fixture = Fixture::new("model-tampering").await;
    let training = Arc::new(Training::new(fixture.store.clone(), "training"));
    pin(&mut fixture, &training).await;
    let registry = registry(&fixture, training);
    let receipt = registry
        .publish(fixture.request.clone(), "editor", 100)
        .await
        .unwrap();
    for name in ["weights", "config"] {
        let path = fixture.root.join("model-artifacts").join(name);
        let bytes = tokio::fs::read(&path).await.unwrap();
        tokio::fs::write(&path, b"different bytes").await.unwrap();
        let hidden = registry
            .get(&receipt.evaluation_id, "viewer", 101)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(hidden.state, EvidenceState::CorruptArtifact);
        assert!(hidden.manifest.is_none() && hidden.metrics.is_empty());
        assert_eq!(registry.sweep("retention-worker", 101).await.unwrap(), 0);
        tokio::fs::write(&path, bytes).await.unwrap();
    }
    let key = version_key(
        fixture.store.as_ref(),
        &fixture
            .request
            .manifest
            .variant
            .model
            .as_ref()
            .unwrap()
            .version,
    )
    .await;
    let original = fixture.store.get(&key).await.unwrap().unwrap();
    let mut changed: Value = serde_json::from_slice(&original).unwrap();
    changed["package"]["runtime"] = json!("python");
    fixture
        .store
        .put(&key, serde_json::to_vec(&changed).unwrap())
        .await
        .unwrap();
    assert_eq!(
        registry
            .get(&receipt.evaluation_id, "viewer", 101)
            .await
            .unwrap()
            .unwrap()
            .state,
        EvidenceState::Forbidden
    );
    changed["package"]["artifacts"][0]["digest"] = json!("0".repeat(64));
    fixture
        .store
        .put(&key, serde_json::to_vec(&changed).unwrap())
        .await
        .unwrap();
    assert_eq!(
        registry
            .get(&receipt.evaluation_id, "viewer", 101)
            .await
            .unwrap()
            .unwrap()
            .state,
        EvidenceState::CorruptArtifact
    );
    fixture.store.put(&key, original).await.unwrap();
    let mut revoked = fixture.request.manifest.clone();
    revoked.variant.model.as_mut().unwrap().version = "0".repeat(64);
    fixture.approve(&revoked).await;
    let hidden = registry
        .cases(
            &receipt.evaluation_id,
            &receipt.version,
            None,
            None,
            "viewer",
            102,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(hidden.state, EvidenceState::Forbidden);
    assert!(hidden.cases.is_empty());
    fixture.approve(&fixture.request.manifest).await;
    assert_eq!(
        registry
            .get(&receipt.evaluation_id, "viewer", 102)
            .await
            .unwrap()
            .unwrap()
            .state,
        EvidenceState::Complete
    );
    assert_eq!(
        registry
            .get(&receipt.evaluation_id, "viewer", receipt.expires_at)
            .await
            .unwrap()
            .unwrap()
            .state,
        EvidenceState::Expired
    );
}

#[tokio::test]
async fn missing_owner_package_or_approval_is_refused_before_publication() {
    let mut fixture = Fixture::new("model-refusals").await;
    let training = Arc::new(Training::new(fixture.store.clone(), "training"));
    pin(&mut fixture, &training).await;
    assert!(matches!(
        fixture
            .registry()
            .publish(fixture.request.clone(), "editor", 100)
            .await,
        Err(EvaluationError::Unavailable(EvidenceState::Forbidden))
    ));
    let owner = registry(&fixture, training.clone());
    let mut missing = fixture.request.clone();
    missing.manifest.variant.model.as_mut().unwrap().version = "0".repeat(64);
    fixture.approve(&missing.manifest).await;
    assert!(matches!(
        owner.publish(missing, "editor", 100).await,
        Err(EvaluationError::Unavailable(EvidenceState::DeletedSource))
    ));
    fixture.approve(&fixture.request.manifest).await;
    let path = fixture.root.join("model-package.json");
    let bytes = tokio::fs::read(&path).await.unwrap();
    tokio::fs::remove_file(&path).await.unwrap();
    assert!(matches!(
        owner.publish(fixture.request.clone(), "editor", 100).await,
        Err(EvaluationError::Unavailable(EvidenceState::DeletedSource))
    ));
    tokio::fs::write(path, bytes).await.unwrap();
    let legacy = training
        .register_model(
            serde_json::from_value(json!({
                "name": "fti-model", "run_id": "model-run", "checkpoint_uri": "legacy"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    fixture
        .request
        .manifest
        .variant
        .model
        .as_mut()
        .unwrap()
        .version = legacy.version.version;
    fixture.approve(&fixture.request.manifest).await;
    assert!(matches!(
        owner.publish(fixture.request.clone(), "editor", 100).await,
        Err(EvaluationError::Unavailable(EvidenceState::Forbidden))
    ));
    assert!(fixture.store.list("evaluations/").await.unwrap().is_empty());
}

#[tokio::test]
async fn server_wiring_resolves_model_pins_and_protects_http_evidence() {
    use aiwatcher_auth::{AuthConfig, AuthMode};
    use aiwatcher_server::config::{BackendKind, Config, PromptStoreKind};
    let mut fixture = Fixture::new("model-http").await;
    let runtime = aiwatcher_server::wiring::build(Config {
        bus: BackendKind::Memory,
        prompt_store: PromptStoreKind::Memory,
        data_dir: fixture.root.join("runtime").to_str().unwrap().into(),
        evaluation_source_dir: Some(fixture.root.to_str().unwrap().into()),
        auth: AuthConfig {
            mode: AuthMode::Proxy,
            ..AuthConfig::default()
        },
        ..Config::default()
    })
    .await
    .unwrap();
    let training = runtime.state.training.as_ref().unwrap();
    pin(&mut fixture, training).await;
    runtime
        .state
        .datasets
        .as_ref()
        .unwrap()
        .publish(fixture.rows.clone())
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let router = aiwatcher_api::routes::router(runtime.state.clone());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap();
    let endpoint = format!("{base}/api/v1/evaluation-results");
    assert_eq!(
        client
            .post(&endpoint)
            .header("x-authentik-username", "viewer")
            .json(&fixture.request)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let response = client
        .post(&endpoint)
        .header("x-authentik-username", "editor")
        .header("x-authentik-groups", "aiwatcher-editors")
        .json(&fixture.request)
        .send()
        .await
        .unwrap();
    assert!(
        response.status().is_success(),
        "{}",
        response.text().await.unwrap()
    );
    let receipt: EvaluationReceipt = response.json().await.unwrap();
    let cases = format!(
        "{endpoint}/{}/cases?version={}",
        receipt.evaluation_id, receipt.version
    );
    assert_eq!(client.get(&cases).send().await.unwrap().status(), 401);
    let response = client
        .get(&cases)
        .header("x-authentik-username", "viewer")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["state"], "complete");
    assert_eq!(body["cases"].as_array().unwrap().len(), 3);
    tokio::fs::remove_file(fixture.root.join("model-artifacts/config"))
        .await
        .unwrap();
    let response = client
        .get(&cases)
        .header("x-authentik-username", "viewer")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["state"], "deleted_source");
    assert_eq!(body["cases"], json!([]));
    assert_eq!(
        client
            .get(format!(
                "{base}/api/v1/evaluations/{}",
                receipt.evaluation_id
            ))
            .header("x-authentik-username", "viewer")
            .send()
            .await
            .unwrap()
            .status(),
        410
    );
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn approved_metadata_cannot_bypass_byte_lengths_package_limits_or_path_confinement() {
    let mut fixture = Fixture::new("model-bounds").await;
    let training = Arc::new(Training::new(fixture.store.clone(), "training"));
    pin(&mut fixture, &training).await;
    let pin = fixture.request.manifest.variant.model.as_ref().unwrap();
    let key = version_key(fixture.store.as_ref(), &pin.version).await;
    let original = fixture.store.get(&key).await.unwrap().unwrap();
    let source = fixture.source().with_training(training.clone());
    let snapshot: Value = serde_json::from_slice(&original).unwrap();
    for (field, value) in [
        ("size_bytes", json!(1)),
        ("size_bytes", json!(100 * 1024 * 1024 + 1)),
    ] {
        let mut changed = snapshot.clone();
        changed["package"]["artifacts"][1][field] = value;
        fixture
            .store
            .put(&key, serde_json::to_vec(&changed).unwrap())
            .await
            .unwrap();
        tokio::fs::write(
            fixture.root.join("model-package.json"),
            serde_json::to_vec(&changed["package"]).unwrap(),
        )
        .await
        .unwrap();
        assert!(matches!(
            source.resolve(&fixture.request.manifest, "viewer").await,
            Err(EvaluationError::Unavailable(EvidenceState::CorruptArtifact))
        ));
    }
    for runtime in ["unspecified", "not-a-runtime"] {
        let mut changed = snapshot.clone();
        changed["package"]["runtime"] = json!(runtime);
        fixture
            .store
            .put(&key, serde_json::to_vec(&changed).unwrap())
            .await
            .unwrap();
        assert!(matches!(
            training.verified_version(&pin.name, &pin.version).await,
            Err(aiwatcher_training::Error::Corrupt { .. })
        ));
    }
    fixture.store.put(&key, original).await.unwrap();
    tokio::fs::write(
        fixture.root.join("model-package.json"),
        serde_json::to_vec(&snapshot["package"]).unwrap(),
    )
    .await
    .unwrap();
    // External cases use the same owner and artifact checks.
    fixture.request.manifest.context.dataset = serde_json::from_str::<EvaluationManifest>(
        include_str!("../../../../contracts/fixtures/evaluation-v1/manifest.json"),
    )
    .unwrap()
    .context
    .dataset;
    fixture.request.manifest.variant.dataset = fixture.request.manifest.context.dataset.clone();
    fixture.approve(&fixture.request.manifest).await;
    source
        .resolve(&fixture.request.manifest, "viewer")
        .await
        .unwrap();
    #[cfg(unix)]
    {
        let weights = fixture.root.join("model-artifacts/weights");
        tokio::fs::remove_file(&weights).await.unwrap();
        std::os::unix::fs::symlink(fixture.root.join("cases.json"), weights).unwrap();
        assert!(matches!(
            source.resolve(&fixture.request.manifest, "viewer").await,
            Err(EvaluationError::Unavailable(EvidenceState::Forbidden))
        ));
    }
}
