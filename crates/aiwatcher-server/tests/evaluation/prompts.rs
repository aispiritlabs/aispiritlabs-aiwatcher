//! Prompt pins use verified owner content and never follow mutable labels.
use super::fixture::Fixture;
use super::*;
use aiwatcher_core::prompts::{PromptName, PromptVersionId};
use aiwatcher_prompts::{PublishRequest, Registry as Prompts, RegistryConfig as PromptConfig};
use aiwatcher_server::evaluation::LocalSource;
use serde_json::{Value, json};

fn authored(text: &str) -> PublishRequest {
    serde_json::from_value(json!({"name": "fti-prompt", "text": text, "label": "production"}))
        .unwrap()
}
async fn pin(fixture: &mut Fixture, prompts: &Prompts) {
    let prompt = prompts
        .publish(authored("Answer {{ question }} exactly."))
        .await
        .unwrap()
        .version;
    fixture.request.manifest.variant.prompt = Some(VersionReference {
        name: prompt.name.to_string(),
        version: prompt.version_id.to_string(),
    });
    fixture.request.manifest.variant.workflow = None;
    fixture.approve(&fixture.request.manifest).await;
}
fn registry(fixture: &Fixture, prompts: Arc<Prompts>) -> Registry {
    Registry::new(
        fixture.store.clone(),
        Arc::new(fixture.source().with_prompts(prompts)),
        RegistryConfig::default(),
    )
    .unwrap()
}

#[tokio::test]
async fn prompt_pins_survive_label_changes_and_index_loss_then_withdraw_on_version_deletion() {
    let mut fixture = Fixture::new("prompt-lifecycle").await;
    let prompts = Arc::new(Prompts::new(
        fixture.store.clone(),
        PromptConfig {
            max_versions_indexed: 1,
            ..PromptConfig::default()
        },
    ));
    pin(&mut fixture, &prompts).await;
    let first = registry(&fixture, prompts.clone());
    let receipt = publish(&first, fixture.request.clone(), "editor", 100)
        .await
        .unwrap();
    prompts
        .publish(authored("A new prompt {{ question }}"))
        .await
        .unwrap();
    let head = fixture
        .store
        .list("prompts/")
        .await
        .unwrap()
        .into_iter()
        .find(|entry| entry.key.ends_with("/head.json"))
        .unwrap();
    fixture.store.delete(&head.key).await.unwrap();
    drop(first);
    drop(prompts);
    let reopened = registry(
        &fixture,
        Arc::new(Prompts::new(fixture.store.clone(), PromptConfig::default())),
    );
    let id = &receipt.evaluation_id;
    assert_eq!(
        publish(&reopened, fixture.request.clone(), "editor", 101)
            .await
            .unwrap(),
        receipt
    );
    assert_eq!(
        reopened
            .cases(id, &receipt.version, None, None, "viewer", 101)
            .await
            .unwrap()
            .unwrap()
            .cases
            .len(),
        3
    );
    let version = &fixture
        .request
        .manifest
        .variant
        .prompt
        .as_ref()
        .unwrap()
        .version;
    let key = fixture
        .store
        .list("prompts/")
        .await
        .unwrap()
        .into_iter()
        .find(|entry| entry.key.ends_with(&format!("/{version}.json")))
        .unwrap()
        .key;
    let bytes = fixture.store.get(&key).await.unwrap().unwrap();
    fixture.store.delete(&key).await.unwrap();
    assert_eq!(reopened.sweep("retention-worker", 102).await.unwrap(), 1);
    assert_eq!(
        reopened
            .get(id, "viewer", 102)
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
        reopened
            .get(id, "viewer", 103)
            .await
            .unwrap()
            .unwrap()
            .state,
        EvidenceState::DeletedSource
    );
}

#[tokio::test]
async fn prompt_evidence_is_hidden_for_tampering_or_revoked_approval_and_still_expires() {
    let mut fixture = Fixture::new("prompt-corruption").await;
    let prompts = Arc::new(Prompts::new(fixture.store.clone(), PromptConfig::default()));
    pin(&mut fixture, &prompts).await;
    let registry = registry(&fixture, prompts);
    let receipt = publish(&registry, fixture.request.clone(), "editor", 100)
        .await
        .unwrap();
    let id = &receipt.evaluation_id;
    let key = fixture
        .store
        .list("prompts/")
        .await
        .unwrap()
        .into_iter()
        .find(|entry| entry.key.contains("/versions/"))
        .unwrap()
        .key;
    let bytes = fixture.store.get(&key).await.unwrap().unwrap();
    let mut tampered: Value = serde_json::from_slice(&bytes).unwrap();
    tampered["text"] = json!("Other instructions {{ question }}");
    fixture
        .store
        .put(&key, serde_json::to_vec(&tampered).unwrap())
        .await
        .unwrap();
    let hidden = registry.get(id, "viewer", 101).await.unwrap().unwrap();
    assert_eq!(hidden.state, EvidenceState::CorruptArtifact);
    assert!(hidden.manifest.is_none() && hidden.metrics.is_empty());
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
    let mut revoked = fixture.request.manifest.clone();
    revoked.variant.prompt.as_mut().unwrap().version = "0".repeat(64);
    fixture.approve(&revoked).await;
    let hidden = registry
        .cases(id, &receipt.version, None, None, "viewer", 103)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(hidden.state, EvidenceState::Forbidden);
    assert!(hidden.cases.is_empty());
    fixture.approve(&fixture.request.manifest).await;
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
async fn prompt_publication_needs_an_owner_approval_and_an_existing_exact_version() {
    let mut fixture = Fixture::new("prompt-refusals").await;
    let prompts = Arc::new(Prompts::new(fixture.store.clone(), PromptConfig::default()));
    pin(&mut fixture, &prompts).await;
    assert!(matches!(
        fixture
            .source()
            .resolve(&fixture.request.manifest, "viewer")
            .await,
        Err(EvaluationError::Unavailable(EvidenceState::Forbidden))
    ));
    assert!(matches!(
        LocalSource::new(None)
            .with_prompts(prompts.clone())
            .resolve(&fixture.request.manifest, "viewer")
            .await,
        Err(EvaluationError::Unavailable(EvidenceState::Forbidden))
    ));
    let mut unapproved = fixture.request.manifest.clone();
    unapproved.variant.prompt.as_mut().unwrap().name = "other".into();
    assert!(matches!(
        fixture
            .source()
            .with_prompts(prompts.clone())
            .resolve(&unapproved, "viewer")
            .await,
        Err(EvaluationError::Unavailable(EvidenceState::Forbidden))
    ));
    let mut missing = fixture.request.clone();
    missing.manifest.variant.prompt.as_mut().unwrap().version = "0".repeat(64);
    fixture.approve(&missing.manifest).await;
    assert!(matches!(
        registry(&fixture, prompts.clone())
            .publish(missing, "editor", 100)
            .await,
        Err(EvaluationError::Unavailable(EvidenceState::DeletedSource))
    ));
    assert!(fixture.store.list("evaluations/").await.unwrap().is_empty());
    for alias in ["production", "latest", "../head", "NOT-A-DIGEST"] {
        assert!(PromptVersionId::parse(alias).is_err());
    }
    // The same prompt adapter also composes with the external fixture.
    fixture.request.manifest.context.dataset = serde_json::from_str::<EvaluationManifest>(
        include_str!("../../../../contracts/fixtures/evaluation-v1/manifest.json"),
    )
    .unwrap()
    .context
    .dataset;
    fixture.request.manifest.variant.dataset = fixture.request.manifest.context.dataset.clone();
    fixture.approve(&fixture.request.manifest).await;
    publish(
        &registry(&fixture, prompts),
        fixture.request.clone(),
        "editor",
        100,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn server_wiring_resolves_prompt_pins_and_protects_http_evidence() {
    use aiwatcher_auth::{AuthConfig, AuthMode};
    use aiwatcher_server::config::{BackendKind, Config, PromptStoreKind};
    let mut fixture = Fixture::new("prompt-http").await;
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
    let prompts = runtime.state.prompts.as_ref().unwrap();
    pin(&mut fixture, prompts).await;
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
    admit(&client, &base, &fixture.request.manifest).await;
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
    prompts
        .publish(authored("New live prompt {{ question }}"))
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
    assert_eq!(body["state"], "complete");
    assert_eq!(body["cases"].as_array().unwrap().len(), 3);
    let reference = fixture.request.manifest.variant.prompt.as_ref().unwrap();
    let pinned = prompts
        .verified_version(
            &PromptName::parse(&reference.name).unwrap(),
            &PromptVersionId::parse(&reference.version).unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pinned.text, "Answer {{ question }} exactly.");
    let objects = runtime.artifacts.as_ref().unwrap();
    let key = objects
        .list("prompts/")
        .await
        .unwrap()
        .into_iter()
        .find(|entry| entry.key.ends_with(&format!("/{}.json", reference.version)))
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
