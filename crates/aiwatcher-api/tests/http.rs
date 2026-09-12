// See the note in aiwatcher-bus/tests.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! The router, exercised over real HTTP requests.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use aiwatcher_annotations::Registry as AnnotationRegistry;
use aiwatcher_bus::MessageSink;
use aiwatcher_bus::adapters::memory::InMemoryBus;
use aiwatcher_conversations::Registry as ConversationArchive;
use aiwatcher_core::ports::{
    LivePublisher, PortError, RerunAccepted, RerunRequest, WorkflowRunner,
};
use aiwatcher_core::{Checkpoint, EventEnvelope, EventType, MessageId, Sdk, Source};
use aiwatcher_datasets::Registry as DatasetRegistry;
use aiwatcher_projector::{LiveHub, ReadModel};
use aiwatcher_prompts::adapters::memory::MemoryObjectStore;
use aiwatcher_prompts::{Registry, RegistryConfig};
use aiwatcher_training::Registry as TrainingRegistry;

use aiwatcher_api::state::{AppState, HealthState};
use aiwatcher_auth::{AuthConfig, AuthMode, Authenticator, IngestToken, RoleMapping};

/// What a producer presents. Long enough that the parser accepts it, which is
/// itself part of what is under test in `aiwatcher_auth`.
const INGEST_TOKEN: &str = "agents=0123456789abcdef0123456789abcdef";
/// A worker's token: the same shape, plus the one queue it may claim on.
const WORKER_TOKEN: &str = "houses[houses]=fedcba9876543210fedcba9876543210";
const WORKER_SECRET: &str = "fedcba9876543210fedcba9876543210";
/// A second worker's token, on a queue this plan's steps never use.
const OTHER_WORKER_TOKEN: &str = "planner[plans]=0f0f0f0f0f0f0f0f0f0f0f0f0f0f";
const OTHER_WORKER_SECRET: &str = "0f0f0f0f0f0f0f0f0f0f0f0f0f0f";
const INGEST_SECRET: &str = "0123456789abcdef0123456789abcdef";

#[tokio::test]
async fn curation_library_publishes_searches_and_checks_editor_permissions() {
    let fixture = Fixture::behind_a_proxy(false).await;
    let solution = json!({
        "id": "fill-missing", "title": "Missing values", "description": "Median imputation",
        "tags": ["PHP", "preparation"], "spec": {"kind": "transform", "steps": "->limit(20)"}
    });
    let (status, _) = fixture
        .post_as("/api/v1/curation-library", "reader", "", solution.clone())
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, saved) = fixture
        .post_as(
            "/api/v1/curation-library",
            "author",
            "aiwatcher-editors",
            solution.clone(),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let (_, same) = fixture
        .post_as(
            "/api/v1/curation-library",
            "author",
            "aiwatcher-editors",
            solution.clone(),
        )
        .await;
    assert_eq!(saved["revision"], same["revision"]);
    assert_eq!(saved["saved_at"], same["saved_at"]);
    let (status, page) = fixture
        .get_as(
            "/api/v1/curation-library?search=MISSING%20php&limit=1",
            "reader",
            "",
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["total"], 1);
    assert_eq!(page["templates"][0]["spec"], solution["spec"]);
    let (_, empty) = fixture
        .get_as("/api/v1/curation-library?search=unknown", "reader", "")
        .await;
    assert_eq!(empty["total"], 0);
    let (_, next) = fixture
        .get_as("/api/v1/curation-library?offset=1&limit=1", "reader", "")
        .await;
    assert_eq!(next["total"], 1);
    assert_eq!(next["templates"], json!([]));
    let mut invalid = solution;
    invalid["spec"] = json!({"kind": "notebook", "notebook": "custom", "params": {}});
    let (status, _) = fixture
        .post_as(
            "/api/v1/curation-library",
            "author",
            "aiwatcher-editors",
            invalid,
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

struct Fixture {
    state: AppState,
    bus: Arc<InMemoryBus>,
    read_model: Arc<ReadModel>,
    live: Arc<LiveHub>,
    /// The same store `state.artifacts` holds, typed — so a test can put the
    /// bytes a pod's launcher would have put, which the port has no way to.
    /// `None` when this instance has no object store.
    artifacts: Option<Arc<MemoryArtifacts>>,
}

impl Fixture {
    fn new(ingest_enabled: bool) -> Self {
        Self::build(ingest_enabled, true, None, None, None)
    }

    /// An instance configured without a prompt store, which is what
    /// `AIWATCHER_PROMPT_STORE=none` produces.
    fn without_registry() -> Self {
        Self::build(false, false, None, None, None)
    }

    /// An instance with a runner wired, which is what
    /// `AIWATCHER_WORKFLOW_RUNNER=http` produces. The default has none, so
    /// every other test also asserts that reruns are 501 by construction.
    fn with_runner(runner: Arc<RecordingRunner>) -> Self {
        Self::build(false, true, Some(runner), None, None)
    }

    /// An instance behind an authenticating reverse proxy, which is what
    /// `AIWATCHER_AUTH_MODE=proxy` produces. Chosen for these tests because it
    /// is the one mode that establishes a real identity with no network at
    /// all: there is no provider to discover, only headers to read.
    async fn behind_a_proxy(ingest_enabled: bool) -> Self {
        let auth = Self::proxy_authenticator().await;
        Self::build(ingest_enabled, true, None, Some(auth), None)
    }

    async fn proxy_authenticator() -> Arc<Authenticator> {
        let auth = Authenticator::connect(AuthConfig {
            mode: AuthMode::Proxy,
            roles: RoleMapping::default(),
            ingest_tokens: vec![
                INGEST_TOKEN
                    .parse::<IngestToken>()
                    .expect("long enough to be accepted"),
                WORKER_TOKEN
                    .parse::<IngestToken>()
                    .expect("long enough to be accepted"),
                OTHER_WORKER_TOKEN
                    .parse::<IngestToken>()
                    .expect("long enough to be accepted"),
            ],
            ..AuthConfig::default()
        })
        .await
        .expect("a proxy-mode authenticator needs nothing running")
        .expect("proxy mode produces an authenticator");
        Arc::new(auth)
    }

    fn with_editor(editor: Arc<RecordingEditor>) -> Self {
        Self::build(false, true, None, None, Some(editor))
    }

    fn build(
        ingest_enabled: bool,
        registry_enabled: bool,
        runner: Option<Arc<RecordingRunner>>,
        auth: Option<Arc<Authenticator>>,
        editor: Option<Arc<RecordingEditor>>,
    ) -> Self {
        let bus = Arc::new(InMemoryBus::new());
        let read_model = Arc::new(ReadModel::default());
        let live = Arc::new(LiveHub::default());
        let health = HealthState::new();
        let artifacts = registry_enabled.then(|| Arc::new(MemoryArtifacts::default()));
        let state = AppState {
            evaluations: None,
            answer_limits: Default::default(),
            query_engine: aiwatcher_datasets::QueryEngine::Flow,
            query_step_timeout_seconds: None,
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
            workflow_definitions: registry_enabled.then(|| {
                Arc::new(aiwatcher_execution::definition::DefinitionRegistry::new(
                    Arc::new(MemoryObjectStore::new()),
                ))
            }),
            // None, like `just run`: a step asking for a pod is refused at
            // registration. `with_pod_templates` is the deployment that has
            // some.
            pod_templates: None,
            schedules: registry_enabled.then(|| {
                Arc::new(aiwatcher_execution::ScheduleStore::new(Arc::new(
                    MemoryObjectStore::new(),
                )))
            }),
            annotations: registry_enabled.then(|| {
                Arc::new(AnnotationRegistry::new(
                    Arc::new(MemoryObjectStore::new()),
                    "annotations",
                ))
            }),
            training: registry_enabled.then(|| {
                Arc::new(TrainingRegistry::new(
                    Arc::new(MemoryObjectStore::new()),
                    "training",
                ))
            }),
            // On whenever the other registries are, with a fixed key: these
            // tests assert the role split and the 501, and both need a real
            // archive behind them. A deployment's default is still off — see
            // `build_conversation_archive`.
            conversations: registry_enabled.then(|| {
                Arc::new(ConversationArchive::new(
                    Arc::new(MemoryObjectStore::new()),
                    "conversations",
                    aiwatcher_conversations::Keyring::single("test", [7; 32]),
                    aiwatcher_conversations::ArchivePolicy::default(),
                ))
            }),
            // The memory adapter, so the execution routes are exercised rather
            // than answering 501 — and it is claimable, so a plan needing a
            // worker is accepted here and refused only where a `file` store
            // says it holds one process.
            executions: registry_enabled.then(|| {
                Arc::new(aiwatcher_execution::ExecutionHandler::new(Arc::new(
                    aiwatcher_execution::store::memory::MemoryWorkflowStore::new(),
                )
                    as Arc<dyn aiwatcher_execution::WorkflowStore>))
            }),
            // Content-addressed and in memory, so the worker routes are
            // exercised rather than answering 501 — and so the check that a
            // reported output really exists has something to check against.
            //
            // Both follow the object store, as the wiring does: they are built
            // from `registries.objects`, so an instance with no
            // `AIWATCHER_PROMPT_STORE` has neither and the routes that read a
            // step's artifacts answer 501 naming it.
            artifacts: artifacts.clone().map(|store| store as _),
            catalog: registry_enabled.then(|| {
                Arc::new(aiwatcher_execution::artifact::memory::MemoryArtifactCatalog::new()) as _
            }),
            // No worker: a router built for a test runs no background task, and
            // an export here is driven by the test rather than by a tick.
            export_worker: None,
            import_worker: None,
            execution_worker: None,
            // The shipped default: words stay with the worker and this instance
            // holds a reference. A run may still ask for `sealed`, and is
            // refused here because no archive is wired.
            execution_payloads: aiwatcher_api::state::PayloadDefault::default(),
            // Empty, like the shipped default: nothing is curated, so no hub
            // result can be promoted past `unclear`.
            sources: Arc::new(aiwatcher_annotations::SourceCatalog::default()),
            // Deliberately never on. These tests must not reach Kaggle or
            // Hugging Face — a suite whose result depends on somebody else's
            // uptime is a suite people learn to re-run rather than read. What
            // is worth asserting here is the 501, and that needs `None`.
            hubs: None,
            runner: runner.map(|runner| runner as Arc<dyn WorkflowRunner>),
            editor: editor.map(|editor| editor as Arc<dyn aiwatcher_core::ports::EditorHost>),
            auth,
            health,
        };
        Self {
            state,
            bus,
            read_model,
            live,
            artifacts,
        }
    }

    async fn post(&self, uri: &str, body: Value) -> (StatusCode, Value) {
        self.with_body("POST", uri, body).await
    }

    async fn put(&self, uri: &str, body: Value) -> (StatusCode, Value) {
        self.with_body("PUT", uri, body).await
    }

    async fn delete(&self, uri: &str) -> (StatusCode, Value) {
        self.request(
            Request::builder()
                .method("DELETE")
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
        .await
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

    /// The same instance with its conversation archive taken away.
    ///
    /// Not a `build` parameter: what is being tested is one route's refusal,
    /// and every other thing this fixture wires — the workflow store above all
    /// — has to stay, or the refusal never runs because an earlier one does.
    fn without_archive(mut self) -> Self {
        self.state.conversations = None;
        self
    }

    /// The same instance with an operator's pod templates read in.
    fn with_pod_templates(mut self, templates: Value) -> Self {
        self.state.pod_templates = Some(Arc::new(
            aiwatcher_execution::pods::PodTemplates::parse(templates.to_string().as_bytes())
                .expect("templates a chart would render"),
        ));
        self
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

    /// A request presenting a shared secret, which is how a worker arrives.
    async fn send_with_token(
        &self,
        method: &str,
        uri: &str,
        token: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let builder = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json");
        let request = match body {
            Some(body) => builder.body(Body::from(body.to_string())),
            None => builder.body(Body::empty()),
        }
        .expect("request");
        self.request(request).await
    }

    /// A POST carrying one extra header, for the routes that read one.
    /// A body that is not JSON. Sealing takes the plaintext as bytes, because
    /// re-encoding somebody's words on the way in would make the digest the
    /// stream records a digest of this route's idea of them.
    async fn post_bytes(&self, uri: &str, body: Vec<u8>) -> (StatusCode, Value) {
        self.request(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .body(Body::from(body))
                .expect("request"),
        )
        .await
    }

    /// And an answer that is not JSON either.
    async fn get_bytes(&self, uri: &str) -> (StatusCode, Vec<u8>) {
        let response = self
            .router()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("a response");
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("a body");
        (status, body.to_vec())
    }

    async fn post_keyed(&self, uri: &str, key: &str, body: Value) -> (StatusCode, Value) {
        self.request(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .header("idempotency-key", key)
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

/// A notebook runtime that records what it was asked to stage.
///
/// The real one reads the rows from the object store and posts them; what the
/// route is responsible for is *which* step, *which* context and *which*
/// revision, so this records the request and answers.
/// A content-addressed artifact store, in memory.
///
/// Behaves like the real one in the way that matters to these tests: the
/// digest is of the bytes it stored, never of anything a caller claimed. A
/// double that took the caller's word would let a worker fabricate a reference
/// and would pass the very test written to stop it.
#[derive(Debug, Default)]
struct MemoryArtifacts {
    objects: std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>,
}

impl MemoryArtifacts {
    /// Store bytes that are not a table — what the pod launcher's `put_log`
    /// writes. Not on the port: nothing reaching this store over HTTP writes
    /// anything but rows, and a `put_bytes` there would be a door the real
    /// adapter does not have.
    fn put_bytes(
        &self,
        name: &str,
        kind: aiwatcher_core::ArtifactKind,
        body: &[u8],
    ) -> aiwatcher_core::ArtifactRef {
        let digest = aiwatcher_jobs::digest(body);
        self.objects
            .lock()
            .expect("not poisoned")
            .insert(digest.clone(), body.to_vec());
        aiwatcher_core::ArtifactRef {
            name: name.to_owned(),
            uri: format!("object://artifacts/{}/{digest}/data", kind.as_str()),
            digest,
            size_bytes: Some(body.len() as u64),
            content_type: "text/plain; charset=utf-8".to_owned(),
            kind,
            schema_ref: None,
        }
    }
}

#[async_trait::async_trait]
impl aiwatcher_core::ports::AttemptArtifacts for MemoryArtifacts {
    async fn read_rows(
        &self,
        artifact: &aiwatcher_core::ArtifactRef,
    ) -> Result<Vec<serde_json::Value>, PortError> {
        let bytes = self.read_bytes(artifact).await?;
        serde_json::from_slice(&bytes).map_err(|error| PortError::Rejected {
            target: "the object store",
            message: format!("{} does not hold a table: {error}", artifact.uri),
        })
    }

    async fn read_bytes(
        &self,
        artifact: &aiwatcher_core::ArtifactRef,
    ) -> Result<Vec<u8>, PortError> {
        self.objects
            .lock()
            .expect("not poisoned")
            .get(&artifact.digest)
            .cloned()
            .ok_or_else(|| PortError::Rejected {
                target: "the object store",
                message: format!("{} holds no object", artifact.uri),
            })
    }

    async fn put_rows(
        &self,
        name: &str,
        rows: Vec<serde_json::Value>,
    ) -> Result<aiwatcher_core::ArtifactRef, PortError> {
        let body = serde_json::to_vec(&rows).expect("json");
        let digest = aiwatcher_jobs::digest(&body);
        self.objects
            .lock()
            .expect("not poisoned")
            .insert(digest.clone(), body.clone());
        Ok(aiwatcher_core::ArtifactRef {
            name: name.to_owned(),
            uri: format!("object://artifacts/rows/{digest}/data"),
            digest,
            size_bytes: Some(body.len() as u64),
            content_type: "application/json".to_owned(),
            kind: aiwatcher_core::ArtifactKind::Rows,
            schema_ref: None,
        })
    }

    async fn holds(&self, artifact: &aiwatcher_core::ArtifactRef) -> Result<bool, PortError> {
        Ok(self
            .objects
            .lock()
            .expect("not poisoned")
            .contains_key(&artifact.digest))
    }
}

#[derive(Debug)]
struct RecordingEditor {
    seen: std::sync::Mutex<Vec<aiwatcher_core::ports::EditorRequest>>,
    refuse: bool,
}

impl RecordingEditor {
    fn new() -> Self {
        Self {
            seen: std::sync::Mutex::new(Vec::new()),
            refuse: false,
        }
    }

    fn refusing() -> Self {
        Self {
            seen: std::sync::Mutex::new(Vec::new()),
            refuse: true,
        }
    }

    fn seen(&self) -> Vec<aiwatcher_core::ports::EditorRequest> {
        self.seen.lock().expect("not poisoned").clone()
    }
}

#[async_trait::async_trait]
impl aiwatcher_core::ports::EditorHost for RecordingEditor {
    async fn open(
        &self,
        request: aiwatcher_core::ports::EditorRequest,
    ) -> Result<aiwatcher_core::ports::EditorSession, PortError> {
        self.seen
            .lock()
            .expect("not poisoned")
            .push(request.clone());
        if self.refuse {
            return Err(PortError::Rejected {
                target: "the notebook runtime",
                message: "there is no notebook called 'pii_scan'".to_owned(),
            });
        }
        Ok(aiwatcher_core::ports::EditorSession {
            app_url: format!("/ml-pipeline/app/{}/", request.notebook),
            context_id: request.context_id,
            notebook: request.notebook,
            code_revision: request.code_revision,
            rows: 3,
        })
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
    assert_eq!(body["comparison"]["comparability"], "unverified");
    assert!(
        body["comparison"]["metrics"][0].get("delta").is_none(),
        "legacy data does not invent version evidence"
    );
    let (status, body) = fixture
        .get("/api/v1/evaluations/eval-2?baseline_id=eval-1")
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["comparison"]["baseline_id"], "eval-1");
    let (status, _) = fixture
        .get("/api/v1/evaluations/eval-2?baseline_id=missing")
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = fixture
        .get("/api/v1/evaluations/eval-2?baseline_id=eval-2")
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["comparison"]["comparability"], "incompatible");
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
    assert_eq!(
        catalog["metric_deltas"],
        json!({}),
        "legacy suite aggregates do not prove matching evaluation versions"
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
async fn production_on_a_rejected_candidate_is_refused_as_a_promotion() {
    // ADR_0011's `promote` never overrides the verdict, and the label route is
    // the one that could. It answers what the model registry answers.
    let fixture = Fixture::new(false);
    let baseline = publish(&fixture, BASELINE).await["version"]["version_id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let (status, record) = fixture
        .post(
            "/api/v1/prompts/planner.floor-plan/optimizations",
            json!({
                "algorithm": "deepeval/SIMBA",
                "baseline": baseline,
                "candidate_text": CANDIDATE,
                "primary_metric": "mean_score",
                "test": [{ "metric": "mean_score", "baseline": 0.60, "candidate": 0.58 }],
                "baseline_evaluation": "eval-baseline",
                "candidate_evaluation": "eval-candidate",
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{record}");
    assert_eq!(record["outcome"], "rejected");
    assert_eq!(record["baseline_evaluation"], "eval-baseline");
    assert_eq!(record["candidate_evaluation"], "eval-candidate");
    let candidate = record["candidate"].as_str().expect("an id");

    let (status, body) = fixture
        .put(
            "/api/v1/prompts/planner.floor-plan/labels/production",
            json!({ "version_id": candidate }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "promotion_refused");
    let message = body["message"].as_str().expect("a message");
    assert!(
        message.contains(record["optimization_id"].as_str().expect("an id")),
        "{message}"
    );

    let (status, _) = fixture
        .put(
            "/api/v1/prompts/planner.floor-plan/labels/staging",
            json!({ "version_id": candidate }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "any other label stays free");
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

// ── Annotations ──────────────────────────────────────────────────────────────

const ANNOTATION_PROJECT: &str = "corpora/example";

/// A vocabulary with no domain in it.
///
/// This build ships none — a project brings its own — so the tests bring one.
/// It is the smallest set that exercises what these routes check: a stroked
/// polyline with a required attribute, a keypoint instance whose required
/// positions the 422 has to name, and a class marked `ignore`.
fn test_classes() -> Value {
    json!([
        {
            "name": "edge",
            "geometry": "polyline",
            "attributes": [
                {"name": "role", "kind": "enum", "values": ["outer", "inner", "unknown"],
                 "required": true, "default": "unknown"},
                {"name": "thickness_px", "kind": "number", "required": true}
            ]
        },
        {"name": "region", "geometry": "polygon"},
        {
            "name": "mark",
            "geometry": "keypoints",
            "keypoints": ["start", "end", "pivot"],
            "optional_keypoints": ["pivot"],
            "links": [{"name": "edge", "targets": ["edge"], "min": 0, "max": 1}],
            "layer": 1
        },
        {"name": "ignore", "geometry": "polygon", "ignore": true}
    ])
}

/// A well-formed 64-character lowercase SHA-256, without hashing anything.
fn fake_image_id(seed: &str) -> String {
    let mut id = String::new();
    while id.len() < 64 {
        id.push_str(seed);
    }
    id.truncate(64);
    id
}

fn annotation_project() -> Value {
    json!({
        "name": ANNOTATION_PROJECT,
        "description": "A corpus with no domain in it",
        "classes": test_classes(),
        "split_salt": "2026-09",
    })
}

fn registered_image(image_id: &str) -> Value {
    json!({
        "project": ANNOTATION_PROJECT,
        "image_id": image_id,
        "uri": format!("{}{image_id}", aiwatcher_annotations::BLOB_SCHEME),
        "width": 1064,
        "height": 1021,
        "group_id": "family-a",
        "source": "example",
        "rights": { "kind": "owned", "grant": "supplier agreement" },
    })
}

#[tokio::test]
async fn every_annotation_route_answers_501_when_no_object_store_is_configured() {
    // The same shape as the prompt and dataset registries: the routes exist in
    // the contract, and this deployment wired no store behind them. A 404
    // would say they do not exist, which sends somebody looking for a missing
    // release rather than a missing variable.
    let fixture = Fixture::without_registry();
    for uri in [
        "/api/v1/annotation-projects",
        "/api/v1/annotation-project?name=x",
        "/api/v1/annotation-images?project=x",
        "/api/v1/annotation-exports?name=x",
    ] {
        let (status, body) = fixture.get(uri).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{uri}: {body}");
        assert_eq!(body["code"], json!("registry_disabled"));
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|message| message.contains("AIWATCHER_PROMPT_STORE")),
            "the message names the variable to set: {body}"
        );
    }
}

#[tokio::test]
async fn the_source_catalogue_is_empty_until_an_instance_loads_one() {
    // It is domain content, not code, so this build ships none. Empty is a
    // working state rather than a broken one: nothing matches a hub result, so
    // every one stays licence-unclear and an import of it records unknown
    // rights — which a commercial export excludes, by name.
    //
    // It answers without an object store because it is a table rather than a
    // stored artifact, and that is worth keeping true.
    let fixture = Fixture::without_registry();
    let (status, body) = fixture.get("/api/v1/annotation-sources").await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["sources"].as_array().map(Vec::len), Some(0), "{body}");
    assert_eq!(body["total"], json!(0));
}

#[tokio::test]
async fn a_viewer_may_read_a_project_and_may_not_draw_on_it() {
    let fixture = Fixture::behind_a_proxy(false).await;
    let (status, body) = fixture
        .post_as(
            "/api/v1/annotation-projects",
            "alice",
            "aiwatcher-editors",
            annotation_project(),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["name"], json!(ANNOTATION_PROJECT));
    let schema_version = body["schema"]["version"].as_str().expect("a version");
    assert_eq!(schema_version.len(), 64);

    let (status, body) = fixture
        .get_as("/api/v1/annotation-projects", "bob", "aiwatcher-viewers")
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["projects"].as_array().expect("projects").len(), 1);

    let (status, body) = fixture
        .post_as(
            "/api/v1/annotation-projects",
            "bob",
            "aiwatcher-viewers",
            annotation_project(),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

#[tokio::test]
async fn a_refused_drawing_is_a_422_carrying_every_problem_rather_than_the_first() {
    // The whole reason `ErrorBody::details` exists. A labeller fixing one
    // error per round trip stops using the tool.
    let fixture = Fixture::behind_a_proxy(false).await;
    let image_id = fake_image_id("ab");
    fixture
        .post_as(
            "/api/v1/annotation-projects",
            "alice",
            "aiwatcher-editors",
            annotation_project(),
        )
        .await;
    let (status, body) = fixture
        .post_as(
            "/api/v1/annotation-images",
            "alice",
            "aiwatcher-editors",
            registered_image(&image_id),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = fixture
        .post_as(
            "/api/v1/annotation-revisions",
            "alice",
            "aiwatcher-editors",
            json!({
                "project": ANNOTATION_PROJECT,
                "image_id": image_id,
                "annotations": [
                    {
                        "id": "edge_1",
                        "class": "edge",
                        "geometry": { "kind": "polyline", "points": [[10.0, 10.0], [400.0, 10.0]] },
                        "attributes": { "role": "load" }
                    },
                    {
                        "id": "thing_1",
                        "class": "nonexistent",
                        "geometry": { "kind": "point", "at": [5.0, 5.0] }
                    }
                ]
            }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], json!("annotation_rejected"));
    let details = body["details"].as_array().expect("details");
    assert!(details.len() >= 3, "{body}");
    let joined = details
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(joined.contains("thickness_px"), "{joined}");
    assert!(joined.contains("load"), "{joined}");
    assert!(joined.contains("nonexistent"), "{joined}");
}

#[tokio::test]
async fn a_drawing_saved_accepted_reaches_an_export_and_its_coco() {
    let fixture = Fixture::behind_a_proxy(false).await;
    let image_id = fake_image_id("cd");
    fixture
        .post_as(
            "/api/v1/annotation-projects",
            "alice",
            "aiwatcher-editors",
            annotation_project(),
        )
        .await;
    fixture
        .post_as(
            "/api/v1/annotation-images",
            "alice",
            "aiwatcher-editors",
            registered_image(&image_id),
        )
        .await;

    let (status, body) = fixture
        .post_as(
            "/api/v1/annotation-revisions",
            "alice",
            "aiwatcher-editors",
            json!({
                "project": ANNOTATION_PROJECT,
                "image_id": image_id,
                "accept": true,
                "annotations": [{
                    "id": "region_1",
                    "class": "region",
                    "geometry": {
                        "kind": "polygon",
                        "exterior": [[0.0, 0.0], [100.0, 0.0], [100.0, 50.0], [0.0, 50.0]]
                    },
                    "attributes": {}
                }]
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["head"]["review"], json!("accepted"));
    // Provenance comes from the caller, never from the body.
    assert_eq!(body["revision"]["author"], json!("alice"));

    let (status, body) = fixture
        .post_as(
            "/api/v1/annotation-exports",
            "alice",
            "aiwatcher-editors",
            json!({ "project": ANNOTATION_PROJECT, "note": "first cut" }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let export = body["manifest"]["export"].as_str().expect("an export id");
    assert_eq!(body["manifest"]["counts"]["images"], json!(1));

    let (status, body) = fixture
        .get_as(
            &format!("/api/v1/annotation-export/coco?project={ANNOTATION_PROJECT}&export={export}"),
            "bob",
            "aiwatcher-viewers",
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["info"]["aiwatcher"]["export"], json!(export));
    assert_eq!(
        body["annotations"].as_array().expect("annotations").len(),
        1
    );
}

#[tokio::test]
async fn an_uploaded_image_comes_back_under_the_digest_the_server_computed() {
    let fixture = Fixture::behind_a_proxy(false).await;
    let png = b"\x89PNG\r\n\x1a\nnot really a png".to_vec();
    let response = fixture
        .router()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/annotation-blobs")
                .header(header::CONTENT_TYPE, "image/png")
                .header("x-authentik-username", "alice")
                .header("x-authentik-groups", "aiwatcher-editors")
                .body(Body::from(png.clone()))
                .expect("request"),
        )
        .await
        .expect("responds");
    assert_eq!(response.status(), StatusCode::CREATED);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("collects")
        .to_bytes();
    let stored: Value = serde_json::from_slice(&bytes).expect("json");
    let image_id = stored["image_id"].as_str().expect("an image id");
    assert_eq!(image_id.len(), 64);
    assert_eq!(
        stored["uri"],
        json!(format!("{}{image_id}", aiwatcher_annotations::BLOB_SCHEME))
    );

    let response = fixture
        .router()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/annotation-blobs/{image_id}"))
                .header("x-authentik-username", "bob")
                .header("x-authentik-groups", "aiwatcher-viewers")
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
        Some("image/png")
    );
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collects")
        .to_bytes();
    assert_eq!(body.as_ref(), png.as_slice());
}

// ── Training ─────────────────────────────────────────────────────────────────

const TRAINING_EXPORT: &str = "floor-plans/dom-projekt@9f3c2b1a";

fn started_run(run_id: &str, dataset: &str) -> Value {
    json!({
        "run_id": run_id,
        "model": "floor-plan-segmenter",
        "dataset": dataset,
        "framework": "pytorch",
        "device": "cuda:0",
        "code": "git:9f3c2b1",
        "params": { "batch_size": 4 },
    })
}

fn epoch_batch(index: u32, loss: f64) -> Value {
    json!({
        "epochs": [{
            "epoch": index,
            "duration_ms": 1000.0,
            "steps": 25,
            "metrics": { "loss": loss },
        }],
    })
}

#[tokio::test]
async fn the_training_routes_answer_501_when_no_object_store_is_configured() {
    let fixture = Fixture::without_registry();
    for uri in ["/api/v1/training-runs", "/api/v1/models"] {
        let (status, body) = fixture.get(uri).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{uri}: {body}");
        assert_eq!(body["code"], json!("registry_disabled"));
    }
}

#[tokio::test]
async fn a_training_run_never_touches_the_event_log() {
    // ADR_0018, asserted rather than described. A training run publishes
    // nothing: no envelope, no span, no live frame. If it did, the read model
    // would grow a row that the runs list has no way to close.
    let fixture = Fixture::behind_a_proxy(true).await;
    fixture
        .post_as(
            "/api/v1/training-runs",
            "alice",
            "aiwatcher-editors",
            started_run("run-1", TRAINING_EXPORT),
        )
        .await;
    fixture
        .post_as(
            "/api/v1/training-runs/run-1/progress",
            "alice",
            "aiwatcher-editors",
            epoch_batch(0, 1.2),
        )
        .await;

    let (status, body) = fixture
        .get_as("/api/v1/runs", "bob", "aiwatcher-viewers")
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["runs"].as_array().expect("runs").len(), 0);
    assert_eq!(body["total_known"], json!(0));
}

#[tokio::test]
async fn a_retried_epoch_lands_on_the_epoch_it_already_wrote() {
    let fixture = Fixture::behind_a_proxy(false).await;
    fixture
        .post_as(
            "/api/v1/training-runs",
            "alice",
            "aiwatcher-editors",
            started_run("run-2", TRAINING_EXPORT),
        )
        .await;
    for batch in [
        epoch_batch(0, 1.2),
        epoch_batch(1, 0.9),
        epoch_batch(1, 0.8),
    ] {
        let (status, body) = fixture
            .post_as(
                "/api/v1/training-runs/run-2/progress",
                "alice",
                "aiwatcher-editors",
                batch,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    let (status, body) = fixture
        .get_as("/api/v1/training-runs/run-2", "bob", "aiwatcher-viewers")
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let epochs = body["epochs"].as_array().expect("epochs");
    assert_eq!(epochs.len(), 2);
    assert_eq!(epochs[1]["metrics"]["loss"], json!(0.8));
}

#[tokio::test]
async fn reusing_a_finished_run_id_is_a_conflict_rather_than_a_second_curve() {
    let fixture = Fixture::behind_a_proxy(false).await;
    fixture
        .post_as(
            "/api/v1/training-runs",
            "alice",
            "aiwatcher-editors",
            started_run("run-3", TRAINING_EXPORT),
        )
        .await;
    let (status, body) = fixture
        .post_as(
            "/api/v1/training-runs/run-3/finish",
            "alice",
            "aiwatcher-editors",
            json!({ "status": "succeeded" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = fixture
        .post_as(
            "/api/v1/training-runs",
            "alice",
            "aiwatcher-editors",
            started_run("run-3", TRAINING_EXPORT),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], json!("run_closed"));
}

#[tokio::test]
async fn an_ingest_token_may_publish_a_training_run_and_may_not_promote_a_model() {
    // The whole reason an ingest token is capped at editor. It sits in a
    // trainer's environment; a leaked one must not be able to decide which
    // weights production loads next.
    let fixture = Fixture::behind_a_proxy(false).await;
    let response = fixture
        .router()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/training-runs")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {INGEST_SECRET}"))
                .body(Body::from(
                    started_run("run-4", TRAINING_EXPORT).to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("responds");
    assert_eq!(response.status(), StatusCode::CREATED);

    let response = fixture
        .router()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/models/floor-plan.segmenter/labels")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {INGEST_SECRET}"))
                .body(Body::from(
                    json!({ "label": "production", "version": "ab" }).to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("responds");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_model_version_carries_the_reason_it_cannot_be_promoted() {
    let fixture = Fixture::behind_a_proxy(false).await;
    fixture
        .post_as(
            "/api/v1/training-runs",
            "alice",
            "aiwatcher-editors",
            started_run("run-5", TRAINING_EXPORT),
        )
        .await;
    let (status, body) = fixture
        .post_as(
            "/api/v1/models",
            "alice",
            "aiwatcher-editors",
            json!({
                "name": "floor-plan.segmenter",
                "run_id": "run-5",
                "checkpoint_uri": "s3://models/run-5.pt",
                "metrics": { "validation": { "miou": 0.91 } },
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    // Recorded, and the reason arrives with the registration rather than three
    // days later when somebody tries to ship it.
    let blocked = body["promotion_blocked"].as_str().expect("a reason");
    assert!(blocked.contains("held-out"), "{blocked}");

    let version = body["version"]["version"].as_str().expect("a version");
    let (status, body) = fixture
        .post_as(
            "/api/v1/models/floor-plan.segmenter/labels",
            "admin",
            "aiwatcher-admins",
            json!({ "label": "production", "version": version }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], json!("promotion_refused"));
}

#[tokio::test]
async fn a_version_with_a_held_out_score_promotes_and_the_head_answers_with_it() {
    let fixture = Fixture::behind_a_proxy(false).await;
    fixture
        .post_as(
            "/api/v1/training-runs",
            "alice",
            "aiwatcher-editors",
            started_run("run-6", TRAINING_EXPORT),
        )
        .await;
    let (_, body) = fixture
        .post_as(
            "/api/v1/models",
            "alice",
            "aiwatcher-editors",
            json!({
                "name": "floor-plan.segmenter",
                "run_id": "run-6",
                "checkpoint_uri": "s3://models/run-6.pt",
                "metrics": {
                    "validation": { "miou": 0.81 },
                    "test": { "miou": 0.74 },
                },
            }),
        )
        .await;
    assert!(body["promotion_blocked"].is_null(), "{body}");
    let version = body["version"]["version"]
        .as_str()
        .expect("a version")
        .to_owned();

    let (status, body) = fixture
        .post_as(
            "/api/v1/models/floor-plan.segmenter/labels",
            "admin",
            "aiwatcher-admins",
            json!({ "label": "production", "version": version }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["labels"]["production"], json!(version));

    // Reading the model with no version asked for resolves `production`.
    let (status, body) = fixture
        .get_as(
            "/api/v1/models/floor-plan.segmenter",
            "bob",
            "aiwatcher-viewers",
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["current"]["version"], json!(version));
    assert_eq!(body["current"]["dataset"], json!(TRAINING_EXPORT));
}

// ── Dataset hubs and importing from one ──────────────────────────────────────

#[tokio::test]
async fn the_hub_routes_answer_501_naming_what_to_set_rather_than_an_empty_list() {
    // Sharper than the registry 501s and worth its own test. An empty search
    // result is a statement about the world — "no such corpus exists" — and a
    // deployment that simply never configured a hub must not make it.
    let fixture = Fixture::without_registry();
    for uri in [
        "/api/v1/dataset-hubs",
        "/api/v1/dataset-hubs/search?q=floor",
    ] {
        let (status, body) = fixture.get(uri).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{uri}: {body}");
        assert_eq!(body["code"], json!("hubs_disabled"));
        let message = body["message"].as_str().unwrap_or_default();
        assert!(message.contains("AIWATCHER_KAGGLE"), "{body}");
        assert!(message.contains("AIWATCHER_HUGGINGFACE_ENABLED"), "{body}");
    }
}

/// A batch of rows in the import schema, all from one building.
fn import_rows(count: usize, group: &str) -> Vec<Value> {
    (0..count)
        .map(|index| {
            json!({
                "image_id": fake_image_id(&format!("{:02x}", 0x10 + index)),
                "uri": format!("https://example.test/plan-{index}.png"),
                "width": 1064,
                "height": 1021,
                "group_id": group,
            })
        })
        .collect()
}

#[tokio::test]
async fn an_import_dry_run_registers_nothing_and_still_reports_every_problem() {
    let fixture = Fixture::behind_a_proxy(false).await;
    fixture
        .post_as(
            "/api/v1/annotation-projects",
            "alice",
            "aiwatcher-editors",
            annotation_project(),
        )
        .await;

    let (status, body) = fixture
        .post_as(
            "/api/v1/annotation-imports",
            "alice",
            "aiwatcher-editors",
            json!({
                "project": ANNOTATION_PROJECT,
                "dry_run": true,
                "source": { "hub": "huggingface", "dataset_id": "someone/plans" },
                "rows": import_rows(2, "komancza-dws"),
            }),
        )
        .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["dry_run"], json!(true));
    assert_eq!(body["accepted"], json!(2));
    assert_eq!(body["families"], json!(1));

    // And nothing was written. A preview that registers is not a preview.
    let (status, listed) = fixture
        .get_as(
            &format!("/api/v1/annotation-images?project={ANNOTATION_PROJECT}"),
            "alice",
            "aiwatcher-editors",
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(
        listed["images"].as_array().map(Vec::len),
        Some(0),
        "{listed}"
    );
}

#[tokio::test]
async fn an_import_with_no_stated_rights_says_what_that_will_cost_the_export() {
    // The safe default, and the one whose consequence is invisible later: a
    // commercial export drops every one of these and only its manifest says so.
    let fixture = Fixture::behind_a_proxy(false).await;
    fixture
        .post_as(
            "/api/v1/annotation-projects",
            "alice",
            "aiwatcher-editors",
            annotation_project(),
        )
        .await;

    let (status, body) = fixture
        .post_as(
            "/api/v1/annotation-imports",
            "alice",
            "aiwatcher-editors",
            json!({
                "project": ANNOTATION_PROJECT,
                "source": {
                    "hub": "huggingface",
                    "dataset_id": "someone/plans",
                    "claimed_license": "mit",
                },
                "rows": import_rows(1, "komancza-dws"),
            }),
        )
        .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    let warnings = body["warnings"]
        .as_array()
        .expect("warnings")
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(
        warnings.contains("commercial export will exclude"),
        "{warnings}"
    );
    // The mirror's claim is preserved and explicitly not believed.
    assert!(warnings.contains("mit"), "{warnings}");
    assert!(warnings.contains("Nobody has checked"), "{warnings}");
}

#[tokio::test]
async fn a_batch_where_every_image_is_its_own_family_is_warned_about_loudly() {
    // The mistake that turns a family split back into a per-image split, with
    // nothing in any later number saying so.
    let fixture = Fixture::behind_a_proxy(false).await;
    fixture
        .post_as(
            "/api/v1/annotation-projects",
            "alice",
            "aiwatcher-editors",
            annotation_project(),
        )
        .await;

    let rows: Vec<Value> = (0..3)
        .map(|index| {
            json!({
                "image_id": fake_image_id(&format!("{:02x}", 0x40 + index)),
                "uri": format!("https://example.test/plan-{index}.png"),
                "width": 800,
                "height": 600,
                "group_id": format!("plan-{index}"),
            })
        })
        .collect();

    let (status, body) = fixture
        .post_as(
            "/api/v1/annotation-imports",
            "alice",
            "aiwatcher-editors",
            json!({
                "project": ANNOTATION_PROJECT,
                "dry_run": true,
                "rights": { "kind": "owned", "grant": "supplier agreement" },
                "rows": rows,
            }),
        )
        .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["families"], json!(3));
    let warnings = body["warnings"].to_string();
    assert!(warnings.contains("its own family"), "{warnings}");
}

#[tokio::test]
async fn a_research_only_corpus_cannot_be_imported_as_commercially_usable() {
    // The one licence decision aiwatcher makes against the caller, and it
    // makes it because a human read that licence at the original, on a date.
    let fixture = Fixture::behind_a_proxy(false).await;
    fixture
        .post_as(
            "/api/v1/annotation-projects",
            "alice",
            "aiwatcher-editors",
            annotation_project(),
        )
        .await;

    let (status, body) = fixture
        .post_as(
            "/api/v1/annotation-imports",
            "alice",
            "aiwatcher-editors",
            json!({
                "project": ANNOTATION_PROJECT,
                "rights": { "kind": "licensed", "license": "MIT" },
                "source": {
                    "hub": "huggingface",
                    "dataset_id": "someone/cubicasa5k-mirror",
                    "claimed_license": "mit",
                    "curated_source": "cubicasa5k",
                    "curated_usage": "non_commercial",
                },
                "rows": import_rows(1, "komancza-dws"),
            }),
        )
        .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let message = body["message"].as_str().unwrap_or_default();
    assert!(message.contains("cubicasa5k"), "{body}");
    assert!(message.contains("research-only"), "{body}");
}

#[tokio::test]
async fn one_unusable_row_does_not_take_the_batch_with_it() {
    let fixture = Fixture::behind_a_proxy(false).await;
    fixture
        .post_as(
            "/api/v1/annotation-projects",
            "alice",
            "aiwatcher-editors",
            annotation_project(),
        )
        .await;

    let (status, body) = fixture
        .post_as(
            "/api/v1/annotation-imports",
            "alice",
            "aiwatcher-editors",
            json!({
                "project": ANNOTATION_PROJECT,
                "rights": { "kind": "research_only", "license": "CC BY-NC 4.0" },
                "rows": [
                    {
                        "image_id": fake_image_id("aa"),
                        "uri": "https://example.test/good.png",
                        "width": 1064,
                        "height": 1021,
                        "group_id": "family-a",
                    },
                    {
                        // No content address. The registry refuses rather than
                        // inventing one: a made-up id is two pictures sharing
                        // a key, which is a training set whose labels belong
                        // to a different image.
                        "uri": "https://example.test/no-digest.png",
                        "width": 1064,
                        "height": 1021,
                        "group_id": "family-a",
                    },
                ],
            }),
        )
        .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["accepted"], json!(1), "{body}");
    assert_eq!(body["rejected"], json!(1), "{body}");
    let outcomes = body["outcomes"].to_string();
    assert!(outcomes.contains("no-digest.png"), "{outcomes}");

    // And the good one is really there.
    let (_, listed) = fixture
        .get_as(
            &format!("/api/v1/annotation-images?project={ANNOTATION_PROJECT}"),
            "alice",
            "aiwatcher-editors",
        )
        .await;
    assert_eq!(
        listed["images"].as_array().map(Vec::len),
        Some(1),
        "{listed}"
    );
}

#[tokio::test]
async fn importing_is_an_editors_job_like_every_other_annotation_write() {
    let fixture = Fixture::behind_a_proxy(false).await;
    let (status, body) = fixture
        .post_as(
            "/api/v1/annotation-imports",
            "bob",
            "aiwatcher-viewers",
            json!({ "project": ANNOTATION_PROJECT, "rows": [] }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

#[tokio::test]
async fn an_imported_image_carries_where_it_came_from_and_what_the_mirror_claimed() {
    let fixture = Fixture::behind_a_proxy(false).await;
    fixture
        .post_as(
            "/api/v1/annotation-projects",
            "alice",
            "aiwatcher-editors",
            annotation_project(),
        )
        .await;
    let image_id = fake_image_id("bc");

    fixture
        .post_as(
            "/api/v1/annotation-imports",
            "alice",
            "aiwatcher-editors",
            json!({
                "project": ANNOTATION_PROJECT,
                "source": {
                    "hub": "kaggle",
                    "dataset_id": "someone/floor-plans",
                    "url": "https://www.kaggle.com/datasets/someone/floor-plans",
                    "claimed_license": "CC0: Public Domain",
                    "pipeline": "data_frame()->read(hub_datasets, search: 'floor plan')",
                },
                "rows": [{
                    "image_id": image_id,
                    "uri": "https://example.test/plan.png",
                    "width": 1064,
                    "height": 1021,
                    "group_id": "family-a",
                }],
            }),
        )
        .await;

    let (status, body) = fixture
        .get_as(
            &format!("/api/v1/annotation-image?project={ANNOTATION_PROJECT}&image_id={image_id}"),
            "alice",
            "aiwatcher-editors",
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["image"]["source"], json!("kaggle:someone/floor-plans"));
    // Recorded beside the rights rather than as them.
    assert_eq!(
        body["image"]["metadata"]["import.claimed_license"],
        json!("CC0: Public Domain")
    );
    assert_eq!(body["image"]["rights"]["kind"], json!("unknown"));
    assert!(
        body["image"]["metadata"]["import.pipeline"]
            .as_str()
            .is_some_and(|value| value.contains("hub_datasets")),
        "the Flow script that produced the row is the provenance: {body}"
    );
}

// ── The governed conversation archive ────────────────────────────────────────
//
// Three things are worth asserting at this layer rather than in the crate's own
// tests: the role split (this is the only area where a role decides whether
// content comes back at all), the 501 an unconfigured deployment answers, and
// that an export is accepted as a *job* rather than as a dataset.

/// A turn whose consent record is complete, as a producer would send it.
fn conversation_turn(message_id: &str, role: &str, text: &str) -> Value {
    json!({
        "conversation_id": "training-demo",
        "message_id": message_id,
        "role": role,
        "content": { "parts": [{ "kind": "text", "text": text }] },
        "provenance": { "run_id": "run-1", "model": "provider-model" },
        "policy": {
            "consent": {
                "subject": "tenant-17",
                "basis": "consent",
                "reference": "ticket-4102",
                "scope": ["train"],
            },
            "retention": { "ttl_days": 30 },
            "redaction": { "redactor": "acme-scrubber@2.1" },
        },
    })
}

#[tokio::test]
async fn every_conversation_route_answers_501_when_this_instance_keeps_no_archive() {
    // The default, and the acceptance criterion it satisfies: a deployment
    // that has decided nothing retains nothing. 501 rather than 404 so a
    // client can tell "nobody can here" from "you may not".
    let fixture = Fixture::without_registry();
    for uri in [
        "/api/v1/conversation-policy",
        "/api/v1/conversation-archive",
        "/api/v1/conversation-turns?conversation_id=x",
        "/api/v1/conversation-exports",
        "/api/v1/conversation-datasets",
    ] {
        let (status, body) = fixture.get(uri).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{uri}: {body}");
        assert_eq!(body["code"], json!("registry_disabled"), "{uri}");
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|message| message.contains("AIWATCHER_CONVERSATION_ARCHIVE")),
            "the 501 names the variable to set: {body}"
        );
    }
}

#[tokio::test]
async fn a_turn_with_no_consent_record_is_refused_with_every_problem_at_once() {
    let fixture = Fixture::behind_a_proxy(false).await;
    let mut turn = conversation_turn("m1", "user", "hello");
    turn["policy"] = json!({});

    let (status, body) = fixture
        .post_as(
            "/api/v1/conversation-turns",
            "agent",
            "aiwatcher-editors",
            json!({ "turns": [turn] }),
        )
        .await;

    // 422, not 400: the request was well formed and its content was refused.
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], json!("turn_rejected"));
    let details = body["details"].as_array().expect("a list of problems");
    assert_eq!(details.len(), 5, "{body}");
}

#[tokio::test]
async fn a_viewer_sees_everything_about_a_turn_except_what_it_says() {
    let fixture = Fixture::behind_a_proxy(false).await;
    let (status, recorded) = fixture
        .post_as(
            "/api/v1/conversation-turns",
            "agent",
            "aiwatcher-editors",
            json!({ "turns": [conversation_turn("m1", "user", "my card is 4111 1111 1111 1111")] }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{recorded}");
    let turn_id = recorded["turns"][0]["turn_id"]
        .as_str()
        .expect("a turn id")
        .to_owned();

    // The head: role, ordering, policy, findings, review state.
    let (status, body) = fixture
        .get_as(
            "/api/v1/conversation-turns?conversation_id=training-demo",
            "bob",
            "aiwatcher-viewers",
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["turns"][0]["role"], json!("user"));
    assert_eq!(body["turns"][0]["review"]["state"], json!("pending"));
    assert_eq!(
        body["turns"][0]["findings"][0]["rule"],
        json!("payment-card")
    );
    // And nowhere in it, the card number the finding is about.
    assert!(!body.to_string().contains("4111"), "{body}");

    // The words: admin only.
    let uri = format!(
        "/api/v1/conversation-turn-content?conversation_id=training-demo&turn_id={turn_id}"
    );
    let (status, body) = fixture.get_as(&uri, "bob", "aiwatcher-viewers").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, body) = fixture.get_as(&uri, "ada", "aiwatcher-admins").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["parts"][0]["text"],
        json!("my card is 4111 1111 1111 1111")
    );
}

#[tokio::test]
async fn an_ingest_token_can_record_a_turn_and_never_read_one_back() {
    // The guardrail, at the one route where breaking it would matter most: an
    // ingest token is a shared secret in an agent's environment, and it exists
    // so a producer can write. A leaked one must not be able to read the
    // archive back out.
    let fixture = Fixture::behind_a_proxy(false).await;
    let (status, body) = fixture
        .request(
            Request::builder()
                .method("POST")
                .uri("/api/v1/conversation-turns")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {INGEST_SECRET}"))
                .body(Body::from(
                    json!({ "turns": [conversation_turn("m1", "user", "hello")] }).to_string(),
                ))
                .expect("request"),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let turn_id = body["turns"][0]["turn_id"].as_str().expect("a turn id");

    let (status, body) = fixture
        .request(
            Request::builder()
                .uri(format!(
                    "/api/v1/conversation-turn-content?conversation_id=training-demo&turn_id={turn_id}"
                ))
                .header(header::AUTHORIZATION, format!("Bearer {INGEST_SECRET}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], json!("forbidden"));
}

#[tokio::test]
async fn an_export_is_accepted_as_a_job_rather_than_announced_as_a_dataset() {
    let fixture = Fixture::behind_a_proxy(false).await;
    for (message_id, role, text) in [
        ("m1", "user", "what is the weather?"),
        ("m2", "assistant", "nine degrees"),
    ] {
        let (status, recorded) = fixture
            .post_as(
                "/api/v1/conversation-turns",
                "agent",
                "aiwatcher-editors",
                json!({ "turns": [conversation_turn(message_id, role, text)] }),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{recorded}");
        let (status, body) = fixture
            .post_as(
                "/api/v1/conversation-turn-reviews",
                "ada",
                "aiwatcher-editors",
                json!({
                    "conversation_id": "training-demo",
                    "turn_id": recorded["turns"][0]["turn_id"],
                    "review": { "state": "approved" },
                }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        // The reviewer is the caller, never the request body.
        assert_eq!(body["review"]["reviewer"], json!("ada"));
    }

    // 202: what comes back is a job, and the corpus does not exist yet.
    let (status, job) = fixture
        .post_as(
            "/api/v1/conversation-exports",
            "ada",
            "aiwatcher-editors",
            json!({ "name": "training/agent-turns", "format": "chat" }),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{job}");
    assert_eq!(job["state"], json!("queued"));
    assert!(
        job["version"].is_null(),
        "a queued job has no version: {job}"
    );

    // Nothing runs a worker in a test, so the job stays queued and the
    // dataset list stays empty — which is the property worth asserting: an
    // interrupted job never appears as a completed dataset version.
    let (status, body) = fixture
        .get_as("/api/v1/conversation-datasets", "bob", "aiwatcher-viewers")
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["exports"], json!([]));
}

#[tokio::test]
async fn erasing_content_needs_the_admin_role_and_names_exactly_one_target() {
    let fixture = Fixture::behind_a_proxy(false).await;
    fixture
        .post_as(
            "/api/v1/conversation-turns",
            "agent",
            "aiwatcher-editors",
            json!({ "turns": [conversation_turn("m1", "user", "hello")] }),
        )
        .await;

    let (status, body) = fixture
        .post_as(
            "/api/v1/conversation-erasures",
            "eve",
            "aiwatcher-editors",
            json!({ "subject": "tenant-17" }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    let (status, body) = fixture
        .post_as(
            "/api/v1/conversation-erasures",
            "ada",
            "aiwatcher-admins",
            json!({ "subject": "tenant-17", "conversation_id": "training-demo" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    let (status, body) = fixture
        .post_as(
            "/api/v1/conversation-erasures",
            "ada",
            "aiwatcher-admins",
            json!({ "subject": "tenant-17" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["turns_erased"], json!(1));
}

#[tokio::test]
async fn reading_content_that_was_erased_is_a_410_rather_than_a_404() {
    // The distinction an auditor came for: "it was here and it is gone" is an
    // answer, and a 404 would make a completed erasure indistinguishable from
    // a turn that never existed.
    let fixture = Fixture::behind_a_proxy(false).await;
    let (_, recorded) = fixture
        .post_as(
            "/api/v1/conversation-turns",
            "agent",
            "aiwatcher-editors",
            json!({ "turns": [conversation_turn("m1", "user", "hello")] }),
        )
        .await;
    let turn_id = recorded["turns"][0]["turn_id"]
        .as_str()
        .expect("a turn id")
        .to_owned();
    fixture
        .post_as(
            "/api/v1/conversation-erasures",
            "ada",
            "aiwatcher-admins",
            json!({ "conversation_id": "training-demo" }),
        )
        .await;

    let (status, body) = fixture
        .get_as(
            &format!(
                "/api/v1/conversation-turn-content?conversation_id=training-demo&turn_id={turn_id}"
            ),
            "ada",
            "aiwatcher-admins",
        )
        .await;
    assert_eq!(status, StatusCode::GONE, "{body}");
    assert_eq!(body["code"], json!("erased"));

    // And the head is still there, with the reason.
    let (status, body) = fixture
        .get_as(
            "/api/v1/conversation-turns?conversation_id=training-demo",
            "bob",
            "aiwatcher-viewers",
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["turns"][0]["state"], json!("erased"));
    assert_eq!(body["turns"][0]["erasure"]["reason"], json!("request"));
    assert_eq!(body["turns"][0]["erasure"]["by"], json!("ada"));
}

// ── Managed execution ────────────────────────────────────────────────────────
//
// The store behind these is the memory adapter, which is claimable and holds
// as many processes as it likes; what a `file` store refuses is asserted in
// `aiwatcher-execution`'s own suite, against the rule rather than through HTTP.

/// A pipeline of a source, a transform and a view — the shape that reaches a
/// dataset version with Flow alone.
fn flow_only_pipeline(name: &str) -> Value {
    json!({
        "name": name,
        "description": "",
        "blocks": [
            {
                "id": "read",
                "position": { "x": 0.0, "y": 0.0 },
                "spec": { "kind": "source", "dataset": "runs", "arguments": {} }
            },
            {
                "id": "clean",
                "position": { "x": 200.0, "y": 0.0 },
                "spec": { "kind": "transform", "steps": "->limit(100)" }
            },
            {
                "id": "write",
                "position": { "x": 400.0, "y": 0.0 },
                "spec": { "kind": "view", "dataset": "clean-runs" }
            }
        ],
        "edges": [
            { "from": "read", "to": "clean" },
            { "from": "clean", "to": "write" }
        ]
    })
}

#[tokio::test]
async fn a_saved_pipeline_compiles_and_starts_and_the_caller_may_leave() {
    // What one process with no Flow service can show: the command is durable
    // and the answer is 202, so nothing about what happens next depends on this
    // connection staying open.
    let fixture = Fixture::new(false);
    let (status, _) = fixture
        .post("/api/v1/curation-pipelines", flow_only_pipeline("pii"))
        .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, accepted) = fixture
        .post(
            "/api/v1/executions",
            json!({ "target": { "kind": "curation_pipeline", "name": "pii" } }),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    assert!(accepted["created"].as_bool().expect("a flag"));

    let execution = &accepted["execution"];
    assert_eq!(execution["definition_name"], "pii");
    assert_eq!(execution["owner"], "local");
    assert_eq!(execution["mode"], "compiled");
    // A source and its transforms are one Flow query, so three blocks are two
    // steps — the fold that makes three canvas boxes light up together.
    let steps = execution["steps"].as_array().expect("the steps");
    assert_eq!(steps.len(), 2, "{execution}");
    assert_eq!(steps[0]["runtime"], "flow_php");
    assert_eq!(steps[1]["runtime"], "publish_dataset");
    // The first step is dispatched and nothing has run: 202 means the command
    // is durable, not that anything happened.
    assert_eq!(steps[0]["state"]["state_type"], "pending");
    assert_eq!(steps[1]["state"]["state_type"], "scheduled");

    // And the run has its own page, read from the transactional store rather
    // than folded from the log.
    let id = execution["execution_id"].as_str().expect("an id");
    let (status, run) = fixture.get(&format!("/api/v1/executions/{id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(run["execution"]["plan_id"], execution["plan_id"]);
    // And what may be done to it, decided where `decide`'s preconditions are
    // rather than by whoever renders the buttons.
    assert_eq!(run["allowed"], json!(["pause", "cancel"]), "{run}");
}

/// Start a hosted run and hand back its id and the version to append at.
async fn hosted_run(fixture: &Fixture, name: &str) -> (String, u64) {
    fixture
        .post("/api/v1/curation-pipelines", flow_only_pipeline(name))
        .await;
    let (status, accepted) = fixture
        .post(
            "/api/v1/executions",
            json!({
                "target": { "kind": "curation_pipeline", "name": name },
                "decided_by": "worker"
            }),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    assert_eq!(accepted["execution"]["owner"], "worker");
    assert_eq!(accepted["execution"]["mode"], "hosted");
    // And nothing was dispatched: the plan is this run's *shape*, and the
    // worker schedules its own next node. A `pending` step here would be a
    // reactor about to run work the worker is also doing.
    for step in accepted["execution"]["steps"]
        .as_array()
        .expect("the steps")
    {
        assert_eq!(step["state"]["state_type"], "scheduled", "{accepted}");
    }
    (
        accepted["execution"]["execution_id"]
            .as_str()
            .expect("an id")
            .to_owned(),
        accepted["execution"]["last_message_version"]
            .as_u64()
            .expect("a version"),
    )
}

fn hosted_messages(count: usize) -> Value {
    json!(
        (0..count)
            .map(|index| json!({
                "message_type": "TurnCompleted",
                "metadata": { "node": "searcher", "index": index },
                "payload": {
                    "reference": format!("agentic://turn/{index}"),
                    "digest": "f".repeat(64),
                    "size": 300,
                    "policy": "external"
                }
            }))
            .collect::<Vec<_>>()
    )
}

#[tokio::test]
async fn a_worker_appends_to_its_own_history_and_the_loser_of_a_race_is_told_where_it_got_to() {
    // Two deciders at one expected version: one 200, one 409 — and not a 503,
    // because the store worked and the caller has something to do about it. The
    // loser reloads and succeeds.
    let fixture = Fixture::new(false);
    let (execution, version) = hosted_run(&fixture, "graph").await;
    let uri = format!("/api/v1/executions/{execution}/stream");

    let (status, first) = fixture
        .post_keyed(
            &uri,
            "batch-1",
            json!({ "expected_version": version, "holder": "worker-a", "messages": hosted_messages(2) }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert!(first["created"].as_bool().expect("a flag"));
    let moved = first["version"].as_u64().expect("a version");
    assert_eq!(moved, version + 3, "one marker and two messages");

    let (status, refused) = fixture
        .post_keyed(
            &uri,
            "batch-2",
            json!({ "expected_version": version, "holder": "worker-a", "messages": hosted_messages(1) }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["code"], "version_conflict", "{refused}");

    // Reload, and the same batch at the version it actually reads goes in.
    let (status, run) = fixture
        .get(&format!("/api/v1/executions/{execution}"))
        .await;
    assert_eq!(status, StatusCode::OK);
    let now_at = run["execution"]["last_message_version"]
        .as_u64()
        .expect("a version");
    let (status, accepted) = fixture
        .post_keyed(
            &uri,
            "batch-2",
            json!({ "expected_version": now_at, "holder": "worker-a", "messages": hosted_messages(1) }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{accepted}");
}

#[tokio::test]
async fn a_repeated_append_returns_the_first_outcome_rather_than_appending_twice() {
    // The `Idempotency-Key` is the inbox key. A worker whose response was lost
    // retries the request, and must not get a second copy of every message.
    let fixture = Fixture::new(false);
    let (execution, version) = hosted_run(&fixture, "retried").await;
    let uri = format!("/api/v1/executions/{execution}/stream");
    let body = json!({ "expected_version": version, "holder": "worker-a", "messages": hosted_messages(2) });

    let (first_status, first) = fixture.post_keyed(&uri, "turn-9", body.clone()).await;
    let (second_status, second) = fixture.post_keyed(&uri, "turn-9", body).await;
    assert_eq!(first_status, StatusCode::OK);
    assert_eq!(second_status, StatusCode::OK, "a redelivery is not a 409");
    assert!(first["created"].as_bool().expect("a flag"));
    assert!(
        !second["created"].as_bool().expect("a flag"),
        "the second request appended nothing"
    );

    // And the history says so: one marker and two messages, once.
    let (status, history) = fixture
        .get(&format!("/api/v1/executions/{execution}/history?limit=500"))
        .await;
    assert_eq!(status, StatusCode::OK);
    let hosted = history["messages"]
        .as_array()
        .expect("the messages")
        .iter()
        .filter(|message| message["message"]["kind"] == "hosted")
        .count();
    assert_eq!(hosted, 3, "{history}");
}

#[tokio::test]
async fn an_append_without_an_idempotency_key_is_refused_rather_than_given_one() {
    // Deriving a key from the body would make two different batches with the
    // same content collide, which is the same bug wearing a hash.
    let fixture = Fixture::new(false);
    let (execution, version) = hosted_run(&fixture, "unkeyed").await;
    let (status, refused) = fixture
        .post(
            &format!("/api/v1/executions/{execution}/stream"),
            json!({ "expected_version": version, "holder": "worker-a", "messages": hosted_messages(1) }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert!(
        refused["message"]
            .as_str()
            .expect("a message")
            .contains("idempotency-key"),
        "{refused}"
    );
}

#[tokio::test]
async fn a_worker_may_not_append_to_a_run_this_system_decides() {
    // Two deciders on one run is what hosted mode exists to prevent, and the
    // refusal is a 409 rather than a 400: it is about the run's state, and the
    // caller's way out is to start one that is hosted.
    let fixture = Fixture::new(false);
    fixture
        .post("/api/v1/curation-pipelines", flow_only_pipeline("compiled"))
        .await;
    let (_, accepted) = fixture
        .post(
            "/api/v1/executions",
            json!({ "target": { "kind": "curation_pipeline", "name": "compiled" } }),
        )
        .await;
    let execution = accepted["execution"]["execution_id"]
        .as_str()
        .expect("an id");

    let version = accepted["execution"]["last_message_version"]
        .as_u64()
        .expect("a version");
    let (status, refused) = fixture
        .post_keyed(
            &format!("/api/v1/executions/{execution}/stream"),
            "batch-1",
            json!({ "expected_version": version, "holder": "worker-a", "messages": hosted_messages(1) }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["code"], "not_hosted", "{refused}");
}

#[tokio::test]
async fn an_append_that_names_no_version_is_refused_rather_than_appended_anyway() {
    // "Append at whatever this is at" is not a compare-and-append, and a
    // decider that did not read the stream has nothing to decide from. Making
    // it optional would have been a default a worker could drift into on the
    // one call where it matters most.
    let fixture = Fixture::new(false);
    let (execution, _) = hosted_run(&fixture, "versionless").await;
    let (status, refused) = fixture
        .post_keyed(
            &format!("/api/v1/executions/{execution}/stream"),
            "batch-1",
            json!({ "holder": "worker-a", "messages": hosted_messages(1) }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert!(
        refused.to_string().contains("expected_version"),
        "{refused}"
    );
}

#[tokio::test]
async fn an_append_that_names_a_field_this_route_does_not_read_is_refused_by_name() {
    // `deny_unknown_fields`, and the field that matters is the one a worker
    // would plausibly send: `version` instead of `expected_version` would
    // otherwise turn a compare-and-append into "append at whatever this is at",
    // which is the one guarantee it came here for.
    let fixture = Fixture::new(false);
    let (execution, version) = hosted_run(&fixture, "typo").await;
    let (status, refused) = fixture
        .post_keyed(
            &format!("/api/v1/executions/{execution}/stream"),
            "batch-1",
            json!({ "version": version, "holder": "worker-a", "messages": hosted_messages(1) }),
        )
        .await;
    // 422 with the field named, which is what axum's `Json` rejection is and
    // what a rerun naming its own endpoint already gets. The status matters far
    // less than the alternative: silently ignored, and accepted.
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    // Named, not merely refused — axum's own rejection carries the field, which
    // is the difference between a worker's typo being findable and it being a
    // guarantee that quietly stopped applying.
    assert!(refused.to_string().contains("version"), "{refused}");
}

#[tokio::test]
async fn a_history_page_walks_a_stream_rather_than_loading_it_whole() {
    // The read side of step 1. `…/history` was already the paged read of this
    // stream, so there is no second one — what changed is that it pages in the
    // store instead of loading everything and slicing it, which is the
    // difference between a curation pipeline and an agent graph that ran for a
    // day.
    let fixture = Fixture::new(false);
    let (execution, version) = hosted_run(&fixture, "paged").await;
    fixture
        .post_keyed(
            &format!("/api/v1/executions/{execution}/stream"),
            "batch-1",
            json!({ "expected_version": version, "holder": "worker-a", "messages": hosted_messages(4) }),
        )
        .await;

    let mut seen = 0;
    let mut after = 0;
    loop {
        let (status, page) = fixture
            .get(&format!(
                "/api/v1/executions/{execution}/history?after={after}&limit=2"
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "{page}");
        let messages = page["messages"].as_array().expect("the messages");
        assert!(messages.len() <= 2, "a page honours its limit");
        seen += messages.len();
        match page["next_after"].as_u64() {
            Some(next) => after = next,
            None => break,
        }
    }
    assert_eq!(seen, (version + 5) as usize, "every message, once");

    // A page past the end of a run that exists is empty, not a 404.
    let (status, page) = fixture
        .get(&format!(
            "/api/v1/executions/{execution}/history?after=9999"
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert!(page["messages"].as_array().expect("an array").is_empty());
}

#[tokio::test]
async fn one_decider_holds_a_hosted_run_and_its_replacement_takes_over_when_it_stops() {
    // Asking for a lease answers 200 either way — being told who has it is an
    // answer to that question — and it is the *append* that 409s while somebody
    // else decides.
    let fixture = Fixture::new(false);
    let (execution, version) = hosted_run(&fixture, "leased").await;
    let lease_uri = format!("/api/v1/executions/{execution}/decider-lease");

    let (status, taken) = fixture
        .post(&lease_uri, json!({ "holder": "worker-a" }))
        .await;
    assert_eq!(status, StatusCode::OK, "{taken}");
    assert_eq!(taken["outcome"], "taken", "{taken}");
    assert_eq!(taken["holder"], "worker-a", "{taken}");

    let (status, held) = fixture
        .post(&lease_uri, json!({ "holder": "worker-b" }))
        .await;
    assert_eq!(status, StatusCode::OK, "{held}");
    assert_eq!(held["outcome"], "held", "{held}");
    assert_eq!(held["holder"], "worker-a", "{held}");
    assert!(
        held["expires_at"].is_string(),
        "the refusal says when it is worth asking again: {held}"
    );

    // And `worker-b` cannot append while it is somebody else's turn — told
    // before it pays for the turn rather than after.
    let (status, blocked) = fixture
        .post_keyed(
            &format!("/api/v1/executions/{execution}/stream"),
            "b-1",
            json!({
                "expected_version": version,
                "holder": "worker-b",
                "messages": hosted_messages(1)
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{blocked}");
    assert_eq!(blocked["code"], "lease_held", "{blocked}");

    // A reader gets the same answer both claimants did — the worker asks, the
    // server answers.
    let (status, read) = fixture.get(&lease_uri).await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(read["holder"], "worker-a", "{read}");

    // Released rather than waited out, and then it is `worker-b`'s.
    let (status, released) = fixture
        .post(
            &format!("{lease_uri}/release"),
            json!({ "holder": "worker-b" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{released}");
    assert!(
        !released["released"].as_bool().expect("a flag"),
        "a lease is released by the one holding it: {released}"
    );

    let (_, released) = fixture
        .post(
            &format!("{lease_uri}/release"),
            json!({ "holder": "worker-a" }),
        )
        .await;
    assert!(
        released["released"].as_bool().expect("a flag"),
        "{released}"
    );

    let (status, gone) = fixture.get(&lease_uri).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "expired and never taken are one answer: {gone}"
    );

    let (status, taken) = fixture
        .post(&lease_uri, json!({ "holder": "worker-b" }))
        .await;
    assert_eq!(status, StatusCode::OK, "{taken}");
    assert_eq!(taken["outcome"], "taken", "{taken}");
    // And no `previous_holder`, which is the distinction worth keeping: a
    // handover is not a takeover. `worker-a` gave the lease up, so there is
    // nobody who was interrupted mid-turn to warn the replacement about. The
    // takeover-after-expiry case is the contract suite's, against all three
    // stores.
    assert!(
        taken.get("previous_holder").is_none(),
        "a clean handover interrupted nobody: {taken}"
    );

    // And now `worker-b` decides: its append goes in, and `worker-a`'s does not.
    let (status, appended) = fixture
        .post_keyed(
            &format!("/api/v1/executions/{execution}/stream"),
            "b-2",
            json!({
                "expected_version": version,
                "holder": "worker-b",
                "messages": hosted_messages(1)
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{appended}");

    let (status, stale) = fixture
        .post_keyed(
            &format!("/api/v1/executions/{execution}/stream"),
            "a-2",
            json!({
                "expected_version": version,
                "holder": "worker-a",
                "messages": hosted_messages(1)
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{stale}");
}

#[tokio::test]
async fn a_saga_s_timeout_is_a_row_this_engine_holds_and_hands_back() {
    // The timer rides the append that scheduled it
    // — one decision, one transaction — and the run's own page says what it is
    // still waiting on, which is the question a decider that restarted asks.
    let fixture = Fixture::new(false);
    let (execution, version) = hosted_run(&fixture, "timed").await;

    let (status, appended) = fixture
        .post_keyed(
            &format!("/api/v1/executions/{execution}/stream"),
            "turn-1",
            json!({
                "expected_version": version,
                "holder": "worker-a",
                "messages": hosted_messages(1),
                "timers": [{
                    "schedule": {
                        "timer_id": "reply-deadline",
                        "due_at": "2030-01-01T00:00:00Z",
                        "message": {
                            "message_type": "saga.timeout_fired",
                            "metadata": { "timeout_id": "reply-deadline" }
                        }
                    }
                }]
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{appended}");

    let (status, waiting) = fixture
        .get(&format!("/api/v1/executions/{execution}/timers"))
        .await;
    assert_eq!(status, StatusCode::OK, "{waiting}");
    let timers = waiting["timers"].as_array().expect("the timers");
    assert_eq!(timers.len(), 1, "{waiting}");
    assert_eq!(timers[0]["timer_id"], "reply-deadline");
    // The message the engine will hand back is the worker's own, stored whole.
    assert_eq!(timers[0]["message"]["message_type"], "saga.timeout_fired");

    // And withdrawing it is the ordinary thing a saga does when what it was
    // waiting for arrived first.
    let version = appended["version"].as_u64().expect("a version");
    let (status, _) = fixture
        .post_keyed(
            &format!("/api/v1/executions/{execution}/stream"),
            "turn-2",
            json!({
                "expected_version": version,
                "holder": "worker-a",
                "messages": hosted_messages(1),
                "timers": [{ "cancel": { "timer_id": "reply-deadline" } }]
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, waiting) = fixture
        .get(&format!("/api/v1/executions/{execution}/timers"))
        .await;
    assert!(
        waiting["timers"].as_array().expect("an array").is_empty(),
        "a cancelled timer is not a row: {waiting}"
    );
}

#[tokio::test]
async fn sealing_a_run_s_words_is_refused_by_name_when_there_is_no_archive() {
    // Both variables, because turning one on without the other refuses again —
    // and a refusal naming one at a time is two deployments' worth of round
    // trips to reach a working instance. Never a quiet downgrade to `external`:
    // a run that asked for its words to be sealed and got them kept somewhere
    // else instead is the failure the policy exists to prevent.
    let fixture = Fixture::new(false).without_archive();
    fixture
        .post("/api/v1/curation-pipelines", flow_only_pipeline("sealed"))
        .await;
    let (status, refused) = fixture
        .post(
            "/api/v1/executions",
            json!({
                "target": { "kind": "curation_pipeline", "name": "sealed" },
                "decided_by": "worker",
                "payloads": "sealed"
            }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    let said = refused["details"]
        .as_array()
        .expect("the reasons")
        .iter()
        .map(|line| line.as_str().unwrap_or_default())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(said.contains("AIWATCHER_CONVERSATION_ARCHIVE"), "{refused}");
    assert!(said.contains("AIWATCHER_CONVERSATION_KEYS"), "{refused}");

    // And sealing a payload there is a 501 naming the store, which is the shape
    // every optional registry here answers with — never a 404.
    let (status, _) = fixture
        .post("/api/v1/executions/whatever/payloads", json!({ "a": true }))
        .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
}

#[tokio::test]
async fn a_sealed_run_s_words_go_through_this_instance_and_come_back_only_to_an_admin() {
    // The other half of the policy, on an instance that has an archive. The
    // stream records a reference this instance issued, a plaintext digest and a
    // size — and never the words, which is the same rule an `external` run
    // keeps by leaving them somewhere else entirely.
    let fixture = Fixture::new(false);
    fixture
        .post(
            "/api/v1/curation-pipelines",
            flow_only_pipeline("sealed-run"),
        )
        .await;
    let (status, accepted) = fixture
        .post(
            "/api/v1/executions",
            json!({
                "target": { "kind": "curation_pipeline", "name": "sealed-run" },
                "decided_by": "worker",
                "payloads": "sealed"
            }),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    let execution = accepted["execution"]["execution_id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let version = accepted["execution"]["last_message_version"]
        .as_u64()
        .expect("a version");

    let (status, sealed) = fixture
        .post_bytes(
            &format!("/api/v1/executions/{execution}/payloads"),
            b"the model said something private".to_vec(),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{sealed}");
    let digest = sealed["digest"].as_str().expect("a digest").to_owned();
    let reference = sealed["reference"]
        .as_str()
        .expect("a reference")
        .to_owned();
    assert!(reference.starts_with("aiwatcher://executions/"), "{sealed}");

    let (status, appended) = fixture
        .post_keyed(
            &format!("/api/v1/executions/{execution}/stream"),
            "turn-1",
            json!({
                "expected_version": version,
                "holder": "worker-a",
                "messages": [{
                    "message_type": "TurnCompleted",
                    "metadata": {},
                    "payload": {
                        "reference": reference,
                        "digest": digest,
                        "size": sealed["size"],
                        "policy": "sealed"
                    }
                }]
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{appended}");

    // The words are not in the history: only the reference to them is.
    let (_, history) = fixture
        .get(&format!("/api/v1/executions/{execution}/history?limit=500"))
        .await;
    assert!(
        !history.to_string().contains("something private"),
        "the stream carries a reference, never the words"
    );

    // And they come back through the route that reads them — an `admin`, as
    // reading a turn's content is. This instance authenticates nobody, so what
    // is checked here is the round trip rather than the refusal.
    let (status, plaintext) = fixture
        .get_bytes(&format!("/api/v1/executions/{execution}/payloads/{digest}"))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(plaintext, b"the model said something private");
}

#[tokio::test]
async fn a_run_that_asks_for_nothing_gets_the_default_and_it_is_the_visible_one() {
    // The shipped default keeps nothing here: the words stay with the worker
    // and this instance holds a reference, a digest and a size. A deployment
    // that wants them held turns that on rather than finding it was already
    // happening.
    let fixture = Fixture::new(false);
    let (execution, version) = hosted_run(&fixture, "default-policy").await;
    let (status, appended) = fixture
        .post_keyed(
            &format!("/api/v1/executions/{execution}/stream"),
            "turn-1",
            json!({
                "expected_version": version,
                "holder": "worker-a",
                "messages": hosted_messages(1)
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{appended}");

    // And a message claiming to be sealed on that run is refused rather than
    // stored: the run's policy is fixed when it starts.
    let (status, refused) = fixture
        .post_keyed(
            &format!("/api/v1/executions/{execution}/stream"),
            "turn-2",
            json!({
                "expected_version": appended["version"],
                "holder": "worker-a",
                "messages": [{
                    "message_type": "TurnCompleted",
                    "metadata": {},
                    "payload": {
                        "reference": "aiwatcher://executions/x/payloads/abc",
                        "digest": "d".repeat(64),
                        "size": 10,
                        "policy": "sealed"
                    }
                }]
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["code"], "payload_policy", "{refused}");
}

#[tokio::test]
async fn one_idempotency_key_repeated_starts_one_execution() {
    // Repeating the same key returns the original command result.
    // The mechanism is the derived execution id and the store's own inbox, so
    // the second request decides nothing rather than deciding again.
    let fixture = Fixture::new(false);
    fixture
        .post("/api/v1/curation-pipelines", flow_only_pipeline("nightly"))
        .await;
    let body = json!({ "target": { "kind": "curation_pipeline", "name": "nightly" } });

    let (first_status, first) = fixture
        .post_keyed("/api/v1/executions", "2026-09-05", body.clone())
        .await;
    let (second_status, second) = fixture
        .post_keyed("/api/v1/executions", "2026-09-05", body)
        .await;

    assert_eq!(first_status, StatusCode::ACCEPTED);
    assert_eq!(second_status, StatusCode::ACCEPTED, "a repeat is not a 409");
    assert_eq!(
        first["execution"]["execution_id"],
        second["execution"]["execution_id"]
    );
    assert!(first["created"].as_bool().expect("a flag"));
    assert!(
        !second["created"].as_bool().expect("a flag"),
        "the second request started nothing"
    );
}

#[tokio::test]
async fn a_pipeline_that_saves_and_does_not_compile_is_refused_with_its_reasons() {
    // Saving checks the *shape* (`order_of`, which the registry owns) and
    // compiling checks what only compilation sees — here, a notebook nobody
    // pinned. A managed run pins the code it ran, and unpinned it cannot say
    // what produced its dataset version.
    let fixture = Fixture::new(false);
    let mut unpinned = flow_only_pipeline("unpinned");
    unpinned["blocks"][2] = json!({
        "id": "detect",
        "position": { "x": 400.0, "y": 0.0 },
        "spec": { "kind": "notebook", "notebook": "pii_scan", "params": {} }
    });
    unpinned["edges"] = json!([
        { "from": "read", "to": "clean" },
        { "from": "clean", "to": "detect" }
    ]);
    let (status, saved) = fixture.post("/api/v1/curation-pipelines", unpinned).await;
    assert_eq!(status, StatusCode::CREATED, "{saved}");

    let (status, refused) = fixture
        .post(
            "/api/v1/executions",
            json!({ "target": { "kind": "curation_pipeline", "name": "unpinned" } }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["code"], "plan_refused");
    assert!(
        !refused["details"]
            .as_array()
            .expect("the reasons")
            .is_empty(),
        "the canvas renders these lines: {refused}"
    );
}

#[tokio::test]
async fn a_definition_nobody_saved_is_a_404_and_not_an_empty_run() {
    let fixture = Fixture::new(false);
    let (status, missing) = fixture
        .post(
            "/api/v1/executions",
            json!({ "target": { "kind": "curation_pipeline", "name": "nothing" } }),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{missing}");

    let (status, _) = fixture.get("/api/v1/executions/never-started").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_start_naming_its_own_endpoint_is_refused() {
    // The same absence as `LaunchBody`'s and `RerunBody`'s, and the same
    // reason: a plan names a binding and its parameters, never a host. An
    // ignored field would read as accepted — 422 from axum's own JSON
    // extractor, as on the launch and the rerun.
    let fixture = Fixture::new(false);
    fixture
        .post("/api/v1/curation-pipelines", flow_only_pipeline("pii"))
        .await;
    let (status, refused) = fixture
        .post(
            "/api/v1/executions",
            json!({
                "target": { "kind": "curation_pipeline", "name": "pii" },
                "flow_url": "http://attacker.invalid/flow"
            }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
}

#[tokio::test]
async fn an_instance_with_no_workflow_store_answers_501_rather_than_404() {
    let fixture = Fixture::without_registry();
    let (status, body) = fixture
        .post(
            "/api/v1/executions",
            json!({ "target": { "kind": "curation_pipeline", "name": "pii" } }),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{body}");
    assert!(
        body["message"]
            .as_str()
            .expect("a message")
            .contains("AIWATCHER_WORKFLOW_STORE"),
        "{body}"
    );
}

// ── Context ──────────────────────────────────────────────────────────────────
//
// An old block opens with its exact historical data and code revision. The two routes answer different questions and the difference is the
// point — one is what a run *did* read, the other what a revision *would*.

#[tokio::test]
async fn an_old_revisions_block_opens_with_the_code_that_revision_pinned() {
    // The whole reason the revision is in the path rather than defaulted to the
    // head: editing the pipeline afterwards must not change what opening the
    // saved one shows.
    let fixture = Fixture::new(false);
    let mut first = flow_only_pipeline("aged");
    first["blocks"][2] = json!({
        "id": "detect",
        "position": { "x": 400.0, "y": 0.0 },
        "spec": {
            "kind": "notebook",
            "notebook": "pii_scan",
            "revision": "cd".repeat(32),
            "params": { "threshold": 0.8 }
        }
    });
    first["edges"] = json!([
        { "from": "read", "to": "clean" },
        { "from": "clean", "to": "detect" }
    ]);
    let (status, saved) = fixture
        .post("/api/v1/curation-pipelines", first.clone())
        .await;
    assert_eq!(status, StatusCode::CREATED, "{saved}");
    let old = saved["pipeline"]["revision"]
        .as_str()
        .expect("a revision")
        .to_owned();

    // Now somebody edits the notebook's pin and saves again.
    let mut second = first;
    second["blocks"][2]["spec"]["revision"] = json!("ef".repeat(32));
    fixture.post("/api/v1/curation-pipelines", second).await;

    let (status, context) = fixture
        .get(&format!(
            "/api/v1/curation-pipelines/aged/revisions/{old}/blocks/detect/context"
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{context}");
    assert_eq!(
        context["runtime"]["code_revision"],
        "cd".repeat(32),
        "the old revision still opens with the code it pinned"
    );
    assert_eq!(context["runtime"]["runtime"], "marimo");
    assert_eq!(context["runtime"]["params"]["threshold"], 0.8);
    // Nothing ran, and empty fields say so rather than borrowing a run's.
    assert!(
        context["input_artifacts"]
            .as_array()
            .expect("an array")
            .is_empty()
    );
    assert!(context.get("state").is_none() || context["state"].is_null());
    assert_eq!(context["definition_revision"], old);
}

#[tokio::test]
async fn any_box_of_the_three_one_flow_step_covers_opens_that_step() {
    // Flow executes one pipeline, so a source and its transforms are one step.
    // Opening either box has to reach it; a lookup by block id would find only
    // the source.
    let fixture = Fixture::new(false);
    let (_, saved) = fixture
        .post("/api/v1/curation-pipelines", flow_only_pipeline("folded"))
        .await;
    let revision = saved["pipeline"]["revision"].as_str().expect("a revision");

    for block in ["read", "clean"] {
        let (status, context) = fixture
            .get(&format!(
                "/api/v1/curation-pipelines/folded/revisions/{revision}/blocks/{block}/context"
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "opening {block}: {context}");
        assert_eq!(context["step_id"], "read");
        // And the script is the compiled one, not something the canvas would
        // have to assemble from the blocks it can see.
        assert!(
            context["runtime"]["script"]
                .as_str()
                .expect("a script")
                .contains("->limit(100)"),
            "{context}"
        );
        assert_eq!(context["allowed"], json!(["validate", "test"]));
    }

    let (status, _) = fixture
        .get(&format!(
            "/api/v1/curation-pipelines/folded/revisions/{revision}/blocks/nothing/context"
        ))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_running_steps_context_is_the_plan_that_run_pinned() {
    // Read from the stream rather than from the definition, which is what makes
    // it exact: the definition may have moved on several revisions since.
    let fixture = Fixture::new(false);
    fixture
        .post("/api/v1/curation-pipelines", flow_only_pipeline("live"))
        .await;
    let (_, accepted) = fixture
        .post(
            "/api/v1/executions",
            json!({ "target": { "kind": "curation_pipeline", "name": "live" } }),
        )
        .await;
    let execution = accepted["execution"]["execution_id"]
        .as_str()
        .expect("an id")
        .to_owned();

    let (status, context) = fixture
        .get(&format!(
            "/api/v1/executions/{execution}/steps/read/context"
        ))
        .await;
    assert_eq!(status, StatusCode::OK, "{context}");
    assert_eq!(context["plan_id"], accepted["execution"]["plan_id"]);
    // Keyed by the attempt its staging is named after, never by a notebook's
    // name.
    assert_eq!(context["context_id"], format!("{execution}/read/1"));
    assert_eq!(context["state"]["state"]["state_type"], "pending");

    // A step the run's plan does not have is a name that never ran here.
    let (status, _) = fixture
        .get(&format!(
            "/api/v1/executions/{execution}/steps/absent/context"
        ))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = fixture
        .get("/api/v1/executions/never/steps/read/context")
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_run_says_which_authored_blocks_became_which_steps() {
    // What lets a canvas light up. A run reports `step.started` for a *step*,
    // and a person is looking at *blocks* — three of which fold into one Flow
    // query. Working that out in the browser would mean working it out from
    // the draft on screen, which is not what this run compiled.
    let fixture = Fixture::new(false);
    let (_, saved) = fixture
        .post("/api/v1/curation-pipelines", flow_only_pipeline("lit"))
        .await;
    let revision = saved["pipeline"]["revision"]
        .as_str()
        .expect("a revision")
        .to_owned();
    let (_, accepted) = fixture
        .post(
            "/api/v1/executions",
            json!({ "target": { "kind": "curation_pipeline", "name": "lit" } }),
        )
        .await;
    let execution = accepted["execution"]["execution_id"]
        .as_str()
        .expect("an id")
        .to_owned();

    let (status, map) = fixture
        .get(&format!("/api/v1/executions/{execution}/blocks"))
        .await;

    assert_eq!(status, StatusCode::OK, "{map}");
    // The revision the canvas has to match before it may draw these states.
    assert_eq!(map["definition_revision"], revision);
    assert_eq!(map["definition_name"], "lit");
    assert_eq!(map["plan_id"], accepted["execution"]["plan_id"]);

    let steps = map["steps"].as_array().expect("the steps");
    let flow = steps
        .iter()
        .find(|step| step["step_id"] == "read")
        .expect("the query step");
    // One step, both boxes: the compiler folds a source and its transforms
    // into a single query, and they light together.
    assert_eq!(flow["runtime"], "flow_php");
    assert_eq!(flow["blocks"], json!(["read", "clean"]));

    let view = steps
        .iter()
        .find(|step| step["runtime"] == "publish_dataset")
        .expect("the view step");
    assert_eq!(view["blocks"], json!(["write"]));

    // A run nobody started has no plan to answer from.
    let (status, _) = fixture.get("/api/v1/executions/never/blocks").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

fn notebook_pipeline(name: &str) -> Value {
    json!({
        "name": name,
        "description": "",
        "blocks": [
            {
                "id": "read",
                "position": { "x": 0.0, "y": 0.0 },
                "spec": { "kind": "source", "dataset": "runs", "arguments": {} }
            },
            {
                "id": "detect",
                "position": { "x": 200.0, "y": 0.0 },
                "spec": {
                    "kind": "notebook",
                    "notebook": "pii_scan",
                    "revision": "cd".repeat(32),
                    "params": { "threshold": 0.8 }
                }
            },
            {
                "id": "write",
                "position": { "x": 400.0, "y": 0.0 },
                "spec": { "kind": "view", "dataset": "scanned" }
            }
        ],
        "edges": [
            { "from": "read", "to": "detect" },
            { "from": "detect", "to": "write" }
        ]
    })
}

#[tokio::test]
async fn opening_a_steps_editor_stages_that_attempts_own_context_and_pinned_revision() {
    // Resolved server-side. What the route is responsible for is
    // *which* context and *which* revision — the rows themselves are the
    // adapter's job, against the object store.
    let editor = Arc::new(RecordingEditor::new());
    let fixture = Fixture::with_editor(Arc::clone(&editor));
    fixture
        .post("/api/v1/curation-pipelines", notebook_pipeline("scan"))
        .await;
    let (_, accepted) = fixture
        .post(
            "/api/v1/executions",
            json!({ "target": { "kind": "curation_pipeline", "name": "scan" } }),
        )
        .await;
    let execution = accepted["execution"]["execution_id"]
        .as_str()
        .expect("an id")
        .to_owned();

    let (status, session) = fixture
        .post(
            &format!("/api/v1/executions/{execution}/steps/detect/editor"),
            json!({}),
        )
        .await;

    assert_eq!(status, StatusCode::OK, "{session}");
    assert_eq!(session["app_url"], "/ml-pipeline/app/pii_scan/");
    // The attempt is in the context because a retry read different rows —
    // and `0` here is the honest answer for a step nothing has dispatched yet,
    // whose editor therefore opens on no rows rather than on the last run's.
    assert_eq!(session["context_id"], format!("{execution}/detect/0"));
    // What ran, beside the app that serves the head. The two are never merged.
    assert_eq!(session["code_revision"], "cd".repeat(32));

    let asked = editor.seen();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].notebook, "pii_scan");
    assert_eq!(asked[0].code_revision, "cd".repeat(32));
    assert_eq!(asked[0].params["threshold"], 0.8);
}

#[tokio::test]
async fn a_step_that_is_not_a_notebook_has_no_editor_and_says_which_runtime_it_is() {
    // A 422 rather than a 404: the step is there. "There is no such step" and
    // "that step runs a Flow query" send somebody to different places.
    let editor = Arc::new(RecordingEditor::new());
    let fixture = Fixture::with_editor(Arc::clone(&editor));
    fixture
        .post("/api/v1/curation-pipelines", notebook_pipeline("scan"))
        .await;
    let (_, accepted) = fixture
        .post(
            "/api/v1/executions",
            json!({ "target": { "kind": "curation_pipeline", "name": "scan" } }),
        )
        .await;
    let execution = accepted["execution"]["execution_id"]
        .as_str()
        .expect("an id")
        .to_owned();

    let (status, body) = fixture
        .post(
            &format!("/api/v1/executions/{execution}/steps/read/editor"),
            json!({}),
        )
        .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"]
            .as_str()
            .expect("a message")
            .contains("flow_php"),
        "{body}"
    );
    // And nothing was staged, so nobody's live app moved.
    assert!(editor.seen().is_empty());
}

#[tokio::test]
async fn a_runtime_that_refuses_to_stage_is_a_bad_gateway_and_not_a_failed_run() {
    let editor = Arc::new(RecordingEditor::refusing());
    let fixture = Fixture::with_editor(Arc::clone(&editor));
    fixture
        .post("/api/v1/curation-pipelines", notebook_pipeline("scan"))
        .await;
    let (_, accepted) = fixture
        .post(
            "/api/v1/executions",
            json!({ "target": { "kind": "curation_pipeline", "name": "scan" } }),
        )
        .await;
    let execution = accepted["execution"]["execution_id"]
        .as_str()
        .expect("an id")
        .to_owned();

    let (status, body) = fixture
        .post(
            &format!("/api/v1/executions/{execution}/steps/detect/editor"),
            json!({}),
        )
        .await;

    // The runtime understood and refused, which it will do identically forever.
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert!(
        body["message"]
            .as_str()
            .expect("a message")
            .contains("no notebook called"),
        "{body}"
    );
}

#[tokio::test]
async fn an_instance_with_no_notebook_runtime_names_what_to_set_rather_than_failing_oddly() {
    let fixture = Fixture::new(false);
    fixture
        .post("/api/v1/curation-pipelines", notebook_pipeline("scan"))
        .await;
    let (_, accepted) = fixture
        .post(
            "/api/v1/executions",
            json!({ "target": { "kind": "curation_pipeline", "name": "scan" } }),
        )
        .await;
    let execution = accepted["execution"]["execution_id"]
        .as_str()
        .expect("an id")
        .to_owned();

    let (status, body) = fixture
        .post(
            &format!("/api/v1/executions/{execution}/steps/detect/editor"),
            json!({}),
        )
        .await;

    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{body}");
    assert!(
        body["message"]
            .as_str()
            .expect("a message")
            .contains("AIWATCHER_ML_PIPELINE_URL"),
        "{body}"
    );
}

#[tokio::test]
async fn a_pause_after_a_resume_is_a_command_and_not_a_redelivery_of_the_first_pause() {
    // The message id names the run's *version*, which is what tells a
    // double-click from a second intention. Derived from the execution and the
    // command name alone, this third request would land on the inbox as a
    // repeat of the first and the run would silently stay running — section
    // 43.10's failure, one layer up.
    let fixture = Fixture::new(false);
    fixture
        .post("/api/v1/curation-pipelines", flow_only_pipeline("nightly"))
        .await;
    let (_, accepted) = fixture
        .post(
            "/api/v1/executions",
            json!({ "target": { "kind": "curation_pipeline", "name": "nightly" } }),
        )
        .await;
    let id = accepted["execution"]["execution_id"]
        .as_str()
        .expect("an id")
        .to_owned();

    let (status, paused) = fixture
        .post(
            &format!("/api/v1/executions/{id}/commands/pause"),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{paused}");
    assert_eq!(paused["execution"]["state"]["state_type"], "paused");
    // A paused run is offered resume, and never pause again.
    assert_eq!(paused["allowed"], json!(["resume", "cancel"]), "{paused}");

    let (status, resumed) = fixture
        .post(
            &format!("/api/v1/executions/{id}/commands/resume"),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{resumed}");
    assert_ne!(
        resumed["execution"]["state"]["state_type"], "paused",
        "a resume un-pauses"
    );

    let (status, again) = fixture
        .post(
            &format!("/api/v1/executions/{id}/commands/pause"),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(
        again["execution"]["state"]["state_type"], "paused",
        "the second pause is its own command"
    );
}

#[tokio::test]
async fn a_cancelled_run_reports_the_state_rather_than_only_accepting_the_command() {
    // The answer is the projection the decision wrote, so a caller learns what
    // its command did without a second read — and the two cannot disagree,
    // because they are one transaction.
    let fixture = Fixture::new(false);
    fixture
        .post("/api/v1/curation-pipelines", flow_only_pipeline("corpus"))
        .await;
    let (_, accepted) = fixture
        .post(
            "/api/v1/executions",
            json!({ "target": { "kind": "curation_pipeline", "name": "corpus" } }),
        )
        .await;
    let id = accepted["execution"]["execution_id"]
        .as_str()
        .expect("an id")
        .to_owned();

    let (status, cancelled) = fixture
        .post(
            &format!("/api/v1/executions/{id}/commands/cancel"),
            json!({ "reason": "the corpus was withdrawn" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{cancelled}");
    // Nothing had been dispatched, so the run is stopped outright rather than
    // waiting for an attempt somebody else's process is still holding.
    assert_eq!(
        cancelled["execution"]["state"]["state_type"], "cancelled",
        "{cancelled}"
    );
    // Nothing is offered for a run that has finished.
    assert_eq!(cancelled["allowed"], json!([]), "{cancelled}");
}

#[tokio::test]
async fn a_command_for_a_run_this_instance_never_heard_of_is_a_404_and_not_a_409() {
    // Two different answers with two different fixes: a 409 invites somebody
    // to try again later, and there is no later for an id that does not exist.
    let fixture = Fixture::new(false);
    let (status, _) = fixture
        .post("/api/v1/executions/never-started/commands/pause", json!({}))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn retrying_a_step_that_has_not_failed_is_refused_and_says_what_state_it_is_in() {
    // `decide` owns this rule and the route does not restate it: a second copy
    // in the API would be a second answer to "may this be retried", and the
    // day they disagree somebody trusts the wrong one.
    let fixture = Fixture::new(false);
    fixture
        .post("/api/v1/curation-pipelines", flow_only_pipeline("pii"))
        .await;
    let (_, accepted) = fixture
        .post(
            "/api/v1/executions",
            json!({ "target": { "kind": "curation_pipeline", "name": "pii" } }),
        )
        .await;
    let id = accepted["execution"]["execution_id"]
        .as_str()
        .expect("an id")
        .to_owned();

    let (status, refused) = fixture
        .post(
            &format!("/api/v1/executions/{id}/steps/read/commands/retry"),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    let message = refused["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("pending"),
        "the refusal names the state the step is actually in: {message}"
    );
}

#[tokio::test]
async fn a_step_nobody_is_asking_a_question_of_refuses_an_answer() {
    let fixture = Fixture::new(false);
    fixture
        .post("/api/v1/curation-pipelines", flow_only_pipeline("pii"))
        .await;
    let (_, accepted) = fixture
        .post(
            "/api/v1/executions",
            json!({ "target": { "kind": "curation_pipeline", "name": "pii" } }),
        )
        .await;
    let id = accepted["execution"]["execution_id"]
        .as_str()
        .expect("an id")
        .to_owned();

    let (status, refused) = fixture
        .post(
            &format!("/api/v1/executions/{id}/steps/read/input"),
            json!({ "attempt": 1, "response": { "approved": true } }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
}

/// A workflow whose first step is a gate asking for `role`.
///
/// A gate first, so starting the execution parks it immediately and no service
/// this fixture does not run has to answer for the run to reach the question.
/// A curation cannot be shaped this way — its chain starts where the rows come
/// from — which is the other half of why the second authored surface exists.
fn gated_workflow(name: &str, role: &str) -> Value {
    json!({
        "name": name,
        "version": "1",
        "steps": [
            {
                "id": "sign-off",
                "approval": {
                    "prompt": "Publish these rows?",
                    "role": role,
                    "choices": ["approve", "reject"]
                }
            }
        ]
    })
}

#[tokio::test]
async fn a_gate_is_answered_by_the_role_its_own_question_asked_for() {
    // The role is a fact about the *step*, from the pinned plan, so the route
    // cannot know it in advance and the editor floor alone is not the answer.
    // An editor holds every other command on this run and is refused this one.
    let fixture = Fixture::behind_a_proxy(false).await;
    let (status, saved) = fixture
        .post_as(
            "/api/v1/workflow-definitions",
            "alice",
            "aiwatcher-editors",
            gated_workflow("promotion", "admin"),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");

    let (_, accepted) = fixture
        .post_as(
            "/api/v1/executions",
            "alice",
            "aiwatcher-editors",
            json!({ "target": { "kind": "workflow", "name": "promotion" } }),
        )
        .await;
    let id = accepted["execution"]["execution_id"]
        .as_str()
        .expect("an id")
        .to_owned();

    // An editor may pause this very run — the floor is held and nothing else
    // moved — and may not answer its gate.
    let (status, _) = fixture
        .post_as(
            &format!("/api/v1/executions/{id}/commands/pause"),
            "alice",
            "aiwatcher-editors",
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = fixture
        .post_as(
            &format!("/api/v1/executions/{id}/commands/resume"),
            "alice",
            "aiwatcher-editors",
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, refused) = fixture
        .post_as(
            &format!("/api/v1/executions/{id}/steps/sign-off/input"),
            "alice",
            "aiwatcher-editors",
            json!({ "attempt": 1, "response": "approve" }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
}

#[tokio::test]
async fn a_gate_that_asks_for_an_editor_is_answered_by_one() {
    // The other half, and the reason the floor is not simply raised for every
    // gate: a question that named nothing stricter is answered by whoever may
    // write here, which is what every curation gate does.
    let fixture = Fixture::behind_a_proxy(false).await;
    fixture
        .post_as(
            "/api/v1/workflow-definitions",
            "alice",
            "aiwatcher-editors",
            gated_workflow("release", "editor"),
        )
        .await;
    let (_, accepted) = fixture
        .post_as(
            "/api/v1/executions",
            "alice",
            "aiwatcher-editors",
            json!({ "target": { "kind": "workflow", "name": "release" } }),
        )
        .await;
    let id = accepted["execution"]["execution_id"]
        .as_str()
        .expect("an id")
        .to_owned();

    let (status, answered) = fixture
        .post_as(
            &format!("/api/v1/executions/{id}/steps/sign-off/input"),
            "alice",
            "aiwatcher-editors",
            json!({ "attempt": 1, "response": "approve" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{answered}");
    assert_eq!(answered["execution"]["state"]["state_type"], "completed");
}

#[tokio::test]
async fn an_answer_may_not_say_who_gave_it() {
    // `answered_by` comes from the session and is not a field. A body that
    // tries to set it is a 422 naming it, rather than a record of a human
    // decision saying whatever the caller preferred.
    let fixture = Fixture::new(false);
    fixture
        .post("/api/v1/curation-pipelines", flow_only_pipeline("pii"))
        .await;
    let (_, accepted) = fixture
        .post(
            "/api/v1/executions",
            json!({ "target": { "kind": "curation_pipeline", "name": "pii" } }),
        )
        .await;
    let id = accepted["execution"]["execution_id"]
        .as_str()
        .expect("an id")
        .to_owned();

    let (status, _) = fixture
        .post(
            &format!("/api/v1/executions/{id}/steps/read/input"),
            json!({ "attempt": 1, "response": {}, "answered_by": "somebody-else" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn a_schedule_is_set_read_back_and_forgotten() {
    let fixture = Fixture::new(false);
    let (status, _) = fixture
        .post("/api/v1/curation-pipelines", flow_only_pipeline("nightly"))
        .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, set) = fixture
        .put(
            "/api/v1/curation-pipelines/nightly/schedule",
            json!({ "cadence": { "every": "daily", "hour": 9, "minute": 0 }, "timezone": "Europe/Warsaw" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{set}");
    // Nothing started: setting when it runs is not asking it to run.
    assert_eq!(set["started"], Value::Null, "{set}");

    let (status, read) = fixture
        .get("/api/v1/curation-pipelines/nightly/schedule")
        .await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(read["schedule"]["schedule"]["cadence"]["every"], "daily");
    assert_eq!(read["schedule"]["schedule"]["cadence"]["hour"], 9);
    assert_eq!(read["schedule"]["schedule"]["timezone"], "Europe/Warsaw");
    assert_eq!(read["schedule"]["schedule"]["enabled"], true);
    // Answered by the server, so the panel never computes an hour of its own.
    assert!(read["next_run"].is_string(), "{read}");
    // The default, because a curation that overlaps itself is two runs writing
    // one dataset version.
    assert_eq!(read["schedule"]["schedule"]["overlap"], "skip");

    let (status, _) = fixture
        .delete("/api/v1/curation-pipelines/nightly/schedule")
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = fixture
        .get("/api/v1/curation-pipelines/nightly/schedule")
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn setting_a_schedule_with_run_now_starts_one_run_and_says_which() {
    // The user's "run it once, and from tomorrow every day at nine": one
    // intention, one request, so the second half cannot fail after the first
    // succeeded.
    let fixture = Fixture::new(false);
    let (status, _) = fixture
        .post("/api/v1/curation-pipelines", flow_only_pipeline("nightly"))
        .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, set) = fixture
        .put(
            "/api/v1/curation-pipelines/nightly/schedule",
            json!({
                "cadence": { "every": "daily", "hour": 9, "minute": 0 },
                "timezone": "UTC", "run_now": true
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{set}");

    let started = set["started"].as_str().expect("a run was started");
    // Written down as a firing, so a card does not read "never fired" straight
    // after somebody watched one start. In the workflow store beside the
    // tick's own firings, never on the schedule object — that field is
    // configuration, and a writer that is not the person setting it is the bug
    // this prevents.
    assert_eq!(set["firings"][0]["outcome"], "started", "{set}");
    assert_eq!(set["firings"][0]["execution_id"], started, "{set}");
    let (status, run) = fixture.get(&format!("/api/v1/executions/{started}")).await;
    assert_eq!(status, StatusCode::OK, "{run}");
    // Recorded as the schedule's, not as somebody clicking Run: a run nobody
    // remembers asking for at three in the morning is a question with an
    // answer.
    assert!(
        run["execution"]["requested_by"]
            .as_str()
            .is_some_and(|who| who.starts_with("schedule:")),
        "{run}"
    );
}

#[tokio::test]
async fn a_schedule_for_a_pipeline_nobody_saved_is_refused_now_rather_than_at_nine() {
    // Otherwise it is a run that fails every morning with nobody watching.
    let fixture = Fixture::new(false);

    let (status, refused) = fixture
        .put(
            "/api/v1/curation-pipelines/never-saved/schedule",
            json!({ "cadence": { "every": "daily", "hour": 9, "minute": 0 }, "timezone": "UTC" }),
        )
        .await;

    assert_eq!(status, StatusCode::NOT_FOUND, "{refused}");
}

#[tokio::test]
async fn a_schedule_nobody_could_mean_is_refused_by_the_field_that_is_wrong() {
    let fixture = Fixture::new(false);
    let (status, _) = fixture
        .post("/api/v1/curation-pipelines", flow_only_pipeline("nightly"))
        .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, refused) = fixture
        .put(
            "/api/v1/curation-pipelines/nightly/schedule",
            json!({ "cadence": { "every": "daily", "hour": 9, "minute": 0 }, "timezone": "Europe/Atlantis" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert!(
        refused["message"]
            .as_str()
            .is_some_and(|message| message.contains("Europe/Atlantis")),
        "{refused}"
    );

    // A cron expression somebody hoped would work is named rather than ignored.
    let (status, refused) = fixture
        .put(
            "/api/v1/curation-pipelines/nightly/schedule",
            json!({ "cadence": { "every": "daily", "hour": 9, "minute": 0 }, "timezone": "UTC", "cron": "0 9 * * *" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
}

#[tokio::test]
async fn editing_a_schedule_keeps_what_the_tick_last_did() {
    // It is a fact about the definition — a run was started for it at that
    // slot — and changing the hour does not make it untrue. Clearing it would
    // make every edit look like a schedule that has never fired.
    let fixture = Fixture::new(false);
    let (status, _) = fixture
        .post("/api/v1/curation-pipelines", flow_only_pipeline("nightly"))
        .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, first) = fixture
        .put(
            "/api/v1/curation-pipelines/nightly/schedule",
            json!({
                "cadence": { "every": "daily", "hour": 9, "minute": 0 },
                "timezone": "UTC", "run_now": true
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let ran = first["firings"][0]["execution_id"].clone();
    assert!(ran.is_string(), "{first}");

    let (status, edited) = fixture
        .put(
            "/api/v1/curation-pipelines/nightly/schedule",
            json!({
                "cadence": { "every": "daily", "hour": 10, "minute": 30 },
                "timezone": "UTC"
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    assert_eq!(edited["schedule"]["schedule"]["cadence"]["hour"], 10);
    // And it survives the edit because nothing about an edit touches it: the
    // firings live in the workflow store and the edit writes the object.
    assert_eq!(edited["firings"][0]["execution_id"], ran, "{edited}");
}

#[tokio::test]
async fn a_repeated_run_now_request_starts_one_run_however_long_the_retry_took() {
    // The exit criterion in its own words: "a repeated `run_now` request has
    // one result even when the retry occurs in a later second". The id used to
    // come from the second the request arrived in, so a proxy repeating the
    // PUT — or somebody clicking again because the first response was slow —
    // started a second curation over the same corpus.
    let fixture = Fixture::new(false);
    let (status, _) = fixture
        .post("/api/v1/curation-pipelines", flow_only_pipeline("nightly"))
        .await;
    assert_eq!(status, StatusCode::CREATED);

    let body = || {
        json!({
            "cadence": { "every": "daily", "hour": 9, "minute": 0 },
            "timezone": "UTC",
            "run_now": true,
            "request_id": "one-click"
        })
    };

    let (status, first) = fixture
        .put("/api/v1/curation-pipelines/nightly/schedule", body())
        .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let started = first["started"].as_str().expect("a run").to_owned();

    // The retry. A different second, deliberately: this is the case the clock
    // could not answer.
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    let (status, again) = fixture
        .put("/api/v1/curation-pipelines/nightly/schedule", body())
        .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["started"], started, "{again}");

    // And one firing, not two: the second request started nothing, so it wrote
    // nothing down either.
    let (status, view) = fixture
        .get("/api/v1/curation-pipelines/nightly/schedule")
        .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    let firings = view["firings"].as_array().expect("firings");
    assert_eq!(firings.len(), 1, "{view}");
    assert_eq!(firings[0]["execution_id"], started, "{view}");
}

#[tokio::test]
async fn a_run_now_without_a_request_id_still_starts_one_run() {
    // The fallback stays: an older client that sends no identity gets the
    // clock's second, which is what it always had.
    let fixture = Fixture::new(false);
    let (status, _) = fixture
        .post("/api/v1/curation-pipelines", flow_only_pipeline("nightly"))
        .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, set) = fixture
        .put(
            "/api/v1/curation-pipelines/nightly/schedule",
            json!({
                "cadence": { "every": "daily", "hour": 9, "minute": 0 },
                "timezone": "UTC", "run_now": true
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{set}");
    assert!(set["started"].is_string(), "{set}");
}

// ── What a run produced ──────────────────────────────────────────────────────
//
// The reader half of ADR_0029's log. The catalog has recorded a pod's output
// against the attempt that printed it since 2.4; what these check is that a
// step's view can get it back — and that it can only get back what this run
// produced.

/// What the pod launcher does when a Job ends: the bytes, then the row that
/// indexes them.
async fn keep_a_log(
    fixture: &Fixture,
    execution: &str,
    step: &str,
    attempt: u32,
    body: &str,
) -> String {
    let store = fixture.artifacts.as_ref().expect("an object store");
    let artifact = store.put_bytes("log", aiwatcher_core::ArtifactKind::Log, body.as_bytes());
    let digest = artifact.digest.clone();
    fixture
        .state
        .catalog
        .as_ref()
        .expect("a catalog")
        .record(aiwatcher_execution::CatalogedArtifact {
            artifact,
            produced_by: Some(aiwatcher_execution::Provenance {
                execution_id: aiwatcher_execution::ExecutionId::new(execution.to_owned()),
                step_id: step.to_owned(),
                attempt,
            }),
            inputs: Vec::new(),
            created_at: time::OffsetDateTime::UNIX_EPOCH,
        })
        .await
        .expect("a catalog row");
    digest
}

#[tokio::test]
async fn a_pods_log_is_listed_against_the_attempt_that_printed_it() {
    // The join the step's view is drawn from: a row names the execution, the
    // step and the attempt, so a view holding those three can find its own log
    // without a route per noun.
    let fixture = Fixture::new(false);
    keep_a_log(&fixture, "run-1", "analyze", 2, "Traceback…\n").await;
    keep_a_log(&fixture, "run-1", "persist", 1, "written\n").await;

    let (status, produced) = fixture.get("/api/v1/executions/run-1/artifacts").await;
    assert_eq!(status, StatusCode::OK, "{produced}");
    let rows = produced.as_array().expect("a list");
    assert_eq!(rows.len(), 2, "{produced}");
    let analyze = rows
        .iter()
        .find(|row| row["produced_by"]["step_id"] == "analyze")
        .expect("the failing step's row");
    assert_eq!(analyze["produced_by"]["attempt"], 2);
    assert_eq!(analyze["artifact"]["kind"], "log");
}

#[tokio::test]
async fn a_kept_log_is_read_back_as_the_text_the_pod_printed() {
    // "Reading why a stage failed", which is the scenario the requirement is
    // written around.
    let fixture = Fixture::new(false);
    let digest = keep_a_log(
        &fixture,
        "run-1",
        "analyze",
        1,
        "Traceback (most recent call last):\n  ValueError\n",
    )
    .await;

    let (status, content) = fixture
        .get(&format!("/api/v1/executions/run-1/artifacts/{digest}"))
        .await;
    assert_eq!(status, StatusCode::OK, "{content}");
    assert!(
        content["text"]
            .as_str()
            .expect("the text")
            .contains("ValueError"),
        "{content}"
    );
    assert_eq!(content["artifact"]["digest"], digest);
}

#[tokio::test]
async fn one_runs_artifact_is_not_readable_through_another_runs_id() {
    // The digest is the whole address, and this is what stops it being an
    // oracle over a store that also holds prompts, datasets, annotations,
    // conversations and training: a digest no row of *this* run names is a 404
    // whatever is under it.
    let fixture = Fixture::new(false);
    let digest = keep_a_log(&fixture, "run-1", "analyze", 1, "secrets\n").await;

    let (status, refused) = fixture
        .get(&format!("/api/v1/executions/run-2/artifacts/{digest}"))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{refused}");
    let (status, _) = fixture
        .get("/api/v1/executions/run-1/artifacts/00000000")
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_instance_with_no_object_store_says_it_keeps_no_artifacts() {
    // An empty list would say the run produced nothing, which is a different
    // problem with a different fix. The prompt registry's rule, in a sixth
    // place.
    let fixture = Fixture::without_registry();
    for uri in [
        "/api/v1/executions/run-1/artifacts",
        "/api/v1/executions/run-1/artifacts/abcd",
    ] {
        let (status, refused) = fixture.get(uri).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{uri}: {refused}");
        assert_eq!(refused["code"], "step_artifacts_disabled", "{refused}");
        assert!(
            refused["message"]
                .as_str()
                .expect("a message")
                .contains("AIWATCHER_PROMPT_STORE"),
            "the refusal names what to set: {refused}"
        );
    }
}

// ── The worker protocol ──────────────────────────────────────────────────────
//
// What these check is the *seam*: the server keeps every rule the
// reactor keeps and the worker keeps none of them, so a claimant cannot decide
// its own lease, answer its own cache, or name an artifact this instance never
// stored.

/// A two-step plan whose steps a worker claims, on the `houses` queue.
///
/// Built and started directly rather than through `POST /executions`, because
/// nothing compiles a `python_task` step yet — the authoring path is the next
/// slice, and the protocol is testable without it.
fn worker_plan() -> aiwatcher_execution::ExecutionPlan {
    use aiwatcher_execution::plan::{
        CachePolicy, DefinitionKind, DefinitionRevision, PlanEdge, PlanStep, PythonTaskSpec,
        RetryPolicy, RuntimeBinding,
    };
    use aiwatcher_execution::plan::{InputBinding, OutputDeclaration};
    let step = |id: &str, task: &str| PlanStep {
        id: id.to_owned(),
        runtime: RuntimeBinding::PythonTask(PythonTaskSpec {
            task_ref: task.to_owned(),
            queue: "houses".to_owned(),
            params: [("threshold".to_owned(), json!(0.8))].into_iter().collect(),
        }),
        inputs: Vec::new(),
        outputs: Vec::new(),
        retry: RetryPolicy::default(),
        timeout_seconds: 60,
        cache: CachePolicy::Never,
    };
    // `review` reads what `stage` produced, which is what makes the artifact
    // routes worth having: the rows cross a process boundary twice.
    let mut stage = step("stage", "stage@1");
    stage.outputs = vec![OutputDeclaration {
        name: "rows".to_owned(),
        kind: aiwatcher_core::ArtifactKind::Rows,
        schema_ref: None,
    }];
    let mut review = step("review", "review@1");
    review.inputs = vec![InputBinding::Step {
        step: "stage".to_owned(),
        output: "rows".to_owned(),
    }];
    aiwatcher_execution::ExecutionPlan::seal(
        DefinitionKind::Workflow,
        "house-import".to_owned(),
        DefinitionRevision("ab".repeat(32)),
        vec![stage, review],
        vec![PlanEdge {
            from: "stage".to_owned(),
            to: "review".to_owned(),
        }],
    )
}

impl Fixture {
    /// Start `worker_plan` under `execution_id`, so there is something to claim.
    async fn seed_worker_run(&self, execution_id: &str) {
        use aiwatcher_execution::message::{MessageMetadata, SCHEMA_VERSION};
        use aiwatcher_execution::{
            ExecutionId, ExecutionMode, ExecutionOwner, Now, WorkflowCommand, WorkflowMessage,
        };

        let execution = ExecutionId::new(execution_id.to_owned());
        let handler = self.state.executions.as_ref().expect("a store");
        handler
            .handle(
                &execution,
                WorkflowMessage::Command(WorkflowCommand::StartExecution {
                    execution_id: execution.clone(),
                    plan: Box::new(worker_plan()),
                    // `local`, not `worker`: the Rust decider schedules this
                    // static plan and workers only *perform* its steps.
                    // `worker`/`hosted` is a decider living in the worker,
                    // which is the hosted mode and not this.
                    owner: ExecutionOwner::Local,
                    mode: ExecutionMode::Compiled,
                    payloads: Default::default(),
                    requested_by: "mk".to_owned(),
                    input: std::collections::BTreeMap::new(),
                }),
                MessageMetadata {
                    schema_version: SCHEMA_VERSION,
                    message_id: aiwatcher_core::MessageId::new(format!("start/{execution_id}")),
                    occurred_at: time::OffsetDateTime::now_utc(),
                    correlation_id: aiwatcher_core::CorrelationId::new(execution_id),
                    causation_id: aiwatcher_core::CausationId::new(execution_id),
                    trace_id: None,
                    span_id: None,
                    step_id: None,
                    attempt: None,
                },
                Now::at(time::OffsetDateTime::now_utc()),
            )
            .await
            .expect("a start");
    }

    async fn claim_as(&self, token: &str, body: Value) -> (StatusCode, Value) {
        self.send_with_token("POST", "/api/v1/worker/claims", token, Some(body))
            .await
    }
}

#[tokio::test]
async fn a_worker_is_handed_one_attempt_with_everything_needed_to_run_it() {
    // The claim does the whole front half of the reactor: the row, the plan the
    // run pinned, the cache lookup and `step.started`. What comes back is the
    // work and nothing about how to decide anything.
    let fixture = Fixture::behind_a_proxy(false).await;
    fixture.seed_worker_run("exec-w1").await;

    let (status, body) = fixture
        .claim_as(
            WORKER_SECRET,
            json!({ "worker": "laptop-1", "tasks": ["stage@1", "review@1"] }),
        )
        .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["execution_id"], "exec-w1");
    assert_eq!(body["step_id"], "stage", "the root, not the step behind it");
    assert_eq!(body["attempt"], 1);
    assert_eq!(body["task_ref"], "stage@1");
    assert_eq!(body["queue"], "houses");
    assert_eq!(body["context_id"], "exec-w1/stage/1");
    assert_eq!(body["params"]["threshold"], json!(0.8));
    assert!(!body["is_retake"].as_bool().expect("a flag"));
    assert!(
        body["lease_expires_at"].is_string(),
        "a deadline, not a duration: the two clocks are the whole question"
    );

    // And exactly one: a claim is one attempt, so a second poll finds the
    // second step still waiting on the first.
    let (status, body) = fixture
        .claim_as(
            WORKER_SECRET,
            json!({ "worker": "laptop-2", "tasks": ["stage@1", "review@1"] }),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
}

#[tokio::test]
async fn a_worker_that_does_not_hold_the_pinned_version_is_handed_nothing() {
    // The first guardrail, in the pulled half. A worker mid-deploy holds one
    // version; taking the other's attempt would fail a run over a rollout
    // rather than leave it for the pod that can run it.
    let fixture = Fixture::behind_a_proxy(false).await;
    fixture.seed_worker_run("exec-w2").await;

    let (status, body) = fixture
        .claim_as(
            WORKER_SECRET,
            json!({ "worker": "laptop-1", "tasks": ["stage@2"] }),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    // Negative control: the same worker with the right version gets it, so the
    // refusal above is about the pin and not about the claim path being broken.
    let (status, body) = fixture
        .claim_as(
            WORKER_SECRET,
            json!({ "worker": "laptop-1", "tasks": ["stage@1"] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn a_token_that_does_not_authorise_a_queue_is_refused_by_name() {
    // Silently narrowing to the queues the token does hold would leave a
    // misconfigured worker polling an empty filter forever, with nothing
    // anywhere saying why.
    let fixture = Fixture::behind_a_proxy(false).await;
    fixture.seed_worker_run("exec-w3").await;

    let (status, body) = fixture
        .claim_as(
            WORKER_SECRET,
            json!({ "worker": "laptop-1", "queues": ["plans"], "tasks": ["stage@1"] }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"]
            .as_str()
            .expect("a message")
            .contains("plans"),
        "{body}"
    );

    // And a producer's token claims nothing at all, however it asks. The role
    // is the same `Editor`; what it lacks is a queue.
    let (status, body) = fixture
        .claim_as(
            "0123456789abcdef0123456789abcdef",
            json!({ "worker": "laptop-1", "tasks": ["stage@1"] }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

#[tokio::test]
async fn a_workers_result_reaches_the_decider_and_starts_what_comes_next() {
    let fixture = Fixture::behind_a_proxy(false).await;
    fixture.seed_worker_run("exec-w4").await;
    let (_, claimed) = fixture
        .claim_as(
            WORKER_SECRET,
            json!({ "worker": "laptop-1", "tasks": ["stage@1", "review@1"] }),
        )
        .await;
    assert_eq!(claimed["step_id"], "stage");

    // Rows go through this attempt's own route, which content-addresses them.
    let (status, stored) = fixture
        .send_with_token(
            "POST",
            "/api/v1/worker/claims/exec-w4/stage/1/outputs/rows?worker=laptop-1",
            WORKER_SECRET,
            Some(json!({ "rows": [{ "id": 1, "town": "Poznań" }] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{stored}");
    assert_eq!(
        stored["digest"].as_str().expect("a digest").len(),
        64,
        "the digest is of the bytes this instance wrote"
    );

    let (status, settled) = fixture
        .send_with_token(
            "POST",
            "/api/v1/worker/claims/exec-w4/stage/1/result",
            WORKER_SECRET,
            Some(json!({
                "worker": "laptop-1",
                "outcome": "completed",
                "outputs": [stored],
                "result": { "rows": 1 }
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{settled}");
    assert!(settled["succeeded"].as_bool().expect("a flag"), "{settled}");

    // The decider took it from there: the second step is now claimable, which
    // is the whole point of the report going through the handler rather than
    // being written down by the worker.
    let (status, next) = fixture
        .claim_as(
            WORKER_SECRET,
            json!({ "worker": "laptop-1", "tasks": ["stage@1", "review@1"] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{next}");
    assert_eq!(next["step_id"], "review");
}

#[tokio::test]
async fn a_worker_that_stopped_to_ask_parks_its_attempt_and_nobody_else_may_take_it() {
    // The other kind of gate, end to end over the protocol. An authored
    // `HumanInput` step waits from the start and never reaches the claim table;
    // this one was already running and holds a lease something has to release.
    let fixture = Fixture::behind_a_proxy(false).await;
    fixture.seed_worker_run("exec-w20").await;
    let (_, claimed) = fixture
        .claim_as(
            WORKER_SECRET,
            json!({ "worker": "laptop-1", "tasks": ["stage@1", "review@1"] }),
        )
        .await;
    assert_eq!(claimed["step_id"], "stage");

    let (status, settled) = fixture
        .send_with_token(
            "POST",
            "/api/v1/worker/claims/exec-w20/stage/1/result",
            WORKER_SECRET,
            Some(json!({
                "worker": "laptop-1",
                "outcome": "parked",
                "prompt": "Send this to the council?",
                "choices": ["approve", "reject"],
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{settled}");
    assert_eq!(settled["outcome"], "parked", "{settled}");
    assert!(
        !settled["succeeded"].as_bool().expect("a flag"),
        "a question is not a success, and the older field still says so: {settled}"
    );

    // The question reached the run's own page, which is where somebody answers
    // it. Read through the same route the panel reads.
    let (status, run) = fixture
        .get_as("/api/v1/executions/exec-w20", "alice", "aiwatcher-editors")
        .await;
    assert_eq!(status, StatusCode::OK, "{run}");
    let step = run["execution"]["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .find(|step| step["step_id"] == "stage")
        .expect("the parked step");
    assert_eq!(step["state"]["state_type"], "awaiting_input", "{step}");
    assert_eq!(step["awaiting"]["prompt"], "Send this to the council?");

    // And nobody may pick it up. The row is kept for exactly this: a released
    // lease is not an invitation, because the work is not waiting for a worker
    // — it is waiting for a person.
    let (status, next) = fixture
        .claim_as(
            WORKER_SECRET,
            json!({ "worker": "laptop-2", "tasks": ["stage@1", "review@1"] }),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "a question a worker is waiting on was handed to another worker: {next}"
    );
}

#[tokio::test]
async fn a_question_a_worker_asked_for_an_admin_is_refused_to_an_editor() {
    // A gate the plan declared carries its role in the plan; a question a
    // running attempt asked carries it on the question. Either way the route
    // holds the editor floor and then the role the question named.
    let fixture = Fixture::behind_a_proxy(false).await;
    fixture.seed_worker_run("exec-w23").await;
    fixture
        .claim_as(
            WORKER_SECRET,
            json!({ "worker": "laptop-1", "tasks": ["stage@1", "review@1"] }),
        )
        .await;
    let (status, settled) = fixture
        .send_with_token(
            "POST",
            "/api/v1/worker/claims/exec-w23/stage/1/result",
            WORKER_SECRET,
            Some(json!({
                "worker": "laptop-1",
                "outcome": "parked",
                "prompt": "Promote the candidate to production?",
                "choices": ["promote", "keep"],
                "role": "admin",
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{settled}");

    let (status, refused) = fixture
        .post_as(
            "/api/v1/executions/exec-w23/steps/stage/input",
            "alice",
            "aiwatcher-editors",
            json!({ "attempt": 1, "response": "promote" }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");

    let (status, run) = fixture
        .get_as("/api/v1/executions/exec-w23", "alice", "aiwatcher-editors")
        .await;
    assert_eq!(status, StatusCode::OK, "{run}");
    let step = run["execution"]["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .find(|step| step["step_id"] == "stage")
        .expect("the parked step");
    assert_eq!(
        step["state"]["state_type"], "awaiting_input",
        "the refusal answered nothing: {step}"
    );

    let (status, answered) = fixture
        .post_as(
            "/api/v1/executions/exec-w23/steps/stage/input",
            "root",
            "aiwatcher-admins",
            json!({ "attempt": 1, "response": "promote" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{answered}");
}

#[tokio::test]
async fn an_answer_hands_the_same_step_back_to_a_worker_as_a_new_attempt() {
    // The resumed attempt is a *new* one, so the attempt that asked stays
    // immutable — and it carries the answer, because it re-runs the work from
    // the beginning and would otherwise ask the same question again.
    let fixture = Fixture::behind_a_proxy(false).await;
    fixture.seed_worker_run("exec-w21").await;
    fixture
        .claim_as(
            WORKER_SECRET,
            json!({ "worker": "laptop-1", "tasks": ["stage@1", "review@1"] }),
        )
        .await;
    let (status, settled) = fixture
        .send_with_token(
            "POST",
            "/api/v1/worker/claims/exec-w21/stage/1/result",
            WORKER_SECRET,
            Some(json!({
                "worker": "laptop-1",
                "outcome": "parked",
                "prompt": "Send this to the council?",
                "choices": ["approve", "reject"],
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{settled}");

    let (status, answered) = fixture
        .post_as(
            "/api/v1/executions/exec-w21/steps/stage/input",
            "alice",
            "aiwatcher-editors",
            json!({ "attempt": 1, "response": "approve" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{answered}");

    let (status, next) = fixture
        .claim_as(
            WORKER_SECRET,
            json!({ "worker": "laptop-1", "tasks": ["stage@1", "review@1"] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{next}");
    assert_eq!(next["step_id"], "stage", "the same step, not the next one");
    assert_eq!(next["attempt"], 2, "{next}");
    assert_eq!(
        next["answers"],
        json!([{ "attempt": 1, "answered_by": "alice", "response": "approve" }]),
        "the resumed attempt is handed what the previous one asked for: {next}"
    );
}

#[tokio::test]
async fn a_question_nobody_could_answer_as_asked_is_refused_by_the_one_rule_set() {
    // A worker's park is a third authored surface for one question, so it is
    // refused by `aiwatcher_core::human_input` rather than by a third rule set
    // — with every problem at once, as a canvas block and a workflow step are.
    let fixture = Fixture::behind_a_proxy(false).await;
    fixture.seed_worker_run("exec-w22").await;
    fixture
        .claim_as(
            WORKER_SECRET,
            json!({ "worker": "laptop-1", "tasks": ["stage@1", "review@1"] }),
        )
        .await;

    let (status, refused) = fixture
        .send_with_token(
            "POST",
            "/api/v1/worker/claims/exec-w22/stage/1/result",
            WORKER_SECRET,
            Some(json!({
                "worker": "laptop-1",
                "outcome": "parked",
                // Blank, and asking for a role a gate may not name. A gate only
                // ever raises the editor floor.
                "prompt": "   ",
                "role": "viewer",
            })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["code"], "question_refused");
    assert_eq!(
        refused["details"].as_array().expect("details").len(),
        2,
        "one problem per round trip teaches somebody to press the button again: {refused}"
    );
}
#[tokio::test]
async fn a_worker_may_not_settle_an_attempt_it_does_not_hold() {
    // Worker names are not secret, so this is the check that stops one from
    // reporting over somebody else's work by guessing the name it was claimed
    // under. Its own work is discarded — the reactor's rule, that a claimant
    // whose lease went must not write beside its replacement.
    let fixture = Fixture::behind_a_proxy(false).await;
    fixture.seed_worker_run("exec-w5").await;
    fixture
        .claim_as(
            WORKER_SECRET,
            json!({ "worker": "laptop-1", "tasks": ["stage@1"] }),
        )
        .await;

    for (what, body) in [
        (
            "a result",
            json!({ "worker": "impostor", "outcome": "completed" }),
        ),
        ("a heartbeat", json!({ "worker": "impostor" })),
    ] {
        let uri = if what == "a result" {
            "/api/v1/worker/claims/exec-w5/stage/1/result"
        } else {
            "/api/v1/worker/claims/exec-w5/stage/1/heartbeat"
        };
        let (status, answer) = fixture
            .send_with_token("POST", uri, WORKER_SECRET, Some(body))
            .await;
        assert_eq!(status, StatusCode::CONFLICT, "{what}: {answer}");
        assert_eq!(answer["code"], "lease_lost", "{what}");
    }

    // Negative control: the holder is accepted, so the refusals above are
    // about the name rather than about the routes being broken.
    let (status, answer) = fixture
        .send_with_token(
            "POST",
            "/api/v1/worker/claims/exec-w5/stage/1/heartbeat",
            WORKER_SECRET,
            Some(json!({ "worker": "laptop-1" })),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{answer}");
}

#[tokio::test]
async fn an_output_this_instance_never_stored_is_refused_rather_than_recorded() {
    // A worker could otherwise describe a reference instead of writing one, and
    // a completed step pointing at an object that 404s is the one failure
    // nothing downstream would catch.
    let fixture = Fixture::behind_a_proxy(false).await;
    fixture.seed_worker_run("exec-w6").await;
    fixture
        .claim_as(
            WORKER_SECRET,
            json!({ "worker": "laptop-1", "tasks": ["stage@1"] }),
        )
        .await;

    let (status, body) = fixture
        .send_with_token(
            "POST",
            "/api/v1/worker/claims/exec-w6/stage/1/result",
            WORKER_SECRET,
            Some(json!({
                "worker": "laptop-1",
                "outcome": "completed",
                "outputs": [{
                    "name": "rows",
                    "uri": "object://artifacts/rows/deadbeef/data",
                    "digest": "de".repeat(32),
                    "kind": "rows",
                }]
            })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"]
            .as_str()
            .expect("a message")
            .contains("outputs route"),
        "{body}"
    );

    // And the attempt is still claimable by its holder, so a refused report
    // costs the work rather than the run.
    let (status, again) = fixture
        .send_with_token(
            "POST",
            "/api/v1/worker/claims/exec-w6/stage/1/heartbeat",
            WORKER_SECRET,
            Some(json!({ "worker": "laptop-1" })),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{again}");
}

#[tokio::test]
async fn a_failed_attempt_is_the_deciders_business_and_not_the_end_of_the_run() {
    // The worker classifies its failure because the process that made the call
    // is the one that knows; what happens next is the decider's, and a worker
    // that scheduled its own retry would be the second orchestrator ADR_0025
    // refuses.
    let fixture = Fixture::behind_a_proxy(false).await;
    fixture.seed_worker_run("exec-w7").await;
    fixture
        .claim_as(
            WORKER_SECRET,
            json!({ "worker": "laptop-1", "tasks": ["stage@1"] }),
        )
        .await;

    let (status, settled) = fixture
        .send_with_token(
            "POST",
            "/api/v1/worker/claims/exec-w7/stage/1/result",
            WORKER_SECRET,
            Some(json!({
                "worker": "laptop-1",
                "outcome": "failed",
                "class": "transient",
                "message": "the geocoder answered 503"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{settled}");
    assert!(
        !settled["succeeded"].as_bool().expect("a flag"),
        "{settled}"
    );

    // A retryable class means a second attempt, not a dead run. It is not
    // claimable *yet*, and that is the decider's backoff rather than an
    // absence — a worker that took it back instantly would be retrying at the
    // rate of its own poll loop.
    let (status, waiting) = fixture
        .claim_as(
            WORKER_SECRET,
            json!({ "worker": "laptop-1", "tasks": ["stage@1"] }),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{waiting}");

    let (status, run) = fixture
        .get_as("/api/v1/executions/exec-w7", "alice", "aiwatcher-editors")
        .await;
    assert_eq!(status, StatusCode::OK, "{run}");
    assert_eq!(run["execution"]["state"]["state_type"], "running", "{run}");
    let step = run["execution"]["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .find(|step| step["step_id"] == "stage")
        .expect("the step that failed");
    assert_eq!(step["state"]["name"], "AwaitingRetry", "{step}");
    assert_eq!(step["current_attempt"], 2, "{step}");
    assert!(
        step["attempts"][1]["not_before"].is_string(),
        "the backoff is why the claim above was empty: {step}"
    );
}

#[tokio::test]
async fn a_second_step_reads_what_the_first_produced_and_nothing_else() {
    // The proxied artifact path, end to end and across two claims. Presigning
    // was the alternative and this is why it was not taken: what the route can
    // check is that the caller holds the lease on the attempt whose input it
    // is asking for, and a URL scoped by a prefix and a clock cannot.
    let fixture = Fixture::behind_a_proxy(false).await;
    fixture.seed_worker_run("exec-w8").await;
    let tasks = json!({ "worker": "laptop-1", "tasks": ["stage@1", "review@1"] });

    fixture.claim_as(WORKER_SECRET, tasks.clone()).await;
    let (_, stored) = fixture
        .send_with_token(
            "POST",
            "/api/v1/worker/claims/exec-w8/stage/1/outputs/rows?worker=laptop-1",
            WORKER_SECRET,
            Some(json!({ "rows": [{ "id": 1, "town": "Poznań" }, { "id": 2, "town": "Kraków" }] })),
        )
        .await;
    fixture
        .send_with_token(
            "POST",
            "/api/v1/worker/claims/exec-w8/stage/1/result",
            WORKER_SECRET,
            Some(json!({
                "worker": "laptop-1",
                "outcome": "completed",
                "outputs": [stored]
            })),
        )
        .await;

    let (status, next) = fixture.claim_as(WORKER_SECRET, tasks).await;
    assert_eq!(status, StatusCode::OK, "{next}");
    assert_eq!(next["step_id"], "review");
    assert_eq!(
        next["inputs"][0]["name"], "rows",
        "the assignment names what the parent produced: {next}"
    );

    let (status, rows) = fixture
        .send_with_token(
            "GET",
            "/api/v1/worker/claims/exec-w8/review/1/inputs/rows?worker=laptop-1",
            WORKER_SECRET,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{rows}");
    assert_eq!(rows["rows"].as_array().expect("rows").len(), 2);
    assert_eq!(rows["rows"][1]["town"], "Kraków");

    // An input this attempt does not have is a 404, and one belonging to an
    // attempt this worker does not hold is a 409 — two different sentences,
    // because "there is no such input" and "you no longer hold this" send a
    // worker to two different places.
    let (status, missing) = fixture
        .send_with_token(
            "GET",
            "/api/v1/worker/claims/exec-w8/review/1/inputs/nope?worker=laptop-1",
            WORKER_SECRET,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{missing}");

    let (status, refused) = fixture
        .send_with_token(
            "GET",
            "/api/v1/worker/claims/exec-w8/review/1/inputs/rows?worker=impostor",
            WORKER_SECRET,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["code"], "lease_lost");
}

#[tokio::test]
async fn holding_the_lease_is_not_enough_if_the_token_is_for_another_queue() {
    // The other half of `worker::held`, and the half a name check does not
    // reach: this caller names the worker that really holds the attempt and
    // gets the name right. What it does not have is a token for the queue the
    // attempt was dispatched to — and worker names are not secret, so without
    // this check that is all it would need.
    let fixture = Fixture::behind_a_proxy(false).await;
    fixture.seed_worker_run("exec-w9").await;
    let (status, claimed) = fixture
        .claim_as(
            WORKER_SECRET,
            json!({ "worker": "laptop-1", "tasks": ["stage@1"] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{claimed}");

    for (what, uri, body) in [
        (
            "a heartbeat",
            "/api/v1/worker/claims/exec-w9/stage/1/heartbeat",
            json!({ "worker": "laptop-1" }),
        ),
        (
            "a result",
            "/api/v1/worker/claims/exec-w9/stage/1/result",
            json!({ "worker": "laptop-1", "outcome": "completed" }),
        ),
    ] {
        let (status, answer) = fixture
            .send_with_token("POST", uri, OTHER_WORKER_SECRET, Some(body))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{what}: {answer}");
        assert_eq!(answer["code"], "forbidden", "{what}");
    }

    // And the rows too: a token for another queue may not read what this
    // attempt was handed, which is the check that makes proxying the bytes
    // worth more than a presigned URL.
    let (status, refused) = fixture
        .send_with_token(
            "GET",
            "/api/v1/worker/claims/exec-w9/stage/1/inputs/rows?worker=laptop-1",
            OTHER_WORKER_SECRET,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");

    // Negative control: the token that does hold the queue is accepted with
    // the same worker name, so the refusals above are about the scope.
    let (status, ok) = fixture
        .send_with_token(
            "POST",
            "/api/v1/worker/claims/exec-w9/stage/1/heartbeat",
            WORKER_SECRET,
            Some(json!({ "worker": "laptop-1" })),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{ok}");
}

/// One step asking for a pod from planner's template.
fn podded_workflow(image: &str, cpu: &str) -> Value {
    json!({"name":"pod-import", "version":"1", "steps":[
        {"id":"acquire", "task_ref":"acquire@1", "queue":"planner-import", "timeout_seconds":30,
         "pod": {"template": "planner-import", "image": image, "cpu": cpu}}
    ]})
}

fn planner_pod_templates() -> Value {
    json!({"planner-import": {
        "images": ["ghcr.io/planner/import"],
        "resources": {"max": {"cpu": "2", "memory": "4Gi"}},
        "command": ["python", "-m", "aiwatcher_sdk.worker", "run-attempt"]
    }})
}

#[tokio::test]
async fn a_step_asking_for_a_pod_its_template_does_not_allow_is_refused_and_nothing_is_stored() {
    let fixture = Fixture::new(true).with_pod_templates(planner_pod_templates());

    let (status, refused) = fixture
        .post(
            "/api/v1/workflow-definitions",
            podded_workflow("ghcr.io/planner/import-debug:1", "4"),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    let details: Vec<&str> = refused["details"]
        .as_array()
        .expect("details")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    // Every problem at once, each naming the step, the value and the template.
    assert_eq!(details.len(), 2, "{details:?}");
    assert!(
        details
            .iter()
            .any(|problem| problem.starts_with("acquire: ")
                && problem.contains("ghcr.io/planner/import-debug:1")
                && problem.contains("planner-import")),
        "{details:?}"
    );
    assert!(
        details
            .iter()
            .any(|problem| problem.contains("cpu '4'") && problem.contains("ceiling of 2")),
        "{details:?}"
    );
    let (status, _) = fixture.get("/api/v1/workflow-definitions/pod-import").await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a refused definition is not stored"
    );

    // On the list and under the ceiling, the same step registers.
    let (status, saved) = fixture
        .post(
            "/api/v1/workflow-definitions",
            podded_workflow("ghcr.io/planner/import:1.4", "1500m"),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(
        saved["definition"]["steps"][0]["pod"]["image"],
        "ghcr.io/planner/import:1.4"
    );
}

#[tokio::test]
async fn a_step_asking_for_a_pod_where_no_template_is_configured_is_refused_naming_the_variable() {
    // `just run`: no templates, so a definition no launcher could ever run
    // here is not one to store.
    let fixture = Fixture::new(true);
    let (status, refused) = fixture
        .post(
            "/api/v1/workflow-definitions",
            podded_workflow("ghcr.io/planner/import:1.4", "1"),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert!(
        refused["details"][0]
            .as_str()
            .is_some_and(|problem| problem.contains("AIWATCHER_POD_TEMPLATES")),
        "{refused}"
    );
}

#[tokio::test]
async fn a_pods_attempt_is_claimed_by_its_key_and_by_no_worker_that_did_not_name_it() {
    let fixture = Fixture::new(true).with_pod_templates(planner_pod_templates());
    let (status, saved) = fixture
        .post(
            "/api/v1/workflow-definitions",
            podded_workflow("ghcr.io/planner/import:1.4", "1"),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let (status, started) = fixture
        .post(
            "/api/v1/executions",
            json!({"target":{"kind":"workflow", "name":"pod-import", "revision":saved["revision"]},
                   "parameters":{}}),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    let id = started["execution"]["execution_id"].as_str().expect("id");

    // A long-lived worker on the pod's queue, holding its code: ADR_0029's
    // hole. It would run the step outside any pod.
    let (status, body) = fixture
        .post(
            "/api/v1/worker/claims",
            json!({"worker":"laptop-1", "queues":["planner-import"], "tasks":["acquire@1"]}),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    // The pod, naming its attempt, is handed what any worker is handed.
    let (status, assignment) = fixture
        .post(
            "/api/v1/worker/claims",
            json!({"worker":"aiwatcher-0a1b-xyz", "queues":["planner-import"], "tasks":["acquire@1"],
                   "attempt":{"execution_id":id, "step_id":"acquire", "attempt":1}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{assignment}");
    assert_eq!(assignment["task_ref"], "acquire@1");
    assert_eq!(assignment["queue"], "planner-import");
    let (status, settled) = fixture
        .post(
            &format!("/api/v1/worker/claims/{id}/acquire/1/result"),
            json!({"worker":"aiwatcher-0a1b-xyz", "outcome":"completed", "result":{"houses":1}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{settled}");
    assert_eq!(settled["outcome"], "completed");
}

fn authored_worker_workflow() -> Value {
    json!({"name":"sdk-import", "version":"1", "steps":[
        {"id":"acquire", "task_ref":"acquire@1", "queue":"planner-import", "timeout_seconds":30,
         "outputs":["rows"], "retry":{"max_attempts":2,"delays_seconds":[0],"delays_seconds_unavailable":[0]}},
        {"id":"persist", "task_ref":"persist@1", "queue":"planner-import", "timeout_seconds":30,
         "inputs":[{"step":"acquire","output":"rows"}]}
    ]})
}

#[tokio::test]
async fn authored_workflows_run_and_retry_through_the_same_durable_engine_and_event_log() {
    use aiwatcher_execution::{ExecutionId, WorkflowEvent, WorkflowStore};
    let fixture = Fixture::new(true);
    let (status, saved) = fixture
        .post("/api/v1/workflow-definitions", authored_worker_workflow())
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let (_, repeated) = fixture
        .post("/api/v1/workflow-definitions", authored_worker_workflow())
        .await;
    assert_eq!(saved, repeated);
    let (status, started) = fixture.post("/api/v1/executions", json!({"target":{
        "kind":"workflow", "name":"sdk-import", "revision":saved["revision"]}, "parameters":{"house":1}})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    let id = started["execution"]["execution_id"].as_str().expect("id");
    let claim =
        json!({"worker":"one", "queues":["planner-import"], "tasks":["acquire@1","persist@1"]});
    let (status, first) = fixture.post("/api/v1/worker/claims", claim.clone()).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["step_id"], "acquire");
    assert_eq!(first["workflow_id"], "sdk-import");
    let (status, body) = fixture
        .post(
            &format!("/api/v1/worker/claims/{id}/acquire/1/result"),
            json!({"worker":"one", "outcome":"failed", "class":"transient", "message":"try again"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, second) = fixture.post("/api/v1/worker/claims", claim.clone()).await;
    assert_eq!(second["attempt"], 2);
    assert_ne!(first["parent_span_id"], second["parent_span_id"]);
    let (status, artifact) = fixture
        .post(
            &format!("/api/v1/worker/claims/{id}/acquire/2/outputs/rows?worker=one"),
            json!({"rows":[{"house":1}]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{artifact}");
    let (status, body) = fixture
        .post(
            &format!("/api/v1/worker/claims/{id}/acquire/2/result"),
            json!({"worker":"one", "outcome":"completed", "outputs":[artifact]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, third) = fixture.post("/api/v1/worker/claims", claim.clone()).await;
    assert_eq!(third["step_id"], "persist");
    let (status, rows) = fixture
        .get(&format!(
            "/api/v1/worker/claims/{id}/persist/1/inputs/rows?worker=one"
        ))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rows, json!({"rows":[{"house":1}]}));
    fixture.post(&format!("/api/v1/worker/claims/{id}/persist/1/result"),
        json!({"worker":"one", "outcome":"failed", "class":"user_code", "message":"repair then retry"})).await;
    let (status, body) = fixture
        .post(
            &format!("/api/v1/executions/{id}/steps/persist/commands/retry"),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, retried) = fixture.post("/api/v1/worker/claims", claim).await;
    assert_eq!(retried["step_id"], "persist");
    assert_eq!(retried["attempt"], 2);
    fixture
        .post(
            &format!("/api/v1/worker/claims/{id}/persist/2/result"),
            json!({"worker":"one", "outcome":"completed", "result":{"saved":true}}),
        )
        .await;
    let store = fixture.state.executions.as_ref().expect("handler").store();
    let stream = store.load(&ExecutionId::new(id)).await.expect("stream");
    assert!(
        stream
            .events()
            .any(|event| matches!(event, WorkflowEvent::ExecutionCompleted))
    );
    assert_eq!(
        stream
            .events()
            .filter(|event| matches!(event, WorkflowEvent::StepFailed { .. }))
            .count(),
        2
    );
    let (status, history) = fixture
        .get(&format!("/api/v1/executions/{id}/history?limit=2"))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(history["messages"].as_array().expect("messages").len(), 2);
    assert!(history["next_after"].is_number());
    aiwatcher_execution::publish_pending(
        store.as_ref(),
        fixture.bus.as_ref(),
        time::OffsetDateTime::now_utc(),
    )
    .await
    .expect("publish");
    use aiwatcher_bus::MessageSource;
    let events = fixture
        .bus
        .read(&Checkpoint::beginning(), 1000)
        .await
        .expect("events");
    assert!(!events.is_empty());
    for event in &events {
        fixture.read_model.apply(event).await;
    }
    let (status, graph) = fixture
        .get(&format!("/api/v1/workflow-executions/{id}"))
        .await;
    assert_eq!(status, StatusCode::OK, "{graph}");
    assert_eq!(graph["nodes"].as_array().expect("nodes").len(), 2);
}

#[tokio::test]
async fn workflow_registration_validates_the_graph_and_requires_an_editor() {
    let fixture = Fixture::behind_a_proxy(true).await;
    let (status, _) = fixture
        .post_as(
            "/api/v1/workflow-definitions",
            "viewer",
            "aiwatcher-viewers",
            authored_worker_workflow(),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let mut invalid = authored_worker_workflow();
    invalid["steps"][0]["task_ref"] = json!("unpinned");
    invalid["steps"][0]["after"] = json!(["persist"]);
    let (status, body) = fixture
        .post_as(
            "/api/v1/workflow-definitions",
            "editor",
            "aiwatcher-editors",
            invalid,
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body["details"].as_array().expect("problems").len() >= 2);
    let (_, definitions) = fixture
        .get_as(
            "/api/v1/workflow-definitions",
            "viewer",
            "aiwatcher-viewers",
        )
        .await;
    assert_eq!(definitions, json!([]));
}

#[tokio::test]
async fn a_registered_workflow_is_scheduled_beside_a_pipeline_that_shares_its_name() {
    // The second half of "a definition somebody can save and schedule". The
    // store has always been keyed by kind *and* name and everything below the
    // handlers already read the kind off the stored object; what was hard-coded
    // was the three handlers, so a worker workflow could be saved and started
    // and never left to run unattended — which is most of what authoring one is
    // for.
    //
    // Two kinds sharing one name is the case worth pinning, because its failure
    // is silent: one schedule overwriting the other's hour, on a card that still
    // reads correctly.
    let fixture = Fixture::new(false);
    let daily = |hour: u8| {
        json!({ "cadence": { "every": "daily", "hour": hour, "minute": 0 },
                "timezone": "Europe/Warsaw" })
    };

    // Refused now rather than at nine tomorrow, and by the workflow compiler:
    // a schedule for a definition nobody saved is a run that fails every
    // morning with nobody watching.
    let (status, body) = fixture
        .put("/api/v1/workflow-definitions/sdk-import/schedule", daily(9))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    let (status, saved) = fixture
        .post("/api/v1/workflow-definitions", authored_worker_workflow())
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let (status, _) = fixture
        .post(
            "/api/v1/curation-pipelines",
            flow_only_pipeline("sdk-import"),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, set) = fixture
        .put("/api/v1/workflow-definitions/sdk-import/schedule", daily(9))
        .await;
    assert_eq!(status, StatusCode::OK, "{set}");
    // Setting when something runs is not asking it to run.
    assert_eq!(set["started"], Value::Null, "{set}");
    assert_eq!(set["schedule"]["definition_kind"], "workflow");
    // From the server, so nothing else works out when the clocks change.
    assert!(set["next_run"].is_string(), "{set}");

    let (status, other) = fixture
        .put("/api/v1/curation-pipelines/sdk-import/schedule", daily(10))
        .await;
    assert_eq!(status, StatusCode::OK, "{other}");
    assert_eq!(other["schedule"]["definition_kind"], "curation_pipeline");

    let (status, read) = fixture
        .get("/api/v1/workflow-definitions/sdk-import/schedule")
        .await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(
        read["schedule"]["schedule"]["cadence"]["hour"], 9,
        "the pipeline's ten must not have landed on the workflow: {read}"
    );

    // Forgetting one leaves the other, for the same reason.
    let (status, _) = fixture
        .delete("/api/v1/workflow-definitions/sdk-import/schedule")
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = fixture
        .get("/api/v1/workflow-definitions/sdk-import/schedule")
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, kept) = fixture
        .get("/api/v1/curation-pipelines/sdk-import/schedule")
        .await;
    assert_eq!(status, StatusCode::OK, "{kept}");
    assert_eq!(kept["schedule"]["schedule"]["cadence"]["hour"], 10);
}

#[tokio::test]
async fn scheduling_a_workflow_run_now_starts_the_run_a_worker_then_claims() {
    // The compiler the schedule route checks with and the one the tick starts
    // with are the same function, so this is also what says the tick can start
    // a workflow at all: `run_now` goes through `executions::start` exactly as
    // a slot does.
    let fixture = Fixture::new(true);
    let (status, saved) = fixture
        .post("/api/v1/workflow-definitions", authored_worker_workflow())
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");

    let (status, set) = fixture
        .put(
            "/api/v1/workflow-definitions/sdk-import/schedule",
            json!({ "cadence": { "every": "daily", "hour": 9, "minute": 0 },
                    "timezone": "Europe/Warsaw", "run_now": true,
                    "request_id": "one-click" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{set}");
    let started = set["started"].as_str().expect("a run").to_owned();

    // Recorded as the schedule's, not as somebody pressing Run on a canvas.
    let (status, run) = fixture.get(&format!("/api/v1/executions/{started}")).await;
    assert_eq!(status, StatusCode::OK, "{run}");
    assert!(
        run["execution"]["requested_by"]
            .as_str()
            .is_some_and(|who| who.starts_with("schedule:")),
        "{run}"
    );

    // And it is a real worker plan: the first step is claimable by a worker
    // holding that queue and that pinned version, which is the whole point of
    // authoring one.
    let (status, first) = fixture
        .post(
            "/api/v1/worker/claims",
            json!({"worker":"one", "queues":["planner-import"],
                   "tasks":["acquire@1","persist@1"]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["execution_id"], started);
    assert_eq!(first["step_id"], "acquire");

    // The same request again is the same slot and the same run — the id is
    // derived from the request, so a repeated PUT starts nothing beside it.
    let (status, again) = fixture
        .put(
            "/api/v1/workflow-definitions/sdk-import/schedule",
            json!({ "cadence": { "every": "daily", "hour": 9, "minute": 0 },
                    "timezone": "Europe/Warsaw", "run_now": true,
                    "request_id": "one-click" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["started"], Value::String(started));
}

#[tokio::test]
async fn worker_reports_replay_from_history_without_duplicate_decisions_or_queue_access() {
    let fixture = Fixture::behind_a_proxy(false).await;
    fixture.seed_worker_run("receipt").await;
    fixture.seed_worker_run("neighbour").await;
    let target = json!({"execution_id":"receipt", "step_id":"stage", "attempt":1});
    let (status, claim) = fixture
        .claim_as(
            WORKER_SECRET,
            json!({"worker":"one", "tasks":["stage@1"], "attempt":target}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{claim}");
    assert_eq!(claim["execution_id"], "receipt");
    assert_eq!(claim["outputs"], json!(["rows"]));
    assert_eq!(claim["report_idempotent"], true);
    let path = "/api/v1/worker/claims/receipt/stage/1/result";
    let missing = json!({"worker":"one", "outcome":"completed"});
    let (status, _) = fixture
        .send_with_token("POST", path, WORKER_SECRET, Some(missing))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (_, artifact) = fixture
        .send_with_token(
            "POST",
            "/api/v1/worker/claims/receipt/stage/1/outputs/rows?worker=one",
            WORKER_SECRET,
            Some(json!({"rows":[{"number":1}]})),
        )
        .await;
    let body =
        json!({"worker":"one", "outcome":"completed", "outputs":[artifact], "result":{"count":1}});
    let (status, original) = fixture
        .send_with_token("POST", path, WORKER_SECRET, Some(body.clone()))
        .await;
    assert_eq!(status, StatusCode::OK, "{original}");
    let store = fixture.state.executions.as_ref().expect("handler").store();
    let execution = aiwatcher_execution::ExecutionId::new("receipt");
    let before = store.load(&execution).await.expect("history");
    let (status, duplicate) = fixture
        .send_with_token("POST", path, WORKER_SECRET, Some(body.clone()))
        .await;
    assert_eq!(status, StatusCode::OK, "{duplicate}");
    assert_eq!(original, duplicate);
    assert_eq!(
        store.load(&execution).await.expect("history").version,
        before.version
    );
    let (status, _) = fixture
        .send_with_token("POST", path, OTHER_WORKER_SECRET, Some(body.clone()))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let mut conflict = body;
    conflict["result"] = json!({"count":2});
    let (status, error) = fixture
        .send_with_token("POST", path, WORKER_SECRET, Some(conflict))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{error}");
    assert_eq!(error["code"], "worker_report_conflict");
    let (status, _) = fixture
        .send_with_token(
            "POST",
            "/api/v1/worker/claims/receipt/stage/1/heartbeat",
            WORKER_SECRET,
            Some(json!({"worker":"one"})),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "receipt must not restore the retired lease"
    );
    assert_eq!(
        store.load(&execution).await.expect("history").version,
        before.version
    );
}

#[tokio::test]
async fn a_failed_worker_report_is_acknowledged_after_the_next_attempt_is_dispatched() {
    let fixture = Fixture::behind_a_proxy(false).await;
    fixture.seed_worker_run("failed-receipt").await;
    fixture
        .claim_as(WORKER_SECRET, json!({"worker":"one", "tasks":["stage@1"]}))
        .await;
    let path = "/api/v1/worker/claims/failed-receipt/stage/1/result";
    let body =
        json!({"worker":"one", "outcome":"failed", "class":"transient", "message":"network down"});
    let (status, first) = fixture
        .send_with_token("POST", path, WORKER_SECRET, Some(body.clone()))
        .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let (status, second) = fixture
        .send_with_token("POST", path, WORKER_SECRET, Some(body))
        .await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(first, second);
    assert_eq!(first["succeeded"], false);
}

#[tokio::test]
async fn concurrent_worker_report_deliveries_share_one_recorded_outcome() {
    let fixture = Fixture::behind_a_proxy(false).await;
    fixture.seed_worker_run("concurrent-receipt").await;
    fixture
        .claim_as(WORKER_SECRET, json!({"worker":"one", "tasks":["stage@1"]}))
        .await;
    let path = "/api/v1/worker/claims/concurrent-receipt/stage/1/result";
    let body =
        json!({"worker":"one", "outcome":"failed", "class":"user_code", "message":"broken input"});
    let (first, second) = tokio::join!(
        fixture.send_with_token("POST", path, WORKER_SECRET, Some(body.clone())),
        fixture.send_with_token("POST", path, WORKER_SECRET, Some(body)),
    );
    assert_eq!(first.0, StatusCode::OK, "{:?}", first.1);
    assert_eq!(second, first);
    let history = fixture
        .state
        .executions
        .as_ref()
        .expect("handler")
        .store()
        .load(&aiwatcher_execution::ExecutionId::new("concurrent-receipt"))
        .await
        .expect("history");
    let failures = history
        .messages
        .iter()
        .filter(|row| {
            row.direction == aiwatcher_execution::message::Direction::Output
                && matches!(
                    row.message.event(),
                    Some(aiwatcher_execution::WorkflowEvent::StepFailed { .. })
                )
        })
        .count();
    assert_eq!(failures, 1);
}

#[derive(Debug, Default)]
struct EvaluationSource(std::sync::atomic::AtomicBool);
#[async_trait::async_trait]
impl aiwatcher_evaluation::SourceAuthority for EvaluationSource {
    async fn resolve(
        &self,
        _: &aiwatcher_evaluation::EvaluationManifest,
        _: &str,
    ) -> aiwatcher_evaluation::Result<aiwatcher_evaluation::SourceEvidence> {
        if self.0.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(aiwatcher_evaluation::EvaluationError::Unavailable(
                aiwatcher_evaluation::EvidenceState::DeletedSource,
            ));
        }
        Ok(aiwatcher_evaluation::SourceEvidence {
            expected: ["capital-pl", "two-plus-two", "empty"]
                .into_iter()
                .map(|id| (id.into(), json!({"answer": ""})))
                .collect(),
            expires_at: None,
        })
    }
}
fn durable_request(id: &str) -> Value {
    let mut manifest: Value = serde_json::from_str(include_str!(
        "../../../contracts/fixtures/evaluation-v1/manifest.json"
    ))
    .unwrap();
    manifest["origin"]["evaluation_id"] = json!(id);
    json!({"manifest": manifest, "status": "succeeded", "cases": (["capital-pl", "two-plus-two", "empty"].into_iter().map(|id| json!({
        "case_id": id, "repetition_id": "measurement-1", "actual": {"answer": ""}, "metrics": {"accuracy": 1.0}
    })).collect::<Vec<_>>())})
}

#[derive(Debug, Default)]
struct InterruptedEvaluation(std::sync::atomic::AtomicU8);
#[async_trait::async_trait]
impl aiwatcher_evaluation::SourceAuthority for InterruptedEvaluation {
    async fn resolve(
        &self,
        manifest: &aiwatcher_evaluation::EvaluationManifest,
        subject: &str,
    ) -> aiwatcher_evaluation::Result<aiwatcher_evaluation::SourceEvidence> {
        if self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1 {
            return Err(aiwatcher_evaluation::EvaluationError::Unavailable(
                aiwatcher_evaluation::EvidenceState::Forbidden,
            ));
        }
        EvaluationSource::default().resolve(manifest, subject).await
    }
}

#[tokio::test]
async fn durable_abandoned_uploads_are_gone_and_cannot_reappear_through_legacy_routes() {
    let mut fixture = Fixture::new(false);
    fixture
        .seed_evaluation("abandoned", "suite", "data", json!({"accuracy": 0.9}))
        .await;
    fixture
        .seed_evaluation("legacy", "suite", "data", json!({"accuracy": 0.5}))
        .await;
    let registry = Arc::new(
        aiwatcher_evaluation::Registry::new(
            Arc::new(MemoryObjectStore::new()),
            Arc::new(InterruptedEvaluation::default()),
            Default::default(),
        )
        .unwrap(),
    );
    fixture.state.evaluations = Some(registry.clone());
    assert_eq!(
        fixture
            .post("/api/v1/evaluation-results", durable_request("abandoned"))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let later = time::OffsetDateTime::now_utc().unix_timestamp()
        + aiwatcher_evaluation::PUBLICATION_GRACE_SECONDS
        + 1;
    assert_eq!(registry.sweep("retention-worker", later).await.unwrap(), 0);
    for path in [
        "/api/v1/evaluation-results/abandoned",
        "/api/v1/evaluation-results/abandoned/cases?version=missing",
        "/api/v1/evaluations/abandoned",
        "/api/v1/evaluations/legacy?baseline_id=abandoned",
    ] {
        assert_eq!(fixture.get(path).await.0, StatusCode::GONE, "{path}");
    }
    assert_eq!(
        fixture
            .post("/api/v1/evaluation-results", durable_request("abandoned"))
            .await
            .0,
        StatusCode::GONE
    );
    let (_, legacy) = fixture.get("/api/v1/evaluations").await;
    assert_eq!(legacy["evaluations"].as_array().unwrap().len(), 1);
    let (_, detail) = fixture.get("/api/v1/evaluations/legacy").await;
    assert!(detail.get("comparison").is_none());
    let (_, suites) = fixture.get("/api/v1/evaluation-suites").await;
    assert_eq!(suites["suites"][0]["evaluations"], 1);
    let (_, durable) = fixture.get("/api/v1/evaluation-results").await;
    assert!(durable["evaluations"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn durable_reports_override_legacy_ids_and_erasure_never_falls_back() {
    let mut fixture = Fixture::new(false);
    fixture
        .seed_evaluation(
            "durable",
            "legacy-suite",
            "legacy-data",
            json!({"accuracy": 0.1}),
        )
        .await;
    fixture
        .seed_evaluation(
            "legacy",
            "legacy-suite",
            "legacy-data",
            json!({"accuracy": 0.2}),
        )
        .await;
    let source = Arc::new(EvaluationSource::default());
    fixture.state.evaluations = Some(Arc::new(
        aiwatcher_evaluation::Registry::new(
            Arc::new(MemoryObjectStore::new()),
            source.clone(),
            Default::default(),
        )
        .unwrap(),
    ));
    let (status, receipt) = fixture
        .post("/api/v1/evaluation-results", durable_request("durable"))
        .await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    let (status, detail) = fixture.get("/api/v1/evaluations/durable").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["summary"]["metrics"]["accuracy"], 1.0);
    let (_, retry) = fixture
        .post("/api/v1/evaluation-results", durable_request("durable"))
        .await;
    assert_eq!(receipt, retry);
    let mut changed = durable_request("durable");
    changed["cases"][0]["metrics"]["accuracy"] = json!(0.0);
    assert_eq!(
        fixture.post("/api/v1/evaluation-results", changed).await.0,
        StatusCode::CONFLICT
    );
    let (_, page) = fixture
        .get(&format!(
            "/api/v1/evaluation-results/durable/cases?version={}&limit=2",
            receipt["version"].as_str().unwrap()
        ))
        .await;
    assert_eq!(page["cases"].as_array().unwrap().len(), 2);
    assert!(page["next_cursor"].is_string());
    // Legacy discovery and automatic baselines cannot resurrect the shadowed ID.
    let (_, list) = fixture.get("/api/v1/evaluations").await;
    assert_eq!(list["evaluations"].as_array().unwrap().len(), 1);
    let (_, legacy) = fixture.get("/api/v1/evaluations/legacy").await;
    assert!(legacy.get("comparison").is_none());
    source.0.store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        fixture.get("/api/v1/evaluations/durable").await.0,
        StatusCode::GONE
    );
    let (_, tombstone) = fixture.get("/api/v1/evaluation-results/durable").await;
    assert_eq!(tombstone["state"], "deleted_source");
    assert!(tombstone["manifest"].is_null());
    let (_, suites) = fixture.get("/api/v1/evaluation-suites").await;
    assert_eq!(suites["suites"][0]["evaluations"], 1);
    // Losing the entire projection leaves durable discovery intact.
    fixture.state.read_model = Arc::new(ReadModel::new(Default::default()));
    let (_, durable) = fixture.get("/api/v1/evaluation-results").await;
    assert_eq!(durable["evaluations"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn durable_publication_requires_editor_and_a_source_authority() {
    let mut fixture = Fixture::behind_a_proxy(false).await;
    fixture.state.evaluations = Some(Arc::new(
        aiwatcher_evaluation::Registry::new(
            Arc::new(MemoryObjectStore::new()),
            Arc::new(EvaluationSource::default()),
            Default::default(),
        )
        .unwrap(),
    ));
    let (status, _) = fixture
        .post_as(
            "/api/v1/evaluation-results",
            "reader",
            "",
            durable_request("forbidden"),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(
        fixture
            .state
            .evaluations
            .as_ref()
            .unwrap()
            .known_ids()
            .await
            .unwrap()
            .is_empty()
    );
    let (status, _) = Fixture::without_registry()
        .post("/api/v1/evaluation-results", durable_request("disabled"))
        .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
}
