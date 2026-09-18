//! HTTP authorization; native source/byte checks use the real server adapter tests.
use super::project_cohorts::{derive, fixture, source};
use super::*;

async fn seed(f: &IamFixture, cookie: &str, root: &str) -> Value {
    let request = source(f, cookie, root, "question").await;
    let cohort = derive(f, cookie, root, request.clone()).await;
    let (status, card) = f.request("POST", &format!("{root}/evaluation-scorecards"), Some(cookie), json!({
        "name":"quality", "scorers":[{"metric":"exact", "scorer":{"kind":"exact_match", "ignore_case":false,"trim":false}}]
    }), true).await;
    assert_eq!(status, StatusCode::OK, "{card}");
    let (status, recording) = f
        .request(
            "PUT",
            &format!("{root}/evaluation-recordings/answers"),
            Some(cookie),
            json!({"answers":[{"case_id":"one", "answer":"answer"}]}),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{recording}");
    let mut variant = durable_request("declaration")["manifest"]["variant"].clone();
    variant["dataset"] = request["dataset"].clone();
    json!({"evaluation_id":"declaration", "repetition_id":"measurement-1", "variant":variant,
        "cohort":cohort["cohort"], "scorecard":{"name":"quality","version":card["version"]}, "answers":recording})
}
async fn declare(f: &IamFixture, cookie: &str, root: &str, body: Value) -> Value {
    let (status, value) = f
        .request(
            "POST",
            &format!("{root}/evaluation-runs"),
            Some(cookie),
            body,
            true,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    value
}
fn id(value: &Value) -> &str {
    value["declaration"]["id"].as_str().unwrap()
}

#[tokio::test]
async fn declaration_http_is_project_local_idempotent_and_never_starts_a_measurement() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let outsider = f.cookie("outsider", Role::Admin);
    let org = f.create(&owner).await;
    let other_org = f.create(&outsider).await;
    let a = project(&f, &owner, &org).await;
    let b = project(&f, &owner, &org).await;
    let c = project(&f, &outsider, &other_org).await;
    let root = base(&org, &a);
    let body = seed(&f, &owner, &root).await;
    let declared = declare(&f, &owner, &root, body.clone()).await;
    assert_eq!(declared["admitted"], false);
    assert_eq!(declared["declaration"]["declared_by"], "owner");
    assert_eq!(declare(&f, &owner, &root, body.clone()).await, declared);
    let url = format!("{root}/evaluation-runs/{}", id(&declared));
    assert_eq!(
        f.request("GET", &url, Some(&owner), Value::Null, false)
            .await
            .1,
        declared
    );
    // A project declaration cannot be started through the global route either.
    assert_eq!(
        f.request(
            "POST",
            &format!("/api/v1/evaluation-runs/{}/start", id(&declared)),
            Some(&owner),
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    for (other_root, cookie, subject) in [
        (base(&org, &b), &owner, "owner"),
        (base(&other_org, &c), &outsider, "outsider"),
        ("/api/v1".into(), &owner, "owner"),
    ] {
        assert_eq!(
            f.request(
                "GET",
                &format!("{other_root}/evaluation-runs/{}", id(&declared)),
                Some(cookie),
                Value::Null,
                false
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            f.request(
                "POST",
                &format!("{other_root}/evaluation-runs"),
                Some(cookie),
                body.clone(),
                true
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        // Identical declarations are independently present, not copied on read.
        assert_eq!(seed(&f, cookie, &other_root).await, body);
        let local = declare(&f, cookie, &other_root, body.clone()).await;
        assert_eq!(id(&local), id(&declared));
        assert_eq!(local["declaration"]["declared_by"], subject);
    }
    for (method, path, payload) in [
        ("GET", url.clone(), String::new()),
        ("POST", format!("{root}/evaluation-runs"), body.to_string()),
    ] {
        let response = f
            .fixture
            .router()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(header::COOKIE, &owner)
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("x-aiwatcher-iam", "1")
                    .body(Body::from(payload))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
    assert_eq!(
        f.request(
            "POST",
            &format!("{url}/start"),
            Some(&owner),
            Value::Null,
            true
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let mut unsupported = body.clone();
    unsupported["answers"] =
        json!({"generated_by":{"task":"task@1","queue":"workers","params":{}}});
    let (status, _) = f
        .request(
            "POST",
            &format!("{root}/evaluation-runs"),
            Some(&owner),
            unsupported.clone(),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let run: aiwatcher_evaluation::ScoringRun = serde_json::from_value(unsupported).unwrap();
    assert_eq!(
        f.request(
            "GET",
            &format!("{root}/evaluation-runs/{}", run.id().unwrap()),
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
async fn declaration_http_rechecks_current_grants_after_upload_and_never_uses_instance_admin_as_project_access()
 {
    let mut f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let member = f.cookie("member", Role::Viewer);
    let instance_admin = f.cookie("member", Role::Admin);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    let body = seed(&f, &owner, &root).await;
    let declared = declare(&f, &owner, &root, body.clone()).await;
    let url = format!("{root}/evaluation-runs/{}", id(&declared));
    let write = format!("{root}/evaluation-runs");
    assert_eq!(
        f.request("GET", &url, Some(&instance_admin), Value::Null, false)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.request("POST", &write, Some(&instance_admin), body.clone(), true)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let grant_id = grant(
        &f,
        &owner,
        &org,
        &p,
        "editor",
        json!({"valid_from":1001,"edit_until":1003,"read_until":1005}),
    )
    .await;
    for (time, read, write_status) in [
        (1000, StatusCode::NOT_FOUND, StatusCode::NOT_FOUND),
        (1001, StatusCode::OK, StatusCode::OK),
        (1003, StatusCode::OK, StatusCode::FORBIDDEN),
        (1005, StatusCode::NOT_FOUND, StatusCode::NOT_FOUND),
    ] {
        f.clock.0.store(time, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            f.request("GET", &url, Some(&member), Value::Null, false)
                .await
                .0,
            read
        );
        assert_eq!(
            f.request("POST", &write, Some(&member), body.clone(), true)
                .await
                .0,
            write_status
        );
    }
    f.clock.0.store(1001, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        f.request("POST", &write, Some(&member), body.clone(), false)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let mut fresh = body.clone();
    fresh["evaluation_id"] = json!("slow-upload");
    let run: aiwatcher_evaluation::ScoringRun = serde_json::from_value(fresh.clone()).unwrap();
    let clock = f.clock.clone();
    let upload = Body::from_stream(futures::stream::once(async move {
        clock.0.store(1003, std::sync::atomic::Ordering::SeqCst);
        Ok::<_, std::io::Error>(fresh.to_string())
    }));
    let response = f
        .fixture
        .router()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&write)
                .header(header::COOKIE, &instance_admin)
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-aiwatcher-iam", "1")
                .body(upload)
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(
        f.request(
            "GET",
            &format!("{root}/evaluation-runs/{}", run.id().unwrap()),
            Some(&owner),
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    f.clock.0.store(1001, std::sync::atomic::Ordering::SeqCst);
    command(
        &f,
        &owner,
        &org,
        json!({"type":"revoke_grant","project":p,"grant":grant_id}),
    )
    .await;
    assert_eq!(
        f.request("GET", &url, Some(&member), Value::Null, false)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.request("POST", &write, Some(&member), body, true).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.request("GET", &url, None, Value::Null, false).await.0,
        StatusCode::UNAUTHORIZED
    );
    f.fixture.state.iam = None;
    assert_eq!(
        f.request("GET", &url, Some(&owner), Value::Null, false)
            .await
            .0,
        StatusCode::NOT_IMPLEMENTED
    );
}
