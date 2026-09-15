use super::*;

async fn fixture() -> IamFixture {
    let mut f = IamFixture::new().await;
    f.fixture.state.annotations = Some(Arc::new(AnnotationRegistry::new(
        Arc::new(MemoryObjectStore::new()),
        "annotations",
    )));
    f
}
async fn raw(
    f: &IamFixture,
    method: &str,
    path: &str,
    cookie: Option<&str>,
    body: Body,
    marker: bool,
) -> axum::response::Response {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "image/png");
    if let Some(cookie) = cookie {
        request = request.header(header::COOKIE, cookie);
    }
    if marker {
        request = request.header("x-aiwatcher-iam", "1");
    }
    f.fixture
        .router()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap()
}
async fn upload(f: &IamFixture, cookie: &str, root: &str, bytes: &[u8]) -> Value {
    let response = raw(
        f,
        "POST",
        &format!("{root}/annotation-blobs"),
        Some(cookie),
        Body::from(bytes.to_vec()),
        true,
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}
fn drawing(id: &str, x: f64) -> Value {
    json!({"project":ANNOTATION_PROJECT,"image_id":id,"accept":true,"annotations":[{"id":"region-1","class":"region","geometry":{"kind":"polygon","exterior":[[0.0,0.0],[x,0.0],[x,50.0],[0.0,50.0]]},"attributes":{}}]})
}
fn reads(id: &str, revision: &str, export: &str) -> Vec<String> {
    vec![
        "annotation-projects".into(),
        format!("annotation-project?name={ANNOTATION_PROJECT}"),
        format!(
            "annotation-images?project={ANNOTATION_PROJECT}&review=accepted&limit=1&offset=0&group_id=family-a"
        ),
        format!("annotation-image?project={ANNOTATION_PROJECT}&image_id={id}&revision={revision}"),
        format!("annotation-exports?name={ANNOTATION_PROJECT}"),
        format!("annotation-export?project={ANNOTATION_PROJECT}&export={export}"),
        format!("annotation-export/coco?project={ANNOTATION_PROJECT}&export={export}"),
    ]
}
fn writes(id: &str, revision: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("annotation-projects", annotation_project()),
        ("annotation-images", registered_image(id)),
        ("annotation-revisions", drawing(id, 100.0)),
        (
            "annotation-reviews",
            json!({"project":ANNOTATION_PROJECT,"image_id":id,"review":"accepted","revision":revision,"note":"review note"}),
        ),
        (
            "annotation-exports",
            json!({"project":ANNOTATION_PROJECT,"note":"test cut"}),
        ),
    ]
}
async fn seed(
    f: &IamFixture,
    cookie: &str,
    root: &str,
    bytes: &[u8],
    x: f64,
) -> (String, String, String) {
    let blob = upload(f, cookie, root, bytes).await;
    let id = blob["image_id"].as_str().unwrap().to_owned();
    for (route, body) in [
        ("annotation-projects", annotation_project()),
        ("annotation-images", registered_image(&id)),
    ] {
        let (status, body) = f
            .request("POST", &format!("{root}/{route}"), Some(cookie), body, true)
            .await;
        assert_eq!(status, StatusCode::OK, "{route}: {body}");
    }
    let (status, saved) = f
        .request(
            "POST",
            &format!("{root}/annotation-revisions"),
            Some(cookie),
            drawing(&id, x),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{saved}");
    let revision = saved["revision"]["revision"].as_str().unwrap().to_owned();
    let (status, export) = f
        .request(
            "POST",
            &format!("{root}/annotation-exports"),
            Some(cookie),
            json!({"project":ANNOTATION_PROJECT}),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{export}");
    assert_eq!(export["manifest"]["counts"]["images"], 1);
    (
        id,
        revision,
        export["manifest"]["export"].as_str().unwrap().into(),
    )
}

#[tokio::test]
async fn project_annotations_isolate_drawings_reviews_exports_coco_and_binary_content() {
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
    let mut identities = Vec::new();
    for (root, cookie, bytes, x) in [
        (&roots[0], &owner, b"private image A".as_slice(), 100.0),
        (&roots[1], &owner, b"private image B".as_slice(), 80.0),
        (&roots[2], &outsider, b"private image C".as_slice(), 70.0),
        (&roots[3], &owner, b"legacy image".as_slice(), 60.0),
    ] {
        let (id, revision, export) = seed(&f, cookie, root, bytes, x).await;
        for route in reads(&id, &revision, &export) {
            let (status, result) = f
                .request(
                    "GET",
                    &format!("{root}/{route}"),
                    Some(cookie),
                    Value::Null,
                    false,
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{route}: {result}");
            if route.starts_with("annotation-export/coco") {
                assert_eq!(result["images"].as_array().unwrap().len(), 1);
                assert_eq!(result["annotations"].as_array().unwrap().len(), 1);
            }
        }
        let response = raw(
            &f,
            "GET",
            &format!("{root}/annotation-blobs/{id}"),
            Some(cookie),
            Body::empty(),
            false,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/png");
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            if root == "/api/v1" {
                "private, max-age=31536000, immutable"
            } else {
                "no-store"
            }
        );
        assert_eq!(
            response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .as_ref(),
            bytes
        );
        identities.push((id, revision, export));
    }
    let (id, revision, export) = &identities[0];
    for (root, cookie) in [
        (&roots[1], &owner),
        (&roots[2], &outsider),
        (&roots[3], &owner),
    ] {
        for route in [
            format!(
                "annotation-image?project={ANNOTATION_PROJECT}&image_id={id}&revision={revision}"
            ),
            format!("annotation-export?project={ANNOTATION_PROJECT}&export={export}"),
            format!("annotation-export/coco?project={ANNOTATION_PROJECT}&export={export}"),
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
                StatusCode::NOT_FOUND
            );
        }
        assert_eq!(
            raw(
                &f,
                "GET",
                &format!("{root}/annotation-blobs/{id}"),
                Some(cookie),
                Body::empty(),
                false
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
    }
    for root in [&roots[0], &base(&other_org, &a)] {
        for route in reads(id, revision, export) {
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
        for (route, body) in writes(id, revision) {
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
        for method in ["GET", "POST"] {
            assert_eq!(
                raw(
                    &f,
                    method,
                    &format!(
                        "{root}/annotation-blobs{}",
                        if method == "GET" {
                            format!("/{id}")
                        } else {
                            String::new()
                        }
                    ),
                    Some(&outsider),
                    Body::from("attempt"),
                    true
                )
                .await
                .status(),
                StatusCode::NOT_FOUND
            );
        }
    }
    // A revision from another collection instance cannot become B's accepted drawing.
    assert_eq!(f.request("POST",&format!("{}/annotation-reviews",roots[1]),Some(&owner),json!({"project":ANNOTATION_PROJECT,"image_id":identities[1].0,"review":"accepted","revision":revision}),true).await.0,StatusCode::NOT_FOUND);
    let (_, detail) = f
        .request(
            "GET",
            &format!(
                "{}/annotation-image?project={ANNOTATION_PROJECT}&image_id={}",
                roots[1], identities[1].0
            ),
            Some(&owner),
            Value::Null,
            false,
        )
        .await;
    assert_eq!(detail["accepted"], identities[1].1);
    // Same blob uploaded independently has the same hash but is a newly stored object.
    let same = upload(&f, &owner, &roots[1], b"private image A").await;
    assert_eq!(same["image_id"], *id);
}

#[tokio::test]
async fn project_annotation_roles_headers_and_no_store_cover_every_local_operation() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let member = f.cookie("member", Role::Viewer);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    let (id, revision, export) = seed(&f, &owner, &root, b"image", 100.0).await;
    grant(&f, &owner, &org, &p, "viewer", json!({"valid_from":0})).await;
    for (route, body) in writes(&id, &revision) {
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
    for (cookie, marker) in [(&member, true), (&owner, false)] {
        assert_eq!(
            raw(
                &f,
                "POST",
                &format!("{root}/annotation-blobs"),
                Some(cookie),
                Body::from("new"),
                marker
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
    }
    for route in reads(&id, &revision, &export) {
        let response = raw(
            &f,
            "GET",
            &format!("{root}/{route}"),
            Some(&member),
            Body::empty(),
            false,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
    grant(&f, &owner, &org, &p, "editor", json!({"valid_from":0})).await;
    for (route, body) in writes(&id, &revision) {
        let (status, response) = f
            .request(
                "POST",
                &format!("{root}/{route}"),
                Some(&member),
                body,
                true,
            )
            .await;
        assert!(status.is_success(), "{route}: {response}");
    }
    upload(&f, &member, &root, b"editor's new bytes").await;
    let (status, authored) = f
        .request(
            "POST",
            &format!("{root}/annotation-revisions"),
            Some(&member),
            drawing(&id, 120.0),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(authored["revision"]["author"], "member");

    let (_,review)=f.request("POST",&format!("{root}/annotation-reviews"),Some(&member),json!({"project":ANNOTATION_PROJECT,"image_id":id,"review":"accepted","revision":revision,"note":"human review"}),true).await;
    assert_eq!(review["reviewed_by"], "member");
    // Unsupported operations are not silently routed through global services.
    for route in ["annotation-imports", "annotation-sources"] {
        assert_eq!(
            raw(
                &f,
                if route.ends_with("imports") {
                    "POST"
                } else {
                    "GET"
                },
                &format!("{root}/{route}"),
                Some(&owner),
                Body::empty(),
                true
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
    }
}

#[tokio::test]
async fn project_image_bytes_recheck_grant_windows_and_revocation_in_one_session() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let member = f.cookie("member", Role::Viewer);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    let (id, revision, export) = seed(&f, &owner, &root, b"secret bytes", 100.0).await;
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
        let response = raw(
            &f,
            "GET",
            &format!("{root}/annotation-blobs/{id}"),
            Some(&member),
            Body::empty(),
            false,
        )
        .await;
        assert_eq!(response.status(), read_status);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        for route in reads(&id, &revision, &export) {
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
                &format!("{root}/annotation-projects"),
                Some(&member),
                annotation_project(),
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
        raw(
            &f,
            "GET",
            &format!("{root}/annotation-blobs/{id}"),
            Some(&member),
            Body::empty(),
            false
        )
        .await
        .status(),
        StatusCode::OK
    );
    command(
        &f,
        &owner,
        &org,
        json!({"type":"revoke_grant","project":p,"grant":permanent}),
    )
    .await;
    assert_eq!(
        raw(
            &f,
            "GET",
            &format!("{root}/annotation-blobs/{id}"),
            Some(&member),
            Body::empty(),
            false
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    for (route, body) in writes(&id, &revision) {
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
async fn annotation_json_and_binary_uploads_recheck_access_before_storage() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let member = f.cookie("member", Role::Viewer);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    let (id, revision, export) = seed(&f, &owner, &root, b"image", 100.0).await;
    grant(
        &f,
        &owner,
        &org,
        &p,
        "editor",
        json!({"valid_from":0,"edit_until":1001,"read_until":1002}),
    )
    .await;
    let mut snapshots = Vec::new();
    for route in reads(&id, &revision, &export) {
        snapshots.push(
            f.request(
                "GET",
                &format!("{root}/{route}"),
                Some(&owner),
                Value::Null,
                false,
            )
            .await
            .1,
        );
    }
    for (route, body) in writes(&id, &revision) {
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
    f.clock.0.store(1000, std::sync::atomic::Ordering::SeqCst);
    let clock = f.clock.clone();
    let stream = Body::from_stream(futures::stream::once(async move {
        clock.0.store(1001, std::sync::atomic::Ordering::SeqCst);
        Ok::<_, std::io::Error>("new binary after expiry")
    }));
    assert_eq!(
        raw(
            &f,
            "POST",
            &format!("{root}/annotation-blobs"),
            Some(&member),
            stream,
            true
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    for (route, before) in reads(&id, &revision, &export).into_iter().zip(snapshots) {
        assert_eq!(
            f.request(
                "GET",
                &format!("{root}/{route}"),
                Some(&owner),
                Value::Null,
                false
            )
            .await
            .1,
            before
        );
    }
    // A new upload by the owner must create it; the refused upload left no blob/sidecar.
    upload(&f, &owner, &root, b"new binary after expiry").await;
}

#[tokio::test]
async fn project_annotations_reject_foreign_blob_references_and_fail_closed_without_iam() {
    let mut f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    f.request(
        "POST",
        &format!("{root}/annotation-projects"),
        Some(&owner),
        annotation_project(),
        true,
    )
    .await;
    let legacy = upload(&f, &owner, "/api/v1", b"legacy bytes").await;
    let id = legacy["image_id"].as_str().unwrap();
    assert_eq!(
        f.request(
            "POST",
            &format!("{root}/annotation-images"),
            Some(&owner),
            registered_image(id),
            true
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let blob = upload(&f, &owner, &root, b"private bytes").await;
    let private = blob["image_id"].as_str().unwrap();
    let mut mismatch = registered_image(private);
    mismatch["uri"] = legacy["uri"].clone();
    assert_eq!(
        f.request(
            "POST",
            &format!("{root}/annotation-images"),
            Some(&owner),
            mismatch,
            true
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        raw(
            &f,
            "GET",
            &format!("{root}/annotation-blobs/{private}"),
            None,
            Body::empty(),
            false
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.request(
            "GET",
            "/api/v1/orgs/invalid/projects/invalid/annotation-projects",
            Some(&owner),
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let registry = f.fixture.state.annotations.take();
    assert_eq!(
        f.request(
            "GET",
            &format!("{root}/annotation-projects"),
            Some(&owner),
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::NOT_IMPLEMENTED
    );
    f.fixture.state.annotations = registry;
    f.fixture.state.auth = None;
    assert_eq!(
        f.request(
            "GET",
            &format!("{root}/annotation-projects"),
            None,
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    f.fixture.state.iam = None;
    assert_eq!(
        f.request(
            "GET",
            &format!("{root}/annotation-projects"),
            None,
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::NOT_IMPLEMENTED
    );
    assert_eq!(
        raw(
            &f,
            "GET",
            &format!("/api/v1/annotation-blobs/{private}"),
            None,
            Body::empty(),
            false
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let (_, page) = f
        .request(
            "GET",
            "/api/v1/annotation-projects",
            None,
            Value::Null,
            false,
        )
        .await;
    assert!(page["projects"].as_array().unwrap().is_empty());
}
