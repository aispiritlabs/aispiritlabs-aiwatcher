//! A native source is verified by its owner on publication and every read.
use super::fixture::Fixture;
use super::*;
use aiwatcher_server::evaluation::LocalSource;
use serde_json::{Value, json};

#[tokio::test]
async fn curation_evidence_survives_restart_and_head_moves_but_not_source_deletion() {
    let fixture = Fixture::new("lifecycle").await;
    let first = fixture.registry();
    let receipt = first
        .publish(fixture.request.clone(), "editor", 100)
        .await
        .unwrap();
    let mut next = fixture.rows.clone();
    next.items[0].insert("expected".into(), json!({"answer": "different"}));
    fixture.datasets.publish(next).await.unwrap();
    drop(first);
    let restarted = fixture.registry();
    let id = &fixture.request.manifest.origin.evaluation_id;
    let page = restarted
        .cases(id, &receipt.version, None, Some(2), "viewer", 101)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(page.cases.len(), 2);
    assert!(page.next_cursor.is_some());
    assert!(
        page.cases
            .iter()
            .any(|case| case.expected == json!({"answer": "Warsaw"}))
    );
    assert_eq!(
        restarted
            .publish(fixture.request.clone(), "editor", 200)
            .await
            .unwrap(),
        receipt
    );

    // Revocation hides evidence without renewing or deleting its retention.
    let mut revoked = fixture.request.manifest.clone();
    revoked.variant.experiment_id = "new approval".into();
    fixture.approve(&revoked).await;
    let hidden = restarted.get(id, "viewer", 201).await.unwrap().unwrap();
    assert_eq!(hidden.state, EvidenceState::Forbidden);
    assert!(hidden.manifest.is_none() && hidden.metrics.is_empty());
    let hidden_page = restarted
        .cases(id, &receipt.version, None, None, "viewer", 201)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(hidden_page.state, EvidenceState::Forbidden);
    assert!(hidden_page.cases.is_empty() && hidden_page.next_cursor.is_none());
    fixture.approve(&fixture.request.manifest).await;
    assert_eq!(
        restarted
            .get(id, "viewer", 202)
            .await
            .unwrap()
            .unwrap()
            .state,
        EvidenceState::Complete
    );

    let pin = &fixture.request.manifest.context.dataset.version;
    let key = fixture
        .store
        .list("datasets/")
        .await
        .unwrap()
        .into_iter()
        .find(|entry| entry.key.ends_with(&format!("/{pin}.json")))
        .unwrap()
        .key;
    let bytes = fixture.store.get(&key).await.unwrap().unwrap();
    fixture.store.delete(&key).await.unwrap();
    assert_eq!(restarted.sweep("retention-worker", 203).await.unwrap(), 1);
    assert_eq!(
        restarted
            .get(id, "viewer", 203)
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
            .any(|entry| entry.key.contains("/content/"))
    );
    fixture.store.put(&key, bytes).await.unwrap();
    assert_eq!(
        restarted
            .get(id, "viewer", 204)
            .await
            .unwrap()
            .unwrap()
            .state,
        EvidenceState::DeletedSource
    );
}

#[tokio::test]
async fn curation_requires_operator_approval_an_owner_and_matching_inputs_not_only_answers() {
    let fixture = Fixture::new("mismatch").await;
    assert!(matches!(
        LocalSource::new(Some(fixture.root.to_str().unwrap().into()))
            .resolve(&fixture.request.manifest, "viewer")
            .await,
        Err(EvaluationError::Unavailable(EvidenceState::Forbidden))
    ));
    assert!(matches!(
        LocalSource::new(None)
            .with_curation(fixture.datasets.clone())
            .resolve(&fixture.request.manifest, "viewer")
            .await,
        Err(EvaluationError::Unavailable(EvidenceState::Forbidden))
    ));
    let mut changed = fixture.rows.clone();
    changed.items[0].insert(
        "input".into(),
        json!({"question": "different work, same answer"}),
    );
    let pin = fixture
        .datasets
        .publish(changed)
        .await
        .unwrap()
        .dataset
        .latest
        .version;
    let mut request = fixture.request.clone();
    request.manifest.context.dataset.version = pin.clone();
    request.manifest.variant.dataset.version = pin;
    assert!(matches!(
        fixture.source().resolve(&request.manifest, "viewer").await,
        Err(EvaluationError::Unavailable(EvidenceState::Forbidden))
    ));
    fixture.approve(&request.manifest).await;
    assert!(matches!(
        fixture.registry().publish(request, "editor", 100).await,
        Err(EvaluationError::Unavailable(EvidenceState::CorruptArtifact))
    ));
    assert!(fixture.store.list("evaluations/").await.unwrap().is_empty());
}

#[tokio::test]
async fn damaged_native_bytes_hide_published_evidence_without_becoming_a_false_deletion() {
    let fixture = Fixture::new("corruption").await;
    let registry = fixture.registry();
    let id = &fixture.request.manifest.origin.evaluation_id;
    let receipt = registry
        .publish(fixture.request.clone(), "editor", 100)
        .await
        .unwrap();
    let pin = &fixture.request.manifest.context.dataset.version;
    let key = fixture
        .store
        .list("datasets/")
        .await
        .unwrap()
        .into_iter()
        .find(|entry| entry.key.ends_with(&format!("/{pin}.json")))
        .unwrap()
        .key;
    let bytes = fixture.store.get(&key).await.unwrap().unwrap();
    let mut forged: Value = serde_json::from_slice(&bytes).unwrap();
    forged["items"][0]["expected"]["answer"] = json!("forged");
    fixture
        .store
        .put(&key, serde_json::to_vec(&forged).unwrap())
        .await
        .unwrap();
    assert_eq!(
        registry
            .get(id, "viewer", 101)
            .await
            .unwrap()
            .unwrap()
            .state,
        EvidenceState::CorruptArtifact
    );
    assert_eq!(registry.sweep("retention-worker", 101).await.unwrap(), 0);
    fixture.store.put(&key, bytes).await.unwrap();
    assert_eq!(
        registry
            .get(id, "viewer", 102)
            .await
            .unwrap()
            .unwrap()
            .state,
        EvidenceState::Complete
    );
    assert_eq!(
        registry
            .get(id, "viewer", receipt.expires_at)
            .await
            .unwrap()
            .unwrap()
            .state,
        EvidenceState::Expired
    );
}

#[tokio::test]
async fn server_wiring_uses_the_native_owner_and_preserves_http_roles() {
    use aiwatcher_auth::{AuthConfig, AuthMode};
    use aiwatcher_server::config::{BackendKind, Config, PromptStoreKind};
    let fixture = Fixture::new("http").await;
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
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let router = aiwatcher_api::routes::router(runtime.state.clone());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap();
    let publish = format!("{base}/api/v1/evaluation-results");
    let response = client
        .post(&publish)
        .header("x-authentik-username", "viewer")
        .json(&fixture.request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    let response = client
        .post(format!("{base}/api/v1/datasets"))
        .header("x-authentik-username", "editor")
        .header("x-authentik-groups", "aiwatcher-editors")
        .json(&fixture.rows)
        .send()
        .await
        .unwrap();
    assert!(
        response.status().is_success(),
        "{}",
        response.text().await.unwrap()
    );
    let response = client
        .post(&publish)
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
    let detail = format!(
        "{publish}/{}",
        fixture.request.manifest.origin.evaluation_id
    );
    assert_eq!(client.get(&detail).send().await.unwrap().status(), 401);
    let page = format!("{detail}/cases?version={}&limit=2", receipt.version);
    let response = client
        .get(&page)
        .header("x-authentik-username", "viewer")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["cases"].as_array().unwrap().len(), 2);
    let mut revoked = fixture.request.manifest.clone();
    revoked.variant.experiment_id = "revoked".into();
    fixture.approve(&revoked).await;
    let response = client
        .get(&page)
        .header("x-authentik-username", "viewer")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let hidden: Value = response.json().await.unwrap();
    assert_eq!(hidden["state"], "forbidden");
    assert_eq!(hidden["cases"], json!([]));
    let legacy = format!("{base}/api/v1/evaluations/{}", receipt.evaluation_id);
    assert_eq!(
        client
            .get(&legacy)
            .header("x-authentik-username", "viewer")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    fixture.approve(&fixture.request.manifest).await;
    tokio::fs::remove_file(fixture.root.join("cases.json"))
        .await
        .unwrap();
    for url in [&detail, &page] {
        let response = client
            .get(url)
            .header("x-authentik-username", "viewer")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["state"], "deleted_source");
        assert!(body.get("manifest").is_none_or(Value::is_null));
        assert!(body.get("cases").is_none_or(|cases| cases == &json!([])));
    }
    assert_eq!(
        client
            .get(&legacy)
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
