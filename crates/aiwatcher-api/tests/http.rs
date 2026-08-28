// See the note in aiwatcher-bus/tests.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! The router, exercised over real HTTP requests.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use aiwatcher_bus::MessageSink;
use aiwatcher_bus::adapters::memory::InMemoryBus;
use aiwatcher_core::ports::LivePublisher;
use aiwatcher_core::{Checkpoint, EventEnvelope, EventType, MessageId, Sdk, Source};
use aiwatcher_projector::{LiveHub, ReadModel};

use aiwatcher_api::state::{AppState, HealthState};

struct Fixture {
    state: AppState,
    bus: Arc<InMemoryBus>,
    read_model: Arc<ReadModel>,
    live: Arc<LiveHub>,
}

impl Fixture {
    fn new(ingest_enabled: bool) -> Self {
        let bus = Arc::new(InMemoryBus::new());
        let read_model = Arc::new(ReadModel::default());
        let live = Arc::new(LiveHub::default());
        let health = HealthState::new();
        let state = AppState {
            read_model: Arc::clone(&read_model),
            live: Arc::clone(&live),
            source: Arc::clone(&bus) as _,
            sink: ingest_enabled.then(|| Arc::clone(&bus) as Arc<dyn MessageSink>),
            health,
        };
        Self {
            state,
            bus,
            read_model,
            live,
        }
    }

    fn router(&self) -> axum::Router {
        aiwatcher_api::router(self.state.clone())
    }

    async fn get(&self, uri: &str) -> (StatusCode, Value) {
        self.request(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
        .await
    }

    async fn request(&self, request: Request<Body>) -> (StatusCode, Value) {
        let response = self.router().oneshot(request).await.expect("responds");
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("collects")
            .to_bytes();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into_owned()))
        };
        (status, body)
    }

    /// Push a run through the log and into the read model, the way the
    /// projector would.
    async fn seed_run(&self, run_id: &str) {
        let events = vec![
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
        let appended = self.bus.append(events).await.expect("appends");
        for event in &appended.recorded {
            self.read_model.apply(event).await;
            self.live
                .publish(aiwatcher_core::ports::LiveEvent::from(event))
                .await
                .expect("publishes");
        }
    }

    /// Push an evaluation through the log, the way the projector would.
    async fn seed_evaluation(
        &self,
        evaluation_id: &str,
        suite: &str,
        dataset: &str,
        metrics: Value,
    ) {
        let events = vec![
            envelope(
                &format!("{evaluation_id}-1"),
                EventType::EvalStarted,
                evaluation_id,
                json!({ "suite": suite, "dataset": dataset, "params": { "model": "gpt-5-mini" } }),
            ),
            envelope(
                &format!("{evaluation_id}-2"),
                EventType::EvalCase,
                evaluation_id,
                json!({ "case_id": "K-1", "passed": true, "score": 0.9 }),
            ),
            envelope(
                &format!("{evaluation_id}-3"),
                EventType::EvalCompleted,
                evaluation_id,
                json!({ "metrics": metrics, "report": { "note": "the document log_dict used to write" } }),
            ),
        ];
        let appended = self.bus.append(events).await.expect("appends");
        for event in &appended.recorded {
            self.read_model.apply(event).await;
        }
    }
}

fn envelope(event_id: &str, event_type: EventType, run_id: &str, data: Value) -> EventEnvelope {
    let mut envelope = EventEnvelope::new(
        event_type,
        run_id,
        time::OffsetDateTime::now_utc(),
        Source::new("test-service", Sdk::Python),
    )
    .with_data(data);
    envelope.event_id = Some(MessageId::new(event_id));
    envelope.agent_id = Some("researcher".to_owned());
    envelope
}

#[tokio::test]
async fn listing_runs_returns_a_page_with_totals() {
    let fixture = Fixture::new(false);
    fixture.seed_run("run-1").await;
    fixture.seed_run("run-2").await;

    let (status, body) = fixture.get("/api/v1/runs").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["runs"].as_array().expect("an array").len(), 2);
    assert_eq!(body["total_known"], 2);
    // Newest first.
    assert_eq!(body["runs"][0]["run_id"], "run-2");
    assert_eq!(body["runs"][0]["status"], "succeeded");
    assert_eq!(body["runs"][0]["input_tokens"], 100);
}

#[tokio::test]
async fn a_run_detail_carries_its_summary() {
    let fixture = Fixture::new(false);
    fixture.seed_run("run-1").await;

    let (status, body) = fixture.get("/api/v1/runs/run-1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["summary"]["run_id"], "run-1");
    assert_eq!(body["summary"]["event_count"], 3);
    assert!(body["spans"].is_array());
}

#[tokio::test]
async fn an_unknown_run_is_a_404_with_a_machine_readable_code() {
    let fixture = Fixture::new(false);
    let (status, body) = fixture.get("/api/v1/runs/nope").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
    assert!(body["message"].as_str().expect("a string").contains("nope"));
}

#[tokio::test]
async fn the_raw_event_log_for_a_run_comes_from_the_durable_log() {
    let fixture = Fixture::new(false);
    fixture.seed_run("run-1").await;
    fixture.seed_run("run-2").await;

    let (status, body) = fixture.get("/api/v1/runs/run-1/events").await;
    assert_eq!(status, StatusCode::OK);
    let events = body["events"].as_array().expect("an array");
    assert_eq!(events.len(), 3, "only run-1's events");
    assert_eq!(body["has_more"], false);
    assert_eq!(events[0]["metadata"]["stream_name"], "run:run-1");
    assert_eq!(events[0]["metadata"]["stream_position"], 1);
    assert!(
        events[0]["metadata"]["correlation_id"].is_string(),
        "the recorded form carries the resolved ids"
    );
}

#[tokio::test]
async fn an_event_page_resumes_from_its_cursor_without_repeating_an_event() {
    let fixture = Fixture::new(false);
    fixture.seed_run("run-1").await;

    let (_, first) = fixture.get("/api/v1/runs/run-1/events?limit=2").await;
    assert_eq!(first["events"].as_array().expect("an array").len(), 2);
    assert_eq!(first["has_more"], true);
    let cursor = first["next_cursor"].as_u64().expect("a cursor");

    let (_, second) = fixture
        .get(&format!("/api/v1/runs/run-1/events?limit=2&after={cursor}"))
        .await;
    let events = second["events"].as_array().expect("an array");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["metadata"]["stream_position"], 3);
    assert_eq!(second["has_more"], false);
    assert!(second["next_cursor"].is_null());
}

#[tokio::test]
async fn a_search_narrows_a_page_but_still_reports_what_it_scanned() {
    let fixture = Fixture::new(false);
    fixture.seed_run("run-1").await;

    let (status, body) = fixture.get("/api/v1/runs/run-1/events?q=OPUS").await;
    assert_eq!(status, StatusCode::OK);
    let events = body["events"].as_array().expect("an array");
    assert_eq!(events.len(), 1, "only the llm event mentions the model");
    assert_eq!(events[0]["event_type"], "llm.completed");
    // Three read, one kept: without `scanned` an empty page and a filtered-out
    // page look the same.
    assert_eq!(body["scanned"], 3);
}

#[tokio::test]
async fn a_dimension_groups_runs_by_the_kind_in_the_path() {
    let fixture = Fixture::new(false);
    fixture.seed_run("run-1").await;
    fixture.seed_run("run-2").await;

    let (status, body) = fixture.get("/api/v1/dimensions/runtime").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["kind"], "runtime");
    let rows = body["rows"].as_array().expect("an array");
    assert_eq!(rows.len(), 1, "both runs came from one service");
    assert_eq!(rows[0]["key"], "test-service");
    assert_eq!(rows[0]["runs"], 2);

    let (status, body) = fixture.get("/api/v1/dimensions/agent").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["rows"][0]["key"], "researcher");
}

#[tokio::test]
async fn an_unknown_dimension_is_rejected_rather_than_silently_empty() {
    let fixture = Fixture::new(false);
    let (status, _) = fixture.get("/api/v1/dimensions/nonsense").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn the_flat_span_list_carries_the_run_each_span_belongs_to() {
    let fixture = Fixture::new(false);
    fixture.seed_run("run-1").await;

    let (status, body) = fixture.get("/api/v1/spans").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["spans"].is_array());
    for span in body["spans"].as_array().expect("an array") {
        assert_eq!(span["run_id"], "run-1");
    }
}

#[tokio::test]
async fn ingest_accepts_a_batch_and_reports_the_checkpoint() {
    let fixture = Fixture::new(true);
    let (status, body) = fixture
        .request(
            Request::builder()
                .method("POST")
                .uri("/api/v1/events")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "events": [{
                            "event_type": "run.started",
                            "occurred_at": "2026-08-27T18:20:11Z",
                            "run_id": "run-http",
                            "source": { "service": "browser", "sdk": "typescript" },
                            "data": {}
                        }]
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await;

    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(body["accepted"], 1);
    assert_eq!(
        body["last_checkpoint"],
        Checkpoint::from_global_position(1).to_string()
    );
}

#[tokio::test]
async fn ingest_rejects_an_envelope_that_cannot_be_recorded() {
    let fixture = Fixture::new(true);
    let (status, body) = fixture
        .request(
            Request::builder()
                .method("POST")
                .uri("/api/v1/events")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "events": [{
                            "event_type": "run.started",
                            "occurred_at": "2026-08-27T18:20:11Z",
                            "run_id": "",
                            "source": { "service": "browser", "sdk": "typescript" },
                            "data": {}
                        }]
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "bad_request");
    assert!(
        body["message"]
            .as_str()
            .expect("a string")
            .contains("run_id")
    );
}

#[tokio::test]
async fn ingest_is_refused_when_the_instance_has_no_sink() {
    let fixture = Fixture::new(false);
    let (status, body) = fixture
        .request(
            Request::builder()
                .method("POST")
                .uri("/api/v1/events")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "events": [] }).to_string()))
                .expect("request"),
        )
        .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "ingest_disabled");
}

#[tokio::test]
async fn readiness_is_separate_from_liveness() {
    let fixture = Fixture::new(false);

    let (status, _) = fixture.get("/livez").await;
    assert_eq!(status, StatusCode::OK, "the process is up from the start");

    let (status, _) = fixture.get("/readyz").await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "but not ready until the projector says so"
    );

    fixture.state.health.mark_ready();
    let (status, _) = fixture.get("/readyz").await;
    assert_eq!(status, StatusCode::OK);
}

/// Read the stream until the catch-up marker, then stop. The connection stays
/// open by design, so a plain `collect()` would hang.
async fn read_until_caught_up(response: axum::response::Response) -> String {
    let mut body = response.into_body().into_data_stream();
    let mut text = String::new();
    while let Some(chunk) = futures::StreamExt::next(&mut body).await {
        text.push_str(&String::from_utf8_lossy(&chunk.expect("a chunk")));
        if text.contains("event: caught_up") {
            break;
        }
    }
    text
}

#[tokio::test]
async fn a_stream_opened_without_a_cursor_is_live_only() {
    let fixture = Fixture::new(false);
    fixture.seed_run("run-1").await;

    let response = fixture
        .router()
        .oneshot(
            Request::builder()
                .uri("/api/v1/runs/run-1/stream")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("responds");

    let text = read_until_caught_up(response).await;
    assert_eq!(
        text.matches("event: event").count(),
        0,
        "history comes from GET /runs/{{id}}; replaying it here would duplicate it: {text}"
    );
    assert!(text.contains("event: caught_up"), "{text}");
}

#[tokio::test]
async fn the_sse_stream_replays_history_then_announces_it_is_live() {
    let fixture = Fixture::new(false);
    fixture.seed_run("run-1").await;

    let response = fixture
        .router()
        .oneshot(
            Request::builder()
                .uri("/api/v1/runs/run-1/stream")
                .header("last-event-id", Checkpoint::beginning().to_string())
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("responds");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );

    let text = read_until_caught_up(response).await;
    assert_eq!(
        text.matches("event: event").count(),
        3,
        "all three history events were replayed: {text}"
    );
    assert!(text.contains("\"event_type\":\"run.started\""), "{text}");
    assert!(
        text.contains(&format!("id: {}", Checkpoint::from_global_position(1))),
        "each frame is tagged for Last-Event-ID resume: {text}"
    );
}

#[tokio::test]
async fn a_resume_point_skips_what_the_client_already_saw() {
    let fixture = Fixture::new(false);
    fixture.seed_run("run-1").await;

    let response = fixture
        .router()
        .oneshot(
            Request::builder()
                .uri("/api/v1/runs/run-1/stream")
                .header(
                    "last-event-id",
                    Checkpoint::from_global_position(2).to_string(),
                )
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("responds");

    let text = read_until_caught_up(response).await;
    assert_eq!(
        text.matches("event: event").count(),
        1,
        "only the third event was missed: {text}"
    );
    assert!(text.contains("\"event_type\":\"run.completed\""), "{text}");
}

#[tokio::test]
async fn a_malformed_resume_point_is_rejected_rather_than_ignored() {
    let fixture = Fixture::new(false);
    let (status, body) = fixture
        .request(
            Request::builder()
                .uri("/api/v1/runs/run-1/stream")
                .header("last-event-id", "not-a-checkpoint")
                .body(Body::empty())
                .expect("request"),
        )
        .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body["code"], "bad_request",
        "silently starting from the beginning would replay the whole log"
    );
}

// ── Evaluations ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn an_evaluation_report_is_listed_with_its_params_and_metrics() {
    let fixture = Fixture::new(false);
    fixture
        .seed_evaluation("eval-1", "catalog", "cases@1", json!({ "mean_score": 0.8 }))
        .await;

    let (status, body) = fixture.get("/api/v1/evaluations").await;
    assert_eq!(status, StatusCode::OK);
    let rows = body["evaluations"].as_array().expect("an array");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["evaluation_id"], "eval-1");
    assert_eq!(rows[0]["suite"], "catalog");
    assert_eq!(rows[0]["status"], "succeeded");
    assert_eq!(rows[0]["params"]["model"], "gpt-5-mini");
    assert_eq!(rows[0]["metrics"]["mean_score"], 0.8);
}

/// The fold that keeps the two views from contradicting each other: an
/// evaluation is an execution, and it is not an agent run.
#[tokio::test]
async fn an_evaluation_does_not_appear_in_the_runs_list() {
    let fixture = Fixture::new(false);
    fixture.seed_run("run-1").await;
    fixture
        .seed_evaluation("eval-1", "catalog", "cases@1", json!({ "mean_score": 0.8 }))
        .await;

    let (_, runs) = fixture.get("/api/v1/runs").await;
    assert_eq!(runs["total_known"], 1);
    assert_eq!(runs["runs"][0]["run_id"], "run-1");

    // Its raw events are still auditable through the log, which is where an
    // "is this what we actually recorded" question has to be answerable.
    let (status, events) = fixture.get("/api/v1/runs/eval-1/events").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(events["events"].as_array().expect("an array").len(), 3);
}

#[tokio::test]
async fn an_evaluation_detail_carries_its_cases_document_and_baseline() {
    let fixture = Fixture::new(false);
    fixture
        .seed_evaluation("eval-1", "catalog", "cases@1", json!({ "mean_score": 0.8 }))
        .await;
    fixture
        .seed_evaluation("eval-2", "catalog", "cases@1", json!({ "mean_score": 0.9 }))
        .await;

    let (status, body) = fixture.get("/api/v1/evaluations/eval-2").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["cases"].as_array().expect("an array").len(), 1);
    assert!(body["report"]["note"].is_string());
    assert_eq!(body["comparison"]["baseline_id"], "eval-1");
    let delta = body["comparison"]["metrics"]
        .as_array()
        .expect("an array")
        .iter()
        .find(|metric| metric["name"] == "mean_score")
        .expect("the metric")["delta"]
        .as_f64()
        .expect("a number");
    assert!((delta - 0.1).abs() < 1e-9);
}

#[tokio::test]
async fn an_unknown_evaluation_is_a_404() {
    let fixture = Fixture::new(false);
    let (status, body) = fixture.get("/api/v1/evaluations/nope").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
}

#[tokio::test]
async fn suites_are_the_level_above_a_report() {
    let fixture = Fixture::new(false);
    fixture
        .seed_evaluation("eval-1", "catalog", "cases@1", json!({ "mean_score": 0.8 }))
        .await;
    fixture
        .seed_evaluation("eval-2", "catalog", "cases@1", json!({ "mean_score": 0.9 }))
        .await;
    fixture
        .seed_evaluation("eval-3", "tone", "cases@1", json!({ "mean_score": 0.5 }))
        .await;

    let (status, body) = fixture.get("/api/v1/evaluation-suites").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 2);
    let catalog = body["suites"]
        .as_array()
        .expect("an array")
        .iter()
        .find(|suite| suite["suite"] == "catalog")
        .expect("the suite");
    assert_eq!(catalog["evaluations"], 2);
    assert_eq!(catalog["last_evaluation_id"], "eval-2");
    assert!(
        (catalog["metric_deltas"]["mean_score"]
            .as_f64()
            .expect("a number")
            - 0.1)
            .abs()
            < 1e-9
    );
}
