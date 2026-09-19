//! What the log's folds answer, one project at a time (ADR_0033, IAM-02 E3/E4).
//!
//! Every other scoped family here resolves a *store* per project. This one has
//! no second store to resolve: the log is one log and the fold is one fold with
//! the project in the row, so what a route resolves is the **side** it answers
//! on. Two halves of one rule, and the second is the one that is easy to leave
//! out:
//!
//! * a project's route answers that project's runs — what somebody asked for;
//! * the instance route answers **none** of them — what makes it a boundary
//!   rather than a badge, since an instance viewer who kept seeing a project's
//!   runs would be reading a project they hold no grant on.
//!
//! And the live half, which is where the revocation window used to be the
//! session cookie's TTL: a stream carries its side in the subscription, a
//! resume is refused once the grant is gone, and an open stream is closed
//! rather than left running to the end of the session.
use super::*;

use aiwatcher_core::attrs::{aiwatcher as own, genai};
use aiwatcher_core::ports::{CompletedSpan, LiveEvent, SpanKind, SpanStatus, attr};
use aiwatcher_core::{SpanId, TraceId};

/// The events of one run, published into one project or into none.
///
/// Written straight onto the bus and folded, rather than posted through
/// `/api/v1/events`: the ingest route's own rule — the credential's scope
/// overwrites the body's, always — has its test beside it in `http.rs`. What
/// is under test here is what the reads do with the rows that arrive.
async fn seed(f: &IamFixture, run_id: &str, project: Option<&str>) {
    let scope = project.map(|key| aiwatcher_core::ProjectScope::parse(key).expect("a scope"));
    let mut events = vec![
        envelope(
            &format!("{run_id}-1"),
            EventType::RunStarted,
            run_id,
            json!({}),
        ),
        envelope(
            &format!("{run_id}-2"),
            EventType::LlmCompleted,
            run_id,
            json!({ "call_id": "c1", "model": "claude-opus-5", "prompt_tokens": 100 }),
        ),
        envelope(
            &format!("{run_id}-3"),
            EventType::RunCompleted,
            run_id,
            json!({ "status": "succeeded" }),
        ),
    ];
    for event in &mut events {
        event.project = scope;
    }
    let appended = f.fixture.bus.append(events).await.expect("appends");
    for event in &appended.recorded {
        f.fixture.read_model.apply(event).await;
        f.fixture
            .live
            .publish(LiveEvent::from(event))
            .await
            .expect("publishes");
    }
    // The span the assembler would have written for that call, with the pair
    // of attributes it puts on one when the event carried a scope. There is no
    // assembler in this fixture, and a span carries its own project rather
    // than reading its run's — so writing it by hand is writing the thing
    // under test's input, not a shortcut past it.
    let trace_id = TraceId::derive(run_id);
    let start = time::OffsetDateTime::now_utc();
    let mut attributes = vec![
        attr(own::run::ID, run_id),
        attr(genai::OPERATION_NAME, genai::operation::CHAT),
        attr(genai::REQUEST_MODEL, "claude-opus-5"),
    ];
    if let Some(scope) = scope {
        attributes.push(attr(
            own::project::ORGANIZATION,
            scope.organization.to_string(),
        ));
        attributes.push(attr(own::project::ID, scope.project.to_string()));
    }
    f.fixture
        .read_model
        .record_spans(&[CompletedSpan {
            trace_id,
            span_id: SpanId::derive(trace_id, "llm:c1"),
            parent_span_id: None,
            name: format!("chat {run_id}"),
            kind: SpanKind::Client,
            start,
            end: start + time::Duration::milliseconds(120),
            status: SpanStatus::Ok,
            attributes,
            events: Vec::new(),
            links: Vec::new(),
        }])
        .await;
}

/// `<organization>/<project>` as the log spells it, from the two ids the
/// control plane handed back.
fn key(org: &str, project: &str) -> String {
    format!("{org}/{project}")
}

async fn get(f: &IamFixture, path: &str, cookie: &str) -> (StatusCode, Value) {
    f.request("GET", path, Some(cookie), Value::Null, false)
        .await
}

fn run_ids(page: &Value) -> Vec<String> {
    page["runs"]
        .as_array()
        .expect("a page")
        .iter()
        .map(|run| run["run_id"].as_str().expect("an id").to_owned())
        .collect()
}

#[tokio::test]
async fn a_read_answers_the_side_its_path_names_and_never_the_other() {
    let f = IamFixture::new().await;
    let owner = f.cookie("owner", Role::Admin);
    let member = f.cookie("member", Role::Viewer);
    let org = f.create(&owner).await;
    let mine = project(&f, &owner, &org).await;
    let yours = project(&f, &owner, &org).await;

    seed(&f, "run-global", None).await;
    seed(&f, "run-mine", Some(&key(&org, &mine))).await;
    seed(&f, "run-yours", Some(&key(&org, &yours))).await;

    // The instance read, under instance authorization, exactly as before —
    // and holding nothing of either project's.
    let (status, page) = get(&f, "/api/v1/runs", &member).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(run_ids(&page), ["run-global"]);
    assert_eq!(page["total_known"], 1, "one side's cursor counts one side");

    // A member with no grant is told the project is not there. Not 403, which
    // would confirm it exists, and not 503, which would promise a retry for a
    // boundary that never moves.
    let scoped = format!("{}/runs", base(&org, &mine));
    assert_eq!(get(&f, &scoped, &member).await.0, StatusCode::NOT_FOUND);

    f.request(
        "POST",
        &format!("{ROOT}/{org}/commands"),
        Some(&owner),
        json!({"type":"set_member","principal":{"provider":f.issuer,"subject":"member"},"role":"member"}),
        true,
    )
    .await;
    let (status, granted) = f
        .request(
            "POST",
            &format!("{ROOT}/{org}/commands"),
            Some(&owner),
            json!({"type":"grant","project":mine,
                "grantee":{"kind":"user","value":{"provider":f.issuer,"subject":"member"}},
                "role":"viewer","window":{"valid_from":0}}),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{granted}");

    let (status, page) = get(&f, &scoped, &member).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(run_ids(&page), ["run-mine"], "its own runs");
    assert_eq!(
        page["total_known"], 1,
        "and not the global run, nor the other project's"
    );
    // A grant on one project is no grant on the next.
    assert_eq!(
        get(&f, &format!("{}/runs", base(&org, &yours)), &member)
            .await
            .0,
        StatusCode::NOT_FOUND
    );

    // The same rule on every read of the fold, so no one of them is the way in.
    for (path, holds, misses) in [
        ("spans", "run-mine", "run-global"),
        ("dimensions/agent", "researcher", "run-global"),
    ] {
        let (status, body) = get(&f, &format!("{}/{path}", base(&org, &mine)), &member).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
        assert!(body.to_string().contains(holds), "{path}: {body}");
        assert!(!body.to_string().contains(misses), "{path}: {body}");
    }
    let (_, metrics) = get(&f, &format!("{}/metrics", base(&org, &mine)), &member).await;
    assert_eq!(metrics["totals"]["runs"], 1);
    assert_eq!(
        metrics["window"]["runs_retained"], 1,
        "retention is reported over the side that was read"
    );

    // A detail, and the route that answers past the read model.
    for suffix in ["", "/events"] {
        assert_eq!(
            get(
                &f,
                &format!("{}/runs/run-global{suffix}", base(&org, &mine)),
                &member
            )
            .await
            .0,
            StatusCode::NOT_FOUND,
            "a project read must not reach out of one: {suffix}"
        );
        let (status, _) = get(&f, &format!("/api/v1/runs/run-mine{suffix}"), &member).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "and an instance read must not reach into one: {suffix}"
        );
        let (status, body) = get(
            &f,
            &format!("{}/runs/run-mine{suffix}", base(&org, &mine)),
            &member,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{suffix}: {body}");
    }

    // A scoped answer is never cached: the grant behind it is a snapshot with
    // an `evaluated_at`, not a capability somebody holds afterwards.
    let response = f
        .fixture
        .router()
        .oneshot(
            Request::builder()
                .uri(&scoped)
                .header(header::COOKIE, &member)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
}

#[tokio::test]
async fn a_live_stream_carries_its_side_and_a_resume_cannot_widen_it() {
    let f = IamFixture::new().await;
    let owner = f.cookie("owner", Role::Admin);
    let org = f.create(&owner).await;
    let mine = project(&f, &owner, &org).await;

    seed(&f, "run-global", None).await;
    seed(&f, "run-mine", Some(&key(&org, &mine))).await;

    // The owner's own grant on the project they created: the one explicit
    // grant creating a project makes.
    let text = stream(&f, &format!("{}/events/stream", base(&org, &mine)), &owner).await;
    assert_eq!(
        text.matches("event: event").count(),
        3,
        "one run's three events and nothing else: {text}"
    );
    assert!(text.contains("run-mine"), "{text}");
    assert!(
        !text.contains("run-global"),
        "the global side is not in a project's stream: {text}"
    );

    let text = stream(&f, "/api/v1/events/stream", &owner).await;
    assert_eq!(text.matches("event: event").count(), 3, "{text}");
    assert!(text.contains("run-global"), "{text}");
    assert!(
        !text.contains("run-mine"),
        "and a project's is not in the instance's: {text}"
    );

    // A run stream needs no existence check: every frame carries its own
    // project, so another side's run admits nothing.
    let text = stream(
        &f,
        &format!("{}/runs/run-global/stream", base(&org, &mine)),
        &owner,
    )
    .await;
    assert_eq!(text.matches("event: event").count(), 0, "{text}");

    // And the resume. `Last-Event-ID` is a position in the log and not a key
    // to it: with the grant gone the connection is refused before a frame is
    // replayed, which is what stops a reconnect being a way to read on after
    // access was taken away.
    let (_, access) = get(&f, &format!("{ROOT}/{org}/projects/{mine}/access"), &owner).await;
    for entry in access["grants"].as_array().expect("grants") {
        f.request(
            "POST",
            &format!("{ROOT}/{org}/commands"),
            Some(&owner),
            json!({"type":"revoke_grant","project":mine,"grant":entry["grant"]["id"]}),
            true,
        )
        .await;
    }
    let response = f
        .fixture
        .router()
        .oneshot(
            Request::builder()
                .uri(format!("{}/events/stream", base(&org, &mine)))
                .header(header::COOKIE, &owner)
                .header("last-event-id", Checkpoint::beginning().to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// Open a stream from the beginning of the log and read until it says it is
/// live. The router is driven directly, because `Fixture::get` collects a body
/// and an SSE body does not end.
async fn stream(f: &IamFixture, uri: &str, cookie: &str) -> String {
    let response = f
        .fixture
        .router()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::COOKIE, cookie)
                .header("last-event-id", Checkpoint::beginning().to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "{uri}");
    super::read_until_caught_up(response).await
}
