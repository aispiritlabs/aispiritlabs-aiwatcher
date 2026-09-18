use super::*;
use aiwatcher_evaluation::{EvaluationManifest, SourceAuthority, SourceEvidence};

// HTTP tests exercise IAM and scoped registry storage. Server integration tests
// separately exercise LocalSource against actual project-owned bytes.
#[derive(Debug, Clone, Default)]
struct ProjectSource(Option<aiwatcher_iam::ProjectScope>);
#[async_trait::async_trait]
impl SourceAuthority for ProjectSource {
    fn for_project_evidence(
        &self,
        scope: aiwatcher_iam::ProjectScope,
    ) -> aiwatcher_evaluation::Result<Arc<dyn SourceAuthority>> {
        assert!(self.0.is_none_or(|current| current == scope));
        Ok(Arc::new(Self(Some(scope))))
    }
    async fn resolve(
        &self,
        manifest: &EvaluationManifest,
        subject: &str,
    ) -> aiwatcher_evaluation::Result<SourceEvidence> {
        EvaluationSource::default().resolve(manifest, subject).await
    }
}
pub(super) async fn fixture() -> IamFixture {
    let mut f = IamFixture::new().await;
    f.fixture.state.evaluations = Some(Arc::new(
        aiwatcher_evaluation::Registry::new(
            Arc::new(MemoryObjectStore::new()),
            Arc::new(ProjectSource::default()),
            Default::default(),
        )
        .unwrap(),
    ));
    f
}
pub(super) async fn approve(f: &IamFixture, cookie: &str, root: &str, body: &Value) -> Value {
    let (status, value) = f
        .request(
            "POST",
            &format!("{root}/evaluation-approvals"),
            Some(cookie),
            body["manifest"].clone(),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    value
}
pub(super) async fn publish(f: &IamFixture, cookie: &str, root: &str, body: Value) -> Value {
    let (status, value) = f
        .request(
            "POST",
            &format!("{root}/evaluation-results"),
            Some(cookie),
            body,
            true,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    value
}

#[tokio::test]
async fn project_evidence_fails_closed_without_auth_iam_or_a_scoped_resolver() {
    let mut f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    let body = durable_request("hidden");
    approve(&f, &owner, &root, &body).await;
    publish(&f, &owner, &root, body.clone()).await;
    for (method, suffix, value) in [
        ("GET", "evaluation-results", Value::Null),
        ("GET", "evaluation-approvals", Value::Null),
        ("POST", "evaluation-results", body.clone()),
        ("POST", "evaluation-approvals", body["manifest"].clone()),
        ("DELETE", "evaluation-results/hidden", Value::Null),
    ] {
        assert_eq!(
            f.request(method, &format!("{root}/{suffix}"), None, value, true)
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
    }
    let auth = f.fixture.state.auth.take();
    assert_eq!(
        f.request(
            "GET",
            &format!("{root}/evaluation-results"),
            None,
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    f.fixture.state.auth = auth;
    f.fixture.state.iam = None;
    assert_eq!(
        f.request(
            "GET",
            &format!("{root}/evaluation-results"),
            Some(&owner),
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::NOT_IMPLEMENTED
    );
    assert_eq!(
        f.request(
            "GET",
            "/api/v1/evaluation-results/hidden",
            Some(&owner),
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    f.fixture.state.iam = Some(f.store.clone());
    f.fixture.state.evaluations = Some(Arc::new(
        aiwatcher_evaluation::Registry::new(
            Arc::new(MemoryObjectStore::new()),
            Arc::new(EvaluationSource::default()),
            Default::default(),
        )
        .unwrap(),
    ));
    assert_eq!(
        f.request(
            "GET",
            &format!("{root}/evaluation-results"),
            Some(&owner),
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    f.fixture.state.evaluations = None;
    assert_eq!(
        f.request(
            "GET",
            &format!("{root}/evaluation-results"),
            Some(&owner),
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::NOT_IMPLEMENTED
    );
}

#[tokio::test]
async fn project_evidence_isolates_approvals_results_cases_comparisons_and_deletion() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let outsider = f.cookie("outsider", Role::Admin);
    let org = f.create(&owner).await;
    let other_org = f.create(&outsider).await;
    let a = project(&f, &owner, &org).await;
    let b = project(&f, &owner, &org).await;
    let c = project(&f, &outsider, &other_org).await;
    let root = base(&org, &a);
    let body = durable_request("same-id");
    assert_eq!(
        f.request(
            "POST",
            &format!("{root}/evaluation-results"),
            Some(&owner),
            body.clone(),
            true
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let admission = approve(&f, &owner, &root, &body).await;
    let id = admission["record"]["approval_id"].as_str().unwrap();
    let receipt = publish(&f, &owner, &root, body.clone()).await;
    let version = receipt["version"].as_str().unwrap();
    assert_eq!(publish(&f, &owner, &root, body.clone()).await, receipt);
    let baseline = durable_request("baseline");
    publish(&f, &owner, &root, baseline).await;
    for suffix in [
        "evaluation-results".to_owned(),
        "evaluation-approvals".into(),
        "evaluation-results/same-id".into(),
        format!("evaluation-results/same-id/cases?version={version}&limit=1"),
        "evaluation-results/same-id/comparison?baseline=baseline".into(),
        "evaluation-results/same-id/comparison/cases?baseline=baseline&limit=1".into(),
    ] {
        let response = f
            .fixture
            .router()
            .oneshot(
                Request::builder()
                    .uri(format!("{root}/{suffix}"))
                    .header(header::COOKIE, &owner)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{suffix}");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
    for (other, cookie) in [
        (base(&org, &b), &owner),
        (base(&other_org, &c), &outsider),
        ("/api/v1".into(), &owner),
    ] {
        for suffix in [
            "evaluation-results/same-id".to_owned(),
            format!("evaluation-results/same-id/cases?version={version}"),
            "evaluation-results/same-id/comparison?baseline=baseline".into(),
            "evaluation-results/same-id/comparison/cases?baseline=baseline".into(),
        ] {
            assert_eq!(
                f.request(
                    "GET",
                    &format!("{other}/{suffix}"),
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
            f.request(
                "DELETE",
                &format!("{other}/evaluation-approvals/{id}"),
                Some(cookie),
                Value::Null,
                true
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            f.request(
                "DELETE",
                &format!("{other}/evaluation-results/same-id"),
                Some(cookie),
                Value::Null,
                true
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            f.request(
                "POST",
                &format!("{other}/evaluation-results"),
                Some(cookie),
                body.clone(),
                true
            )
            .await
            .0,
            StatusCode::CONFLICT
        );
        approve(&f, cookie, &other, &body).await;
        publish(&f, cookie, &other, body.clone()).await;
    }
    assert_eq!(
        f.request(
            "GET",
            &format!("{root}/evaluation-results/same-id"),
            Some(&outsider),
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.request(
            "DELETE",
            &format!("{root}/evaluation-approvals/{id}"),
            Some(&owner),
            Value::Null,
            true
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.request(
            "POST",
            &format!("{root}/evaluation-results"),
            Some(&owner),
            body.clone(),
            true
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (_, hidden) = f
        .request(
            "GET",
            &format!("{root}/evaluation-results/same-id"),
            Some(&owner),
            Value::Null,
            false,
        )
        .await;
    assert_eq!(hidden["state"], "forbidden");
    let (_, other) = f
        .request(
            "GET",
            &format!("{}/evaluation-results/same-id", base(&org, &b)),
            Some(&owner),
            Value::Null,
            false,
        )
        .await;
    assert_eq!(other["state"], "complete");
    assert_eq!(
        f.request(
            "DELETE",
            &format!("{root}/evaluation-results/same-id"),
            Some(&owner),
            Value::Null,
            true
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn project_evidence_roles_headers_expiry_and_revocation_are_checked_on_every_request() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let member = f.cookie("member", Role::Viewer);
    let instance_admin = f.cookie("member", Role::Admin);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    let body = durable_request("roles");
    let approval = approve(&f, &owner, &root, &body).await;
    let id = approval["record"]["approval_id"].as_str().unwrap();
    publish(&f, &owner, &root, body.clone()).await;
    assert_eq!(
        f.request(
            "GET",
            &format!("{root}/evaluation-results"),
            Some(&instance_admin),
            Value::Null,
            false
        )
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
                &format!("{root}/evaluation-results"),
                Some(&member),
                Value::Null,
                false
            )
            .await
            .0,
            read
        );
        assert_eq!(
            f.request(
                "POST",
                &format!("{root}/evaluation-results"),
                Some(&member),
                body.clone(),
                true
            )
            .await
            .0,
            write
        );
    }
    f.clock.0.store(1001, std::sync::atomic::Ordering::SeqCst);
    for (method, suffix, value) in [
        (
            "POST",
            "evaluation-approvals".into(),
            body["manifest"].clone(),
        ),
        ("DELETE", format!("evaluation-approvals/{id}"), Value::Null),
        ("DELETE", "evaluation-results/roles".into(), Value::Null),
    ] {
        assert_eq!(
            f.request(
                method,
                &format!("{root}/{suffix}"),
                Some(&instance_admin),
                value.clone(),
                true
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            f.request(
                method,
                &format!("{root}/{suffix}"),
                Some(&owner),
                value,
                false
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        f.request(
            "POST",
            &format!("{root}/evaluation-results"),
            Some(&member),
            body.clone(),
            false
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    command(
        &f,
        &owner,
        &org,
        json!({"type":"revoke_grant","project":p,"grant":grant_id}),
    )
    .await;
    assert_eq!(
        f.request(
            "GET",
            &format!("{root}/evaluation-results/roles"),
            Some(&member),
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    grant(&f, &owner, &org, &p, "admin", json!({"valid_from":0})).await;
    approve(&f, &member, &root, &body).await; // instance Viewer, project Admin
}

#[tokio::test]
async fn evidence_upload_rechecks_admin_and_editor_after_reading_the_body() {
    for admin in [false, true] {
        let f = fixture().await;
        let owner = f.cookie("owner", Role::Admin);
        let member = f.cookie("member", Role::Admin);
        let org = f.create(&owner).await;
        let p = project(&f, &owner, &org).await;
        let root = base(&org, &p);
        let body = durable_request("delayed");
        grant(
            &f,
            &owner,
            &org,
            &p,
            if admin { "admin" } else { "editor" },
            json!({"valid_from":0,"edit_until":1001,"read_until":1003}),
        )
        .await;
        if admin {
            grant(&f, &owner, &org, &p, "editor", json!({"valid_from":0})).await;
        } else {
            approve(&f, &owner, &root, &body).await;
        }
        let bytes = if admin {
            body["manifest"].to_string()
        } else {
            body.to_string()
        };
        let clock = f.clock.clone();
        let upload = Body::from_stream(futures::stream::once(async move {
            clock.0.store(1001, std::sync::atomic::Ordering::SeqCst);
            Ok::<_, std::io::Error>(bytes)
        }));
        let suffix = if admin {
            "evaluation-approvals"
        } else {
            "evaluation-results"
        };
        let (status, value) = f
            .fixture
            .request(
                Request::builder()
                    .method("POST")
                    .uri(format!("{root}/{suffix}"))
                    .header(header::COOKIE, member)
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("x-aiwatcher-iam", "1")
                    .body(upload)
                    .unwrap(),
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{value}");
        assert_eq!(
            f.request(
                "GET",
                &format!("{root}/evaluation-results/delayed"),
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
