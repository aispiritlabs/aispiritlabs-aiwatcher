use super::*;

async fn fixture() -> IamFixture {
    let mut f = IamFixture::new().await;
    f.fixture.state.training = Some(Arc::new(TrainingRegistry::new(
        Arc::new(MemoryObjectStore::new()),
        "training",
    )));
    f
}
fn run(dataset: &str) -> Value {
    json!({"run_id":"same-run","model":"same-model","dataset":dataset,"framework":"test","workflow_run_id":"original-workflow"})
}
fn model() -> Value {
    json!({"name":"same-model","run_id":"same-run","checkpoint_uri":"s3://original/weights","metrics":{"test":{"quality":0.8}}})
}
fn reads() -> [&'static str; 4] {
    [
        "training-runs",
        "training-runs/same-run",
        "models",
        "models/same-model",
    ]
}
fn writes(pin: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("training-runs", run("data@abcd")),
        (
            "training-runs/same-run/progress",
            json!({"epochs":[{"epoch":0,"duration_ms":12.0,"steps":2,"metrics":{"loss":0.25}}]}),
        ),
        (
            "training-runs/same-run/finish",
            json!({"status":"succeeded"}),
        ),
        ("models", model()),
        (
            "models/same-model/labels",
            json!({"label":"production","version":pin}),
        ),
    ]
}
async fn seed(f: &IamFixture, cookie: &str, root: &str, dataset: &str) -> String {
    let (status, body) = f
        .request(
            "POST",
            &format!("{root}/training-runs"),
            Some(cookie),
            run(dataset),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, body) = f
        .request(
            "POST",
            &format!("{root}/models"),
            Some(cookie),
            model(),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["version"]["version"].as_str().unwrap().into()
}

#[tokio::test]
async fn project_training_isolates_runs_models_provenance_and_all_route_families() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let outsider = f.cookie("outsider", Role::Admin);
    let org = f.create(&owner).await;
    let other_org = f.create(&outsider).await;
    let a = project(&f, &owner, &org).await;
    let b = project(&f, &owner, &org).await;
    let c = project(&f, &outsider, &other_org).await;
    let roots = [
        base(&org, &a),
        base(&org, &b),
        base(&other_org, &c),
        "/api/v1".into(),
    ];
    let mut pins = Vec::new();
    for (root, cookie, dataset) in [
        (&roots[0], &owner, "a@aaaa"),
        (&roots[1], &owner, "b@bbbb"),
        (&roots[2], &outsider, "c@cccc"),
        (&roots[3], &owner, "legacy@dddd"),
    ] {
        pins.push(seed(&f, cookie, root, dataset).await);
        let (status, page) = f
            .request(
                "GET",
                &format!(
                    "{root}/training-runs?model=same-model&status=running&dataset={dataset}&limit=1"
                ),
                Some(cookie),
                Value::Null,
                false,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{page}");
        assert_eq!(page["total"], 1);
        assert_eq!(page["runs"][0]["dataset"], dataset);
        let (_, page) = f
            .request(
                "GET",
                &format!("{root}/models"),
                Some(cookie),
                Value::Null,
                false,
            )
            .await;
        assert_eq!(page["models"].as_array().unwrap().len(), 1);
        let (_, detail) = f
            .request(
                "GET",
                &format!("{root}/models/same-model"),
                Some(cookie),
                Value::Null,
                false,
            )
            .await;
        assert_eq!(detail["current"]["dataset"], dataset);
    }
    for (root, cookie) in [
        (&roots[1], &owner),
        (&roots[2], &outsider),
        (&roots[3], &owner),
    ] {
        let (status, detail) = f
            .request(
                "GET",
                &format!("{root}/models/same-model?version={}", pins[0]),
                Some(cookie),
                Value::Null,
                false,
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            detail["current"].is_null(),
            "known foreign version: {detail}"
        );
        assert_eq!(
            f.request(
                "POST",
                &format!("{root}/models/same-model/labels"),
                Some(cookie),
                json!({"label":"production","version":pins[0]}),
                true
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }
    let mut only_a = run("private@abcd");
    only_a["run_id"] = json!("only-a");
    assert_eq!(
        f.request(
            "POST",
            &format!("{}/training-runs", roots[0]),
            Some(&owner),
            only_a,
            true
        )
        .await
        .0,
        StatusCode::CREATED
    );
    let mut foreign_model = model();
    foreign_model["run_id"] = json!("only-a");
    assert_eq!(
        f.request(
            "POST",
            &format!("{}/models", roots[1]),
            Some(&owner),
            foreign_model,
            true
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    for root in [&roots[0], &base(&other_org, &a)] {
        for route in reads() {
            assert_eq!(
                f.request(
                    "GET",
                    &format!("{root}/{route}"),
                    Some(&outsider),
                    Value::Null,
                    false
                )
                .await
                .0,
                StatusCode::NOT_FOUND
            );
        }
        for (route, body) in writes(&pins[0]) {
            assert_eq!(
                f.request(
                    "POST",
                    &format!("{root}/{route}"),
                    Some(&outsider),
                    body,
                    true
                )
                .await
                .0,
                StatusCode::NOT_FOUND
            );
        }
    }
    for (route, body) in writes(&pins[0]) {
        let (status, result) = f
            .request(
                "POST",
                &format!("{}/{route}", roots[0]),
                Some(&owner),
                body,
                true,
            )
            .await;
        assert!(status.is_success(), "{route}: {result}");
    }
    let (_, run_b) = f
        .request(
            "GET",
            &format!("{}/training-runs/same-run", roots[1]),
            Some(&owner),
            Value::Null,
            false,
        )
        .await;
    assert_eq!(run_b["status"], "running");
    assert!(run_b["epochs"].as_array().unwrap().is_empty());
    let (_, model_b) = f
        .request(
            "GET",
            &format!("{}/models/same-model", roots[1]),
            Some(&owner),
            Value::Null,
            false,
        )
        .await;
    let detail: aiwatcher_training::ModelDetail = serde_json::from_value(model_b).unwrap();
    assert!(detail.head.labels.is_empty());
}

#[tokio::test]
async fn project_model_promotion_requires_project_admin_and_headers_without_instance_bypass() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let member = f.cookie("member", Role::Viewer);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    let pin = seed(&f, &owner, &root, "data@abcd").await;
    grant(&f, &owner, &org, &p, "viewer", json!({"valid_from":0})).await;
    for (route, body) in writes(&pin) {
        for cookie in [&member, &f.cookie("member", Role::Admin)] {
            assert_eq!(
                f.request(
                    "POST",
                    &format!("{root}/{route}"),
                    Some(cookie),
                    body.clone(),
                    true
                )
                .await
                .0,
                StatusCode::FORBIDDEN
            );
        }
        assert_eq!(
            f.request(
                "POST",
                &format!("{root}/{route}"),
                Some(&owner),
                body,
                false
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
    }
    grant(&f, &owner, &org, &p, "editor", json!({"valid_from":0})).await;
    for (route, body) in writes(&pin) {
        let response = f
            .fixture
            .router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("{root}/{route}"))
                    .header(header::COOKIE, &member)
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("x-aiwatcher-iam", "1")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        if route.ends_with("labels") {
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        } else {
            assert!(
                response.status().is_success(),
                "{route}: {}",
                response.status()
            );
        }
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
    grant(&f, &owner, &org, &p, "admin", json!({"valid_from":0})).await;
    assert_eq!(
        f.request(
            "POST",
            &format!("{root}/models/same-model/labels"),
            Some(&member),
            json!({"label":"production","version":pin}),
            true
        )
        .await
        .0,
        StatusCode::OK
    );
    for route in reads() {
        let response = f
            .fixture
            .router()
            .oneshot(
                Request::builder()
                    .uri(format!("{root}/{route}"))
                    .header(header::COOKIE, &member)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{route}");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
    // Project admin cannot bypass the existing evidence gate.
    let mut unmeasured = model();
    unmeasured["metrics"] = json!({});
    let (_, saved) = f
        .request(
            "POST",
            &format!("{root}/models"),
            Some(&member),
            unmeasured,
            true,
        )
        .await;
    assert_eq!(
        f.request(
            "POST",
            &format!("{root}/models/same-model/labels"),
            Some(&member),
            json!({"label":"production","version":saved["version"]["version"]}),
            true
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    // Owning the org does not imply access after the explicit creator grant is removed.
    let access = f
        .store
        .access(
            aiwatcher_iam::ProjectScope {
                organization: serde_json::from_value(json!(org)).unwrap(),
                project: serde_json::from_value(json!(p)).unwrap(),
            },
            &Principal {
                provider: f.issuer.clone(),
                subject: "owner".into(),
            },
        )
        .await
        .unwrap();
    for g in access.grants {
        command(
            &f,
            &owner,
            &org,
            json!({"type":"revoke_grant","project":p,"grant":g.grant.id}),
        )
        .await;
    }
    assert_eq!(
        f.request(
            "GET",
            &format!("{root}/models"),
            Some(&owner),
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn project_training_rechecks_grants_and_preserves_independent_access() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let member = f.cookie("member", Role::Viewer);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    let pin = seed(&f, &owner, &root, "data@abcd").await;
    let temporary = grant(
        &f,
        &owner,
        &org,
        &p,
        "admin",
        json!({"valid_from":1001,"edit_until":1003,"read_until":1005}),
    )
    .await;
    for (clock, read_status, write_status) in [
        (1000, StatusCode::NOT_FOUND, StatusCode::NOT_FOUND),
        (1001, StatusCode::OK, StatusCode::OK),
        (1003, StatusCode::OK, StatusCode::FORBIDDEN),
        (1005, StatusCode::NOT_FOUND, StatusCode::NOT_FOUND),
    ] {
        f.clock.0.store(clock, std::sync::atomic::Ordering::SeqCst);
        for route in reads() {
            assert_eq!(
                f.request(
                    "GET",
                    &format!("{root}/{route}"),
                    Some(&member),
                    Value::Null,
                    false
                )
                .await
                .0,
                read_status
            );
        }
        assert_eq!(
            f.request(
                "POST",
                &format!("{root}/models/same-model/labels"),
                Some(&member),
                json!({"label":"production","version":pin}),
                true
            )
            .await
            .0,
            write_status
        );
    }
    let permanent = grant(&f, &owner, &org, &p, "editor", json!({"valid_from":0})).await;
    command(
        &f,
        &owner,
        &org,
        json!({"type":"revoke_grant","project":p,"grant":temporary}),
    )
    .await;
    assert_eq!(
        f.request(
            "GET",
            &format!("{root}/models"),
            Some(&member),
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::OK
    );
    command(
        &f,
        &owner,
        &org,
        json!({"type":"revoke_grant","project":p,"grant":permanent}),
    )
    .await;
    for (route, body) in writes(&pin) {
        assert_eq!(
            f.request(
                "POST",
                &format!("{root}/{route}"),
                Some(&member),
                body,
                true
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }
}

#[tokio::test]
async fn delayed_training_uploads_recheck_the_original_role_including_admin() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let member = f.cookie("member", Role::Viewer);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    let pin = seed(&f, &owner, &root, "data@abcd").await;
    grant(
        &f,
        &owner,
        &org,
        &p,
        "admin",
        json!({"valid_from":0,"edit_until":1001,"read_until":1002}),
    )
    .await;
    let (_, before) = f
        .request(
            "GET",
            &format!("{root}/training-runs/same-run"),
            Some(&owner),
            Value::Null,
            false,
        )
        .await;
    for (route, body) in writes(&pin) {
        f.clock.0.store(1000, std::sync::atomic::Ordering::SeqCst);
        let clock = f.clock.clone();
        let stream = Body::from_stream(futures::stream::once(async move {
            clock.0.store(1001, std::sync::atomic::Ordering::SeqCst);
            Ok::<_, std::io::Error>(body.to_string())
        }));
        let (status, response) = f
            .fixture
            .request(
                Request::builder()
                    .method("POST")
                    .uri(format!("{root}/{route}"))
                    .header(header::COOKIE, &member)
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("x-aiwatcher-iam", "1")
                    .body(stream)
                    .unwrap(),
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{route}: {response}");
    }
    // Editor access surviving expiry of admin must still not permit a label write.
    grant(&f, &owner, &org, &p, "editor", json!({"valid_from":0})).await;
    f.clock.0.store(1000, std::sync::atomic::Ordering::SeqCst);
    let clock = f.clock.clone();
    let stream = Body::from_stream(futures::stream::once(async move {
        clock.0.store(1001, std::sync::atomic::Ordering::SeqCst);
        Ok::<_, std::io::Error>(json!({"label":"production","version":pin}).to_string())
    }));
    let (status, _) = f
        .fixture
        .request(
            Request::builder()
                .method("POST")
                .uri(format!("{root}/models/same-model/labels"))
                .header(header::COOKIE, &member)
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-aiwatcher-iam", "1")
                .body(stream)
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (_, after) = f
        .request(
            "GET",
            &format!("{root}/training-runs/same-run"),
            Some(&owner),
            Value::Null,
            false,
        )
        .await;
    assert_eq!(before, after);
    let (_, model) = f
        .request(
            "GET",
            &format!("{root}/models/same-model"),
            Some(&owner),
            Value::Null,
            false,
        )
        .await;
    let detail: aiwatcher_training::ModelDetail = serde_json::from_value(model).unwrap();
    assert!(detail.head.labels.is_empty());
}

#[tokio::test]
async fn project_training_fails_closed_and_legacy_routes_cannot_reach_project_keys() {
    let mut f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    let pin = seed(&f, &owner, &root, "data@abcd").await;
    seed(&f, &owner, "/api/v1", "legacy@eeee").await;
    for path in [&root, &"/api/v1".into()] {
        assert_eq!(
            f.request(
                "GET",
                &format!("{path}/models/same-model?version=..%2F..%2Fscopes%2Fsecret"),
                Some(&owner),
                Value::Null,
                false
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            f.request(
                "POST",
                &format!("{path}/models/same-model/labels"),
                Some(&owner),
                json!({"label":"production","version":"../../scopes/secret"}),
                true
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        f.request("GET", &format!("{root}/models"), None, Value::Null, false)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.request(
            "GET",
            "/api/v1/orgs/invalid/projects/invalid/models",
            Some(&owner),
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let registry = f.fixture.state.training.take();
    assert_eq!(
        f.request(
            "GET",
            &format!("{root}/models"),
            Some(&owner),
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::NOT_IMPLEMENTED
    );
    f.fixture.state.training = registry;
    f.fixture.state.auth = None;
    assert_eq!(
        f.request("GET", &format!("{root}/models"), None, Value::Null, false)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    f.fixture.state.iam = None;
    assert_eq!(
        f.request("GET", &format!("{root}/models"), None, Value::Null, false)
            .await
            .0,
        StatusCode::NOT_IMPLEMENTED
    );
    let (status, detail) = f
        .request(
            "GET",
            &format!("/api/v1/models/same-model?version={pin}"),
            None,
            Value::Null,
            false,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(detail["current"].is_null());
    let (_, page) = f
        .request("GET", "/api/v1/training-runs", None, Value::Null, false)
        .await;
    assert_eq!(page["total"], 1);
    assert_eq!(page["runs"][0]["dataset"], "legacy@eeee");
}
