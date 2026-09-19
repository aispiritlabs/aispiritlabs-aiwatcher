use super::*;
use aiwatcher_evaluation::{CohortFiles, CohortRequest, SourceAuthority};
use aiwatcher_iam::ProjectScope;

// The HTTP tests exercise admission and storage. Native parsing, split semantics
// and byte verification are covered with LocalSource in the server tests.
#[derive(Debug)]
struct CohortSource {
    datasets: Arc<DatasetRegistry>,
    scope: Option<ProjectScope>,
}
#[async_trait::async_trait]
impl SourceAuthority for CohortSource {
    fn for_project_cohorts(
        &self,
        scope: ProjectScope,
    ) -> aiwatcher_evaluation::Result<Arc<dyn SourceAuthority>> {
        assert!(self.scope.is_none_or(|bound| bound == scope));
        Ok(Arc::new(Self {
            datasets: Arc::new(self.datasets.for_project(scope).unwrap()),
            scope: Some(scope),
        }))
    }
    fn for_project_evidence(
        &self,
        scope: ProjectScope,
    ) -> aiwatcher_evaluation::Result<Arc<dyn SourceAuthority>> {
        self.for_project_cohorts(scope)
    }
    async fn resolve(
        &self,
        _: &aiwatcher_evaluation::EvaluationManifest,
        _: &str,
    ) -> aiwatcher_evaluation::Result<aiwatcher_evaluation::SourceEvidence> {
        // Declaring does not admit server-measured evidence. Production
        // resolver coverage lives in the server's project_declarations tests.
        Err(aiwatcher_evaluation::EvaluationError::Unavailable(
            aiwatcher_evaluation::EvidenceState::Forbidden,
        ))
    }
    async fn derive_cohort(
        &self,
        request: &CohortRequest,
        _: &str,
    ) -> aiwatcher_evaluation::Result<CohortFiles> {
        if request.dataset.kind != aiwatcher_evaluation::DatasetKind::Curation {
            return Err(aiwatcher_evaluation::EvaluationError::Unavailable(
                aiwatcher_evaluation::EvidenceState::Forbidden,
            ));
        }
        let version = self
            .datasets
            .verified_version(&request.dataset.name, &request.dataset.version)
            .await
            .map_err(|_| {
                aiwatcher_evaluation::EvaluationError::Unavailable(
                    aiwatcher_evaluation::EvidenceState::DeletedSource,
                )
            })?;
        let available = version.items.len() as u64;
        Ok(CohortFiles {
            cases: serde_json::to_vec(&version.items).unwrap(),
            input_schema: b"{}".to_vec(),
            expectations_schema: b"{}".to_vec(),
            count: available,
            available,
            unsplit: None,
        })
    }
}
/// The fixture's own resolver, for a test that wraps it.
pub(super) fn authority(datasets: Arc<DatasetRegistry>) -> Arc<dyn SourceAuthority> {
    Arc::new(CohortSource {
        datasets,
        scope: None,
    })
}

pub(super) async fn fixture() -> IamFixture {
    let mut f = IamFixture::new().await;
    let datasets = Arc::new(DatasetRegistry::new(
        Arc::new(MemoryObjectStore::new()),
        "datasets",
    ));
    f.fixture.state.datasets = Some(datasets.clone());
    f.fixture.state.evaluations = Some(Arc::new(
        aiwatcher_evaluation::Registry::new(
            Arc::new(MemoryObjectStore::new()),
            Arc::new(CohortSource {
                datasets,
                scope: None,
            }),
            Default::default(),
        )
        .unwrap(),
    ));
    f
}
pub(super) async fn source(f: &IamFixture, cookie: &str, root: &str, words: &str) -> Value {
    let (status, value) = f.request("POST", &format!("{root}/datasets"), Some(cookie),
        json!({"name":"golden/cases","pipeline":"fixture","source":"fixture","columns":["case_id","input","expected"],
            "items":[{"case_id":"one","input":{"question":words},"expected":{"answer":words}}]}), true).await;
    assert!(
        matches!(status, StatusCode::OK | StatusCode::CREATED),
        "{value}"
    );
    json!({"dataset":{"kind":"curation","name":"golden/cases","version":value["dataset"]["latest"]["version"]}, "split":"test"})
}
pub(super) async fn derive(f: &IamFixture, cookie: &str, root: &str, request: Value) -> Value {
    let (status, result) = f
        .request(
            "POST",
            &format!("{root}/evaluation-cohorts"),
            Some(cookie),
            request,
            true,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    result
}
fn detail(root: &str, result: &Value) -> String {
    format!(
        "{root}/evaluation-cohorts/{}",
        result["cohort"]["case_manifest"]["digest"]
            .as_str()
            .unwrap()
    )
}

#[tokio::test]
async fn project_cohort_derivation_and_lookup_isolate_source_versions_and_metadata() {
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
    let request = source(&f, &owner, &roots[0], "A secret").await;
    let original = derive(&f, &owner, &roots[0], request.clone()).await;
    for (root, cookie) in [
        (&roots[1], &owner),
        (&roots[2], &outsider),
        (&roots[3], &owner),
    ] {
        assert_ne!(
            f.request(
                "POST",
                &format!("{root}/evaluation-cohorts"),
                Some(cookie),
                request.clone(),
                true
            )
            .await
            .0,
            StatusCode::OK
        );
        assert_eq!(
            f.request(
                "GET",
                &detail(root, &original),
                Some(cookie),
                Value::Null,
                false
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }
    for (root, cookie, words) in [
        (&roots[1], &owner, "B secret"),
        (&roots[2], &outsider, "C secret"),
        (&roots[3], &owner, "legacy secret"),
    ] {
        let requested = source(&f, cookie, root, words).await;
        let result = derive(&f, cookie, root, requested.clone()).await;
        assert_eq!(result["request"], requested);
        assert_ne!(result["cohort"], original["cohort"]);
        assert_eq!(
            f.request(
                "GET",
                &detail(root, &original),
                Some(cookie),
                Value::Null,
                false
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }
    source(&f, &owner, &roots[0], "new head").await;
    assert_eq!(derive(&f, &owner, &roots[0], request).await, original);
    let response = f
        .fixture
        .router()
        .oneshot(
            Request::builder()
                .uri(detail(&roots[0], &original))
                .header(header::COOKIE, &owner)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(
        f.request(
            "GET",
            &detail(&base(&other, &a), &original),
            Some(&owner),
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.request(
            "GET",
            &detail(&roots[0], &original),
            Some(&outsider),
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn project_cohorts_enforce_roles_mutation_headers_and_current_grant_intervals() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let member = f.cookie("member", Role::Viewer);
    let admin = f.cookie("member", Role::Admin);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    let request = source(&f, &owner, &root, "words").await;
    let result = derive(&f, &owner, &root, request.clone()).await;
    let url = format!("{root}/evaluation-cohorts");
    assert_eq!(
        f.request("POST", &url, Some(&admin), request.clone(), true)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.request("POST", &url, Some(&owner), request.clone(), false)
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
                &detail(&root, &result),
                Some(&member),
                Value::Null,
                false
            )
            .await
            .0,
            read
        );
        assert_eq!(
            f.request("POST", &url, Some(&admin), request.clone(), true)
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
    let fresh = source(&f, &owner, &root, "fresh words").await;
    assert_eq!(
        derive(&f, &member, &root, fresh).await["derived_by"],
        "member"
    );
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
            &detail(&root, &result),
            Some(&member),
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.request("POST", &url, Some(&member), request, true)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn delayed_cohort_body_cannot_derive_after_write_access_expires() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let member = f.cookie("member", Role::Viewer);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    let request = source(&f, &owner, &root, "words").await;
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
        Ok::<_, std::io::Error>(request.to_string())
    }));
    assert_eq!(
        f.fixture
            .request(
                Request::builder()
                    .method("POST")
                    .uri(format!("{root}/evaluation-cohorts"))
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
    // No successful first derivation was attributed to the late requester.
    let request = source(&f, &owner, &root, "words").await;
    assert_eq!(
        derive(&f, &owner, &root, request).await["derived_by"],
        "owner"
    );
}

#[tokio::test]
async fn project_cohorts_fail_closed_without_auth_iam_or_a_scoped_source_factory() {
    let mut f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    let request = source(&f, &owner, &root, "words").await;
    let result = derive(&f, &owner, &root, request.clone()).await;
    assert_eq!(
        f.request("GET", &detail(&root, &result), None, Value::Null, false)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    for base in [&root, &"/api/v1".to_owned()] {
        assert_eq!(
            f.request(
                "GET",
                &format!("{base}/evaluation-cohorts/..%2Fprivate"),
                Some(&owner),
                Value::Null,
                false
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    let iam = f.fixture.state.iam.take();
    assert_eq!(
        f.request(
            "GET",
            &detail(&root, &result),
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
            &detail("/api/v1", &result),
            Some(&owner),
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    f.fixture.state.iam = iam;
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
            "POST",
            &format!("{root}/evaluation-cohorts"),
            Some(&owner),
            request,
            true
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}
