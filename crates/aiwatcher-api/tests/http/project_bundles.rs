use super::*;
use aiwatcher_evaluation::{ApprovalBundles, StagedFile};
use aiwatcher_iam::ProjectScope;
use std::collections::BTreeMap;

// HTTP admission test double. The production LocalSource and real files are
// exercised separately by the server's project_bundles tests.
#[derive(Debug, Default)]
struct Bundles {
    files: Arc<tokio::sync::Mutex<BTreeMap<String, Vec<u8>>>>,
    scope: Option<ProjectScope>,
    unsupported: bool,
}
impl Bundles {
    fn prefix(&self, id: &str) -> String {
        format!("{:?}/{id}/", self.scope)
    }
}
#[async_trait::async_trait]
impl ApprovalBundles for Bundles {
    fn for_project(
        &self,
        scope: ProjectScope,
    ) -> aiwatcher_evaluation::Result<Arc<dyn ApprovalBundles>> {
        if self.unsupported || self.scope.is_some_and(|current| current != scope) {
            return Err(aiwatcher_evaluation::EvaluationError::Unavailable(
                aiwatcher_evaluation::EvidenceState::Forbidden,
            ));
        }
        Ok(Arc::new(Self {
            files: self.files.clone(),
            scope: Some(scope),
            unsupported: false,
        }))
    }
    async fn stage(
        &self,
        id: &str,
        name: &str,
        bytes: Vec<u8>,
    ) -> aiwatcher_evaluation::Result<StagedFile> {
        let size_bytes = bytes.len() as u64;
        self.files
            .lock()
            .await
            .insert(format!("{}{name}", self.prefix(id)), bytes);
        Ok(StagedFile {
            name: name.into(),
            size_bytes,
        })
    }
    async fn staged(&self, id: &str) -> aiwatcher_evaluation::Result<Vec<StagedFile>> {
        let prefix = self.prefix(id);
        Ok(self
            .files
            .lock()
            .await
            .iter()
            .filter_map(|(key, bytes)| {
                key.strip_prefix(&prefix).map(|name| StagedFile {
                    name: name.into(),
                    size_bytes: bytes.len() as u64,
                })
            })
            .collect())
    }
    async fn discard(&self, id: &str) -> aiwatcher_evaluation::Result<usize> {
        let prefix = self.prefix(id);
        let mut files = self.files.lock().await;
        let count = files.len();
        files.retain(|key, _| !key.starts_with(&prefix));
        Ok(count - files.len())
    }
}
async fn fixture() -> IamFixture {
    let mut f = IamFixture::new().await;
    f.fixture.state.evaluation_bundles = Some(Arc::new(Bundles::default()));
    f
}
fn url(root: &str) -> String {
    format!("{root}/evaluation-approvals/{}/bundle", "a".repeat(64))
}
async fn stage(f: &IamFixture, cookie: &str, root: &str, name: &str, words: &str) -> Value {
    let response = f
        .fixture
        .router()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("{}/{name}", url(root)))
                .header(header::COOKIE, cookie)
                .header("x-aiwatcher-iam", "1")
                .body(Body::from(words.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    if root != "/api/v1" {
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
    serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap(),
    )
    .unwrap()
}
async fn list(f: &IamFixture, cookie: &str, root: &str) -> Value {
    let (status, result) = f
        .request("GET", &url(root), Some(cookie), Value::Null, false)
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    result
}

#[tokio::test]
async fn project_bundles_isolate_staging_lists_and_deletion_across_scopes() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let outsider = f.cookie("outsider", Role::Admin);
    let org = f.create(&owner).await;
    let other = f.create(&outsider).await;
    let a = project(&f, &owner, &org).await;
    let b = project(&f, &owner, &org).await;
    let c = project(&f, &outsider, &other).await;
    let root = base(&org, &a);
    let file = stage(&f, &owner, &root, "model-artifacts/weights.bin", "a secret").await;
    assert_eq!(
        file,
        json!({"name":"model-artifacts/weights.bin", "size_bytes":8})
    );
    assert_eq!(
        stage(&f, &owner, &root, "model-artifacts/weights.bin", "a secret").await,
        file
    );
    for (other_root, cookie) in [
        (base(&org, &b), &owner),
        (base(&other, &c), &outsider),
        ("/api/v1".into(), &owner),
    ] {
        assert_eq!(list(&f, cookie, &other_root).await, json!([]));
        assert_eq!(
            f.request("DELETE", &url(&other_root), Some(cookie), Value::Null, true)
                .await
                .0,
            StatusCode::NO_CONTENT
        );
        assert_eq!(list(&f, &owner, &root).await, json!([file.clone()]));
        stage(&f, cookie, &other_root, "manifest.json", "other").await;
    }
    for (target, cookie) in [(root.clone(), &outsider), (base(&other, &a), &owner)] {
        for (method, path) in [
            ("GET", url(&target)),
            ("DELETE", url(&target)),
            ("PUT", format!("{}/manifest.json", url(&target))),
        ] {
            assert_eq!(
                f.request(method, &path, Some(cookie), Value::Null, true)
                    .await
                    .0,
                StatusCode::NOT_FOUND
            );
        }
    }
    let response = f
        .fixture
        .router()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(url(&root))
                .header(header::COOKIE, &owner)
                .header("x-aiwatcher-iam", "1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(list(&f, &owner, &root).await, json!([]));
    for (other_root, cookie) in [
        (base(&org, &b), &owner),
        (base(&other, &c), &outsider),
        ("/api/v1".into(), &owner),
    ] {
        assert_eq!(
            list(&f, cookie, &other_root).await,
            json!([{"name":"manifest.json","size_bytes":5}])
        );
    }
}

#[tokio::test]
async fn project_bundle_mutations_require_admin_and_current_grants_including_delete_marker() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let member = f.cookie("member", Role::Viewer);
    let instance_admin = f.cookie("member", Role::Admin);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    for (method, path) in [
        ("PUT", format!("{}/manifest.json", url(&root))),
        ("DELETE", url(&root)),
    ] {
        assert_eq!(
            f.request(method, &path, Some(&owner), Value::Null, false)
                .await
                .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            f.request(method, &path, Some(&instance_admin), Value::Null, true)
                .await
                .0,
            StatusCode::NOT_FOUND
        );
    }
    let editor = grant(&f, &owner, &org, &p, "editor", json!({"valid_from":0})).await;
    for method in ["PUT", "DELETE"] {
        let path = if method == "PUT" {
            format!("{}/manifest.json", url(&root))
        } else {
            url(&root)
        };
        assert_eq!(
            f.request(method, &path, Some(&instance_admin), Value::Null, true)
                .await
                .0,
            StatusCode::FORBIDDEN
        );
    }
    let temporary = grant(
        &f,
        &owner,
        &org,
        &p,
        "admin",
        json!({"valid_from":1001,"edit_until":1003,"read_until":1005}),
    )
    .await;
    for (clock, status) in [
        (1000, StatusCode::FORBIDDEN),
        (1001, StatusCode::OK),
        (1003, StatusCode::FORBIDDEN),
        (1005, StatusCode::FORBIDDEN),
    ] {
        f.clock.0.store(clock, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            f.request(
                "PUT",
                &format!("{}/manifest.json", url(&root)),
                Some(&member),
                Value::Null,
                true
            )
            .await
            .0,
            status
        );
        assert_eq!(
            f.request("GET", &url(&root), Some(&member), Value::Null, false)
                .await
                .0,
            StatusCode::OK
        );
        assert_eq!(
            f.request("DELETE", &url(&root), Some(&member), Value::Null, true)
                .await
                .0,
            if status == StatusCode::OK {
                StatusCode::NO_CONTENT
            } else {
                status
            }
        );
    }
    command(
        &f,
        &owner,
        &org,
        json!({"type":"revoke_grant","project":p,"grant":temporary}),
    )
    .await;
    command(
        &f,
        &owner,
        &org,
        json!({"type":"revoke_grant","project":p,"grant":editor}),
    )
    .await;
    assert_eq!(
        f.request("GET", &url(&root), Some(&member), Value::Null, false)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.request("DELETE", &url(&root), Some(&member), Value::Null, true)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn delayed_bundle_upload_cannot_reuse_expired_admin_even_with_permanent_editor() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let member = f.cookie("member", Role::Admin);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    grant(&f, &owner, &org, &p, "editor", json!({"valid_from":0})).await;
    grant(
        &f,
        &owner,
        &org,
        &p,
        "admin",
        json!({"valid_from":0,"edit_until":1001}),
    )
    .await;
    let clock = f.clock.clone();
    let body = Body::from_stream(futures::stream::once(async move {
        clock.0.store(1001, std::sync::atomic::Ordering::SeqCst);
        Ok::<_, std::io::Error>("late bytes")
    }));
    assert_eq!(
        f.fixture
            .request(
                Request::builder()
                    .method("PUT")
                    .uri(format!("{}/manifest.json", url(&root)))
                    .header(header::COOKIE, &member)
                    .header("x-aiwatcher-iam", "1")
                    .body(body)
                    .unwrap()
            )
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(list(&f, &owner, &root).await, json!([]));
}

#[tokio::test]
async fn project_bundles_fail_closed_without_auth_iam_or_explicit_factory() {
    let mut f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    stage(&f, &owner, &root, "manifest.json", "private").await;
    for (method, path) in [
        ("GET", url(&root)),
        ("DELETE", url(&root)),
        ("PUT", format!("{}/manifest.json", url(&root))),
    ] {
        assert_eq!(
            f.request(method, &path, None, Value::Null, true).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    let auth = f.fixture.state.auth.take();
    assert_eq!(
        f.request("GET", &url(&root), None, Value::Null, false)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    f.fixture.state.auth = auth;
    let iam = f.fixture.state.iam.take();
    assert_eq!(
        f.request("GET", &url(&root), Some(&owner), Value::Null, false)
            .await
            .0,
        StatusCode::NOT_IMPLEMENTED
    );
    assert_eq!(list(&f, &owner, "/api/v1").await, json!([]));
    f.fixture.state.iam = iam;
    f.fixture.state.evaluation_bundles = Some(Arc::new(Bundles {
        unsupported: true,
        ..Default::default()
    }));
    assert_eq!(
        f.request("GET", &url(&root), Some(&owner), Value::Null, false)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    f.fixture.state.evaluation_bundles = None;
    assert_eq!(
        f.request("GET", &url(&root), Some(&owner), Value::Null, false)
            .await
            .0,
        StatusCode::NOT_IMPLEMENTED
    );
}
