//! Conversation evidence keeps the archive's encryption, content role and clock.
use super::{fixture::Fixture, *};
use aiwatcher_conversations::{ArchivePolicy, Keyring, Registry as Conversations, ReviewRequest};
use aiwatcher_server::evaluation::ConversationCipher;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn keys() -> Keyring {
    Keyring::single("evaluation-test", [23; 32])
}
fn owner(store: Arc<dyn ObjectStore>) -> Arc<Conversations> {
    Arc::new(Conversations::new(
        store,
        "conversations",
        keys(),
        ArchivePolicy::default(),
    ))
}
fn review(state: &str) -> ReviewRequest {
    serde_json::from_value(json!({"state": state, "note": "fixture decision"})).unwrap()
}
fn digest(value: &Value) -> String {
    hex::encode(Sha256::digest(serde_json::to_vec(value).unwrap()))
}
async fn pin(f: &mut Fixture, archive: &Conversations) -> Value {
    for (id, text) in [
        ("q1", "SYNTHETIC_PRIVATE_QUESTION_ONE"),
        ("a1", "SYNTHETIC_PRIVATE_ANSWER_ONE"),
        ("q2", "SYNTHETIC_PRIVATE_QUESTION_TWO"),
        ("a2", "SYNTHETIC_PRIVATE_ANSWER_TWO"),
    ] {
        let turn = archive.record(serde_json::from_value(json!({
            "conversation_id":"fti/conversation", "message_id":id,
            "ordinal": match id { "q1" => 0, "a1" => 1, "q2" => 2, _ => 3 },
            "role": if id.starts_with('q') {"user"} else {"assistant"},
            "content":{"parts":[{"kind":"text", "text":text}]},
            "policy": {
                "consent":{"subject":"synthetic-fti", "basis":"synthetic", "reference":"local-test", "scope":["evaluate"]},
                "retention":{"ttl_days":1}, "redaction":{"redactor":"synthetic-fixture", "rules":[]}
            }
        })).unwrap()).await.unwrap();
        archive
            .review(
                "fti/conversation",
                &turn.turn_id,
                "reviewer",
                &review("approved"),
            )
            .await
            .unwrap();
    }
    let job = archive
        .create_export(
            serde_json::from_value(json!({
                "name":"fti/evaluation", "format":"prompt_response", "required_scope":"evaluate",
                "selection":{"conversations":["fti/conversation"]}
            }))
            .unwrap(),
            "admin",
        )
        .await
        .unwrap();
    let job = archive
        .run_export(&job.job_id, "fixture-worker")
        .await
        .unwrap();
    assert_eq!(
        job.state,
        aiwatcher_conversations::JobState::Completed,
        "{:?}",
        job.error
    );
    let version = job.version.unwrap();
    let rows = archive
        .export_rows("fti/evaluation", &version, 0, 200)
        .await
        .unwrap()
        .rows;
    assert_eq!(rows.len(), 2);
    let cases: Vec<_> = rows
        .iter()
        .map(|row| {
            json!({
                "case_id":row["eligibility"][1]["turn_id"],
                "input_digest":digest(&json!({"question":row["prompt"]})),
                "expected_digest":digest(&json!({"answer":row["response"]})),
            })
        })
        .collect();
    let bundle = json!({"schema_version":1,"cases":cases});
    let bytes = serde_json::to_vec(&bundle).unwrap();
    tokio::fs::write(f.root.join("cases.json"), &bytes)
        .await
        .unwrap();
    let context = &mut f.request.manifest.context;
    context.dataset = DatasetReference {
        kind: DatasetKind::Conversations,
        name: "fti/evaluation".into(),
        version,
    };
    context.case_count = 2;
    context.split = "test".into();
    context.case_manifest.digest = hex::encode(Sha256::digest(&bytes));
    context.case_manifest.size_bytes = Some(bytes.len() as u64);
    f.request.manifest.variant.dataset = context.dataset.clone();
    f.request.cases.truncate(2);
    for (case, row) in f.request.cases.iter_mut().zip(&rows) {
        case.case_id = row["eligibility"][1]["turn_id"].as_str().unwrap().into();
        case.actual = Some(json!({"answer":row["response"]}));
    }
    f.approve(&f.request.manifest).await;
    json!(rows)
}
fn registry(f: &Fixture, archive: Arc<Conversations>) -> Registry {
    Registry::new(
        f.store.clone(),
        Arc::new(f.source().with_conversations(archive)),
        RegistryConfig::default(),
    )
    .unwrap()
    .with_cipher(Arc::new(ConversationCipher(keys())))
    .with_content_access(true)
}
fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}
/// Governed evidence, read the way a reader reads it: the header, and then the
/// page behind it. A sealed shard is verified when its page is, so damage to
/// one shows there rather than in a summary that is one intact object.
async fn state(registry: &Registry, id: &str) -> EvidenceState {
    let detail = registry.get(id, "admin", now()).await.unwrap().unwrap();
    if !matches!(
        detail.state,
        EvidenceState::Complete | EvidenceState::Partial
    ) {
        return detail.state;
    }
    evidence(registry, &detail.receipt, "admin", now()).await
}
async fn content_keys(store: &Arc<dyn ObjectStore>) -> Vec<String> {
    store
        .list("evaluations/")
        .await
        .unwrap()
        .into_iter()
        .filter(|e| e.key.contains("/content/"))
        .map(|e| e.key)
        .collect()
}

#[tokio::test]
async fn conversation_evidence_is_sealed_restartable_paged_and_erased_with_its_source() {
    let mut f = Fixture::new("conversation-lifecycle").await;
    // Filesystem restart also proves the encrypted CAS representation persists.
    f.store = Arc::new(FileObjectStore::open(f.root.join("objects")).await.unwrap());
    let archive = owner(f.store.clone());
    pin(&mut f, &archive).await;
    let registry = registry(&f, archive.clone());
    let receipt = publish(&registry, f.request.clone(), "admin", now())
        .await
        .unwrap();
    assert!(receipt.expires_at <= now() + 86400);
    for entry in f.store.list("evaluations/").await.unwrap() {
        let bytes = f.store.get(&entry.key).await.unwrap().unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("SYNTHETIC_PRIVATE"));
        if entry.key.contains("/content/") {
            let value: Value = serde_json::from_slice(&bytes).unwrap();
            assert!(value.get("evaluation_sealed_v1").is_some());
        }
    }
    for entry in std::fs::read_dir(&f.root).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            assert!(
                !String::from_utf8_lossy(&std::fs::read(entry.path()).unwrap())
                    .contains("SYNTHETIC_PRIVATE")
            );
        }
    }
    let restarted = self::registry(&f, owner(f.store.clone()));
    assert_eq!(
        publish(&restarted, f.request.clone(), "admin", now() + 1)
            .await
            .unwrap(),
        receipt
    );
    assert_eq!(restarted.collect_orphans(now() + 7200).await.unwrap(), 0);
    let first = restarted
        .cases(
            &receipt.evaluation_id,
            &receipt.version,
            None,
            Some(1),
            "admin",
            now(),
        )
        .await
        .unwrap()
        .unwrap();
    let second = restarted
        .cases(
            &receipt.evaluation_id,
            &receipt.version,
            first.next_cursor.as_deref(),
            Some(1),
            "admin",
            now(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.cases.len(), 1);
    assert_eq!(second.cases.len(), 1);
    assert_ne!(
        first.cases[0].measurement.case_id,
        second.cases[0].measurement.case_id
    );
    archive
        .erase_subject("synthetic-fti", "admin")
        .await
        .unwrap();
    assert_eq!(restarted.sweep("retention-worker", now()).await.unwrap(), 1);
    assert_eq!(
        state(&restarted, &receipt.evaluation_id).await,
        EvidenceState::DeletedSource
    );
    assert!(content_keys(&f.store).await.is_empty());
    assert!(
        publish(&restarted, f.request.clone(), "admin", now())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn conversation_roles_are_explicit_and_retention_never_renews() {
    let mut f = Fixture::new("conversation-policy").await;
    let archive = owner(f.store.clone());
    let rows = pin(&mut f, &archive).await;
    let registry = registry(&f, archive.clone());
    let unprivileged = registry.clone().with_content_access(false);
    assert!(matches!(
        publish(&unprivileged, f.request.clone(), "admin", now()).await,
        Err(EvaluationError::Unavailable(EvidenceState::Forbidden))
    ));
    assert!(f.store.list("evaluations/").await.unwrap().is_empty());
    let receipt = publish(&registry, f.request.clone(), "admin", now())
        .await
        .unwrap();
    assert_eq!(
        state(&unprivileged, &receipt.evaluation_id).await,
        EvidenceState::Forbidden
    );
    assert_eq!(
        unprivileged.sweep("retention-worker", now()).await.unwrap(),
        0
    );
    let turn = rows[0]["eligibility"][1]["turn_id"].as_str().unwrap();
    archive
        .review("fti/conversation", turn, "reviewer", &review("rejected"))
        .await
        .unwrap();
    assert_eq!(
        state(&registry, &receipt.evaluation_id).await,
        EvidenceState::Forbidden
    );
    archive
        .review("fti/conversation", turn, "reviewer", &review("approved"))
        .await
        .unwrap();
    assert_eq!(
        state(&registry, &receipt.evaluation_id).await,
        EvidenceState::Complete
    );
    assert_eq!(
        publish(&registry, f.request.clone(), "admin", now() + 60)
            .await
            .unwrap(),
        receipt
    );
    assert_eq!(
        registry
            .get(&receipt.evaluation_id, "admin", receipt.expires_at)
            .await
            .unwrap()
            .unwrap()
            .state,
        EvidenceState::Expired
    );
    assert!(content_keys(&f.store).await.is_empty());
}

#[tokio::test]
async fn conversation_integrity_and_key_loss_hide_without_false_erasure() {
    let mut f = Fixture::new("conversation-integrity").await;
    let archive = owner(f.store.clone());
    pin(&mut f, &archive).await;
    let registry = registry(&f, archive.clone());
    let receipt = publish(&registry, f.request.clone(), "admin", now())
        .await
        .unwrap();
    let lost_key = registry
        .clone()
        .with_cipher(Arc::new(ConversationCipher(Keyring::single(
            "other", [8; 32],
        ))));
    assert_eq!(
        state(&lost_key, &receipt.evaluation_id).await,
        EvidenceState::Forbidden
    );
    assert_eq!(lost_key.sweep("retention-worker", now()).await.unwrap(), 0);
    let keys = content_keys(&f.store).await;
    for key in &keys {
        let saved = f.store.get(key).await.unwrap().unwrap();
        let mut envelope: Value = serde_json::from_slice(&saved).unwrap();
        envelope["evaluation_sealed_v1"]["ciphertext"] = json!("broken");
        f.store
            .put(key, serde_json::to_vec(&envelope).unwrap())
            .await
            .unwrap();
        assert_eq!(
            state(&registry, &receipt.evaluation_id).await,
            EvidenceState::CorruptArtifact
        );
        f.store.put(key, saved).await.unwrap();
    }
    // A valid ciphertext from another path cannot be substituted.
    let saved = f.store.get(&keys[0]).await.unwrap().unwrap();
    f.store
        .put(&keys[0], f.store.get(&keys[1]).await.unwrap().unwrap())
        .await
        .unwrap();
    assert_eq!(
        state(&registry, &receipt.evaluation_id).await,
        EvidenceState::CorruptArtifact
    );
    f.store.put(&keys[0], saved).await.unwrap();
    let manifest = archive
        .export(
            "fti/evaluation",
            &f.request.manifest.context.dataset.version,
        )
        .await
        .unwrap();
    let source_key = f
        .store
        .list("conversations/")
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.key.contains("/exports/shards/") && e.key.contains(&manifest.job_id))
        .unwrap()
        .key;
    let saved = f.store.get(&source_key).await.unwrap().unwrap();
    f.store.put(&source_key, b"{}".to_vec()).await.unwrap();
    assert_eq!(
        state(&registry, &receipt.evaluation_id).await,
        EvidenceState::CorruptArtifact
    );
    f.store.put(&source_key, saved).await.unwrap();
    assert_eq!(
        state(&registry, &receipt.evaluation_id).await,
        EvidenceState::Complete
    );
    f.store.delete(&source_key).await.unwrap();
    assert_eq!(registry.sweep("retention-worker", now()).await.unwrap(), 1);
    assert!(content_keys(&f.store).await.is_empty());
}

#[tokio::test]
async fn conversation_publication_requires_cipher_exact_cohort_and_native_policy() {
    let mut f = Fixture::new("conversation-refusal").await;
    let archive = owner(f.store.clone());
    pin(&mut f, &archive).await;
    let unsealed = Registry::new(
        f.store.clone(),
        Arc::new(f.source().with_conversations(archive.clone())),
        RegistryConfig::default(),
    )
    .unwrap()
    .with_content_access(true);
    assert!(matches!(
        publish(&unsealed, f.request.clone(), "admin", now()).await,
        Err(EvaluationError::Unavailable(EvidenceState::Forbidden))
    ));
    for split in ["train", "validation", "all"] {
        let mut request = f.request.clone();
        request.manifest.context.split = split.into();
        f.approve(&request.manifest).await;
        assert!(
            registry(&f, archive.clone())
                .publish(request, "admin", now())
                .await
                .is_err()
        );
    }
    f.approve(&f.request.manifest).await;
    let mut cases: Value =
        serde_json::from_slice(&tokio::fs::read(f.root.join("cases.json")).await.unwrap()).unwrap();
    cases["cases"][0]["input_digest"] = json!("0".repeat(64));
    let bytes = serde_json::to_vec(&cases).unwrap();
    tokio::fs::write(f.root.join("cases.json"), &bytes)
        .await
        .unwrap();
    f.request.manifest.context.case_manifest.digest = hex::encode(Sha256::digest(&bytes));
    f.request.manifest.context.case_manifest.size_bytes = Some(bytes.len() as u64);
    f.approve(&f.request.manifest).await;
    assert!(matches!(
        registry(&f, archive.clone())
            .publish(f.request.clone(), "admin", now())
            .await,
        Err(EvaluationError::Unavailable(EvidenceState::CorruptArtifact))
    ));
    assert!(f.store.list("evaluations/").await.unwrap().is_empty());
    for alias in ["latest", "../head", &"AB".repeat(32)] {
        assert!(
            archive
                .verified_evaluation_rows("fti/evaluation", alias)
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn conversation_http_uses_admin_for_publication_detail_pages_list_and_legacy() {
    use aiwatcher_auth::{AuthConfig, AuthMode};
    use aiwatcher_server::config::{BackendKind, Config, PromptStoreKind};
    let mut f = Fixture::new("conversation-http").await;
    let runtime = aiwatcher_server::wiring::build(Config {
        bus: BackendKind::Memory,
        prompt_store: PromptStoreKind::Memory,
        data_dir: f.root.join("runtime").to_str().unwrap().into(),
        evaluation_source_dir: Some(f.root.to_str().unwrap().into()),
        conversation_archive: true,
        conversation_keys: Some(
            "evaluation-test:FxcXFxcXFxcXFxcXFxcXFxcXFxcXFxcXFxcXFxcXFxc".into(),
        ),
        auth: AuthConfig {
            mode: AuthMode::Proxy,
            ..AuthConfig::default()
        },
        ..Config::default()
    })
    .await
    .unwrap();
    let archive = runtime.state.conversations.as_ref().unwrap();
    pin(&mut f, archive).await;
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
    for group in ["aiwatcher-viewers", "aiwatcher-editors"] {
        let response = client
            .post(&endpoint)
            .header("x-authentik-username", "admin")
            .header("x-authentik-groups", group)
            .json(&f.request)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 403);
    }
    admit(&client, &base, &f.request.manifest).await;
    let response = client
        .post(&endpoint)
        .header("x-authentik-username", "reader")
        .header("x-authentik-groups", "aiwatcher-admins")
        .json(&f.request)
        .send()
        .await
        .unwrap();
    assert!(
        response.status().is_success(),
        "{}",
        response.text().await.unwrap()
    );
    let receipt: EvaluationReceipt = response.json().await.unwrap();
    let detail = format!("{endpoint}/{}", receipt.evaluation_id);
    let cases = format!("{detail}/cases?version={}", receipt.version);
    let legacy = format!("{base}/api/v1/evaluations/{}", receipt.evaluation_id);
    assert_eq!(client.get(&cases).send().await.unwrap().status(), 401);
    for group in ["aiwatcher-viewers", "aiwatcher-editors"] {
        for url in [&endpoint, &detail, &cases] {
            let response = client
                .get(url)
                .header("x-authentik-username", "admin")
                .header("x-authentik-groups", group)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            let body: Value = response.json().await.unwrap();
            assert!(!body.to_string().contains("SYNTHETIC_PRIVATE"));
            let result = if url == &endpoint {
                &body["evaluations"][0]
            } else {
                &body
            };
            assert_eq!(result["state"], "forbidden");
            assert!(result.get("manifest").is_none_or(Value::is_null));
        }
        assert_eq!(
            client
                .get(&legacy)
                .header("x-authentik-username", "admin")
                .header("x-authentik-groups", group)
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
    }
    for url in [&detail, &cases, &legacy] {
        let response = client
            .get(url)
            .header("x-authentik-username", "reader")
            .header("x-authentik-groups", "aiwatcher-admins")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "{url}");
    }
    archive
        .erase_conversation("fti/conversation", "admin")
        .await
        .unwrap();
    // The same capability explicitly held by the real retention worker.
    runtime
        .state
        .evaluations
        .as_ref()
        .unwrap()
        .as_ref()
        .clone()
        .with_content_access(true)
        .sweep("retention-worker", now())
        .await
        .unwrap();
    let response = client
        .get(&cases)
        .header("x-authentik-username", "viewer")
        .send()
        .await
        .unwrap();
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["state"], "deleted_source");
    assert_eq!(body["cases"], json!([]));
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

#[tokio::test]
async fn conversation_owner_checks_shard_identity_current_consent_and_shorter_retention() {
    let mut f = Fixture::new("conversation-owner").await;
    let archive = owner(f.store.clone());
    let rows = pin(&mut f, &archive).await;
    let version = f.request.manifest.context.dataset.version.clone();
    let registry = registry(&f, archive.clone());
    let receipt = publish(&registry, f.request.clone(), "admin", now())
        .await
        .unwrap();
    let inventory = f.store.list("conversations/").await.unwrap();
    let manifest_key = &inventory
        .iter()
        .find(|e| e.key.contains("/exports/manifests/"))
        .unwrap()
        .key;
    let original = f.store.get(manifest_key).await.unwrap().unwrap();
    for pointer in ["/counts/rows", "/request/description", "/shards/0/digest"] {
        let mut manifest: Value = serde_json::from_slice(&original).unwrap();
        *manifest.pointer_mut(pointer).unwrap() = if pointer == "/counts/rows" {
            json!(999)
        } else {
            json!("tampered")
        };
        f.store
            .put(manifest_key, serde_json::to_vec(&manifest).unwrap())
            .await
            .unwrap();
        assert_eq!(
            state(&registry, &receipt.evaluation_id).await,
            EvidenceState::CorruptArtifact
        );
        f.store.put(manifest_key, original.clone()).await.unwrap();
    }
    let shard_key = &inventory
        .iter()
        .find(|e| e.key.contains("/exports/shards/"))
        .unwrap()
        .key;
    let original_shard = f.store.get(shard_key).await.unwrap().unwrap();
    let resealed = keys().seal(shard_key, b"{}\n").unwrap();
    f.store
        .put(shard_key, serde_json::to_vec(&resealed).unwrap())
        .await
        .unwrap();
    assert_eq!(
        state(&registry, &receipt.evaluation_id).await,
        EvidenceState::CorruptArtifact
    );
    f.store.put(shard_key, original_shard).await.unwrap();
    let id = rows[0]["eligibility"][0]["turn_id"].as_str().unwrap();
    let turn_key = &inventory
        .iter()
        .find(|e| e.key.contains("/turns/") && e.key.contains(id))
        .unwrap()
        .key;
    let original_turn = f.store.get(turn_key).await.unwrap().unwrap();
    for (pointer, value) in [
        ("/policy/consent/scope", json!(["train"])),
        ("/policy/consent/basis", json!("unknown")),
        ("/policy/consent/reference", json!("")),
    ] {
        let mut turn: Value = serde_json::from_slice(&original_turn).unwrap();
        *turn.pointer_mut(pointer).unwrap() = value;
        f.store
            .put(turn_key, serde_json::to_vec(&turn).unwrap())
            .await
            .unwrap();
        assert_eq!(
            state(&registry, &receipt.evaluation_id).await,
            EvidenceState::Forbidden
        );
        assert_eq!(registry.sweep("retention-worker", now()).await.unwrap(), 0);
        f.store.put(turn_key, original_turn.clone()).await.unwrap();
    }
    // The source's derived export index is not the immutable version itself.
    for entry in &inventory {
        if entry.key.contains("/exports/index/") {
            f.store.delete(&entry.key).await.unwrap();
        }
    }
    assert_eq!(
        archive
            .verified_evaluation_rows("fti/evaluation", &version)
            .await
            .unwrap()
            .rows
            .len(),
        2
    );
    let shortened = Arc::new(Conversations::new(
        f.store.clone(),
        "conversations",
        keys(),
        ArchivePolicy {
            max_ttl_days: 0,
            ..ArchivePolicy::default()
        },
    ));
    let shortened_registry = self::registry(&f, shortened);
    assert_eq!(
        state(&shortened_registry, &receipt.evaluation_id).await,
        EvidenceState::Expired
    );
    assert!(content_keys(&f.store).await.is_empty());
}

#[tokio::test]
async fn sealed_content_rejects_plaintext_downgrades_and_concurrent_retries_keep_one_claim() {
    let store: Arc<dyn ObjectStore> = Arc::new(MemoryObjectStore::new());
    let registry = super::registry(store.clone(), Arc::new(Source::default()))
        .with_cipher(Arc::new(ConversationCipher(keys())))
        .with_content_access(true);
    let mut request = request("sealed-race", 401);
    request.manifest.context.dataset.kind = DatasetKind::Conversations;
    request.manifest.variant.dataset = request.manifest.context.dataset.clone();
    let (left, right) = tokio::join!(
        publish(&registry, request.clone(), "admin", 100),
        publish(&registry, request.clone(), "admin", 101)
    );
    let receipt = left.unwrap();
    assert_eq!(receipt, right.unwrap());
    for key in content_keys(&store).await {
        let original = store.get(&key).await.unwrap().unwrap();
        let envelope: Value = serde_json::from_slice(&original).unwrap();
        let plain = keys()
            .open(
                &key,
                &serde_json::from_value(envelope["evaluation_sealed_v1"].clone()).unwrap(),
            )
            .unwrap();
        store.put(&key, plain).await.unwrap();
        assert_eq!(
            evidence(&registry, &receipt, "admin", 102).await,
            EvidenceState::CorruptArtifact,
            "a plaintext downgrade is never an accepted fallback"
        );
        store.put(&key, original).await.unwrap();
    }
    request.cases[0].actual = Some(json!({"answer":"losing sealed content"}));
    assert!(matches!(
        publish(&registry, request, "admin", 103).await,
        Err(EvaluationError::Conflict)
    ));
    assert!(registry.collect_orphans(104).await.unwrap() > 0);
    let first = registry
        .cases(
            &receipt.evaluation_id,
            &receipt.version,
            None,
            Some(200),
            "admin",
            105,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.cases.len(), 200);
    assert!(first.next_cursor.is_some());
    assert_eq!(
        registry
            .get(&receipt.evaluation_id, "admin", 106)
            .await
            .unwrap()
            .unwrap()
            .state,
        EvidenceState::Complete
    );
}
