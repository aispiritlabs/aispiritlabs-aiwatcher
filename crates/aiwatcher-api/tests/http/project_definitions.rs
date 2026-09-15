use super::*;

async fn fixture() -> IamFixture {
    let mut f = IamFixture::new().await;
    f.fixture = Fixture::build(false, true, None, f.fixture.state.auth.clone(), None)
        .with_pod_templates(planner_pod_templates());
    f.fixture.state.iam = Some(f.store.clone());
    f
}
fn definition(label: &str) -> Value {
    let mut spec = authored_worker_workflow();
    spec["name"] = json!("house/import");
    spec["steps"][0]["params"] = json!({"source":label});
    spec
}
fn reads(pin: &Value) -> Vec<String> {
    vec![
        "workflow-definitions".into(),
        "workflow-definitions/house%2Fimport".into(),
        format!(
            "workflow-definitions/house%2Fimport?revision={}",
            pin["revision"].as_str().unwrap()
        ),
    ]
}
async fn save(f: &IamFixture, cookie: &str, root: &str, label: &str) -> Value {
    let (status, body) = f
        .request(
            "POST",
            &format!("{root}/workflow-definitions"),
            Some(cookie),
            definition(label),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}
async fn snapshot(f: &IamFixture, cookie: &str, root: &str, pin: &Value) -> Vec<Value> {
    let mut values = Vec::new();
    for route in reads(pin) {
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
        assert_eq!(response.status(), StatusCode::OK, "{route}");
        if root != "/api/v1" {
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        }
        values.push(
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap(),
        );
    }
    values
}

#[tokio::test]
async fn project_definitions_isolate_names_versions_and_legacy_execution_resolution() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let outsider = f.cookie("outsider", Role::Admin);
    let org = f.create(&owner).await;
    let other = f.create(&outsider).await;
    let a = project(&f, &owner, &org).await;
    let b = project(&f, &owner, &org).await;
    let c = project(&f, &outsider, &other).await;
    let roots = [
        base(&org, &a),
        base(&org, &b),
        base(&other, &c),
        "/api/v1".into(),
    ];
    let first = save(&f, &owner, &roots[0], "private A").await;
    for (root, cookie) in [
        (&roots[1], &owner),
        (&roots[2], &outsider),
        (&roots[3], &owner),
    ] {
        let (status, page) = f
            .request(
                "GET",
                &format!("{root}/workflow-definitions"),
                Some(cookie),
                Value::Null,
                false,
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(page, json!([]));
        for route in reads(&first).into_iter().skip(1) {
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
                StatusCode::NOT_FOUND
            );
        }
    }
    // Real legacy execution and schedule routes cannot resolve a scoped-only definition.
    for revision in [Value::Null, first["revision"].clone()] {
        let mut target = json!({"kind":"workflow","name":"house/import"});
        if !revision.is_null() {
            target["revision"] = revision;
        }
        let (status, refusal) = f
            .request(
                "POST",
                "/api/v1/executions",
                Some(&owner),
                json!({"target":target}),
                false,
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{refusal}");
    }
    let schedule = json!({"cadence":{"every":"daily","hour":9,"minute":0},"timezone":"UTC"});
    let (status, refusal) = f
        .request(
            "PUT",
            "/api/v1/workflow-definitions/house%2Fimport/schedule",
            Some(&owner),
            schedule,
            false,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{refusal}");
    let second = save(&f, &owner, &roots[1], "private B").await;
    let third = save(&f, &outsider, &roots[2], "private C").await;
    let legacy = save(&f, &owner, &roots[3], "legacy").await;
    for (root, cookie, pin, label) in [
        (&roots[0], &owner, &first, "private A"),
        (&roots[1], &owner, &second, "private B"),
        (&roots[2], &outsider, &third, "private C"),
        (&roots[3], &owner, &legacy, "legacy"),
    ] {
        let values = snapshot(&f, cookie, root, pin).await;
        assert_eq!(values[0].as_array().unwrap().len(), 1);
        assert_eq!(values[1], *pin);
        assert_eq!(values[2], *pin);
        assert_eq!(
            values[1]["definition"]["steps"][0]["params"]["source"],
            label
        );
    }
    let updated = save(&f, &owner, &roots[0], "A second version").await;
    assert_ne!(updated["revision"], first["revision"]);
    assert_eq!(snapshot(&f, &owner, &roots[0], &first).await[2], first);
    assert_eq!(snapshot(&f, &owner, &roots[1], &second).await[1], second);
    // Independent publication of identical content retains the same revision.
    let copy = save(&f, &owner, &roots[1], "private A").await;
    assert_eq!(copy["revision"], first["revision"]);
    assert_eq!(save(&f, &owner, &roots[1], "private A").await, copy);
    assert_eq!(
        f.request(
            "GET",
            &format!("{}/workflow-definitions", base(&other, &a)),
            Some(&owner),
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    // A legacy pinned start never substitutes a same-named scoped revision.
    let (status,body)=f.request("POST","/api/v1/executions",Some(&owner),json!({"target":{"kind":"workflow","name":"house/import","revision":first["revision"]}}),false).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
}

#[tokio::test]
async fn project_definition_roles_preserve_graph_and_pod_validation() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let member = f.cookie("member", Role::Viewer);
    let admin = f.cookie("member", Role::Admin);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    let pin = save(&f, &owner, &root, "original").await;
    for route in reads(&pin) {
        assert_eq!(
            f.request(
                "GET",
                &format!("{root}/{route}"),
                Some(&admin),
                Value::Null,
                false
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }
    grant(&f, &owner, &org, &p, "viewer", json!({"valid_from":0})).await;
    snapshot(&f, &member, &root, &pin).await;
    assert_eq!(
        f.request(
            "POST",
            &format!("{root}/workflow-definitions"),
            Some(&admin),
            definition("forbidden"),
            true
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    grant(&f, &owner, &org, &p, "editor", json!({"valid_from":0})).await;
    assert_eq!(
        f.request(
            "POST",
            &format!("{root}/workflow-definitions"),
            Some(&member),
            definition("no header"),
            false
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let saved = save(&f, &member, &root, "member changes").await;
    assert_eq!(saved["registered_by"], "member");
    let mut broken = definition("invalid");
    broken["steps"][0]["task_ref"] = json!("not-pinned");
    broken["steps"][0]["after"] = json!(["persist"]);
    for body in [
        broken,
        podded_workflow("ghcr.io/planner/import-debug:1", "4"),
    ] {
        let (status, refusal) = f
            .request(
                "POST",
                &format!("{root}/workflow-definitions"),
                Some(&member),
                body,
                true,
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refusal}");
        assert!(!refusal["details"].as_array().unwrap().is_empty());
    }
    assert_eq!(
        snapshot(&f, &member, &root, &saved).await[0],
        json!([saved])
    );
    let (status, body) = f
        .request(
            "POST",
            &format!("{root}/workflow-definitions"),
            Some(&member),
            podded_workflow("ghcr.io/planner/import:1", "1"),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // Defining a worker task or pod does not add scoped execution/schedule routes.
    for (method, path) in [
        ("POST", "executions"),
        ("PUT", "workflow-definitions/house%2Fimport/schedule"),
    ] {
        let response = f
            .fixture
            .router()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(format!("{root}/{path}"))
                    .header(header::COOKIE, &owner)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}

#[tokio::test]
async fn project_definitions_recheck_expiry_revocation_and_independent_grants() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let member = f.cookie("member", Role::Viewer);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    let pin = save(&f, &owner, &root, "original").await;
    let temporary = grant(
        &f,
        &owner,
        &org,
        &p,
        "editor",
        json!({"valid_from":1001,"edit_until":1003,"read_until":1005}),
    )
    .await;
    for (clock, read, write) in [
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
                read
            );
        }
        assert_eq!(
            f.request(
                "POST",
                &format!("{root}/workflow-definitions"),
                Some(&member),
                definition("changed"),
                true
            )
            .await
            .0,
            write
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
    snapshot(&f, &member, &root, &pin).await;
    command(
        &f,
        &owner,
        &org,
        json!({"type":"revoke_grant","project":p,"grant":permanent}),
    )
    .await;
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
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        f.request(
            "POST",
            &format!("{root}/workflow-definitions"),
            Some(&member),
            definition("after revocation"),
            true
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn slow_definition_upload_does_not_extend_a_grant_or_publish_a_version() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let member = f.cookie("member", Role::Viewer);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    let pin = save(&f, &owner, &root, "original").await;
    grant(
        &f,
        &owner,
        &org,
        &p,
        "editor",
        json!({"valid_from":0,"edit_until":1001,"read_until":1002}),
    )
    .await;
    let body = definition("delayed");
    let revision =
        serde_json::from_value::<aiwatcher_execution::definition::WorkflowSpec>(body.clone())
            .unwrap()
            .revision()
            .0;
    let clock = f.clock.clone();
    let stream = Body::from_stream(futures::stream::once(async move {
        clock.0.store(1001, std::sync::atomic::Ordering::SeqCst);
        Ok::<_, std::io::Error>(body.to_string())
    }));
    let (status, body) = f
        .fixture
        .request(
            Request::builder()
                .method("POST")
                .uri(format!("{root}/workflow-definitions"))
                .header(header::COOKIE, &member)
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-aiwatcher-iam", "1")
                .body(stream)
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(snapshot(&f, &owner, &root, &pin).await[1], pin);
    assert_eq!(
        f.request(
            "GET",
            &format!("{root}/workflow-definitions/house%2Fimport?revision={revision}"),
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
async fn definitions_fail_closed_for_missing_scope_auth_iam_and_registry() {
    let mut f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    let pin = save(&f, &owner, &root, "private").await;
    for base in [&root, &"/api/v1".to_owned()] {
        for revision in [
            "..%2F..%2Fhead",
            "..%5Csecret",
            "%2Fabsolute",
            &"A".repeat(64),
        ] {
            assert_eq!(
                f.request(
                    "GET",
                    &format!("{base}/workflow-definitions/house%2Fimport?revision={revision}"),
                    Some(&owner),
                    Value::Null,
                    false
                )
                .await
                .0,
                StatusCode::NOT_FOUND
            );
        }
    }
    assert_eq!(
        f.request(
            "GET",
            &format!("{root}/workflow-definitions"),
            None,
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.request(
            "GET",
            "/api/v1/orgs/bad/projects/bad/workflow-definitions",
            Some(&owner),
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let auth = f.fixture.state.auth.take();
    assert_eq!(
        f.request(
            "GET",
            &format!("{root}/workflow-definitions"),
            None,
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    f.fixture.state.auth = auth;
    let iam = f.fixture.state.iam.take();
    assert_eq!(
        f.request(
            "GET",
            &format!("{root}/workflow-definitions"),
            Some(&owner),
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::NOT_IMPLEMENTED
    );
    let (_, page) = f
        .request(
            "GET",
            "/api/v1/workflow-definitions",
            Some(&owner),
            Value::Null,
            false,
        )
        .await;
    assert_eq!(page, json!([]));
    for route in reads(&pin).into_iter().skip(1) {
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
            StatusCode::NOT_FOUND
        );
    }
    f.fixture.state.iam = iam;
    f.fixture.state.workflow_definitions = None;
    assert_eq!(
        f.request(
            "GET",
            &format!("{root}/workflow-definitions"),
            Some(&owner),
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::NOT_IMPLEMENTED
    );
}
