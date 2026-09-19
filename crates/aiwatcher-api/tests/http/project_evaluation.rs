use super::*;

async fn fixture() -> IamFixture {
    let mut f = IamFixture::new().await;
    f.fixture.state.evaluations = Some(Arc::new(
        aiwatcher_evaluation::Registry::new(
            Arc::new(MemoryObjectStore::new()),
            Arc::new(EvaluationSource::default()),
            Default::default(),
        )
        .unwrap(),
    ));
    f
}
fn rubric(label: &str) -> Value {
    let mut value = helpfulness(["bad", "good", "great"]);
    value["question"] = json!(label);
    value
}
fn card(version: &str) -> Value {
    json!({"name":"answer-quality", "scorers":[{"metric":"helpful", "scorer":{
        "kind":"judge", "rubric":{"name":"helpfulness","version":version}
    }}]})
}
fn assessment(version: &str, rationale: &str) -> Value {
    let mut value = about_that_case("good");
    value["rubric_version"] = json!(version);
    value["rationale"] = json!(rationale);
    value
}
fn writes(pin: &[Value; 3]) -> Vec<(&'static str, Value)> {
    vec![
        ("evaluation-rubrics", rubric("changed")),
        (
            "evaluation-scorecards",
            card(pin[0]["version"].as_str().unwrap()),
        ),
        (
            "evaluation-assessments",
            assessment(pin[0]["version"].as_str().unwrap(), "changed"),
        ),
    ]
}
fn reads(pin: &[Value; 3]) -> Vec<String> {
    vec![
        "evaluation-rubrics".into(),
        format!(
            "evaluation-rubrics/helpfulness?version={}",
            pin[0]["version"].as_str().unwrap()
        ),
        format!("evaluation-assessments?{ONE_CASE}"),
        format!(
            "evaluation-assessments/{}/{}?limit=1",
            pin[2]["target_id"].as_str().unwrap(),
            pin[2]["standing_id"].as_str().unwrap()
        ),
        "evaluation-scorecards".into(),
        format!(
            "evaluation-scorecards/answer-quality?version={}",
            pin[1]["version"].as_str().unwrap()
        ),
        "evaluation-scorecards/answer-quality/versions".into(),
        format!(
            "evaluation-scorecards/answer-quality/diff?from={0}&to={0}",
            pin[1]["version"].as_str().unwrap()
        ),
    ]
}
async fn seed(f: &IamFixture, cookie: &str, root: &str, label: &str) -> [Value; 3] {
    let (status, rubric) = f
        .request(
            "POST",
            &format!("{root}/evaluation-rubrics"),
            Some(cookie),
            rubric(label),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{rubric}");
    let version = rubric["version"].as_str().unwrap();
    let (status, card) = f
        .request(
            "POST",
            &format!("{root}/evaluation-scorecards"),
            Some(cookie),
            card(version),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{card}");
    let (status, assessment) = f
        .request(
            "POST",
            &format!("{root}/evaluation-assessments"),
            Some(cookie),
            assessment(version, label),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{assessment}");
    [rubric, card, assessment]
}
async fn snapshot(f: &IamFixture, cookie: &str, root: &str, pin: &[Value; 3]) -> Vec<Value> {
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
async fn authored_evaluation_isolates_forms_cards_assessments_and_history() {
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
    let pa = seed(&f, &owner, &roots[0], "a secret").await;
    for (root, cookie) in [
        (&roots[1], &owner),
        (&roots[2], &outsider),
        (&roots[3], &owner),
    ] {
        for path in ["evaluation-rubrics", "evaluation-scorecards"] {
            let (_, value) = f
                .request(
                    "GET",
                    &format!("{root}/{path}"),
                    Some(cookie),
                    Value::Null,
                    false,
                )
                .await;
            let field = if path.ends_with("rubrics") {
                "rubrics"
            } else {
                "scorecards"
            };
            assert_eq!(value[field], json!([]));
        }
        for route in reads(&pa)
            .into_iter()
            .filter(|p| p.contains("version=") || p.contains("/diff?"))
        {
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
        // A known pin from A cannot be used to publish a card or judgement elsewhere.
        for (route, body) in writes(&pa).into_iter().skip(1) {
            assert_eq!(
                f.request("POST", &format!("{root}/{route}"), Some(cookie), body, true)
                    .await
                    .0,
                StatusCode::BAD_REQUEST
            );
        }
    }
    let pb = seed(&f, &owner, &roots[1], "b secret").await;
    let pc = seed(&f, &outsider, &roots[2], "c secret").await;
    let legacy = seed(&f, &owner, &roots[3], "legacy secret").await;
    for (root, cookie, pin, label) in [
        (&roots[0], &owner, &pa, "a secret"),
        (&roots[1], &owner, &pb, "b secret"),
        (&roots[2], &outsider, &pc, "c secret"),
        (&roots[3], &owner, &legacy, "legacy secret"),
    ] {
        let values = snapshot(&f, cookie, root, pin).await;
        assert_eq!(values[0]["rubrics"].as_array().unwrap().len(), 1);
        assert_eq!(values[1]["rubric"]["question"], label);
        assert_eq!(values[2]["assessments"][0]["rationale"], label);
        assert_eq!(values[3]["revisions"][0], pin[2]);
        assert_eq!(values[6]["versions"].as_array().unwrap().len(), 1);
    }
    assert_eq!(pa[2]["target_id"], pb[2]["target_id"]);
    assert_eq!(pa[2]["standing_id"], pb[2]["standing_id"]);
    let before = snapshot(&f, &owner, &roots[1], &pb).await;
    let (_, updated) = f
        .request(
            "POST",
            &format!("{}/evaluation-assessments", roots[0]),
            Some(&owner),
            assessment(pa[0]["version"].as_str().unwrap(), "new thought"),
            true,
        )
        .await;
    assert_eq!(updated["revision"], 2);
    assert_eq!(snapshot(&f, &owner, &roots[1], &pb).await, before);
    let history = &reads(&pa)[3];
    let (_, page) = f
        .request(
            "GET",
            &format!("{}/{history}", roots[0]),
            Some(&owner),
            Value::Null,
            false,
        )
        .await;
    assert_eq!(page["revisions"][0]["revision"], 2);
    let (_, page) = f
        .request(
            "GET",
            &format!("{}/{history}&before=2", roots[0]),
            Some(&owner),
            Value::Null,
            false,
        )
        .await;
    assert_eq!(page["revisions"][0], pa[2]);
    // A project ID under a different organization is never a valid alias.
    assert_eq!(
        f.request(
            "GET",
            &format!("{}/evaluation-rubrics", base(&other, &a)),
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
async fn project_roles_and_mutation_headers_cover_all_authored_evaluation_operations() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let member = f.cookie("member", Role::Viewer);
    let instance_admin = f.cookie("member", Role::Admin);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    let pin = seed(&f, &owner, &root, "original").await;
    for route in reads(&pin) {
        assert_eq!(
            f.request(
                "GET",
                &format!("{root}/{route}"),
                Some(&instance_admin),
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
    for (route, body) in writes(&pin) {
        assert_eq!(
            f.request(
                "POST",
                &format!("{root}/{route}"),
                Some(&instance_admin),
                body.clone(),
                true
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
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
    let member_pin = seed(&f, &member, &root, "member words").await;
    assert_eq!(member_pin[0]["published_by"], "member");
    assert_eq!(member_pin[1]["published_by"], "member");
    assert_eq!(member_pin[2]["recorded_by"], "member");
    assert_eq!(member_pin[2]["author"], "member");
    // Other evaluation route families have no scoped aliases in this slice.
    for (method, route) in [("GET", "evaluation-scorers")] {
        let response = f
            .fixture
            .router()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(format!("{root}/{route}"))
                    .header(header::COOKIE, &owner)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{route}");
    }
    // The start *does* have a scoped twin now (IAM-02/D), and it is a write:
    // without `X-AIWatcher-IAM` it is refused before the declaration is even
    // looked for, which is what stops a cross-origin form with a session
    // cookie starting somebody's measurement.
    let response = f
        .fixture
        .router()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("{root}/evaluation-runs/run/start"))
                .header(header::COOKIE, &owner)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn authored_evaluation_rechecks_expiry_revocation_and_independent_grants() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let member = f.cookie("member", Role::Viewer);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    let pin = seed(&f, &owner, &root, "original").await;
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
                read,
                "{route}"
            );
        }
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
                write,
                "{route}"
            );
        }
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
async fn delayed_authored_evaluation_writes_cannot_extend_an_expired_grant() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let member = f.cookie("member", Role::Viewer);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    let pin = seed(&f, &owner, &root, "original").await;
    grant(
        &f,
        &owner,
        &org,
        &p,
        "editor",
        json!({"valid_from":0,"edit_until":1001,"read_until":1002}),
    )
    .await;
    let before = snapshot(&f, &owner, &root, &pin).await;
    for (route, mut body) in writes(&pin) {
        if route == "evaluation-scorecards" {
            body["scorers"][0]["scorer"]["pass_level"] = json!("great");
        }
        f.clock.0.store(1000, std::sync::atomic::Ordering::SeqCst);
        let clock = f.clock.clone();
        let stream = Body::from_stream(futures::stream::once(async move {
            clock.0.store(1001, std::sync::atomic::Ordering::SeqCst);
            Ok::<_, std::io::Error>(body.to_string())
        }));
        let (status, value) = f
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
        assert_eq!(status, StatusCode::FORBIDDEN, "{route}: {value}");
    }
    assert_eq!(snapshot(&f, &owner, &root, &pin).await, before);
}

#[tokio::test]
async fn authored_evaluation_fails_closed_and_cannot_traverse_legacy_or_project_keys() {
    let mut f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    let pin = seed(&f, &owner, &root, "private").await;
    for base in [&root, &"/api/v1".to_owned()] {
        for route in [
            "evaluation-rubrics/helpfulness?version=..%2F..%2Fhead",
            "evaluation-scorecards/answer-quality?version=..%2F..%2Fhead",
            "evaluation-scorecards/answer-quality/diff?from=..%2Fhead&to=x",
            "evaluation-assessments/x%2F..%2Fy/z",
        ] {
            assert_eq!(
                f.request(
                    "GET",
                    &format!("{base}/{route}"),
                    Some(&owner),
                    Value::Null,
                    false
                )
                .await
                .0,
                StatusCode::BAD_REQUEST,
                "{route}"
            );
        }
    }
    assert_eq!(
        f.request(
            "GET",
            &format!("{root}/evaluation-rubrics"),
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
            "/api/v1/orgs/not-a-uuid/projects/not-a-uuid/evaluation-rubrics",
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
            &format!("{root}/evaluation-rubrics"),
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
            &format!("{root}/evaluation-rubrics"),
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
            "/api/v1/evaluation-rubrics",
            Some(&owner),
            Value::Null,
            false,
        )
        .await;
    assert_eq!(page["rubrics"], json!([]));
    for route in reads(&pin).into_iter().filter(|p| p.contains("version=")) {
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
    f.fixture.state.evaluations = None;
    assert_eq!(
        f.request(
            "GET",
            &format!("{root}/evaluation-rubrics"),
            Some(&owner),
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::NOT_IMPLEMENTED
    );
}
