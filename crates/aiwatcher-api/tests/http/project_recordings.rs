use super::*;

const RECORDING: &str =
    "{\n \"answers\": [{\"case_id\":\"one\",\"answer\": 1.00000000000000001}]\n}";

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
fn upload(root: &str) -> String {
    format!("{root}/evaluation-recordings/answers%2Ftest")
}
fn download(root: &str, artifact: &Value) -> String {
    format!(
        "{root}/evaluation-recordings/{}/content",
        artifact["digest"].as_str().unwrap()
    )
}
async fn stage(f: &IamFixture, cookie: &str, root: &str) -> Value {
    let (status, value) = f
        .fixture
        .request(
            Request::builder()
                .method("PUT")
                .uri(upload(root))
                .header(header::COOKIE, cookie)
                .header("x-aiwatcher-iam", "1")
                .body(Body::from(RECORDING))
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(value["name"], "answers/test");
    value
}

#[tokio::test]
async fn project_recordings_isolate_bytes_across_projects_organizations_and_legacy() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let outsider = f.cookie("outsider", Role::Admin);
    let org = f.create(&owner).await;
    let other = f.create(&outsider).await;
    let a = project(&f, &owner, &org).await;
    let b = project(&f, &owner, &org).await;
    let c = project(&f, &outsider, &other).await;
    let root = base(&org, &a);
    let artifact = stage(&f, &owner, &root).await;
    assert_eq!(stage(&f, &owner, &root).await, artifact);
    for (other_root, cookie) in [
        (base(&org, &b), &owner),
        (base(&other, &c), &outsider),
        ("/api/v1".into(), &owner),
    ] {
        assert_eq!(
            f.request(
                "GET",
                &download(&other_root, &artifact),
                Some(cookie),
                Value::Null,
                false
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        // Explicitly uploading identical bytes creates a separate local object.
        assert_eq!(stage(&f, cookie, &other_root).await, artifact);
    }
    for (url, cookie) in [
        (download(&root, &artifact), &outsider),
        (download(&base(&other, &a), &artifact), &owner),
    ] {
        assert_eq!(
            f.request("GET", &url, Some(cookie), Value::Null, false)
                .await
                .0,
            StatusCode::NOT_FOUND
        );
    }
    let response = f
        .fixture
        .router()
        .oneshot(
            Request::builder()
                .uri(download(&root, &artifact))
                .header(header::COOKIE, &owner)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    assert_eq!(
        axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap()
            .as_ref(),
        RECORDING.as_bytes()
    );
}

#[tokio::test]
async fn project_recordings_require_current_roles_headers_and_grants() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let member = f.cookie("member", Role::Viewer);
    let admin = f.cookie("member", Role::Admin);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    let artifact = stage(&f, &owner, &root).await;
    let body: Value = serde_json::from_str(RECORDING).unwrap();
    assert_eq!(
        f.request("PUT", &upload(&root), Some(&admin), body.clone(), true)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.request("PUT", &upload(&root), Some(&owner), body.clone(), false)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
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
        assert_eq!(
            f.request(
                "GET",
                &download(&root, &artifact),
                Some(&member),
                Value::Null,
                false
            )
            .await
            .0,
            read
        );
        assert_eq!(
            f.request("PUT", &upload(&root), Some(&admin), body.clone(), true)
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
    assert_eq!(stage(&f, &member, &root).await, artifact);
    command(
        &f,
        &owner,
        &org,
        json!({"type":"revoke_grant","project":p,"grant":permanent}),
    )
    .await;
    assert_eq!(
        f.request(
            "GET",
            &download(&root, &artifact),
            Some(&member),
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.request("PUT", &upload(&root), Some(&member), body, true)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn delayed_recording_upload_cannot_write_after_grant_expires() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let member = f.cookie("member", Role::Viewer);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    let artifact = stage(&f, &owner, "/api/v1").await;
    grant(
        &f,
        &owner,
        &org,
        &p,
        "editor",
        json!({"valid_from":0,"edit_until":1001,"read_until":1002}),
    )
    .await;
    let clock = f.clock.clone();
    let body = Body::from_stream(futures::stream::once(async move {
        clock.0.store(1001, std::sync::atomic::Ordering::SeqCst);
        Ok::<_, std::io::Error>(RECORDING)
    }));
    assert_eq!(
        f.fixture
            .request(
                Request::builder()
                    .method("PUT")
                    .uri(upload(&root))
                    .header(header::COOKIE, &member)
                    .header("x-aiwatcher-iam", "1")
                    .body(body)
                    .unwrap()
            )
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.request(
            "GET",
            &download(&root, &artifact),
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
async fn project_recordings_fail_closed_without_auth_or_iam_and_reject_bad_content() {
    let mut f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    let artifact = stage(&f, &owner, &root).await;
    let body: Value = serde_json::from_str(RECORDING).unwrap();
    for (method, url) in [("GET", download(&root, &artifact)), ("PUT", upload(&root))] {
        assert_eq!(
            f.request(method, &url, None, body.clone(), true).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    for root in [&root, &"/api/v1".to_owned()] {
        assert_eq!(
            f.request(
                "PUT",
                &upload(root),
                Some(&owner),
                json!({"answers":[{"case_id":"", "answer":1}]}),
                true
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
        for digest in ["..%2Fsecret", "not-a-hash", "%5Cprivate", ".."] {
            assert_eq!(
                f.request(
                    "GET",
                    &format!("{root}/evaluation-recordings/{digest}/content"),
                    Some(&owner),
                    Value::Null,
                    false
                )
                .await
                .0,
                StatusCode::BAD_REQUEST
            );
        }
    }
    let auth = f.fixture.state.auth.take();
    for (method, url) in [("GET", download(&root, &artifact)), ("PUT", upload(&root))] {
        assert_eq!(
            f.request(method, &url, None, body.clone(), true).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    f.fixture.state.auth = auth;
    f.fixture.state.iam = None;
    assert_eq!(
        f.request(
            "GET",
            &download(&root, &artifact),
            Some(&owner),
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::NOT_IMPLEMENTED
    );
    assert_eq!(
        f.request("PUT", &upload(&root), Some(&owner), body.clone(), true)
            .await
            .0,
        StatusCode::NOT_IMPLEMENTED
    );
    assert_eq!(
        f.request(
            "GET",
            &download("/api/v1", &artifact),
            Some(&owner),
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    // Legacy mutations retain the instance editor requirement.
    let viewer = f.cookie("owner", Role::Viewer);
    assert_eq!(
        f.request("PUT", &upload("/api/v1"), Some(&viewer), body, false)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
}
