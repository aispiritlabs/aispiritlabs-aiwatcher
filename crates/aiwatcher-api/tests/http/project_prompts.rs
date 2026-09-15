use super::*;

async fn fixture() -> IamFixture {
    let mut f = IamFixture::new().await;
    f.fixture.state.prompts = Some(Arc::new(Registry::new(
        Arc::new(MemoryObjectStore::new()),
        RegistryConfig::default(),
    )));
    f
}
fn prompt(text: &str) -> Value {
    json!({"name":"shared.prompt","text":text,"label":"production","description":"Scoped description","tags":["private"]})
}
fn optimization(baseline: &str) -> Value {
    json!({"optimization_id":"same-optimization","algorithm":"test","baseline":baseline,
        "candidate_text":"Improved {{question}}", "primary_metric":"quality",
        "test":[{"metric":"quality","baseline":0.5,"candidate":0.7}],
        "dataset":"cases@unchanged", "evaluation_id":"report-original", "promote":true,
        "report":{"private":"evidence"}})
}
async fn publish(f: &IamFixture, cookie: &str, root: &str, text: &str) -> String {
    let (status, body) = f
        .request(
            "POST",
            &format!("{root}/prompts"),
            Some(cookie),
            prompt(text),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["version"]["version_id"].as_str().unwrap().into()
}
fn reads(pin: &str) -> Vec<String> {
    vec![
        "prompts".into(),
        "prompts/shared.prompt".into(),
        format!("prompts/shared.prompt/versions/{pin}"),
        "prompts/shared.prompt/optimizations/same-optimization".into(),
    ]
}
fn writes(pin: &str) -> Vec<(&'static str, &'static str, Value)> {
    vec![
        ("POST", "prompts", prompt("unauthorized overwrite")),
        (
            "PUT",
            "prompts/shared.prompt/labels/production",
            json!({"version_id":pin}),
        ),
        (
            "POST",
            "prompts/shared.prompt/optimizations",
            optimization(pin),
        ),
        ("POST", "prompts/shared.prompt/rebuild", Value::Null),
    ]
}

#[tokio::test]
async fn project_prompts_isolate_all_operations_labels_reports_and_known_hashes() {
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
    for (root, cookie, text) in [
        (&roots[0], &owner, "A {{question}}"),
        (&roots[1], &owner, "B {{question}}"),
        (&roots[2], &outsider, "C {{question}}"),
        (&roots[3], &owner, "Legacy {{question}}"),
    ] {
        pins.push(publish(&f, cookie, root, text).await);
        let (status, page) = f
            .request(
                "GET",
                &format!("{root}/prompts?search=shared&tag=private&limit=1"),
                Some(cookie),
                Value::Null,
                false,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{page}");
        assert_eq!(page["total"], 1);
        assert_eq!(page["prompts"].as_array().unwrap().len(), 1);
        let (_, detail) = f
            .request(
                "GET",
                &format!("{root}/prompts/shared.prompt"),
                Some(cookie),
                Value::Null,
                false,
            )
            .await;
        assert_eq!(detail["current"]["text"], text);
    }
    let (status, record) = f
        .request(
            "POST",
            &format!("{}/prompts/shared.prompt/optimizations", roots[0]),
            Some(&owner),
            optimization(&pins[0]),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{record}");
    assert_eq!(record["dataset"], "cases@unchanged");
    assert_eq!(record["evaluation_id"], "report-original");
    let candidate = record["candidate"].as_str().unwrap();
    for route in reads(candidate) {
        assert_eq!(
            f.request(
                "GET",
                &format!("{}/{route}", roots[0]),
                Some(&owner),
                Value::Null,
                false
            )
            .await
            .0,
            StatusCode::OK,
            "{route}"
        );
    }
    for (root, cookie) in [
        (&roots[1], &owner),
        (&roots[2], &outsider),
        (&roots[3], &owner),
    ] {
        for route in [
            format!("prompts/shared.prompt/versions/{}", pins[0]),
            format!("prompts/shared.prompt/versions/{candidate}"),
            "prompts/shared.prompt/optimizations/same-optimization".into(),
        ] {
            assert_eq!(
                f.request(
                    "GET",
                    &format!("{root}/{route}"),
                    Some(cookie),
                    Value::Null,
                    false
                )
                .await
                .0,
                StatusCode::NOT_FOUND,
                "{root}/{route}"
            );
        }
        // Knowing another project's baseline or candidate cannot copy it into this one.
        for (method, route, body) in writes(&pins[0])
            .into_iter()
            .filter(|(_, route, _)| route.contains("labels") || route.contains("optimizations"))
        {
            assert_eq!(
                f.request(method, &format!("{root}/{route}"), Some(cookie), body, true)
                    .await
                    .0,
                StatusCode::NOT_FOUND
            );
        }
    }
    // Another organization's instance admin has no project permission. A mixed pair of IDs also fails.
    for root in [&roots[0], &base(&other_org, &a)] {
        for route in reads(candidate) {
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
        for (method, route, body) in writes(&pins[0]) {
            assert_eq!(
                f.request(
                    method,
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
    // Label movement, rebuild and optimization are real project writes.
    for (method, route, body) in [
        ("PUT", "labels/production", json!({"version_id":pins[0]})),
        ("POST", "rebuild", Value::Null),
    ] {
        let (status, head) = f
            .request(
                method,
                &format!("{}/prompts/shared.prompt/{route}", roots[0]),
                Some(&owner),
                body,
                true,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{head}");
        assert_eq!(head["labels"]["production"], pins[0]);
    }
    let (status, unchanged) = f
        .request(
            "GET",
            &format!("{}/prompts/shared.prompt", roots[1]),
            Some(&owner),
            Value::Null,
            false,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(unchanged["current"]["version_id"], pins[1]);
    assert_eq!(
        publish(&f, &owner, &roots[1], "A {{question}}").await,
        pins[0]
    );
    let (_, page) = f
        .request(
            "GET",
            &format!("{}/prompts?after=shared.prompt", roots[0]),
            Some(&owner),
            Value::Null,
            false,
        )
        .await;
    assert!(page["prompts"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn project_prompt_roles_headers_and_no_store_apply_to_every_operation() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let member = f.cookie("member", Role::Viewer);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    let pin = publish(&f, &owner, &root, "Baseline {{question}}").await;
    grant(&f, &owner, &org, &p, "viewer", json!({"valid_from":0})).await;
    // Every mutation rejects a viewer, even with instance admin authority.
    let instance_admin = f.cookie("member", Role::Admin);
    for (method, route, body) in writes(&pin) {
        for cookie in [&member, &instance_admin] {
            assert_eq!(
                f.request(
                    method,
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
                method,
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
    for (method, route, body) in writes(&pin) {
        let response = f
            .fixture
            .router()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(format!("{root}/{route}"))
                    .header(header::COOKIE, &member)
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("x-aiwatcher-iam", "1")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            response.status().is_success(),
            "{method} {route}: {}",
            response.status()
        );
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
    for route in reads(&pin) {
        for (cookie, expected) in [
            (&member, StatusCode::OK),
            (&f.cookie("outsider", Role::Admin), StatusCode::NOT_FOUND),
        ] {
            let response = f
                .fixture
                .router()
                .oneshot(
                    Request::builder()
                        .uri(format!("{root}/{route}"))
                        .header(header::COOKIE, cookie)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), expected, "{route}");
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        }
    }
    // Owning an organization alone does not authorize its data.
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
    for source in access.grants {
        command(
            &f,
            &owner,
            &org,
            json!({"type":"revoke_grant","project":p,"grant":source.grant.id}),
        )
        .await;
    }
    assert_eq!(
        f.request(
            "GET",
            &format!("{root}/prompts"),
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
async fn project_prompts_recheck_expiring_and_revoked_grants_in_the_same_session() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let member = f.cookie("member", Role::Viewer);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    let pin = publish(&f, &owner, &root, "Baseline {{question}}").await;
    f.request(
        "POST",
        &format!("{root}/prompts/shared.prompt/optimizations"),
        Some(&owner),
        optimization(&pin),
        true,
    )
    .await;
    let id = grant(
        &f,
        &owner,
        &org,
        &p,
        "editor",
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
        for route in reads(&pin) {
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
                &format!("{root}/prompts/shared.prompt/rebuild"),
                Some(&member),
                Value::Null,
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
        json!({"type":"revoke_grant","project":p,"grant":id}),
    )
    .await;
    assert_eq!(
        f.request(
            "GET",
            &format!("{root}/prompts"),
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
    for (method, route, body) in writes(&pin) {
        assert_eq!(
            f.request(
                method,
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
async fn project_prompt_json_writes_recheck_access_after_slow_uploads() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let member = f.cookie("member", Role::Viewer);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    let pin = publish(&f, &owner, &root, "Baseline {{question}}").await;
    grant(
        &f,
        &owner,
        &org,
        &p,
        "editor",
        json!({"valid_from":0,"edit_until":1001,"read_until":1002}),
    )
    .await;
    let (_, before) = f
        .request(
            "GET",
            &format!("{root}/prompts/shared.prompt"),
            Some(&owner),
            Value::Null,
            false,
        )
        .await;
    for (method, route, body) in writes(&pin)
        .into_iter()
        .filter(|(_, route, _)| !route.ends_with("rebuild"))
    {
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
                    .method(method)
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
    let (_, after) = f
        .request(
            "GET",
            &format!("{root}/prompts/shared.prompt"),
            Some(&owner),
            Value::Null,
            false,
        )
        .await;
    assert_eq!(before, after);
    assert_eq!(
        f.request(
            "GET",
            &format!("{root}/prompts/shared.prompt/optimizations/same-optimization"),
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
async fn project_prompts_fail_closed_and_disabling_iam_does_not_expose_project_data() {
    let mut f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    let pin = publish(&f, &owner, &root, "Project secret").await;
    assert_eq!(
        f.request("GET", &format!("{root}/prompts"), None, Value::Null, false)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.request(
            "GET",
            "/api/v1/orgs/invalid/projects/invalid/prompts",
            Some(&owner),
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    // Encoded separators cannot turn a legacy prompt name into a storage path.
    for route in [
        format!("prompts/scopes%2F{org}%2F{p}%2Fregistry%2Fshared.prompt"),
        "prompts/shared.prompt/optimizations/..%2Fsecret".into(),
    ] {
        assert_eq!(
            f.request(
                "GET",
                &format!("/api/v1/{route}"),
                Some(&owner),
                Value::Null,
                false
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    let registry = f.fixture.state.prompts.take();
    assert_eq!(
        f.request(
            "GET",
            &format!("{root}/prompts"),
            Some(&owner),
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::NOT_IMPLEMENTED
    );
    f.fixture.state.prompts = registry;
    f.fixture.state.auth = None;
    assert_eq!(
        f.request("GET", &format!("{root}/prompts"), None, Value::Null, false)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    f.fixture.state.iam = None;
    assert_eq!(
        f.request("GET", &format!("{root}/prompts"), None, Value::Null, false)
            .await
            .0,
        StatusCode::NOT_IMPLEMENTED
    );
    let (status, page) = f
        .request("GET", "/api/v1/prompts", None, Value::Null, false)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["total"], 0);
    assert!(page["prompts"].as_array().unwrap().is_empty());
    assert_eq!(
        f.request(
            "GET",
            &format!("/api/v1/prompts/shared.prompt/versions/{pin}"),
            None,
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}
