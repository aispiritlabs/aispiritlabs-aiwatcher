//! Native annotation cases carry exact COCO inputs and pinned vector targets.
use super::fixture::Fixture;
use super::*;
use aiwatcher_annotations::{Registry as Annotations, Split};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

async fn pin(fixture: &mut Fixture, owner: &Annotations) -> Value {
    owner.save_project(serde_json::from_value(json!({
        "name": "fti/annotations", "classes": [{"name": "point", "geometry": "point", "color": "#334155"}],
        "split_overrides": {"held-out": "test", "development": "train"}
    })).unwrap()).await.unwrap();
    for (index, group) in ["held-out", "held-out", "development"].iter().enumerate() {
        let blob = owner.put_blob(format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\"><text>{index}</text></svg>").into_bytes(), "image/svg+xml").await.unwrap();
        owner.register_image(serde_json::from_value(json!({
            "project": "fti/annotations", "image_id": blob.image_id, "uri": blob.uri,
            "width": 10, "height": 10, "group_id": group, "rights": {"kind": "owned", "grant": "synthetic fixture"}
        })).unwrap()).await.unwrap();
        owner.save_revision(serde_json::from_value(json!({
            "project": "fti/annotations", "image_id": blob.image_id, "accept": true,
            "annotations": [{"id": "point-1", "class": "point", "origin": "human", "geometry": {"kind": "point", "at": [2.0, 3.0]}}]
        })).unwrap(), "reviewer").await.unwrap();
    }
    let manifest = owner
        .export(serde_json::from_value(json!({"project": "fti/annotations"})).unwrap())
        .await
        .unwrap()
        .manifest;
    // Ordinary COCO provides the producer's bundle independently of verified reads.
    let coco = owner
        .coco(&manifest.project, &manifest.export, Some(Split::Test))
        .await
        .unwrap();
    assert_eq!(coco["images"].as_array().unwrap().len(), 2);
    assert_eq!(coco["annotations"][0]["bbox"], json!([2.0, 3.0, 0.0, 0.0]));
    let cases: Vec<Value> = coco["images"].as_array().unwrap().iter().map(|image| json!({
        "case_id": image["file_name"], "input": image,
        "expected": {"categories": coco["categories"], "annotations": coco["annotations"].as_array().unwrap().iter().filter(|a| a["image_id"] == image["id"]).collect::<Vec<_>>()}
    })).collect();
    let bytes = serde_json::to_vec(&json!({"schema_version": 1, "cases": cases})).unwrap();
    tokio::fs::write(fixture.root.join("cases.json"), &bytes)
        .await
        .unwrap();
    let c = &mut fixture.request.manifest.context;
    c.dataset = DatasetReference {
        kind: DatasetKind::Annotations,
        name: manifest.project,
        version: manifest.export,
    };
    c.split = "test".into();
    c.case_count = 2;
    c.case_manifest.digest = hex::encode(Sha256::digest(&bytes));
    c.case_manifest.size_bytes = Some(bytes.len() as u64);
    fixture.request.manifest.variant.dataset = c.dataset.clone();
    fixture.request.cases = cases
        .iter()
        .map(|case| CaseMeasurement {
            case_id: case["case_id"].as_str().unwrap().into(),
            repetition_id: fixture.request.manifest.origin.repetition_id.clone(),
            actual: Some(case["expected"].clone()),
            metrics: BTreeMap::from([("accuracy".into(), 1.0)]),
            error: None,
            trace_id: None,
            span_id: None,
        })
        .collect();
    let original = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../contracts/fixtures/evaluation-annotations-v1");
    let context = &mut fixture.request.manifest.context;
    for artifact in [&mut context.input_schema, &mut context.expectations_schema] {
        let name = if artifact.name.contains("input") {
            "input-schema.json"
        } else {
            "expectations-schema.json"
        };
        let bytes = tokio::fs::read(original.join(name)).await.unwrap();
        tokio::fs::write(fixture.root.join(&artifact.name), &bytes)
            .await
            .unwrap();
        artifact.digest = hex::encode(Sha256::digest(&bytes));
        artifact.size_bytes = Some(bytes.len() as u64);
    }
    for (name, reference) in [
        ("scorer.py", &mut context.scorer),
        ("suite.json", &mut context.suite),
    ] {
        let bytes = tokio::fs::read(original.join(name)).await.unwrap();
        tokio::fs::write(fixture.root.join(name), &bytes)
            .await
            .unwrap();
        reference.name = "annotation-exact-fixture".into();
        reference.version = hex::encode(Sha256::digest(&bytes));
    }
    fixture.approve(&fixture.request.manifest).await;
    coco
}
fn registry(fixture: &Fixture, owner: Arc<Annotations>) -> Registry {
    Registry::new(
        fixture.store.clone(),
        Arc::new(fixture.source().with_annotations(owner)),
        RegistryConfig::default(),
    )
    .unwrap()
}
async fn key(fixture: &Fixture, contains: &str, suffix: &str) -> String {
    fixture
        .store
        .list("annotations/")
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.key.contains(contains) && e.key.ends_with(suffix))
        .unwrap()
        .key
}
async fn state(registry: &Registry, id: &str) -> EvidenceState {
    registry
        .get(id, "viewer", 101)
        .await
        .unwrap()
        .unwrap()
        .state
}

#[tokio::test]
async fn annotation_pin_survives_new_revision_and_index_loss_then_retires_with_its_blob() {
    let mut f = Fixture::new("annotation-lifecycle").await;
    let owner = Arc::new(Annotations::new(f.store.clone(), "annotations"));
    let coco = pin(&mut f, &owner).await;
    let first = registry(&f, owner.clone());
    let receipt = first
        .publish(f.request.clone(), "editor", 100)
        .await
        .unwrap();
    let image = coco["images"][0]["file_name"].as_str().unwrap();
    owner.save_revision(serde_json::from_value(json!({
        "project": "fti/annotations", "image_id": image, "accept": true,
        "annotations": [{"id": "point-new", "class": "point", "origin": "human", "geometry": {"kind": "point", "at": [7.0, 8.0]}}]
    })).unwrap(), "reviewer").await.unwrap();
    let index = key(&f, "/exports/", "index.json").await;
    f.store.delete(&index).await.unwrap();
    drop(first);
    drop(owner);
    let reopened = registry(
        &f,
        Arc::new(Annotations::new(f.store.clone(), "annotations")),
    );
    assert_eq!(
        reopened
            .publish(f.request.clone(), "editor", 101)
            .await
            .unwrap(),
        receipt
    );
    let page = reopened
        .cases(
            &receipt.evaluation_id,
            &receipt.version,
            None,
            Some(1),
            "viewer",
            101,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(page.cases.len(), 1);
    assert!(page.next_cursor.is_some());
    let blob = key(&f, "/blobs/", image).await;
    let bytes = f.store.get(&blob).await.unwrap().unwrap();
    f.store.delete(&blob).await.unwrap();
    assert_eq!(reopened.sweep("retention-worker", 102).await.unwrap(), 1);
    f.store.put(&blob, bytes).await.unwrap();
    assert_eq!(
        state(&reopened, &receipt.evaluation_id).await,
        EvidenceState::DeletedSource
    );
    assert!(
        !f.store
            .list("evaluations/")
            .await
            .unwrap()
            .iter()
            .any(|e| e.key.contains("/content/"))
    );
}

#[tokio::test]
async fn revoked_rights_and_review_hide_annotation_evidence_without_resetting_retention() {
    let mut f = Fixture::new("annotation-rights").await;
    let owner = Arc::new(Annotations::new(f.store.clone(), "annotations"));
    let coco = pin(&mut f, &owner).await;
    let registry = registry(&f, owner.clone());
    let receipt = registry
        .publish(f.request.clone(), "editor", 100)
        .await
        .unwrap();
    let image = coco["images"][0]["file_name"].as_str().unwrap();
    let head_key = key(&f, "/images/", &format!("{image}.json")).await;
    let original = f.store.get(&head_key).await.unwrap().unwrap();
    let mut head: Value = serde_json::from_slice(&original).unwrap();
    for rights in [
        json!({"kind": "unknown"}),
        json!({"kind": "research_only", "license": "fixture-research"}),
    ] {
        head["image"]["rights"] = rights;
        f.store
            .put(&head_key, serde_json::to_vec(&head).unwrap())
            .await
            .unwrap();
        assert_eq!(
            state(&registry, &receipt.evaluation_id).await,
            EvidenceState::Forbidden
        );
        assert_eq!(registry.sweep("retention-worker", 101).await.unwrap(), 0);
    }
    f.store.put(&head_key, original.clone()).await.unwrap();
    owner
        .review(
            serde_json::from_value(
                json!({"project": "fti/annotations", "image_id": image, "review": "rejected"}),
            )
            .unwrap(),
            "reviewer",
        )
        .await
        .unwrap();
    let page = registry
        .cases(
            &receipt.evaluation_id,
            &receipt.version,
            None,
            None,
            "viewer",
            101,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(page.state, EvidenceState::Forbidden);
    assert!(page.cases.is_empty());
    f.store.put(&head_key, original).await.unwrap();
    assert_eq!(
        state(&registry, &receipt.evaluation_id).await,
        EvidenceState::Complete
    );
    let mut changed = f.request.manifest.clone();
    changed.context.split = "train".into();
    f.approve(&changed).await;
    assert_eq!(
        state(&registry, &receipt.evaluation_id).await,
        EvidenceState::Forbidden
    );
    f.approve(&f.request.manifest).await;
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
async fn export_revision_schema_and_image_corruption_never_yield_partial_success() {
    let mut f = Fixture::new("annotation-integrity").await;
    let owner = Arc::new(Annotations::new(f.store.clone(), "annotations"));
    let coco = pin(&mut f, &owner).await;
    let registry = registry(&f, owner.clone());
    let receipt = registry
        .publish(f.request.clone(), "editor", 100)
        .await
        .unwrap();
    let image = coco["images"][0]["file_name"].as_str().unwrap();
    let export = key(
        &f,
        "/exports/",
        &format!("{}.json", f.request.manifest.context.dataset.version),
    )
    .await;
    let revision = key(&f, &format!("/revisions/{image}/"), ".json").await;
    let project = key(&f, "/projects/", "/head.json").await;
    let blob = key(&f, "/blobs/", image).await;
    for (key, pointer, value) in [
        (&export, "/samples/0/width", json!(99)),
        (&revision, "/annotations/0/geometry/at/0", json!(9.0)),
        (&project, "/schema/classes/0/name", json!("other")),
    ] {
        let original = f.store.get(key).await.unwrap().unwrap();
        let mut body: Value = serde_json::from_slice(&original).unwrap();
        *body.pointer_mut(pointer).unwrap() = value;
        f.store
            .put(key, serde_json::to_vec(&body).unwrap())
            .await
            .unwrap();
        assert_eq!(
            state(&registry, &receipt.evaluation_id).await,
            EvidenceState::CorruptArtifact
        );
        let hidden = registry
            .get(&receipt.evaluation_id, "viewer", 101)
            .await
            .unwrap()
            .unwrap();
        assert!(hidden.manifest.is_none() && hidden.metrics.is_empty());
        f.store.put(key, original).await.unwrap();
    }
    let bytes = f.store.get(&blob).await.unwrap().unwrap();
    f.store.put(&blob, b"changed image".to_vec()).await.unwrap();
    assert_eq!(
        state(&registry, &receipt.evaluation_id).await,
        EvidenceState::CorruptArtifact
    );
    f.store.put(&blob, bytes).await.unwrap();
    let bytes = f.store.get(&revision).await.unwrap().unwrap();
    f.store.delete(&revision).await.unwrap();
    assert!(matches!(
        owner
            .verified_coco(
                "fti/annotations",
                &f.request.manifest.context.dataset.version,
                Split::Test
            )
            .await,
        Err(aiwatcher_annotations::Error::NotFound(_))
    ));
    f.store.put(&revision, bytes).await.unwrap();
    owner.save_project(serde_json::from_value(json!({"name": "fti/annotations", "classes": [{"name": "different", "geometry": "point", "color": "#334155"}]})).unwrap()).await.unwrap();
    assert_eq!(
        state(&registry, &receipt.evaluation_id).await,
        EvidenceState::Forbidden
    );
}

#[tokio::test]
async fn annotation_publication_requires_exact_inputs_targets_and_split() {
    let mut f = Fixture::new("annotation-refusals").await;
    let owner = Arc::new(Annotations::new(f.store.clone(), "annotations"));
    pin(&mut f, &owner).await;
    assert!(matches!(
        f.registry().publish(f.request.clone(), "editor", 100).await,
        Err(EvaluationError::Unavailable(EvidenceState::Forbidden))
    ));
    let source = f.source().with_annotations(owner.clone());
    for split in ["train", "validation", "all", "arbitrary"] {
        let mut request = f.request.clone();
        request.manifest.context.split = split.into();
        f.approve(&request.manifest).await;
        assert!(source.resolve(&request.manifest, "editor").await.is_err());
    }
    f.approve(&f.request.manifest).await;
    for alias in ["latest", "../head", &"AB".repeat(32)] {
        assert!(
            owner
                .verified_coco("fti/annotations", alias, Split::Test)
                .await
                .is_err()
        );
    }
    let original = tokio::fs::read(f.root.join("cases.json")).await.unwrap();
    for pointer in [
        "/cases/0/input/width",
        "/cases/0/expected/annotations/0/category_id",
    ] {
        let mut cases: Value = serde_json::from_slice(&original).unwrap();
        *cases.pointer_mut(pointer).unwrap() = json!(999);
        let bytes = serde_json::to_vec(&cases).unwrap();
        tokio::fs::write(f.root.join("cases.json"), &bytes)
            .await
            .unwrap();
        f.request.manifest.context.case_manifest.digest = hex::encode(Sha256::digest(&bytes));
        f.request.manifest.context.case_manifest.size_bytes = Some(bytes.len() as u64);
        f.approve(&f.request.manifest).await;
        assert!(matches!(
            registry(&f, owner.clone())
                .publish(f.request.clone(), "editor", 100)
                .await,
            Err(EvaluationError::Unavailable(EvidenceState::CorruptArtifact))
        ));
    }
    assert!(f.store.list("evaluations/").await.unwrap().is_empty());
}

#[tokio::test]
async fn server_wiring_resolves_annotation_pins_and_protects_http_evidence() {
    use aiwatcher_auth::{AuthConfig, AuthMode};
    use aiwatcher_server::config::{BackendKind, Config, PromptStoreKind};
    let mut fixture = Fixture::new("annotation-http").await;
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
    let owner = runtime.state.annotations.as_ref().unwrap();
    let coco = pin(&mut fixture, owner).await;
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
    assert_eq!(body["cases"].as_array().unwrap().len(), 2);
    let image = coco["images"][0]["file_name"].as_str().unwrap();
    let objects = runtime.artifacts.as_ref().unwrap();
    let key = objects
        .list("annotations/")
        .await
        .unwrap()
        .into_iter()
        .find(|entry| entry.key.contains("/revisions/") && entry.key.contains(image))
        .unwrap()
        .key;
    objects.delete(&key).await.unwrap();
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
async fn verified_annotation_reads_enforce_declared_rights_blob_ownership_and_bounds() {
    let mut f = Fixture::new("annotation-owner").await;
    let owner = Arc::new(Annotations::new(f.store.clone(), "annotations"));
    let coco = pin(&mut f, &owner).await;
    let any = owner
        .export(
            serde_json::from_value(json!({"project": "fti/annotations", "rights_policy": "any"}))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(matches!(
        owner
            .verified_coco("fti/annotations", &any.manifest.export, Split::Test)
            .await,
        Err(aiwatcher_annotations::Error::Rejected(_))
    ));
    let image = coco["images"][0]["file_name"].as_str().unwrap();
    let head_key = key(&f, "/images/", &format!("{image}.json")).await;
    let original = f.store.get(&head_key).await.unwrap().unwrap();
    let mut head: Value = serde_json::from_slice(&original).unwrap();
    head["image"]["rights"] = json!({"kind": "research_only", "license": "fixture-research"});
    f.store
        .put(&head_key, serde_json::to_vec(&head).unwrap())
        .await
        .unwrap();
    let research = owner
        .export(
            serde_json::from_value(
                json!({"project": "fti/annotations", "rights_policy": "research"}),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        owner
            .verified_coco("fti/annotations", &research.manifest.export, Split::Test)
            .await
            .unwrap()["images"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    head["image"]["uri"] = json!("https://unreachable.invalid/no-network");
    f.store
        .put(&head_key, serde_json::to_vec(&head).unwrap())
        .await
        .unwrap();
    assert!(matches!(
        owner
            .verified_coco("fti/annotations", &research.manifest.export, Split::Test)
            .await,
        Err(aiwatcher_annotations::Error::Rejected(_))
    ));
    f.store.put(&head_key, original).await.unwrap();
    let export = key(
        &f,
        "/exports/",
        &format!("{}.json", f.request.manifest.context.dataset.version),
    )
    .await;
    let original = f.store.get(&export).await.unwrap().unwrap();
    f.store
        .put(&export, vec![b' '; 4 * 1024 * 1024 + 1])
        .await
        .unwrap();
    assert!(matches!(
        owner
            .verified_coco(
                "fti/annotations",
                &f.request.manifest.context.dataset.version,
                Split::Test
            )
            .await,
        Err(aiwatcher_annotations::Error::TooLarge { .. })
    ));
    f.store.put(&export, original).await.unwrap();
    for target in [export, head_key] {
        let original = f.store.get(&target).await.unwrap().unwrap();
        f.store.delete(&target).await.unwrap();
        assert!(matches!(
            owner
                .verified_coco(
                    "fti/annotations",
                    &f.request.manifest.context.dataset.version,
                    Split::Test
                )
                .await,
            Err(aiwatcher_annotations::Error::NotFound(_))
        ));
        f.store.put(&target, original).await.unwrap();
    }
}
