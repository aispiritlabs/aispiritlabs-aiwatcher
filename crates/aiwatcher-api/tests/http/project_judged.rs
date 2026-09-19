//! A judged measurement declared and admitted over the scoped routes. The
//! routes are the ones that already existed; what is new is that a judge's
//! dependencies now resolve — each of them in the project the caller holds a
//! current grant on, and none of them next door. Native byte verification and
//! the judge itself are covered by the server tests; this is the IAM boundary.
use super::*;
use aiwatcher_evaluation::{CohortFiles, CohortRequest, SourceAuthority};
use aiwatcher_iam::ProjectScope;

/// Derives a cohort from the project's own dataset registry and resolves a
/// manifest the way the other evidence tests do. Production owner/byte checks
/// live in `aiwatcher-server`'s `project_judge` tests.
#[derive(Debug)]
struct JudgedSource {
    datasets: Arc<DatasetRegistry>,
    scope: Option<ProjectScope>,
}

#[async_trait::async_trait]
impl SourceAuthority for JudgedSource {
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
        manifest: &aiwatcher_evaluation::EvaluationManifest,
        subject: &str,
    ) -> aiwatcher_evaluation::Result<aiwatcher_evaluation::SourceEvidence> {
        // A native cohort's cases come from the project's own dataset version;
        // the producer manifest the calibration set is taken from keeps the
        // fixture's answers. Neither reaches past the bound registry.
        if manifest.context.dataset.kind != aiwatcher_evaluation::DatasetKind::Curation {
            return EvaluationSource::default().resolve(manifest, subject).await;
        }
        let version = self
            .datasets
            .verified_version(
                &manifest.context.dataset.name,
                &manifest.context.dataset.version,
            )
            .await
            .map_err(|_| {
                aiwatcher_evaluation::EvaluationError::Unavailable(
                    aiwatcher_evaluation::EvidenceState::DeletedSource,
                )
            })?;
        Ok(aiwatcher_evaluation::SourceEvidence {
            expected: version
                .items
                .iter()
                .filter_map(|item| {
                    Some((
                        item.get("case_id")?.as_str()?.to_owned(),
                        item.get("expected")?.clone(),
                    ))
                })
                .collect(),
            ..Default::default()
        })
    }

    async fn derive_cohort(
        &self,
        request: &CohortRequest,
        _: &str,
    ) -> aiwatcher_evaluation::Result<CohortFiles> {
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

async fn fixture() -> IamFixture {
    let mut f = IamFixture::new().await;
    let datasets = Arc::new(DatasetRegistry::new(
        Arc::new(MemoryObjectStore::new()),
        "datasets",
    ));
    f.fixture.state.datasets = Some(datasets.clone());
    f.fixture.state.evaluations = Some(Arc::new(
        aiwatcher_evaluation::Registry::new(
            Arc::new(MemoryObjectStore::new()),
            Arc::new(JudgedSource {
                datasets,
                scope: None,
            }),
            Default::default(),
        )
        .unwrap(),
    ));
    f
}

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
    assert_eq!(status, StatusCode::OK, "{suffix}: {value}");
    value
}

/// A dataset, a rubric, a judged card, people's judgements of a published
/// result, the set they were frozen into, a recording and a cohort — every one
/// of them written through this project's own routes.
async fn seed(f: &IamFixture, cookie: &str, root: &str) -> Value {
    let (status, dataset) = f.request("POST", &format!("{root}/datasets"), Some(cookie), json!({
        "name":"golden/cases","pipeline":"fixture","source":"fixture",
        "columns":["case_id","input","expected"],
        "items":[{"case_id":"capital-pl","input":{"question":"Capital?"},"expected":{"answer":"Warsaw"}}]
    }), true).await;
    assert!(
        matches!(status, StatusCode::OK | StatusCode::CREATED),
        "{dataset}"
    );
    let request = json!({"dataset":{"kind":"curation","name":"golden/cases",
        "version":dataset["dataset"]["latest"]["version"]}, "split":"test"});
    let cohort = post(f, cookie, root, "evaluation-cohorts", request.clone()).await;
    let rubric = post(
        f,
        cookie,
        root,
        "evaluation-rubrics",
        json!({"name":"helpful", "question":"Does it help?", "scale":{"kind":"flag"},
            "direction":"higher"}),
    )
    .await;
    let card = post(
        f,
        cookie,
        root,
        "evaluation-scorecards",
        json!({
            "name":"judged-quality", "scorers":[{"metric":"helpful", "answer_path":"/answer",
                "scorer":{"kind":"judge", "rubric":{"name":"helpful","version":rubric["version"]}}}]
        }),
    )
    .await;
    // The result people judged, published in this project and admitted here.
    let people = durable_request("people-judged");
    post(
        f,
        cookie,
        root,
        "evaluation-approvals",
        people["manifest"].clone(),
    )
    .await;
    post(f, cookie, root, "evaluation-results", people).await;
    post(f, cookie, root, "evaluation-assessments", json!({
        "target":{"kind":"case","evaluation_id":"people-judged","case_id":"capital-pl",
                  "repetition_id":"measurement-1"},
        "rubric":"helpful", "rubric_version":rubric["version"], "value":{"type":"flag","value":true}
    })).await;
    let calibration = post(
        f,
        cookie,
        root,
        "evaluation-calibrations",
        json!({
            "name":"people", "evaluation_id":"people-judged",
            "rubrics":[{"name":"helpful","version":rubric["version"]}]
        }),
    )
    .await;
    let (status, recording) = f
        .request(
            "PUT",
            &format!("{root}/evaluation-recordings/answers"),
            Some(cookie),
            json!({"answers":[{"case_id":"capital-pl", "answer":{"answer":"Warsaw"}}]}),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{recording}");
    let mut variant = durable_request("judged-declaration")["manifest"]["variant"].clone();
    variant["dataset"] = request["dataset"].clone();
    json!({"evaluation_id":"judged-declaration", "repetition_id":"measurement-1",
        "variant":variant, "cohort":cohort["cohort"],
        "scorecard":{"name":"judged-quality","version":card["version"]},
        "answers":recording,
        "judge":{"provider":"llamacpp", "model":{"name":"local-judge","version":"q4"},
                 "calibration":{"name":"people","version":calibration["version"]}}})
}

fn id(value: &Value) -> &str {
    value["declaration"]["id"].as_str().unwrap()
}

#[tokio::test]
async fn a_judged_declaration_resolves_its_rubric_card_and_people_in_one_project_only() {
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
    let declared = post(&f, &owner, &root, "evaluation-runs", body.clone()).await;
    assert_eq!(declared["admitted"], false, "declaring admits nothing");
    assert_eq!(declared["declaration"]["declared_by"], "owner");
    assert!(
        declared["manifest"]["context"]["judge"]["configuration"]["digest"]
            .as_str()
            .is_some_and(|digest| digest.len() == 64),
        "the settings this run pins are addressed by their own bytes"
    );
    assert_eq!(
        declared["manifest"]["context"]["judge"]["reads_archive"],
        Value::Null,
        "nothing on this path reads the conversation archive"
    );
    // Idempotent by content, and the same view comes back by address.
    assert_eq!(
        post(&f, &owner, &root, "evaluation-runs", body.clone()).await,
        declared
    );
    let url = format!("{root}/evaluation-runs/{}", id(&declared));
    assert_eq!(
        f.request("GET", &url, Some(&owner), Value::Null, false)
            .await
            .1,
        declared
    );

    // Every dependency is this project's. A neighbour and the instance hold
    // none of them, and an identical address opens nothing there.
    for (other_root, cookie) in [
        (base(&org, &b), &owner),
        (base(&other_org, &c), &outsider),
        ("/api/v1".to_owned(), &owner),
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
            StatusCode::NOT_FOUND,
            "a card nobody published there resolves to nothing"
        );
    }

    // The scoped start exists (IAM-02/D) and the gate is what refuses it:
    // nothing has admitted this pair yet, which is a 409 naming the approval
    // rather than a boundary saying the declaration is not there.
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
        StatusCode::CONFLICT
    );
    // And the *instance's* start still finds no project-only declaration.
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

    // Admitting the pair is the project admin's act, and it is what makes the
    // measurement admissible — never the declaration.
    let manifest = declared["manifest"].clone();
    let admitted = post(&f, &owner, &root, "evaluation-approvals", manifest.clone()).await;
    assert_eq!(
        admitted["record"]["approval_id"], declared["approval_id"],
        "the approval an operator admits is the one the server derived"
    );
    assert_eq!(
        f.request("GET", &url, Some(&owner), Value::Null, false)
            .await
            .1["admitted"],
        json!(true)
    );
    for (other_root, cookie) in [(base(&org, &b), &owner), (base(&other_org, &c), &outsider)] {
        assert_ne!(
            f.request(
                "POST",
                &format!("{other_root}/evaluation-approvals"),
                Some(cookie),
                manifest.clone(),
                true
            )
            .await
            .0,
            StatusCode::OK,
            "an approval admits a pair where its dependencies are"
        );
    }
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
}

#[tokio::test]
async fn a_judged_declaration_needs_a_current_grant_the_mutation_header_and_a_project_admin() {
    let mut f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let member = f.cookie("member", Role::Viewer);
    let instance_admin = f.cookie("member", Role::Admin);
    let org = f.create(&owner).await;
    let p = project(&f, &owner, &org).await;
    let root = base(&org, &p);
    let body = seed(&f, &owner, &root).await;
    let declared = post(&f, &owner, &root, "evaluation-runs", body.clone()).await;
    let url = format!("{root}/evaluation-runs/{}", id(&declared));
    let write = format!("{root}/evaluation-runs");

    // Instance admin is not project access, in either direction.
    for (method, path, payload) in [
        ("GET", url.clone(), Value::Null),
        ("POST", write.clone(), body.clone()),
        (
            "POST",
            format!("{root}/evaluation-approvals"),
            declared["manifest"].clone(),
        ),
    ] {
        assert_eq!(
            f.request(method, &path, Some(&instance_admin), payload, true)
                .await
                .0,
            StatusCode::NOT_FOUND
        );
    }
    // Declaring without the mutation header is refused before anything is read.
    assert_eq!(
        f.request("POST", &write, Some(&owner), body.clone(), false)
            .await
            .0,
        StatusCode::FORBIDDEN
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
    for (time, read, declare) in [
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
            declare
        );
    }
    f.clock.0.store(1001, std::sync::atomic::Ordering::SeqCst);
    // An editor may declare a judged run and may not admit its pair.
    assert_eq!(
        f.request(
            "POST",
            &format!("{root}/evaluation-approvals"),
            Some(&member),
            declared["manifest"].clone(),
            true
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );

    // A body that arrives after the grant went is not written by the grant the
    // request started under.
    let clock = f.clock.clone();
    let slow = body.clone();
    let upload = Body::from_stream(futures::stream::once(async move {
        clock.0.store(1003, std::sync::atomic::Ordering::SeqCst);
        Ok::<_, std::io::Error>(slow.to_string())
    }));
    let response = f
        .fixture
        .router()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&write)
                .header(header::COOKIE, &member)
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
