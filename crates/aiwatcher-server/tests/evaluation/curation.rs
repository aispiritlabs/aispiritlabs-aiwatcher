//! A native source is verified by its owner on publication and every read.
use super::fixture::Fixture;
use super::*;
use aiwatcher_server::evaluation::LocalSource;
use serde_json::{Value, json};

#[tokio::test]
async fn curation_evidence_survives_restart_and_head_moves_but_not_source_deletion() {
    let fixture = Fixture::new("lifecycle").await;
    let first = fixture.registry();
    let receipt = publish(&first, fixture.request.clone(), "editor", 100)
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
        publish(&restarted, fixture.request.clone(), "editor", 200)
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
        publish(&fixture.registry(), request, "editor", 100).await,
        Err(EvaluationError::Unavailable(EvidenceState::CorruptArtifact))
    ));
    assert!(fixture.store.list("evaluations/").await.unwrap().is_empty());
}

#[tokio::test]
async fn damaged_native_bytes_hide_published_evidence_without_becoming_a_false_deletion() {
    let fixture = Fixture::new("corruption").await;
    let registry = fixture.registry();
    let id = &fixture.request.manifest.origin.evaluation_id;
    let receipt = publish(&registry, fixture.request.clone(), "editor", 100)
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
    admit(&client, &base, &fixture.request.manifest).await;
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

#[tokio::test]
async fn a_cohort_derived_from_a_curation_version_is_admitted_and_measured_with_nothing_staged() {
    use aiwatcher_execution::ActivityExecutor;
    // The case manifest used to be written by hand, or copied from a result
    // already published, and staged beside the declaration — a second copy
    // of cases the adapter derives from the owner anyway.
    let fixture = Fixture::new("derived").await;
    let registry = Arc::new(fixture.registry());
    let dataset = fixture.request.manifest.context.dataset.clone();
    let request = CohortRequest {
        dataset: dataset.clone(),
        split: "test".into(),
        limit: Some(2),
    };
    let derived = registry.derive_cohort(&request, "ada", 100).await.unwrap();
    assert_eq!(derived.cohort.case_count, 2, "the owner's first two");
    assert_eq!(derived.available, 3);
    let again = registry.derive_cohort(&request, "bob", 101).await.unwrap();
    assert_eq!(
        again.cohort, derived.cohort,
        "the same cases, the same pins"
    );
    assert_eq!(
        again.derived_by, "ada",
        "and the first derivation is the one kept"
    );
    let whole = registry
        .derive_cohort(
            &CohortRequest {
                limit: None,
                ..request.clone()
            },
            "ada",
            102,
        )
        .await
        .unwrap();
    assert_eq!(whole.cohort.case_count, 3);
    assert_ne!(
        whole.cohort.case_manifest.digest,
        derived.cohort.case_manifest.digest
    );

    let card = Scorecard {
        name: "derived-answers".into(),
        description: String::new(),
        scorers: vec![ScorerSpec {
            metric: "exact".into(),
            answer_path: "/answer".into(),
            expected_path: "/answer".into(),
            input_path: None,
            scorer: Scorer::ExactMatch {
                ignore_case: false,
                trim: true,
            },
        }],
    };
    let version = registry
        .publish_scorecard(&card, "ada", 103)
        .await
        .unwrap()
        .version;
    let cases: Value = serde_json::from_slice(
        &tokio::fs::read(fixture.root.join("cases.json"))
            .await
            .unwrap(),
    )
    .unwrap();
    let answers: Vec<RecordedAnswer> = cases["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| RecordedAnswer {
            case_id: case["case_id"].as_str().unwrap().into(),
            answer: case["expected"].clone(),
            run_id: None,
            trace_id: None,
            span_id: None,
            usage: None,
        })
        .collect();
    let recording = registry
        .stage_recording(
            "answers.json",
            serde_json::to_vec(&json!({ "answers": answers })).unwrap(),
        )
        .await
        .unwrap();
    let run = ScoringRun {
        evaluation_id: "derived-two".into(),
        repetition_id: "measurement-1".into(),
        variant: fixture.request.manifest.variant.clone(),
        cohort: derived.cohort.clone(),
        scorecard: VersionReference {
            name: card.name.clone(),
            version,
        },
        answers: Answers::Recording(recording),
        judge: None,
        external_calibration: None,
        settings: Default::default(),
    };
    let declared = registry
        .declare_scoring_run(&run, "ada", 104)
        .await
        .unwrap();
    let view = registry
        .scoring_run_view(&declared.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        view.cohort.as_ref().map(|cohort| &cohort.request),
        Some(&request),
        "the declaration says where its cohort came from"
    );

    // The bundle on this host holds the contract fixture's own cases.json: a
    // staged member that does not hash to the pin is refused, never replaced.
    fixture.approve(&view.manifest).await;
    assert!(matches!(
        registry.approve(&view.manifest, "operator", 105).await,
        Err(EvaluationError::Unavailable(EvidenceState::CorruptArtifact))
    ));
    for name in [
        COHORT_CASES,
        COHORT_INPUT_SCHEMA,
        COHORT_EXPECTATIONS_SCHEMA,
    ] {
        tokio::fs::remove_file(fixture.root.join(name))
            .await
            .unwrap();
    }
    registry
        .approve(&view.manifest, "operator", 106)
        .await
        .expect("nothing staged, so the adapter derives the three and they match");

    let (command, context) = attempt(&declared.id, "derived-two");
    let result = aiwatcher_server::execution::scoring::ScoreExecutor::new(Arc::clone(&registry))
        .execute(&command, &context)
        .await
        .expect("the step scores the owner's first two cases");
    let reported = result.result.unwrap();
    assert_eq!(reported["selected"], 2, "{reported}");
    assert_eq!(
        reported["unscored"], 0,
        "an answer to the third is not part of it"
    );
    let evidence = registry
        .get("derived-two", "viewer", 107)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(evidence.state, EvidenceState::Complete);
    assert_eq!(evidence.metrics["exact"], 1.0);

    // A cohort of the first two is not a prefix of some other version.
    let mut changed = fixture.rows.clone();
    changed.items[0].insert("expected".into(), json!({"answer": "moved"}));
    let other = fixture
        .datasets
        .publish(changed)
        .await
        .unwrap()
        .dataset
        .latest
        .version;
    let mut elsewhere = view.manifest.clone();
    elsewhere.context.dataset.version = other.clone();
    elsewhere.variant.dataset.version = other;
    fixture.approve(&elsewhere).await;
    assert!(matches!(
        registry.approve(&elsewhere, "operator", 108).await,
        Err(EvaluationError::Unavailable(EvidenceState::DeletedSource))
    ));
}

#[tokio::test]
async fn an_external_or_unowned_dataset_derives_no_cohort_and_says_why() {
    let fixture = Fixture::new("underivable").await;
    let registry = fixture.registry();
    let mut external = fixture.request.manifest.context.dataset.clone();
    external.kind = DatasetKind::External;
    let refused = registry
        .derive_cohort(
            &CohortRequest {
                dataset: external,
                split: "test".into(),
                limit: None,
            },
            "ada",
            100,
        )
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("owns"), "{refused}");

    let mut conversations = fixture.request.manifest.context.dataset.clone();
    conversations.kind = DatasetKind::Conversations;
    assert!(matches!(
        registry
            .derive_cohort(
                &CohortRequest {
                    dataset: conversations,
                    split: "test".into(),
                    limit: None,
                },
                "ada",
                100,
            )
            .await,
        Err(EvaluationError::Unavailable(EvidenceState::Forbidden))
    ));
}
