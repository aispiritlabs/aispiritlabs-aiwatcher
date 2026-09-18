use super::project_evidence::{approve, fixture, publish};
use super::*;

async fn post(f: &IamFixture, cookie: &str, root: &str, suffix: &str, body: Value) -> Value {
    let (status, value) = f
        .request(
            "POST",
            &format!("{root}/{suffix}"),
            Some(cookie),
            body,
            true,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    value
}
async fn rubric(f: &IamFixture, cookie: &str, root: &str) -> Value {
    let rubric = post(f, cookie, root, "evaluation-rubrics", json!({
        "name":"helpful", "question":"Does it help?", "scale":{"kind":"flag"}, "direction":"higher"
    })).await;
    json!({"name":"people", "evaluation_id":"people-judged", "rubrics":[{"name":"helpful", "version":rubric["version"]}]})
}
async fn assess(f: &IamFixture, cookie: &str, root: &str, request: &Value, value: bool) {
    post(f, cookie, root, "evaluation-assessments", json!({
        "target":{"kind":"case","evaluation_id":"people-judged","case_id":"capital-pl","repetition_id":"measurement-1"},
        "rubric":"helpful", "rubric_version":request["rubrics"][0]["version"],
        "value":{"type":"flag","value":value}
    })).await;
}
async fn seed(f: &IamFixture, cookie: &str, root: &str) -> Value {
    let body = durable_request("people-judged");
    approve(f, cookie, root, &body).await;
    publish(f, cookie, root, body).await;
    let request = rubric(f, cookie, root).await;
    assess(f, cookie, root, &request, true).await;
    request
}

#[tokio::test]
async fn calibration_reads_only_its_project_results_rubrics_and_people_and_freezes_their_words() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let other = f.cookie("other", Role::Admin);
    let org = f.create(&owner).await;
    let other_org = f.create(&other).await;
    let a = project(&f, &owner, &org).await;
    let b = project(&f, &owner, &org).await;
    let c = project(&f, &other, &other_org).await;
    let root = base(&org, &a);
    let request = seed(&f, &owner, &root).await;
    let taken = post(
        &f,
        &owner,
        &root,
        "evaluation-calibrations",
        request.clone(),
    )
    .await;
    let version = taken["version"].as_str().unwrap();
    assert_eq!(taken["calibration"]["items"].as_array().unwrap().len(), 1);
    let typed: aiwatcher_evaluation::CalibrationVersion =
        serde_json::from_value(taken.clone()).unwrap();
    assert!(!typed.calibration.from_archive);
    assert_eq!(
        post(
            &f,
            &owner,
            &root,
            "evaluation-calibrations",
            request.clone()
        )
        .await,
        taken
    );
    for (other_root, cookie) in [
        (base(&org, &b), &owner),
        (base(&other_org, &c), &other),
        ("/api/v1".into(), &owner),
    ] {
        let url = format!("{other_root}/evaluation-calibrations/{version}");
        assert_eq!(
            f.request("GET", &url, Some(cookie), Value::Null, false)
                .await
                .0,
            StatusCode::NOT_FOUND
        );
        // The pinned rubric and then the result must both exist locally.
        assert_eq!(
            f.request(
                "POST",
                &format!("{other_root}/evaluation-calibrations"),
                Some(cookie),
                request.clone(),
                true
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
        rubric(&f, cookie, &other_root).await;
        assert_eq!(
            f.request(
                "POST",
                &format!("{other_root}/evaluation-calibrations"),
                Some(cookie),
                request.clone(),
                true
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
        let body = durable_request("people-judged");
        approve(&f, cookie, &other_root, &body).await;
        publish(&f, cookie, &other_root, body).await;
        // Identical result IDs do not import somebody else's assessments.
        assert_eq!(
            f.request(
                "POST",
                &format!("{other_root}/evaluation-calibrations"),
                Some(cookie),
                request.clone(),
                true
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
        assess(&f, cookie, &other_root, &request, true).await;
        let local = post(
            &f,
            cookie,
            &other_root,
            "evaluation-calibrations",
            request.clone(),
        )
        .await;
        assert_eq!(local["calibration"]["items"].as_array().unwrap().len(), 1);
    }
    assess(&f, &owner, &root, &request, false).await;
    let changed = post(
        &f,
        &owner,
        &root,
        "evaluation-calibrations",
        request.clone(),
    )
    .await;
    assert_ne!(changed["version"], taken["version"]);
    let url = format!("{root}/evaluation-calibrations/{version}");
    assert_eq!(
        f.request("GET", &url, Some(&owner), Value::Null, false)
            .await
            .1,
        taken
    );
    let response = f
        .fixture
        .router()
        .oneshot(
            Request::builder()
                .uri(&url)
                .header(header::COOKIE, &owner)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let admission = approve(&f, &owner, &root, &durable_request("people-judged")).await;
    f.request(
        "DELETE",
        &format!(
            "{root}/evaluation-approvals/{}",
            admission["record"]["approval_id"].as_str().unwrap()
        ),
        Some(&owner),
        Value::Null,
        true,
    )
    .await;
    assert_eq!(
        f.request(
            "POST",
            &format!("{root}/evaluation-calibrations"),
            Some(&owner),
            request,
            true
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    // A frozen set holds judgements, not case content. It remains readable.
    assert_eq!(
        f.request("GET", &url, Some(&owner), Value::Null, false)
            .await
            .1,
        taken
    );
}

#[tokio::test]
async fn calibration_grants_are_current_and_upload_rechecks_the_editor_role() {
    let mut f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let member = f.cookie("member", Role::Viewer);
    let instance_admin = f.cookie("member", Role::Admin);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    let request = seed(&f, &owner, &root).await;
    let set = post(
        &f,
        &owner,
        &root,
        "evaluation-calibrations",
        request.clone(),
    )
    .await;
    let url = format!(
        "{root}/evaluation-calibrations/{}",
        set["version"].as_str().unwrap()
    );
    assert_eq!(
        f.request("GET", &url, Some(&instance_admin), Value::Null, false)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let id = grant(
        &f,
        &owner,
        &org,
        &p,
        "editor",
        json!({"valid_from":1001,"edit_until":1003,"read_until":1005}),
    )
    .await;
    for (time, read, write) in [
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
            f.request(
                "POST",
                &format!("{root}/evaluation-calibrations"),
                Some(&member),
                request.clone(),
                true
            )
            .await
            .0,
            write
        );
    }
    f.clock.0.store(1001, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        f.request(
            "POST",
            &format!("{root}/evaluation-calibrations"),
            Some(&member),
            request.clone(),
            false
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let clock = f.clock.clone();
    let upload = Body::from_stream(futures::stream::once(async move {
        clock.0.store(1003, std::sync::atomic::Ordering::SeqCst);
        Ok::<_, std::io::Error>(request.to_string())
    }));
    let response = f
        .fixture
        .router()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("{root}/evaluation-calibrations"))
                .header(header::COOKIE, &instance_admin)
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-aiwatcher-iam", "1")
                .body(upload)
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    f.clock.0.store(1001, std::sync::atomic::Ordering::SeqCst);
    command(
        &f,
        &owner,
        &org,
        json!({"type":"revoke_grant","project":p,"grant":id}),
    )
    .await;
    assert_eq!(
        f.request("GET", &url, Some(&member), Value::Null, false)
            .await
            .0,
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
