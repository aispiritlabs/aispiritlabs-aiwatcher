//! Admitting a pair whose bytes never touched this host's disk.
use super::fixture::Fixture;
use super::*;
use aiwatcher_auth::{AuthConfig, AuthMode};
use aiwatcher_evaluation::{ApprovalBundles, Evaluation};
use aiwatcher_server::config::{BackendKind, Config, PromptStoreKind};
use aiwatcher_server::evaluation::LocalSource;
use serde_json::Value;

/// The bundle as the operator holds it: every file of the fixture, by name.
async fn files(root: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            let name = entry.file_name().to_str().unwrap().to_owned();
            found.push((name, tokio::fs::read(entry.path()).await.unwrap()));
        }
    }
    found
}

fn approval_of(manifest: &EvaluationManifest) -> String {
    let prepared = Evaluation::prepare(manifest.clone()).unwrap();
    aiwatcher_evaluation::approval_id(prepared.variant_id(), prepared.context_id()).unwrap()
}

#[tokio::test]
async fn a_staged_bundle_admits_a_pair_on_an_instance_with_no_directory() {
    let fixture = Fixture::new("staging").await;
    let adapter = Arc::new(
        LocalSource::new(None)
            .with_bundles(fixture.store.clone())
            .with_curation(fixture.datasets.clone()),
    );
    let registry = Registry::new(
        fixture.store.clone(),
        adapter.clone(),
        RegistryConfig::default(),
    )
    .unwrap();
    let manifest = fixture.request.manifest.clone();
    let id = manifest.origin.evaluation_id.clone();
    let approval = approval_of(&manifest);

    // Nothing staged and no directory: a refusal to admit the pair, which is
    // not a source somebody deleted.
    assert!(matches!(
        registry.approve(&manifest, "operator", 100).await,
        Err(EvaluationError::Unavailable(EvidenceState::Forbidden))
    ));

    for (name, bytes) in files(&fixture.root).await {
        adapter.stage(&approval, &name, bytes).await.unwrap();
    }
    let staged = adapter.staged(&approval).await.unwrap();
    assert!(staged.iter().any(|file| file.name == "manifest.json"));
    assert!(staged.iter().all(|file| file.size_bytes > 0));

    // One operator act, over the API, and then the producer publishes.
    registry.approve(&manifest, "operator", 100).await.unwrap();
    let receipt = registry
        .publish(fixture.request.clone(), "producer", 100)
        .await
        .unwrap();
    assert_eq!(
        registry
            .get(&id, "viewer", 101)
            .await
            .unwrap()
            .unwrap()
            .state,
        EvidenceState::Complete
    );

    // A path is not a name, in either adapter.
    for name in ["../manifest.json", "nested/file.json", ".hidden"] {
        assert!(matches!(
            adapter.stage(&approval, name, b"x".to_vec()).await,
            Err(EvaluationError::Unavailable(EvidenceState::Forbidden))
        ));
    }

    // Bytes that arrive after an approval do not widen it: the declaration
    // pins this member's digest, so the pair stops reading instead.
    adapter
        .stage(&approval, "scorer.py", b"# something else\n".to_vec())
        .await
        .unwrap();
    assert_eq!(
        evidence(&registry, &receipt, "viewer", 102).await,
        EvidenceState::CorruptArtifact
    );
    assert!(adapter.discard(&approval).await.unwrap() > 0);
    assert!(adapter.staged(&approval).await.unwrap().is_empty());
}

#[tokio::test]
async fn staging_is_an_operator_route_and_publication_needs_no_step_on_the_host() {
    let fixture = Fixture::new("staging-http").await;
    let runtime = aiwatcher_server::wiring::build(Config {
        bus: BackendKind::Memory,
        prompt_store: PromptStoreKind::Memory,
        data_dir: fixture.root.join("runtime").to_str().unwrap().into(),
        evaluation_source_dir: None,
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
    let response = client
        .post(format!("{base}/api/v1/datasets"))
        .header("x-authentik-username", "editor")
        .header("x-authentik-groups", "aiwatcher-editors")
        .json(&fixture.rows)
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());

    let approval = approval_of(&fixture.request.manifest);
    let bundle = format!("{base}/api/v1/evaluation-approvals/{approval}/bundle");
    for (name, bytes) in files(&fixture.root).await {
        // An editor is what a producer's own ingest token holds, and staging
        // the bytes a pair is admitted by is not a producer's act.
        let refused = client
            .put(format!("{bundle}/{name}"))
            .header("x-authentik-username", "producer")
            .header("x-authentik-groups", "aiwatcher-editors")
            .body(bytes.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(refused.status(), 403);
        let response = client
            .put(format!("{bundle}/{name}"))
            .header("x-authentik-username", "operator")
            .header("x-authentik-groups", "aiwatcher-admins")
            .body(bytes)
            .send()
            .await
            .unwrap();
        assert!(
            response.status().is_success(),
            "{}",
            response.text().await.unwrap()
        );
    }
    // The one folder a bundle has, through the same route: a name, not a path,
    // and the only separator either adapter accepts.
    let nested = client
        .put(format!("{bundle}/model-artifacts/weights.bin"))
        .header("x-authentik-username", "operator")
        .header("x-authentik-groups", "aiwatcher-admins")
        .body(vec![1u8, 2, 3])
        .send()
        .await
        .unwrap();
    assert!(nested.status().is_success());
    let escaped = client
        .put(format!("{bundle}/nested%2Fescape.json"))
        .header("x-authentik-username", "operator")
        .header("x-authentik-groups", "aiwatcher-admins")
        .body(vec![1u8])
        .send()
        .await
        .unwrap();
    assert_eq!(escaped.status(), 403);

    let listed: Vec<Value> = client
        .get(&bundle)
        .header("x-authentik-username", "viewer")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(listed.iter().any(|file| file["name"] == "scorer.py"));
    assert!(
        listed
            .iter()
            .any(|file| file["name"] == "model-artifacts/weights.bin")
    );

    admit(&client, &base, &fixture.request.manifest).await;
    let response = client
        .post(format!("{base}/api/v1/evaluation-results"))
        .header("x-authentik-username", "producer")
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
    let detail: Value = client
        .get(format!(
            "{base}/api/v1/evaluation-results/{}",
            fixture.request.manifest.origin.evaluation_id
        ))
        .header("x-authentik-username", "viewer")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(detail["state"], "complete");

    let discarded = client
        .delete(&bundle)
        .header("x-authentik-username", "operator")
        .header("x-authentik-groups", "aiwatcher-admins")
        .send()
        .await
        .unwrap();
    assert_eq!(discarded.status(), 204);
    server.abort();
    let _ = server.await;
}
