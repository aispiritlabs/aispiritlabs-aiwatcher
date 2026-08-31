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
use aiwatcher_core::ports::{
    LivePublisher, PortError, RerunAccepted, RerunRequest, WorkflowRunner,
};
use aiwatcher_core::{Checkpoint, EventEnvelope, EventType, MessageId, Sdk, Source};
use aiwatcher_datasets::Registry as DatasetRegistry;
use aiwatcher_projector::{LiveHub, ReadModel};
use aiwatcher_prompts::adapters::memory::MemoryObjectStore;
use aiwatcher_prompts::{Registry, RegistryConfig};

use aiwatcher_api::state::{AppState, HealthState};
use aiwatcher_auth::{AuthConfig, AuthMode, Authenticator, IngestToken, RoleMapping};

/// What a producer presents. Long enough that the parser accepts it, which is
/// itself part of what is under test in `aiwatcher_auth`.
const INGEST_TOKEN: &str = "agents=0123456789abcdef0123456789abcdef";
const INGEST_SECRET: &str = "0123456789abcdef0123456789abcdef";

struct Fixture {
    state: AppState,
    bus: Arc<InMemoryBus>,
    read_model: Arc<ReadModel>,
    live: Arc<LiveHub>,
}

impl Fixture {
    fn new(ingest_enabled: bool) -> Self {
        Self::build(ingest_enabled, true, None, None)
    }

    /// An instance configured without a prompt store, which is what
    /// `AIWATCHER_PROMPT_STORE=none` produces.
    fn without_registry() -> Self {
        Self::build(false, false, None, None)
    }

    /// An instance with a runner wired, which is what
    /// `AIWATCHER_WORKFLOW_RUNNER=http` produces. The default has none, so
    /// every other test also asserts that reruns are 501 by construction.
    fn with_runner(runner: Arc<RecordingRunner>) -> Self {
        Self::build(false, true, Some(runner), None)
    }

    /// An instance behind an authenticating reverse proxy, which is what
    /// `AIWATCHER_AUTH_MODE=proxy` produces. Chosen for these tests because it
    /// is the one mode that establishes a real identity with no network at
    /// all: there is no provider to discover, only headers to read.
    async fn behind_a_proxy(ingest_enabled: bool) -> Self {
        let auth = Authenticator::connect(AuthConfig {
            mode: AuthMode::Proxy,
            roles: RoleMapping::default(),
            ingest_tokens: vec![
                INGEST_TOKEN
                    .parse::<IngestToken>()
                    .expect("long enough to be accepted"),
            ],
            ..AuthConfig::default()
        })
        .await
        .expect("a proxy-mode authenticator needs nothing running")
        .expect("proxy mode produces an authenticator");
        Self::build(ingest_enabled, true, None, Some(Arc::new(auth)))
    }

    fn build(
        ingest_enabled: bool,
        registry_enabled: bool,
        runner: Option<Arc<RecordingRunner>>,
        auth: Option<Arc<Authenticator>>,
    ) -> Self {
        let bus = Arc::new(InMemoryBus::new());
        let read_model = Arc::new(ReadModel::default());
        let live = Arc::new(LiveHub::default());
        let health = HealthState::new();
        let state = AppState {
            read_model: Arc::clone(&read_model),
            live: Arc::clone(&live),
            source: Arc::clone(&bus) as _,
            sink: ingest_enabled.then(|| Arc::clone(&bus) as Arc<dyn MessageSink>),
            prompts: registry_enabled.then(|| {
                Arc::new(Registry::new(
                    Arc::new(MemoryObjectStore::new()),
                    RegistryConfig::default(),
                ))
            }),
            datasets: registry_enabled.then(|| {
                Arc::new(DatasetRegistry::new(
                    Arc::new(MemoryObjectStore::new()),
                    "datasets",
                ))
            }),
            runner: runner.map(|runner| runner as Arc<dyn WorkflowRunner>),
            auth,
            health,
        };
        Self {
            state,
            bus,
            read_model,
            live,
        }
    }

    async fn post(&self, uri: &str, body: Value) -> (StatusCode, Value) {
        self.with_body("POST", uri, body).await
    }

    async fn put(&self, uri: &str, body: Value) -> (StatusCode, Value) {
        self.with_body("PUT", uri, body).await
    }

    async fn with_body(&self, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
        self.request(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
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

    /// A request carrying what authentik's outpost puts on one it let through.
    async fn get_as(&self, uri: &str, user: &str, groups: &str) -> (StatusCode, Value) {
        self.request(
            Request::builder()
                .uri(uri)
                .header("x-authentik-username", user)
                .header("x-authentik-groups", groups)
                .body(Body::empty())
                .expect("request"),
        )
        .await
    }

    async fn post_as(
        &self,
        uri: &str,
        user: &str,
        groups: &str,
        body: Value,
    ) -> (StatusCode, Value) {
        self.request(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-authentik-username", user)
                .header("x-authentik-groups", groups)
                .body(Body::from(body.to_string()))
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

    /// Push a four-stage workflow through the log, the way the projector
    /// would: a declaration, then one run per stage, joined by
    /// `workflow_run_id`. The last stage is left unstarted.
    async fn seed_workflow(&self, workflow_id: &str, execution_id: &str) {
        let declare = |suffix: &str| {
            let mut wire = envelope(
                &format!("{execution_id}-{suffix}"),
                EventType::WorkflowDeclared,
                &format!("{execution_id}-driver"),
                json!({
                    "name": "House import",
                    "version": "sha256:f00d",
                    "nodes": [
                        { "id": "acquire", "name": "Acquire" },
                        { "id": "normalize", "name": "Normalize" },
                        { "id": "persist", "name": "Persist" },
                    ],
                    "edges": [
                        { "from": "acquire", "to": "normalize" },
                        { "from": "normalize", "to": "persist" },
                    ],
                }),
            );
            wire.workflow_id = Some(workflow_id.to_owned());
            wire.workflow_run_id = Some(execution_id.to_owned());
            wire
        };

        let mut events = vec![declare("declare")];
        for (index, stage) in ["acquire", "normalize"].iter().enumerate() {
            let run_id = format!("{execution_id}-{stage}");
            for (suffix, event_type, data) in [
                (
                    format!("{index}-start"),
                    EventType::StepStarted,
                    json!({ "node": stage }),
                ),
                (
                    format!("{index}-artifact"),
                    EventType::ArtifactProduced,
                    json!({
                        "node": stage,
                        "uri": format!("s3://planner-flyte/{stage}.json"),
                        "size_bytes": 2048,
                    }),
                ),
                (
                    format!("{index}-end"),
                    EventType::StepCompleted,
                    json!({ "node": stage }),
                ),
            ] {
                let mut wire = envelope(
                    &format!("{execution_id}-{suffix}"),
                    event_type,
                    &run_id,
                    data,
                );
                wire.workflow_id = Some(workflow_id.to_owned());
                wire.workflow_run_id = Some(execution_id.to_owned());
                events.push(wire);
            }
        }

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

/// A runner that records rather than dispatches.
///
/// A fake rather than a mock: what is worth asserting is the request the
/// handler builds, and a mock that asserted on call counts would pass while
/// sending the wrong workflow.
#[derive(Debug, Default)]
struct RecordingRunner {
    seen: std::sync::Mutex<Vec<RerunRequest>>,
    refuse: bool,
}

impl RecordingRunner {
    fn refusing() -> Self {
        Self {
            seen: std::sync::Mutex::new(Vec::new()),
            refuse: true,
        }
    }

    fn seen(&self) -> Vec<RerunRequest> {
        self.seen.lock().expect("not poisoned").clone()
    }
}

#[async_trait::async_trait]
impl WorkflowRunner for RecordingRunner {
    async fn rerun(&self, request: RerunRequest) -> Result<RerunAccepted, PortError> {
        self.seen.lock().expect("not poisoned").push(request);
        if self.refuse {
            return Err(PortError::Rejected {
                target: "workflow-runner",
                message: "400: no such workflow".to_owned(),
            });
        }
        Ok(RerunAccepted {
            reference: Some("import-42".to_owned()),
            url: None,
        })
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
async fn the_global_sse_stream_replays_every_run_from_its_resume_point() {
    let fixture = Fixture::new(false);
    fixture.seed_run("run-1").await;
    fixture.seed_run("run-2").await;

    let response = fixture
        .router()
        .oneshot(
            Request::builder()
                .uri("/api/v1/events/stream")
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
        6,
        "the global stream includes both runs: {text}"
    );
    assert!(text.contains("\"run_id\":\"run-1\""), "{text}");
    assert!(text.contains("\"run_id\":\"run-2\""), "{text}");
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

// ── The prompt registry ─────────────────────────────────────────────────────

const BASELINE: &str = "Describe the floor plan on {{ page }} in {{ language }}.";
const CANDIDATE: &str = "Read {{ page }} closely; describe every room in {{ language }}.";

async fn publish(fixture: &Fixture, text: &str) -> Value {
    let (status, body) = fixture
        .post(
            "/api/v1/prompts",
            json!({
                "name": "planner.floor-plan",
                "text": text,
                "author": "mkubaszek",
                "model": "qwen/qwen3-vl-235b",
                "description": "Floor plan extraction",
                "tags": ["planner"],
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body
}

#[tokio::test]
async fn publishing_a_prompt_returns_the_version_its_text_hashes_to() {
    let fixture = Fixture::new(false);
    let body = publish(&fixture, BASELINE).await;

    // sha256 of the text, which is what a producer computes locally before it
    // ever calls this — `planner` already does.
    let version_id = body["version"]["version_id"].as_str().expect("an id");
    assert_eq!(version_id.len(), 64);
    assert_eq!(body["created"], true);
    assert_eq!(
        body["version"]["variables"],
        json!(["language", "page"]),
        "variables are read from the text, not declared"
    );
    assert_eq!(body["version"]["origin"], "authored");
}

#[tokio::test]
async fn republishing_the_same_text_is_a_200_rather_than_a_second_version() {
    let fixture = Fixture::new(false);
    publish(&fixture, BASELINE).await;

    let (status, body) = fixture
        .post(
            "/api/v1/prompts",
            json!({ "name": "planner.floor-plan", "text": BASELINE }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "not created: it was already there");
    assert_eq!(body["created"], false);
    assert_eq!(
        body["head"]["versions"].as_array().expect("a list").len(),
        1
    );
}

#[tokio::test]
async fn a_prompt_detail_carries_the_text_that_is_live() {
    let fixture = Fixture::new(false);
    publish(&fixture, BASELINE).await;
    publish(&fixture, CANDIDATE).await;

    let (status, body) = fixture.get("/api/v1/prompts/planner.floor-plan").await;
    assert_eq!(status, StatusCode::OK);
    // Nothing promoted yet, so `current` is the newest — the registry is
    // readable from the first publish rather than after a promotion ceremony.
    assert_eq!(body["current"]["text"], CANDIDATE);
    assert_eq!(
        body["head"]["versions"].as_array().expect("a list").len(),
        2
    );
    assert_eq!(body["head"]["description"], "Floor plan extraction");
}

#[tokio::test]
async fn moving_a_label_changes_which_version_is_current() {
    let fixture = Fixture::new(false);
    let baseline = publish(&fixture, BASELINE).await["version"]["version_id"]
        .as_str()
        .expect("an id")
        .to_owned();
    publish(&fixture, CANDIDATE).await;

    let (status, _) = fixture
        .put(
            "/api/v1/prompts/planner.floor-plan/labels/production",
            json!({ "version_id": baseline }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    let (_, body) = fixture.get("/api/v1/prompts/planner.floor-plan").await;
    assert_eq!(
        body["current"]["text"], BASELINE,
        "a moved label wins over recency"
    );
}

#[tokio::test]
async fn a_label_pointing_at_a_version_that_is_not_stored_is_a_404() {
    let fixture = Fixture::new(false);
    publish(&fixture, BASELINE).await;
    let (status, body) = fixture
        .put(
            "/api/v1/prompts/planner.floor-plan/labels/production",
            json!({ "version_id": "0".repeat(64) }),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
}

#[tokio::test]
async fn an_optimisation_is_graded_by_the_server_rather_than_by_its_optimiser() {
    let fixture = Fixture::new(false);
    let baseline = publish(&fixture, BASELINE).await["version"]["version_id"]
        .as_str()
        .expect("an id")
        .to_owned();

    let (status, body) = fixture
        .post(
            "/api/v1/prompts/planner.floor-plan/optimizations",
            json!({
                "algorithm": "deepeval/SIMBA",
                "baseline": baseline,
                "candidate_text": CANDIDATE,
                "primary_metric": "mean_score",
                "dev": [{ "metric": "mean_score", "baseline": 0.61, "candidate": 0.79 }],
                "test": [{ "metric": "mean_score", "baseline": 0.60, "candidate": 0.67 }],
                "dataset": "catalog@1",
                "evaluation_id": "eval-7",
                "iterations": 8,
                "promote": true,
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["outcome"], "admitted");
    assert_eq!(body["variables_lost"], json!([]));

    // The candidate is now a version of the prompt, and it says who wrote it.
    let (_, detail) = fixture.get("/api/v1/prompts/planner.floor-plan").await;
    assert_eq!(detail["current"]["text"], CANDIDATE);
    assert_eq!(detail["current"]["origin"], "optimized");
    assert_eq!(detail["current"]["algorithm"], "deepeval/SIMBA");

    // And the prompt page can answer "what happened lately" in one request.
    let last = &detail["head"]["optimizations"][0];
    assert_eq!(last["outcome"], "admitted");
    assert_eq!(last["test_score"], 0.67);
    assert_eq!(last["evaluation_id"], "eval-7");
}

#[tokio::test]
async fn a_dev_only_gain_is_recorded_and_refused_a_promotion() {
    // The exact shape of an overfit: the optimiser maximised the dev score and
    // has nothing held out to show for it.
    let fixture = Fixture::new(false);
    let baseline = publish(&fixture, BASELINE).await["version"]["version_id"]
        .as_str()
        .expect("an id")
        .to_owned();

    let (status, body) = fixture
        .post(
            "/api/v1/prompts/planner.floor-plan/optimizations",
            json!({
                "algorithm": "deepeval/SIMBA",
                "baseline": baseline,
                "candidate_text": CANDIDATE,
                "primary_metric": "mean_score",
                "dev": [{ "metric": "mean_score", "baseline": 0.60, "candidate": 0.95 }],
                "promote": true,
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["outcome"], "rejected");
    assert_eq!(body["reason"], "no_held_out_measurement");

    let (_, detail) = fixture.get("/api/v1/prompts/planner.floor-plan").await;
    assert_eq!(
        detail["current"]["text"], CANDIDATE,
        "the candidate is still stored and still the newest"
    );
    assert!(
        detail["head"]["labels"].get("production").is_none(),
        "but nothing was promoted"
    );
}

#[tokio::test]
async fn an_optimisation_against_an_unknown_baseline_is_a_404() {
    let fixture = Fixture::new(false);
    publish(&fixture, BASELINE).await;
    let (status, body) = fixture
        .post(
            "/api/v1/prompts/planner.floor-plan/optimizations",
            json!({
                "algorithm": "deepeval/SIMBA",
                "baseline": "0".repeat(64),
                "candidate_text": CANDIDATE,
                "primary_metric": "mean_score",
            }),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
}

#[tokio::test]
async fn a_prompt_name_that_could_be_a_path_never_reaches_the_store() {
    let fixture = Fixture::new(false);
    let (status, body) = fixture
        .post(
            "/api/v1/prompts",
            json!({ "name": "../../etc/passwd", "text": "x" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
}

#[tokio::test]
async fn listing_prompts_searches_on_the_server() {
    let fixture = Fixture::new(false);
    publish(&fixture, BASELINE).await;
    fixture
        .post(
            "/api/v1/prompts",
            json!({ "name": "market.search", "text": "Search for {{ query }}." }),
        )
        .await;

    let (status, body) = fixture.get("/api/v1/prompts").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 2);

    let (_, filtered) = fixture.get("/api/v1/prompts?search=market").await;
    assert_eq!(filtered["prompts"].as_array().expect("a list").len(), 1);
    assert_eq!(filtered["prompts"][0]["name"], "market.search");
    assert_eq!(
        filtered["total"], 2,
        "total is what is stored, not what matched"
    );
}

#[tokio::test]
async fn an_instance_without_a_prompt_store_says_so_rather_than_404ing() {
    // 501 and not 404: the route exists in the contract, and this deployment
    // chose not to wire a store behind it. A client can tell a missing prompt
    // from a missing feature.
    let fixture = Fixture::without_registry();
    let (status, body) = fixture.get("/api/v1/prompts").await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(body["code"], "registry_disabled");
}

// ── Data curation and datasets ──────────────────────────────────────────────

const CURATION: &str = "data_frame()->read(default)->filter(ref('status')->same(lit('succeeded')))";

#[tokio::test]
async fn a_flow_recipe_can_be_saved_and_listed() {
    let fixture = Fixture::new(false);
    let (status, saved) = fixture
        .post(
            "/api/v1/curations",
            json!({
                "name": "production/succeeded",
                "description": "Candidates from production",
                "pipeline": CURATION,
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{saved}");
    assert_eq!(
        saved["recipe"]["revision"].as_str().expect("a hash").len(),
        64
    );

    let (status, page) = fixture.get("/api/v1/curations").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["recipes"][0]["name"], "production/succeeded");
}

#[tokio::test]
async fn a_completed_curation_is_a_content_addressed_dataset_version() {
    let fixture = Fixture::new(false);
    let request = json!({
        "name": "support/conversations",
        "description": "Promoted production sessions",
        "recipe": "production/succeeded",
        "pipeline": CURATION,
        "columns": ["run_id", "conversation_id"],
        "items": [{"run_id": "run-1", "conversation_id": "session-1"}],
        "source": "http://aiwatcher.test",
        "window_seconds": 900,
    });
    let (status, first) = fixture.post("/api/v1/datasets", request.clone()).await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    assert_eq!(first["dataset"]["latest"]["row_count"], 1);
    assert_eq!(
        first["dataset"]["latest"]["version"]
            .as_str()
            .expect("a hash")
            .len(),
        64
    );

    let (status, same) = fixture.post("/api/v1/datasets", request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(same["created"], false);
    assert_eq!(
        same["dataset"]["versions"]
            .as_array()
            .expect("versions")
            .len(),
        1
    );

    let (_, page) = fixture.get("/api/v1/datasets").await;
    assert_eq!(page["datasets"][0]["name"], "support/conversations");
}

#[tokio::test]
async fn dataset_rows_are_lazy_pages_with_server_side_search() {
    let fixture = Fixture::new(false);
    let (status, published) = fixture
        .post(
            "/api/v1/datasets",
            json!({
                "name": "support/conversations",
                "pipeline": CURATION,
                "columns": ["run_id", "conversation_id"],
                "items": [
                    {"run_id": "run-1", "conversation_id": "session-alpha"},
                    {"run_id": "run-2", "conversation_id": "session-beta"}
                ],
                "source": "http://aiwatcher.test"
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{published}");
    let version = published["dataset"]["latest"]["version"]
        .as_str()
        .expect("a version");

    let (status, first) = fixture
        .get(&format!(
            "/api/v1/dataset-rows?name=support%2Fconversations&version={version}&offset=0&limit=1"
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["rows"].as_array().expect("rows").len(), 1);
    assert_eq!(first["rows"][0]["row_index"], 0);
    assert_eq!(first["matching_rows"], 2);
    assert_eq!(first["next_offset"], 1);

    let (status, searched) = fixture
        .get("/api/v1/dataset-rows?name=support%2Fconversations&search=BETA&limit=50")
        .await;
    assert_eq!(status, StatusCode::OK, "{searched}");
    assert_eq!(searched["matching_rows"], 1);
    assert_eq!(searched["rows"][0]["row"]["run_id"], "run-2");

    let (status, missing) = fixture.get("/api/v1/dataset-rows?name=missing").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{missing}");
    assert_eq!(missing["code"], "not_found");
}

#[tokio::test]
async fn a_reader_may_not_save_a_curation_or_dataset() {
    let fixture = Fixture::behind_a_proxy(false).await;
    let (status, _) = fixture
        .post_as(
            "/api/v1/curations",
            "alice",
            "everyone",
            json!({ "name": "production/succeeded", "pipeline": CURATION }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn an_instance_without_an_object_store_disables_datasets_too() {
    let fixture = Fixture::without_registry();
    let (status, body) = fixture.get("/api/v1/datasets").await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(body["code"], "registry_disabled");
}

// ── The workflow graph ───────────────────────────────────────────────────────

#[tokio::test]
async fn the_catalog_lists_a_declared_workflow_with_its_shape() {
    let fixture = Fixture::new(false);
    fixture.seed_workflow("house-import", "exec-1").await;

    let (status, body) = fixture.get("/api/v1/workflows").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["workflows"].as_array().expect("an array").len(), 1);
    assert_eq!(body["workflows"][0]["workflow_id"], "house-import");
    assert_eq!(body["workflows"][0]["name"], "House import");
    assert_eq!(
        body["workflows"][0]["nodes"]
            .as_array()
            .expect("nodes")
            .len(),
        3
    );
    assert_eq!(
        body["workflows"][0]["edges"]
            .as_array()
            .expect("edges")
            .len(),
        2
    );
    assert_eq!(body["workflows"][0]["executions"], 1);
}

#[tokio::test]
async fn an_execution_reports_the_node_that_has_not_run_yet() {
    // The whole reason the declaration rides the log: "what is left" is not
    // answerable from observed events, and it is the question somebody
    // watching a workflow is asking.
    let fixture = Fixture::new(false);
    fixture.seed_workflow("house-import", "exec-1").await;

    let (status, body) = fixture.get("/api/v1/workflow-executions/exec-1").await;
    assert_eq!(status, StatusCode::OK);

    let nodes = body["nodes"].as_array().expect("nodes");
    assert_eq!(nodes.len(), 3);
    assert_eq!(nodes[0]["node_id"], "acquire");
    assert_eq!(nodes[0]["status"], "succeeded");
    assert_eq!(nodes[1]["status"], "succeeded");
    assert_eq!(nodes[2]["node_id"], "persist");
    assert_eq!(nodes[2]["status"], "pending");
    assert_eq!(body["summary"]["nodes_pending"], 1);
    // Two stage pods plus the driver that declared: three runs, one execution.
    // A run filter cannot express that, which is why these routes exist.
    assert_eq!(body["summary"]["runs"].as_array().expect("runs").len(), 3);
    assert_eq!(body["edges"].as_array().expect("edges").len(), 2);
}

#[tokio::test]
async fn an_artifact_is_listed_on_its_node_as_a_reference() {
    let fixture = Fixture::new(false);
    fixture.seed_workflow("house-import", "exec-1").await;

    let (_, body) = fixture.get("/api/v1/workflow-executions/exec-1").await;
    let artifacts = body["nodes"][0]["artifacts"].as_array().expect("artifacts");

    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0]["uri"], "s3://planner-flyte/acquire.json");
    assert_eq!(artifacts[0]["name"], "acquire.json");
    assert_eq!(artifacts[0]["size_bytes"], 2048);
    assert_eq!(body["summary"]["artifacts"], 2);
}

#[tokio::test]
async fn executions_can_be_filtered_to_one_workflow() {
    let fixture = Fixture::new(false);
    fixture.seed_workflow("house-import", "exec-1").await;
    fixture.seed_workflow("parcel-import", "exec-2").await;

    let (status, body) = fixture
        .get("/api/v1/workflow-executions?workflow_id=parcel-import")
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["executions"].as_array().expect("an array").len(), 1);
    assert_eq!(body["executions"][0]["workflow_run_id"], "exec-2");
}

#[tokio::test]
async fn an_unknown_execution_is_a_404_with_a_machine_readable_code() {
    let fixture = Fixture::new(false);
    let (status, body) = fixture.get("/api/v1/workflow-executions/nope").await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
}

#[tokio::test]
async fn a_rerun_without_a_configured_runner_is_a_501_naming_the_variable() {
    // 501, not 404: the route exists in the contract and this deployment did
    // not wire an orchestrator behind it. The panel reads `code` to swap the
    // button for an explanation.
    let fixture = Fixture::new(false);
    fixture.seed_workflow("house-import", "exec-1").await;

    let (status, body) = fixture
        .post("/api/v1/workflows/house-import/rerun", json!({}))
        .await;

    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(body["code"], "runner_disabled");
    assert!(
        body["message"]
            .as_str()
            .expect("a message")
            .contains("AIWATCHER_WORKFLOW_RUNNER"),
        "the message must say which variable is unset: {body}"
    );
}

#[tokio::test]
async fn a_rerun_is_accepted_and_carries_only_names_the_producer_chose() {
    let runner = Arc::new(RecordingRunner::default());
    let fixture = Fixture::with_runner(Arc::clone(&runner));
    fixture.seed_workflow("house-import", "exec-1").await;

    let (status, body) = fixture
        .post(
            "/api/v1/workflows/house-import/rerun",
            json!({
                "workflow_run_id": "exec-1",
                "from_node": "normalize",
                "inputs": { "source_url": "https://example.test/plan.pdf" },
            }),
        )
        .await;

    // 202: nothing has run yet. The evidence is the events it publishes.
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(body["reference"], "import-42");

    let seen = runner.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].workflow_id, "house-import");
    assert_eq!(seen[0].workflow_run_id.as_deref(), Some("exec-1"));
    assert_eq!(seen[0].from_node.as_deref(), Some("normalize"));
    assert_eq!(
        seen[0].inputs["source_url"],
        "https://example.test/plan.pdf"
    );
}

#[tokio::test]
async fn a_rerun_of_a_workflow_nobody_has_heard_of_is_never_dispatched() {
    // Almost always a typo, and dispatching it would turn that typo into a
    // request to another system.
    let runner = Arc::new(RecordingRunner::default());
    let fixture = Fixture::with_runner(Arc::clone(&runner));

    let (status, body) = fixture
        .post("/api/v1/workflows/typo-import/rerun", json!({}))
        .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
    assert!(runner.seen().is_empty(), "nothing left the process");
}

#[tokio::test]
async fn an_orchestrator_that_refuses_is_a_502_not_a_500() {
    // The caller asked for something the orchestrator will refuse identically
    // forever. Saying 500 would invite a retry that cannot work.
    let runner = Arc::new(RecordingRunner::refusing());
    let fixture = Fixture::with_runner(Arc::clone(&runner));
    fixture.seed_workflow("house-import", "exec-1").await;

    let (status, body) = fixture
        .post("/api/v1/workflows/house-import/rerun", json!({}))
        .await;

    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(body["code"], "runner_rejected");
}

#[tokio::test]
async fn a_rerun_body_naming_its_own_endpoint_is_refused() {
    // The runner's target comes from configuration. `deny_unknown_fields` is
    // what makes an attempt to supply one a 400 rather than a silently
    // ignored field that reads as accepted.
    let runner = Arc::new(RecordingRunner::default());
    let fixture = Fixture::with_runner(Arc::clone(&runner));
    fixture.seed_workflow("house-import", "exec-1").await;

    let (status, _) = fixture
        .post(
            "/api/v1/workflows/house-import/rerun",
            json!({ "endpoint": "http://169.254.169.254/latest/meta-data/" }),
        )
        .await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(runner.seen().is_empty(), "nothing left the process");
}

// ── Single sign-on ───────────────────────────────────────────────────────────
//
// Exercised in `proxy` mode throughout, because it is the one mode that
// establishes a real identity with nothing running: there is no provider to
// discover, only the headers authentik's outpost already sets on every request
// it lets through. What is under test is the same for every mode — the layer,
// the role checks and the public-path list — because all three sit above the
// point where the modes differ.

#[tokio::test]
async fn an_instance_with_no_provider_refuses_nobody() {
    // The default, and the thing an upgrade must not change: a release that
    // started answering 401 would be one that took an installation down.
    let fixture = Fixture::new(false);

    let (status, _) = fixture.get("/api/v1/runs").await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = fixture.get("/api/v1/auth/config").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["enabled"], false,
        "the panel renders no sign-in screen"
    );
}

#[tokio::test]
async fn a_request_that_did_not_come_through_the_proxy_is_refused() {
    // In proxy mode the absence of the header means the request did not come
    // through the proxy, which is the one thing that must never read as
    // "nobody is signed in, carry on".
    let fixture = Fixture::behind_a_proxy(false).await;
    let (status, body) = fixture.get("/api/v1/runs").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "unauthenticated");
}

#[tokio::test]
async fn the_probes_and_the_auth_config_answer_without_a_credential() {
    // A kubelet has no session, and a panel that cannot ask whether there is a
    // login here has to guess.
    let fixture = Fixture::behind_a_proxy(false).await;
    for public in ["/livez", "/healthz"] {
        let (status, _) = fixture.get(public).await;
        assert_eq!(status, StatusCode::OK, "{public}");
    }
    // Readiness answers its own question — this fixture never marked itself
    // ready — and the point is that it answers it rather than 401.
    let (status, _) = fixture.get("/readyz").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let (status, body) = fixture.get("/api/v1/auth/config").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["enabled"], true);
    assert_eq!(body["mode"], "proxy");
    assert!(
        body["login_url"].is_null(),
        "in proxy mode signing in already happened before the request arrived"
    );
}

#[tokio::test]
async fn the_current_caller_is_the_one_the_proxy_named() {
    let fixture = Fixture::behind_a_proxy(false).await;

    let (status, _) = fixture.get("/api/v1/auth/me").await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "a 401 here is what tells the panel to show its sign-in screen"
    );

    let (status, body) = fixture
        .get_as("/api/v1/auth/me", "alice", "everyone|aiwatcher-admins")
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["username"], "alice");
    assert_eq!(body["credential"], "proxy");
    assert_eq!(
        body["roles"].as_array().expect("a list").last(),
        Some(&json!("admin"))
    );
}

#[tokio::test]
async fn a_reader_may_read_and_may_not_author_a_prompt() {
    let fixture = Fixture::behind_a_proxy(false).await;

    let (status, _) = fixture.get_as("/api/v1/runs", "alice", "everyone").await;
    assert_eq!(status, StatusCode::OK, "an unmapped user is still a viewer");

    let (status, body) = fixture
        .post_as(
            "/api/v1/prompts",
            "alice",
            "everyone",
            json!({ "name": "planner.floor-plan", "text": BASELINE }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "forbidden");
    assert!(
        body["message"]
            .as_str()
            .expect("a message")
            .contains("editor"),
        "the message has to name the role, because the fix is a group: {body}"
    );
}

#[tokio::test]
async fn an_editor_may_author_a_prompt() {
    let fixture = Fixture::behind_a_proxy(false).await;
    let (status, body) = fixture
        .post_as(
            "/api/v1/prompts",
            "bob",
            "aiwatcher-editors",
            json!({ "name": "planner.floor-plan", "text": BASELINE }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

#[tokio::test]
async fn only_an_admin_may_dispatch_a_rerun() {
    // And the role is checked before the instance says whether it has a runner
    // at all: an editor learning that a rerun endpoint exists and is unwired
    // is a fact they were not entitled to.
    let fixture = Fixture::behind_a_proxy(false).await;

    let (status, _) = fixture
        .post_as(
            "/api/v1/workflows/import/rerun",
            "bob",
            "aiwatcher-editors",
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) = fixture
        .post_as(
            "/api/v1/workflows/import/rerun",
            "alice",
            "aiwatcher-admins",
            json!({}),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::NOT_IMPLEMENTED,
        "past the role check, and this fixture wires no runner: {body}"
    );
}

#[tokio::test]
async fn publishing_events_over_http_needs_an_editor() {
    let fixture = Fixture::behind_a_proxy(true).await;
    let batch = json!({
        "events": [{
            "event_type": "run.started",
            "occurred_at": "2026-08-27T18:20:11Z",
            "run_id": "run-sso",
            "source": { "service": "browser", "sdk": "typescript" },
            "data": {}
        }]
    });

    let (status, _) = fixture
        .post_as("/api/v1/events", "alice", "everyone", batch.clone())
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "reading is not writing");

    let (status, body) = fixture
        .post_as("/api/v1/events", "agent", "aiwatcher-editors", batch)
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
}

#[tokio::test]
async fn the_login_route_says_this_instance_has_no_provider() {
    // 501, not 404: the route is in the contract and this deployment wired
    // nothing behind it — the same answer the prompt registry gives.
    let fixture = Fixture::behind_a_proxy(false).await;
    let (status, body) = fixture.get("/api/v1/auth/login").await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(body["code"], "auth_disabled");
}

#[tokio::test]
async fn a_producer_publishes_with_a_token_and_still_cannot_rerun() {
    // The case that decides whether single sign-on is adoptable at all: an
    // agent runs in the cluster, reaches the Service directly, never passes
    // the proxy that authenticates a browser, and cannot complete an
    // interactive sign-in. Without a credential of its own, turning SSO on
    // would silently stop every SDK publishing over HTTP.
    let fixture = Fixture::behind_a_proxy(true).await;
    let batch = json!({
        "events": [{
            "event_type": "run.started",
            "occurred_at": "2026-08-27T18:20:11Z",
            "run_id": "run-token",
            "source": { "service": "planner", "sdk": "python" },
            "data": {}
        }]
    });

    let publish = |token: &str, body: Value| {
        let request = Request::builder()
            .method("POST")
            .uri("/api/v1/events")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from(body.to_string()))
            .expect("request");
        fixture.request(request)
    };

    let (status, body) = publish(INGEST_SECRET, batch.clone()).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");

    let (status, _) = publish("not-the-token", batch).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Editor, never admin: a secret sitting in an agent's environment must not
    // be able to ask an orchestrator to run something.
    let (status, _) = fixture
        .request(
            Request::builder()
                .method("POST")
                .uri("/api/v1/workflows/import/rerun")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {INGEST_SECRET}"))
                .body(Body::from("{}"))
                .expect("request"),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
