use super::*;

const DATASET: &str = "golden/cases";
const LIST: &str = "evaluation-reviews?dataset=golden/cases";
const TARGET: &str = "evaluation-reviews/of-target?kind=trace&trace_id=shared-trace";
const PUBLISH: &str = "evaluation-reviews/publish?dataset=golden/cases";

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
    f.fixture.state.datasets = Some(Arc::new(DatasetRegistry::new(
        Arc::new(MemoryObjectStore::new()),
        "datasets",
    )));
    f
}
fn proposal(words: &str, content: &str) -> Value {
    json!({"dataset":DATASET,"target":{"kind":"trace","trace_id":"shared-trace"},
        "question":words,"answer":"wrong","content":content,"split":"test"})
}
fn actions(id: &str) -> String {
    format!("evaluation-reviews/{id}/actions?dataset={DATASET}")
}
async fn post(f: &IamFixture, cookie: &str, root: &str, route: &str, body: Value) -> Value {
    let (status, value) = f
        .request("POST", &format!("{root}/{route}"), Some(cookie), body, true)
        .await;
    assert!(
        matches!(status, StatusCode::OK | StatusCode::CREATED),
        "{route}: {status} {value}"
    );
    value
}
async fn read(f: &IamFixture, cookie: &str, root: &str, route: &str) -> Value {
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
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}
async fn ready(f: &IamFixture, cookie: &str, root: &str, words: &str, content: &str) -> String {
    let item = post(
        f,
        cookie,
        root,
        "evaluation-reviews",
        proposal(words, content),
    )
    .await;
    let id = item["review"]["id"].as_str().unwrap().to_owned();
    let expected = post(
        f,
        cookie,
        root,
        &actions(&id),
        json!({"action":"expect","expected":words}),
    )
    .await;
    assert_eq!(expected["state"], "ready");
    id
}

#[tokio::test]
async fn project_reviews_publish_only_local_approved_cases_and_preserve_other_histories() {
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
    let id = ready(&f, &owner, &roots[0], "A secret", "written").await;
    for (root, cookie) in [
        (&roots[1], &owner),
        (&roots[2], &outsider),
        (&roots[3], &owner),
    ] {
        for route in [LIST, TARGET] {
            assert_eq!(read(&f, cookie, root, route).await["items"], json!([]));
        }
        assert_eq!(
            f.request(
                "POST",
                &format!("{root}/{}", actions(&id)),
                Some(cookie),
                json!({"action":"approve"}),
                true
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            f.request(
                "POST",
                &format!("{root}/{PUBLISH}"),
                Some(cookie),
                Value::Null,
                true
            )
            .await
            .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    for (root, cookie, words) in [
        (&roots[1], &owner, "B secret"),
        (&roots[2], &outsider, "C secret"),
        (&roots[3], &owner, "legacy secret"),
    ] {
        assert_eq!(ready(&f, cookie, root, words, "written").await, id);
    }
    // The existing dataset is local too: publishing must append A's prior row, not legacy/B's.
    for (root, cookie, words) in [
        (&roots[0], &owner, "A prior"),
        (&roots[1], &owner, "B prior"),
        (&roots[3], &owner, "legacy prior"),
    ] {
        post(&f, cookie, root, "datasets", json!({"name":DATASET,"pipeline":"fixture","source":"fixture",
            "columns":["case_id","input","expected"],"items":[{"case_id":"prior","input":{"question":words},"expected":{"answer":words}}]})).await;
    }
    let b_before = read(&f, &owner, &roots[1], LIST).await;
    let b_data = read(&f, &owner, &roots[1], "dataset-rows?name=golden/cases").await;
    let pending = f
        .request(
            "POST",
            &format!("{}/{PUBLISH}", roots[0]),
            Some(&owner),
            Value::Null,
            true,
        )
        .await;
    assert_eq!(pending.0, StatusCode::UNPROCESSABLE_ENTITY);
    post(
        &f,
        &owner,
        &roots[0],
        &actions(&id),
        json!({"action":"approve"}),
    )
    .await;
    let published = post(&f, &owner, &roots[0], PUBLISH, Value::Null).await;
    let version = published["dataset"]["dataset"]["latest"]["version"]
        .as_str()
        .unwrap();
    assert_eq!(published["published"][0]["state"], "published");
    assert_eq!(published["published"][0]["published_in"], version);
    assert_eq!(published["published"][0]["recorded_by"], "owner");
    let rows = read(
        &f,
        &owner,
        &roots[0],
        &format!("dataset-rows?name={DATASET}&version={version}"),
    )
    .await;
    assert_eq!(rows["rows"].as_array().unwrap().len(), 2);
    assert_eq!(rows["rows"][0]["row"]["input"]["question"], "A prior");
    assert_eq!(rows["rows"][1]["row"]["expected"]["answer"], "A secret");
    assert_eq!(rows["rows"][1]["row"]["split"], "test");
    assert_eq!(
        read(&f, &owner, &roots[0], TARGET).await["items"],
        published["published"]
    );
    assert_eq!(read(&f, &owner, &roots[1], LIST).await, b_before);
    assert_eq!(
        read(&f, &owner, &roots[1], "dataset-rows?name=golden/cases").await,
        b_data
    );
    for (root, cookie) in [
        (&roots[1], &owner),
        (&roots[2], &outsider),
        (&roots[3], &owner),
    ] {
        assert_eq!(
            f.request(
                "GET",
                &format!("{root}/dataset-rows?name={DATASET}&version={version}"),
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

#[tokio::test]
async fn project_review_roles_headers_and_observed_approval_use_current_project_access() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let member = f.cookie("member", Role::Viewer);
    let instance_admin = f.cookie("member", Role::Admin);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    let id = ready(&f, &owner, &root, "observed words", "observed").await;
    let writes = [
        ("evaluation-reviews".to_owned(), proposal("new", "written")),
        (actions(&id), json!({"action":"approve"})),
        (PUBLISH.into(), Value::Null),
    ];
    for route in [LIST, TARGET] {
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
    for route in [LIST, TARGET] {
        read(&f, &member, &root, route).await;
    }
    for (route, body) in &writes {
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
                body.clone(),
                false
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
    }
    grant(&f, &owner, &org, &p, "editor", json!({"valid_from":0})).await;
    let edited = post(
        &f,
        &member,
        &root,
        &actions(&id),
        json!({"action":"expect","expected":"verified"}),
    )
    .await;
    assert_eq!(edited["recorded_by"], "member");
    assert_eq!(
        f.request(
            "POST",
            &format!("{root}/{}", actions(&id)),
            Some(&instance_admin),
            json!({"action":"approve"}),
            true
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    grant(&f, &owner, &org, &p, "admin", json!({"valid_from":0})).await;
    let approved = post(
        &f,
        &member,
        &root,
        &actions(&id),
        json!({"action":"approve"}),
    )
    .await;
    assert_eq!(approved["decided_by"], "member");
    post(&f, &member, &root, PUBLISH, Value::Null).await;
}

#[tokio::test]
async fn delayed_review_bodies_do_not_extend_editor_or_observed_content_admin_grants() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let member = f.cookie("member", Role::Admin);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    let id = ready(&f, &owner, &root, "observed words", "observed").await;
    grant(
        &f,
        &owner,
        &org,
        &p,
        "editor",
        json!({"valid_from":0,"edit_until":1001,"read_until":1002}),
    )
    .await;
    let before = read(&f, &owner, &root, LIST).await;
    for (route, body) in [
        ("evaluation-reviews".into(), proposal("new", "written")),
        (actions(&id), json!({"action":"expect","expected":"late"})),
    ] {
        f.clock.0.store(1000, std::sync::atomic::Ordering::SeqCst);
        let clock = f.clock.clone();
        let body = Body::from_stream(futures::stream::once(async move {
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
                    .body(body)
                    .unwrap(),
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{value}");
    }
    // Admin expires during upload, while a separate permanent editor grant survives.
    grant(&f, &owner, &org, &p, "editor", json!({"valid_from":0})).await;
    grant(
        &f,
        &owner,
        &org,
        &p,
        "admin",
        json!({"valid_from":0,"edit_until":1002,"read_until":1003}),
    )
    .await;
    let clock = f.clock.clone();
    let body = Body::from_stream(futures::stream::once(async move {
        clock.0.store(1002, std::sync::atomic::Ordering::SeqCst);
        Ok::<_, std::io::Error>(json!({"action":"approve"}).to_string())
    }));
    assert_eq!(
        f.fixture
            .request(
                Request::builder()
                    .method("POST")
                    .uri(format!("{root}/{}", actions(&id)))
                    .header(header::COOKIE, &member)
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("x-aiwatcher-iam", "1")
                    .body(body)
                    .unwrap()
            )
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(read(&f, &owner, &root, LIST).await, before);
}

#[tokio::test]
async fn review_grants_expire_revoke_and_preserve_independent_access() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let member = f.cookie("member", Role::Viewer);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    let id = ready(&f, &owner, &root, "words", "written").await;
    let temporary = grant(
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
        for route in [LIST, TARGET] {
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
        for (route, body) in [
            ("evaluation-reviews".into(), proposal("words", "written")),
            (actions(&id), json!({"action":"expect","expected":"words"})),
        ] {
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
                write_status
            );
        }
        if write_status != StatusCode::OK {
            assert_eq!(
                f.request(
                    "POST",
                    &format!("{root}/{PUBLISH}"),
                    Some(&member),
                    Value::Null,
                    true
                )
                .await
                .0,
                write_status
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
    post(
        &f,
        &member,
        &root,
        &actions(&id),
        json!({"action":"approve"}),
    )
    .await;
    command(
        &f,
        &owner,
        &org,
        json!({"type":"revoke_grant","project":p,"grant":permanent}),
    )
    .await;
    for route in [LIST, TARGET] {
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
            &format!("{root}/{PUBLISH}"),
            Some(&member),
            Value::Null,
            true
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn project_reviews_fail_closed_for_sources_traversal_wrong_scope_and_disabled_iam() {
    let mut f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let outsider = f.cookie("outsider", Role::Admin);
    let org = f.create(&owner).await;
    let other = f.create(&outsider).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    let mut from_result = proposal("words", "written");
    from_result["target"] =
        json!({"kind":"case","evaluation_id":"global-result","case_id":"one","repetition_id":"r1"});
    from_result.as_object_mut().unwrap().remove("question");
    from_result["at"] = json!("known-cursor");
    for body in [from_result, proposal("cannot claim measured", "measured")] {
        assert_eq!(
            f.request(
                "POST",
                &format!("{root}/evaluation-reviews"),
                Some(&owner),
                body,
                true
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(read(&f, &owner, &root, LIST).await["items"], json!([]));
    let id = ready(&f, &owner, &root, "secret", "written").await;
    for base in [&root, &"/api/v1".to_owned()] {
        assert_eq!(
            f.request(
                "POST",
                &format!("{base}/evaluation-reviews/x%2F..%2Fy/actions?dataset={DATASET}"),
                Some(&owner),
                json!({"action":"approve"}),
                true
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    for path in [&root, &base(&other, &p)] {
        assert_eq!(
            f.request(
                "GET",
                &format!("{path}/{LIST}"),
                Some(&outsider),
                Value::Null,
                false
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        f.request("GET", &format!("{root}/{LIST}"), None, Value::Null, false)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    f.fixture.state.iam = None;
    assert_eq!(
        f.request(
            "GET",
            &format!("{root}/{LIST}"),
            Some(&owner),
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::NOT_IMPLEMENTED
    );
    assert_eq!(read(&f, &owner, "/api/v1", LIST).await["items"], json!([]));
    assert_eq!(
        f.request(
            "POST",
            &format!("/api/v1/{}", actions(&id)),
            Some(&owner),
            json!({"action":"approve"}),
            false
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}
