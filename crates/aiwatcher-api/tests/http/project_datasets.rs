use super::*;

async fn fixture() -> IamFixture {
    let mut f = IamFixture::new().await;
    f.fixture.state.datasets = Some(Arc::new(DatasetRegistry::new(
        Arc::new(MemoryObjectStore::new()),
        "datasets",
    )));
    f
}
fn dataset(value: &str) -> Value {
    json!({"name":"shared/name","pipeline":"data_frame()->read(default)","columns":["value"],
        "items":[{"value":value}],"source":"fixture"})
}
#[tokio::test]
async fn project_datasets_isolate_bytes_names_versions_and_legacy_routes() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let outsider = f.cookie("outsider", Role::Admin);
    let org = f.create(&owner).await;
    let other_org = f.create(&outsider).await;
    let a = project(&f, &owner, &org).await;
    let b = project(&f, &owner, &org).await;
    let c = project(&f, &outsider, &other_org).await;
    let paths = [
        base(&org, &a),
        base(&org, &b),
        base(&other_org, &c),
        "/api/v1".into(),
    ];
    for (path, cookie, value) in [
        (&paths[0], &owner, "secret A"),
        (&paths[1], &owner, "secret B"),
        (&paths[2], &outsider, "secret C"),
        (&paths[3], &owner, "legacy"),
    ] {
        let (status, body) = f
            .request(
                "POST",
                &format!("{path}/datasets"),
                Some(cookie),
                dataset(value),
                true,
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    let mut versions = Vec::new();
    for (path, cookie, value) in [
        (&paths[0], &owner, "secret A"),
        (&paths[1], &owner, "secret B"),
        (&paths[2], &outsider, "secret C"),
        (&paths[3], &owner, "legacy"),
    ] {
        let (status, page) = f
            .request(
                "GET",
                &format!("{path}/datasets"),
                Some(cookie),
                Value::Null,
                false,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{page}");
        assert_eq!(page["datasets"].as_array().unwrap().len(), 1);
        versions.push(
            page["datasets"][0]["latest"]["version"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
        let (status, rows) = f
            .request(
                "GET",
                &format!(
                    "{path}/dataset-rows?name=shared/name&search={}",
                    value.replace(' ', "%20")
                ),
                Some(cookie),
                Value::Null,
                false,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{rows}");
        assert_eq!(rows["rows"][0]["row"]["value"], value);
    }
    // A known content hash is not a cross-project capability, including legacy.
    for path in [&paths[1], &paths[3]] {
        assert_eq!(
            f.request(
                "GET",
                &format!(
                    "{path}/dataset-rows?name=shared/name&version={}",
                    versions[0]
                ),
                Some(&owner),
                Value::Null,
                false
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }
    for path in [paths[0].clone(), base(&other_org, &a)] {
        for (method, suffix, body) in [
            ("GET", "datasets", Value::Null),
            ("GET", "dataset-rows?name=shared/name", Value::Null),
            ("POST", "datasets", dataset("overwrite")),
        ] {
            assert_eq!(
                f.request(
                    method,
                    &format!("{path}/{suffix}"),
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
    // Physical namespace survives reconnection, does not affect content hash.
    let (_, same) = f
        .request(
            "POST",
            &format!("{}/datasets", paths[1]),
            Some(&owner),
            dataset("secret A"),
            true,
        )
        .await;
    assert_eq!(same["dataset"]["latest"]["version"], versions[0]);
    assert_eq!(same["created"], true);
}

#[tokio::test]
async fn project_registry_rechecks_grants_expiry_and_mutation_header() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let member = f.cookie("member", Role::Viewer); // Project editor beats instance viewer.
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let path = format!("{}/datasets", base(&org, &p));
    let id = grant(
        &f,
        &owner,
        &org,
        &p,
        "editor",
        json!({"valid_from":1001,"edit_until":1003,"read_until":1005}),
    )
    .await;
    assert_eq!(
        f.request("GET", &path, Some(&member), Value::Null, false)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    f.clock.0.store(1001, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        f.request("POST", &path, Some(&member), dataset("no header"), false)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.request("POST", &path, Some(&member), dataset("allowed"), true)
            .await
            .0,
        StatusCode::CREATED
    );
    f.clock.0.store(1003, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        f.request("POST", &path, Some(&member), dataset("expired edit"), true)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.request("GET", &path, Some(&member), Value::Null, false)
            .await
            .0,
        StatusCode::OK
    );
    f.clock.0.store(1005, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        f.request("GET", &path, Some(&member), Value::Null, false)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let permanent = grant(&f, &owner, &org, &p, "editor", json!({"valid_from":0})).await;
    command(
        &f,
        &owner,
        &org,
        json!({"type":"revoke_grant","project":p,"grant":id}),
    )
    .await;
    assert_eq!(
        f.request("GET", &path, Some(&member), Value::Null, false)
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
    assert_eq!(
        f.request("GET", &path, Some(&member), Value::Null, false)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.request("POST", &path, Some(&member), dataset("revoked"), true)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn all_curation_routes_use_the_same_project_boundary() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let outsider = f.cookie("outsider", Role::Admin);
    let org = f.create(&owner).await;
    let a = project(&f, &owner, &org).await;
    let b = project(&f, &owner, &org).await;
    let root = base(&org, &a);
    let pipeline = json!({"name":"flow","blocks":[
        {"id":"source","title":"Input","spec":{"kind":"source","dataset":"runs"}},
        {"id":"publish","title":"Output","spec":{"kind":"view","dataset":"result"}}
    ],"edges":[{"from":"source","to":"publish"}]});
    let library = json!({"id":"solution","title":"Solution","description":"Saved","tags":[],"spec":{"kind":"transform","steps":"limit(10)"}});
    let mut sample = dataset("sample");
    sample["name"] = json!("shared/name/samples");
    sample["sample"] = json!({"mode":"preview","truncated_stages":[]});
    let mut revision = String::new();
    for (route, field, body) in [
        (
            "curations",
            "recipes",
            json!({"name":"recipe","pipeline":"data_frame()->read(default)"}),
        ),
        ("curation-pipelines", "pipelines", pipeline),
        ("curation-library", "templates", library),
        ("dataset-samples", "datasets", sample),
    ] {
        let (status, saved) = f
            .request(
                "POST",
                &format!("{root}/{route}"),
                Some(&owner),
                body.clone(),
                true,
            )
            .await;
        assert!(status.is_success(), "{route}: {status} {saved}");
        if route == "curation-pipelines" {
            revision = saved["pipeline"]["revision"].as_str().unwrap().into();
        }
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
        let read_route = if route == "dataset-samples" {
            "datasets"
        } else {
            route
        };
        for (path, count) in [
            (root.clone(), 1),
            (base(&org, &b), 0),
            ("/api/v1".into(), 0),
        ] {
            let (status, page) = f
                .request(
                    "GET",
                    &format!("{path}/{read_route}"),
                    Some(&owner),
                    Value::Null,
                    false,
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{page}");
            assert_eq!(
                page[field].as_array().unwrap().len(),
                count,
                "{path}/{read_route}: {page}"
            );
        }
        assert_eq!(
            f.request(
                "GET",
                &format!("{root}/{read_route}"),
                Some(&outsider),
                Value::Null,
                false
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }
    for (path, status) in [
        (root.clone(), StatusCode::OK),
        (base(&org, &b), StatusCode::NOT_FOUND),
        ("/api/v1".into(), StatusCode::NOT_FOUND),
    ] {
        assert_eq!(
            f.request(
                "GET",
                &format!("{path}/curation-pipelines/flow/revisions/{revision}"),
                Some(&owner),
                Value::Null,
                false
            )
            .await
            .0,
            status
        );
    }
    let response = f
        .fixture
        .router()
        .oneshot(
            Request::builder()
                .uri(format!("{root}/datasets"))
                .header(header::COOKIE, &owner)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
}

#[tokio::test]
async fn scoped_registry_fails_closed_without_iam_or_oidc() {
    let mut f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let path = format!("{}/datasets", base(&org, &p));
    assert_eq!(
        f.request("GET", &path, None, Value::Null, false).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.request("POST", &path, Some(&owner), dataset("scoped secret"), true)
            .await
            .0,
        StatusCode::CREATED
    );
    let registry = f.fixture.state.datasets.take();
    assert_eq!(
        f.request("GET", &path, Some(&owner), Value::Null, false)
            .await
            .0,
        StatusCode::NOT_IMPLEMENTED
    );
    f.fixture.state.datasets = registry;
    f.fixture.state.auth = None;
    assert_eq!(
        f.request("GET", &path, None, Value::Null, false).await.0,
        StatusCode::UNAUTHORIZED
    );
    f.fixture.state.iam = None;
    assert_eq!(
        f.request("GET", &path, None, Value::Null, false).await.0,
        StatusCode::NOT_IMPLEMENTED
    );
    let (status, legacy) = f
        .request("GET", "/api/v1/datasets", None, Value::Null, false)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        legacy["datasets"].as_array().unwrap().is_empty(),
        "disabling IAM must not expose scoped data through legacy routes"
    );
}

#[tokio::test]
async fn a_grant_expiring_during_body_upload_cannot_publish() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let member = f.cookie("member", Role::Viewer);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let path = format!("{}/datasets", base(&org, &p));
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
        // Polled by JSON extraction, after the request-parts authorization.
        clock.0.store(1001, std::sync::atomic::Ordering::SeqCst);
        Ok::<_, std::io::Error>(dataset("late body").to_string())
    }));
    let (status, _) = f
        .fixture
        .request(
            Request::builder()
                .method("POST")
                .uri(&path)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &member)
                .header("x-aiwatcher-iam", "1")
                .body(body)
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, page) = f
        .request("GET", &path, Some(&owner), Value::Null, false)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(page["datasets"].as_array().unwrap().is_empty());
}
