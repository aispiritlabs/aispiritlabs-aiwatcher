//! A workshop's labs, one project at a time (ADR_0034).
//!
//! The interesting half is not that a lab is isolated — every authored
//! registry here is — but that a lab's *pins* are resolved in the project that
//! owns them. Naming another project's scorecard is a refusal rather than a
//! lab that reads fine and measures nothing.
use super::*;

async fn fixture() -> IamFixture {
    // Built on the cohort fixture: a lab pins a card and a cohort, and the
    // cohort needs a dataset behind it. A second evaluation registry here
    // would be a second set of objects for the same questions.
    let mut f = super::project_cohorts::fixture().await;
    f.fixture.state.labs = Some(Arc::new(aiwatcher_labs::Registry::new(
        Arc::new(MemoryObjectStore::new()),
        Default::default(),
    )));
    f
}

fn card() -> Value {
    json!({"name":"answer-quality","scorers":[
        {"metric":"exact","expected_path":"/answer","answer_path":"/answer","scorer":{"kind":"exact_match"}}
    ]})
}

fn lab(brief: &str) -> Value {
    json!({"name":"lab-03","title":"Answer the support questions","brief":brief,"position":3})
}

async fn publish_card(f: &IamFixture, cookie: &str, root: &str) -> String {
    let (status, body) = f
        .request(
            "POST",
            &format!("{root}/evaluation-scorecards"),
            Some(cookie),
            card(),
            true,
        )
        .await;
    assert!(
        matches!(status, StatusCode::OK | StatusCode::CREATED),
        "{body}"
    );
    body["version"].as_str().unwrap().to_owned()
}

async fn publish_lab(f: &IamFixture, cookie: &str, root: &str, body: Value) -> (StatusCode, Value) {
    f.request("POST", &format!("{root}/labs"), Some(cookie), body, true)
        .await
}

#[tokio::test]
async fn a_lab_is_read_by_its_project_alone_and_identical_briefs_keep_one_version_id() {
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

    let (status, mine) = publish_lab(&f, &owner, &roots[0], lab("One workshop's brief.")).await;
    assert_eq!(status, StatusCode::CREATED, "{mine}");
    let pin = mine["version"]["version_id"].as_str().unwrap().to_owned();

    for (root, cookie) in [
        (&roots[1], &owner),
        (&roots[2], &outsider),
        (&roots[3], &owner),
    ] {
        for route in [
            "labs".to_owned(),
            "labs/lab-03".to_owned(),
            format!("labs/lab-03/versions/{pin}"),
            "labs/lab-03/measurement".to_owned(),
        ] {
            let (status, body) = f
                .request(
                    "GET",
                    &format!("{root}/{route}"),
                    Some(cookie),
                    Value::Null,
                    false,
                )
                .await;
            if route == "labs" {
                assert_eq!(body["labs"].as_array().unwrap().len(), 0, "{root}/{route}");
            } else {
                assert_eq!(status, StatusCode::NOT_FOUND, "{root}/{route}: {body}");
            }
        }
        // Knowing another project's version id cannot move a label onto it.
        assert_eq!(
            f.request(
                "PUT",
                &format!("{root}/labs/lab-03/labels/published"),
                Some(cookie),
                json!({ "version_id": pin }),
                true,
            )
            .await
            .0,
            StatusCode::NOT_FOUND,
        );
    }

    // The same brief in another project is the same version id and a separate
    // object: scope never enters the content hash, and knowing one grants
    // nothing.
    let (status, theirs) = publish_lab(&f, &owner, &roots[1], lab("One workshop's brief.")).await;
    assert_eq!(status, StatusCode::CREATED, "{theirs}");
    assert_eq!(
        theirs["version"]["version_id"],
        mine["version"]["version_id"]
    );
}

#[tokio::test]
async fn a_lab_pinning_another_projects_card_is_refused_where_it_is_written() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let org = f.create(&owner).await;
    let a = project(&f, &owner, &org).await;
    let b = project(&f, &owner, &org).await;
    let (a_root, b_root) = (base(&org, &a), base(&org, &b));

    let version = publish_card(&f, &owner, &a_root).await;
    let request = super::project_cohorts::source(&f, &owner, &a_root, "A question").await;
    let cohort = super::project_cohorts::derive(&f, &owner, &a_root, request).await;
    let cases = cohort["cohort"]["case_manifest"]["digest"]
        .as_str()
        .unwrap()
        .to_owned();

    let mut pinned = lab("Hold my answers to this card.");
    pinned["tests"] = json!({
        "scorecard": {"name": "answer-quality", "version": version},
        "cases": cases,
    });

    // In the project that owns both, it is a lab with a measurement.
    let (status, published) = publish_lab(&f, &owner, &a_root, pinned.clone()).await;
    assert_eq!(status, StatusCode::CREATED, "{published}");
    let (status, view) = f
        .request(
            "GET",
            &format!("{a_root}/labs/lab-03/measurement"),
            Some(&owner),
            Value::Null,
            false,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    let context_id = view["measurement"]["context_id"].as_str().unwrap();
    assert_eq!(context_id.len(), 64, "{view}");
    assert!(view["unavailable"].is_null(), "{view}");
    assert_eq!(
        view["measurement"]["context"]["metrics"][0]["name"], "exact",
        "the metrics are the card's, never the lab's"
    );
    assert_eq!(view["measurement"]["context"]["case_count"], 1);

    // The same pins in the project next door name nothing that project holds.
    let (status, refused) = publish_lab(&f, &owner, &b_root, pinned).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["code"], "lab_unpinned", "{refused}");
    assert_eq!(
        f.request(
            "GET",
            &format!("{b_root}/labs/lab-03"),
            Some(&owner),
            Value::Null,
            false
        )
        .await
        .0,
        StatusCode::NOT_FOUND,
        "nothing refused was stored"
    );
}

#[tokio::test]
async fn a_lab_with_no_tests_says_so_rather_than_failing_the_read() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let org = f.create(&owner).await;
    let a = project(&f, &owner, &org).await;
    let root = base(&org, &a);

    let (status, body) = publish_lab(&f, &owner, &root, lab("Still being written.")).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, view) = f
        .request(
            "GET",
            &format!("{root}/labs/lab-03/measurement"),
            Some(&owner),
            Value::Null,
            false,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert!(view["measurement"].is_null(), "{view}");
    assert_eq!(view["unavailable"], "this lab pins no tests yet");

    // A scoped answer is never cached: a revoked grant must not go on being
    // true because something in front of the browser still had the page.
    let response = aiwatcher_api::router(f.fixture.state.clone())
        .oneshot(
            Request::builder()
                .uri(format!("{root}/labs"))
                .header(header::COOKIE, &owner)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
}

#[tokio::test]
async fn writing_a_lab_needs_the_mutation_header_a_grant_and_no_cached_answer() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let member = f.cookie("member", Role::Viewer);
    let org = f.create(&owner).await;
    let a = project(&f, &owner, &org).await;
    let root = base(&org, &a);

    // A write without `X-AIWatcher-IAM: 1` is refused before the grant is even
    // read: the header is not simple, so it cannot come from a cross-origin
    // form carrying somebody's session cookie.
    assert_eq!(
        f.request(
            "POST",
            &format!("{root}/labs"),
            Some(&owner),
            lab("x"),
            false
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
    );
    // Somebody with no grant on this project sees no lab and writes none. A
    // project they cannot reach is a project that is not there, which is why
    // the read is a 404 rather than a 403 — the precedent is
    // `StoreError::OutOfScope`, and a 403 would confirm the project exists.
    for (method, mutation, expected) in [
        ("GET", false, StatusCode::NOT_FOUND),
        ("POST", true, StatusCode::NOT_FOUND),
    ] {
        let (status, body) = f
            .request(
                method,
                &format!("{root}/labs"),
                Some(&member),
                lab("not mine"),
                mutation,
            )
            .await;
        assert_eq!(status, expected, "{method}: {body}");
    }

    let (status, body) = publish_lab(&f, &owner, &root, lab("Mine.")).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    // Publishing the same document again is the version that was there.
    let (status, again) = publish_lab(&f, &owner, &root, lab("Mine.")).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["created"], false);
}

#[tokio::test]
async fn a_lab_hands_out_a_pinned_notebook_and_a_pin_nothing_could_resolve_is_refused() {
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let org = f.create(&owner).await;
    let a = project(&f, &owner, &org).await;
    let root = base(&org, &a);

    let mut asked = lab("Work through the notebook, then hand in your answers.");
    asked["notebook"] = json!({"path": "lab_03_agent", "revision": "a".repeat(64)});
    let (status, published) = publish_lab(&f, &owner, &root, asked.clone()).await;
    assert_eq!(status, StatusCode::CREATED, "{published}");
    assert_eq!(
        published["version"]["notebook"]["revision"],
        json!("a".repeat(64)),
        "the pin comes back as it was written: the lab names the file, never its source"
    );
    assert_eq!(
        published["head"]["versions"][0]["has_notebook"],
        json!(true),
        "and the index says which versions hand one out"
    );

    let (status, read) = f
        .request(
            "GET",
            &format!("{root}/labs/lab-03"),
            Some(&owner),
            Value::Null,
            false,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(read["current"]["notebook"]["path"], json!("lab_03_agent"));

    // The runtime's own name and digest rules, refused where the lab is
    // written rather than when a participant opens a notebook that is not
    // there. The registry never reaches the notebook runtime to find out —
    // what it refuses is a pin that could not resolve anywhere.
    for pinned in [
        json!({"path": "Lab_03", "revision": "a".repeat(64)}),
        json!({"path": "lab_03_agent", "revision": "head"}),
        json!({"path": "../../etc/passwd"}),
    ] {
        let mut refused = lab("Work through the notebook.");
        refused["notebook"] = pinned.clone();
        let (status, body) = publish_lab(&f, &owner, &root, refused).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{pinned} gave {body}");
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|message| message.starts_with("notebook.")),
            "the refusal names the field: {body}"
        );
    }
}

#[tokio::test]
async fn a_lab_hands_out_a_notebook_this_instance_never_holds_the_bytes_of() {
    // The workshop shape (ADR_0034, amended): nine steps in a repository with
    // its own environment, opened by the workshop's own recipe on the
    // participant's machine. Nothing here reaches marimo, nothing here holds
    // the file, and no URL is stored — where it runs is the deployment's
    // question or the participant's, never the lab's.
    let f = fixture().await;
    let owner = f.cookie("owner", Role::Admin);
    let org = f.create(&owner).await;
    let a = project(&f, &owner, &org).await;
    let root = base(&org, &a);

    let mut asked = lab("Follow one failed request all the way through.");
    asked["notebook"] = json!({
        "path": "labs/00_foundations/00_llm_workflow_map/notebook.py",
        "command": "just notebook foundations 00_llm_workflow_map",
        "port": 2718,
    });
    let (status, published) = publish_lab(&f, &owner, &root, asked).await;
    assert_eq!(status, StatusCode::CREATED, "{published}");
    assert!(
        published["version"]["notebook"]["revision"].is_null(),
        "a digest this instance cannot check is not written: {published}"
    );
    assert_eq!(published["version"]["notebook"]["port"], json!(2718));
    assert_eq!(
        published["head"]["versions"][0]["has_notebook"],
        json!(true),
        "handing one out is handing one out, digest or no digest"
    );
}
