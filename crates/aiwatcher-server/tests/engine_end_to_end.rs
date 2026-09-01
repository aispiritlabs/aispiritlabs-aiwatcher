// See the note in aiwatcher-bus/tests: the clippy.toml allowances only reach
// `#[cfg(test)]` modules, and this is a separate crate.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! A launch, all the way through, against a control plane that is really there.
//!
//! The other two suites cover the halves. `aiwatcher-pipeline`'s tests drive
//! the adapter against a stand-in flyteadmin with no aiwatcher around it, and
//! `aiwatcher-api`'s drive the routes against a stub engine with no HTTP to
//! Flyte. Both pass while the thing they are halves of is broken, because
//! everything that goes wrong here lives between them: a config variable read
//! into a field nothing wires, a runner that reaches a 501 the engine would
//! have served, a correlation id minted by the API and dropped by the adapter.
//!
//! So this builds an instance the way [`aiwatcher_server::build`] builds the
//! binary — the same config struct, the same projector, the same router —
//! serves it on a loopback socket, and points it at a stand-in control plane on
//! another one. Every assertion is either a response aiwatcher gave over HTTP
//! or a request the control plane received over HTTP.
//!
//! The test worth reading first is
//! [`the_events_that_execution_publishes_land_on_the_launch_that_started_it`]:
//! it is ADR_0016's central claim — start work from the panel, and the
//! telemetry that work publishes arrives back under the id the panel is
//! already watching — and nothing smaller than this suite can check it.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use aiwatcher_server::config::{
    BackendKind, Config, EngineKind, PromptStoreKind, WorkflowRunnerKind,
};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

// ── The stand-in control plane ───────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Seen {
    method: String,
    path: String,
    body: Value,
}

#[derive(Default)]
struct AdminState {
    seen: Vec<Seen>,
}

struct Admin {
    endpoint: String,
    state: Arc<Mutex<AdminState>>,
    /// Distinct per test, and reused as the data directory's discriminator so
    /// two tests running in parallel cannot share a dead-letter queue.
    port: u16,
}

impl Admin {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
        let port = listener.local_addr().expect("an address").port();
        let state = Arc::new(Mutex::new(AdminState::default()));

        let served = Arc::clone(&state);
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let state = Arc::clone(&served);
                tokio::spawn(async move { serve_admin(stream, state).await });
            }
        });

        Self {
            endpoint: format!("http://127.0.0.1:{port}"),
            state,
            port,
        }
    }

    async fn posted(&self, path: &str) -> Vec<Seen> {
        self.state
            .lock()
            .await
            .seen
            .iter()
            .filter(|request| request.method == "POST" && request.path == path)
            .cloned()
            .collect()
    }

    async fn executions(&self) -> Vec<Value> {
        self.posted("/api/v1/executions")
            .await
            .into_iter()
            .map(|request| request.body)
            .collect()
    }
}

async fn serve_admin(mut stream: tokio::net::TcpStream, state: Arc<Mutex<AdminState>>) {
    let mut raw = Vec::new();
    let mut buffer = vec![0_u8; 8192];
    loop {
        let read = stream.read(&mut buffer).await.unwrap_or(0);
        if read == 0 {
            break;
        }
        raw.extend_from_slice(&buffer[..read]);
        let text = String::from_utf8_lossy(&raw);
        let Some((headers, body)) = text.split_once("\r\n\r\n") else {
            continue;
        };
        let expected: usize = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.trim()
                    .eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse().ok())?
            })
            .unwrap_or(0);
        if body.len() >= expected {
            break;
        }
    }

    let text = String::from_utf8_lossy(&raw).into_owned();
    let (headers, body) = text.split_once("\r\n\r\n").unwrap_or((text.as_str(), ""));
    let mut request_line = headers.split_whitespace();
    let method = request_line.next().unwrap_or_default().to_owned();
    let target = request_line.next().unwrap_or("/").to_owned();
    let path = target.split('?').next().unwrap_or("/").to_owned();

    let (status, response) = answer(&method, &path);
    state.lock().await.seen.push(Seen {
        method,
        path,
        body: serde_json::from_str(body).unwrap_or(Value::Null),
    });

    let body = response.to_string();
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.flush().await;
}

fn answer(method: &str, path: &str) -> (&'static str, Value) {
    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
    match (method, segments.as_slice()) {
        (
            "GET",
            [
                "api",
                "v1",
                "named_entities",
                "LAUNCH_PLAN",
                project,
                domain,
            ],
        ) => (
            "200 OK",
            json!({
                "entities": [{
                    "id": { "project": project, "domain": domain, "name": "house_dataset_curation" },
                    "metadata": {
                        "description": "Curate the house corpus into a versioned dataset",
                        "state": "NAMED_ENTITY_ACTIVE",
                    },
                }],
                "token": "",
            }),
        ),
        ("GET", ["api", "v1", "launch_plans", project, domain, name]) => (
            "200 OK",
            json!({ "launch_plans": [plan(project, domain, name, "v7")] }),
        ),
        ("GET", ["api", "v1", "launch_plans", project, domain, name, version]) => {
            ("200 OK", plan(project, domain, name, version))
        }
        ("POST", ["api", "v1", "executions"]) => (
            "201 Created",
            json!({
                "id": {
                    "project": "planner",
                    "domain": "production",
                    "name": "a018f3a2b7c417b3e9d5",
                },
            }),
        ),
        ("GET", ["api", "v1", "executions", project, domain, name]) => (
            "200 OK",
            json!({
                "id": { "project": project, "domain": domain, "name": name },
                "spec": {
                    "launch_plan": {
                        "project": project,
                        "domain": domain,
                        "name": "house_dataset_curation",
                        "version": "v7",
                    },
                },
                "closure": { "phase": "RUNNING", "started_at": "2026-08-31T09:00:00Z" },
            }),
        ),
        _ => ("404 Not Found", json!({ "message": "no such route" })),
    }
}

/// The interface a curation workflow really has: what, where, which rows, over
/// what period — plus the input that lets aiwatcher hand it a correlation id.
fn plan(project: &str, domain: &str, name: &str, version: &str) -> Value {
    json!({
        "id": {
            "resource_type": "LAUNCH_PLAN",
            "project": project,
            "domain": domain,
            "name": name,
            "version": version,
        },
        "spec": { "entity_metadata": { "description": "Curate the house corpus" } },
        "closure": {
            "state": "ACTIVE",
            "updatedAt": "2026-08-30T10:00:00Z",
            "expectedInputs": { "parameters": {
                "dataset": { "var": { "type": { "simple": "STRING" } }, "required": true },
                "since": { "var": { "type": { "simple": "DATETIME" } }, "required": true },
                "until": { "var": { "type": { "simple": "DATETIME" } }, "required": true },
                "agents": { "var": { "type": { "collectionType": { "simple": "STRING" } } }, "required": false },
                "limit": {
                    "var": { "type": { "simple": "INTEGER" } },
                    "default": { "scalar": { "primitive": { "integer": "1000" } } },
                },
                "aiwatcher_workflow_run_id": {
                    "var": { "type": { "simple": "STRING" } },
                    "required": false,
                },
            } },
        },
    })
}

// ── One aiwatcher, wired the way the binary wires it ─────────────────────────

struct Instance {
    base: String,
    http: reqwest::Client,
    shutdown: CancellationToken,
    data_dir: PathBuf,
}

impl Instance {
    /// The same `build`, the same projector task, the same router — only the
    /// listener is the test's, because the port has to be discovered.
    async fn start(config: Config) -> Self {
        let data_dir = PathBuf::from(config.data_dir.clone());
        let runtime = aiwatcher_server::build(config)
            .await
            .expect("the instance builds");
        let (state, _config, projector) = runtime.split();

        let shutdown = CancellationToken::new();
        let projector_shutdown = shutdown.clone();
        tokio::spawn(async move { projector.run(projector_shutdown).await });
        state.health.mark_ready();

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
        let address = listener.local_addr().expect("an address");
        let app = aiwatcher_api::router(state);
        let serving_shutdown = shutdown.clone();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async move { serving_shutdown.cancelled().await })
                .await;
        });

        Self {
            base: format!("http://{address}"),
            http: reqwest::Client::new(),
            shutdown,
            data_dir,
        }
    }

    async fn get(&self, path: &str) -> (reqwest::StatusCode, Value) {
        let response = self
            .http
            .get(format!("{}{path}", self.base))
            .send()
            .await
            .expect("aiwatcher answers");
        let status = response.status();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    async fn post(&self, path: &str, body: Value) -> (reqwest::StatusCode, Value) {
        let response = self
            .http
            .post(format!("{}{path}", self.base))
            .json(&body)
            .send()
            .await
            .expect("aiwatcher answers");
        let status = response.status();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    /// Publish the way an SDK behind a firewall does — over the ingest route,
    /// not by reaching into the read model. What is under test includes the
    /// envelope surviving that trip.
    async fn publish(&self, events: Vec<Value>) {
        let (status, body) = self
            .post("/api/v1/events", json!({ "events": events }))
            .await;
        assert_eq!(status, reqwest::StatusCode::ACCEPTED, "{body}");
    }

    /// Poll a route until it says what the test is waiting for.
    ///
    /// The projector is a separate task consuming a log, so "the events have
    /// been folded" is not something a `POST /events` response can promise.
    async fn eventually(&self, path: &str, done: impl Fn(&Value) -> bool) -> Value {
        for _ in 0..200 {
            let (status, body) = self.get(path).await;
            if status.is_success() && done(&body) {
                return body;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        panic!("{path} never reached the expected state within five seconds");
    }

    async fn stop(self) {
        self.shutdown.cancel();
        // Best effort: a leftover dead-letter file is not worth failing a test
        // that otherwise passed.
        let _ = std::fs::remove_dir_all(&self.data_dir);
    }
}

/// The configuration a deployment with an orchestrator has, minus the parts
/// that need something running: no OTLP, no object store, an in-memory log.
fn config(admin: &Admin) -> Config {
    Config {
        bus: BackendKind::Memory,
        data_dir: std::env::temp_dir()
            .join(format!("aiwatcher-e2e-{}", admin.port))
            .to_string_lossy()
            .into_owned(),
        ingest_enabled: true,
        prompt_store: PromptStoreKind::None,
        engine: EngineKind::Flyte,
        flyte_endpoint: Some(admin.endpoint.clone()),
        flyte_project: "planner".to_owned(),
        flyte_domain: "production".to_owned(),
        flyte_console_url: Some("https://flyte.example".to_owned()),
        ..Config::default()
    }
}

fn curation_inputs() -> Value {
    json!({
        "dataset": "evaluation/production-sessions",
        "since": "2026-08-30T00:00:00Z",
        "until": "2026-08-31T00:00:00Z",
        "agents": ["planner", "importer"],
    })
}

/// One event as an SDK would put it on the wire.
fn event(
    event_type: &str,
    run_id: &str,
    workflow_id: &str,
    workflow_run_id: &str,
    data: Value,
) -> Value {
    json!({
        "event_type": event_type,
        "occurred_at": "2026-08-31T09:00:05Z",
        "run_id": run_id,
        "workflow_id": workflow_id,
        "workflow_run_id": workflow_run_id,
        "source": { "service": "planner-import", "sdk": "python" },
        "data": data,
    })
}

// ── The tests ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_launch_over_http_reaches_the_orchestrator_with_a_pinned_version() {
    let admin = Admin::start().await;
    let instance = Instance::start(config(&admin)).await;

    // The catalog first, because that is how a caller gets an id at all: a
    // reference typed by hand is not what the panel sends.
    let (status, catalog) = instance
        .get("/api/v1/engine/workflows?stage=curation")
        .await;
    assert_eq!(status, reqwest::StatusCode::OK, "{catalog}");
    let workflow = &catalog["workflows"][0];
    assert_eq!(
        workflow["id"], "lp:planner:production:house_dataset_curation:v7",
        "{catalog}"
    );
    let declared: Vec<&str> = workflow["parameters"]
        .as_array()
        .expect("parameters")
        .iter()
        .map(|parameter| parameter["name"].as_str().expect("a name"))
        .collect();
    assert!(declared.contains(&"since"), "{declared:?}");

    // Launch the name rather than the version, which is what a picker sends
    // when somebody has not thought about versions at all.
    let (status, accepted) = instance
        .post(
            "/api/v1/engine/launches",
            json!({
                "workflow": "lp:planner:production:house_dataset_curation",
                "inputs": curation_inputs(),
            }),
        )
        .await;
    assert_eq!(
        status,
        reqwest::StatusCode::ACCEPTED,
        "202: nothing has run yet — {accepted}"
    );
    assert_eq!(
        accepted["reference"],
        "planner:production:a018f3a2b7c417b3e9d5"
    );
    assert_eq!(
        accepted["url"],
        "https://flyte.example/console/projects/planner/domains/production/executions/a018f3a2b7c417b3e9d5"
    );

    let posted = admin.executions().await;
    assert_eq!(posted.len(), 1, "one launch, one execution");
    let body = &posted[0];

    // Pinned, though nobody asked for a version. An execution recorded against
    // "whatever was current" is not something anybody can repeat.
    assert_eq!(body["spec"]["launch_plan"]["version"], "v7");
    assert_eq!(
        body["spec"]["launch_plan"]["name"],
        "house_dataset_curation"
    );

    // Bound to the types the launch plan declares, not to what the caller
    // happened to send.
    let literals = &body["inputs"]["literals"];
    assert_eq!(
        literals["since"]["scalar"]["primitive"]["datetime"],
        "2026-08-30T00:00:00Z"
    );
    assert_eq!(
        literals["agents"]["collection"]["literals"][1]["scalar"]["primitive"]["string_value"],
        "importer"
    );
    // `limit` has a default. Sending nothing is what lets that default survive.
    assert!(literals.get("limit").is_none(), "{literals}");

    // The correlation id: minted by the API, on the execution's labels, in the
    // input the entity declared, and back in the response for the panel.
    let minted = accepted["workflow_run_id"].as_str().expect("an id");
    assert_eq!(
        body["spec"]["labels"]["values"]["aiwatcher-workflow-run-id"],
        minted
    );
    assert_eq!(
        literals["aiwatcher_workflow_run_id"]["scalar"]["primitive"]["string_value"],
        minted
    );

    // And the engine's own view of it, read back through aiwatcher.
    let (status, execution) = instance
        .get("/api/v1/engine/launches/planner:production:a018f3a2b7c417b3e9d5")
        .await;
    assert_eq!(status, reqwest::StatusCode::OK, "{execution}");
    assert_eq!(execution["phase"], "running");
    assert_eq!(
        execution["workflow"],
        "lp:planner:production:house_dataset_curation:v7"
    );

    instance.stop().await;
}

#[tokio::test]
async fn the_events_that_execution_publishes_land_on_the_launch_that_started_it() {
    // ADR_0016's central claim, and the only test that can check it: start the
    // work from aiwatcher, and the telemetry that work publishes arrives back
    // under the id aiwatcher is already watching. Every step here crosses a
    // process boundary the halves of this suite stub out.
    let admin = Admin::start().await;
    let instance = Instance::start(config(&admin)).await;

    let (_, accepted) = instance
        .post(
            "/api/v1/engine/launches",
            json!({
                "workflow": "lp:planner:production:house_dataset_curation:v7",
                "inputs": curation_inputs(),
            }),
        )
        .await;
    let execution_id = accepted["workflow_run_id"]
        .as_str()
        .expect("an id comes back")
        .to_owned();

    // The panel subscribes to this before anything has published, so it has to
    // be a 404 rather than an error — nothing has happened yet, which is not
    // the same as something being wrong.
    let (status, _) = instance
        .get(&format!("/api/v1/workflow-executions/{execution_id}"))
        .await;
    assert_eq!(
        status,
        reqwest::StatusCode::NOT_FOUND,
        "an execution nothing has published about is absent, not broken"
    );

    // Now the producer runs. Two stage pods, each its own run, both publishing
    // under the id that came back from the launch — which is what the SDK's
    // Flyte helper does with the declared input.
    let workflow = "house-import";
    let mut events = vec![event(
        "workflow.declared",
        "driver-run",
        workflow,
        &execution_id,
        json!({
            "name": "House import",
            "version": "sha256:f00d",
            "nodes": [{ "id": "acquire" }, { "id": "normalize" }, { "id": "persist" }],
            "edges": [
                { "from": "acquire", "to": "normalize" },
                { "from": "normalize", "to": "persist" },
            ],
        }),
    )];
    for stage in ["acquire", "normalize"] {
        let run_id = format!("{stage}-pod");
        events.push(event(
            "run.started",
            &run_id,
            workflow,
            &execution_id,
            json!({}),
        ));
        events.push(event(
            "step.started",
            &run_id,
            workflow,
            &execution_id,
            json!({ "node": stage }),
        ));
        events.push(event(
            "step.completed",
            &run_id,
            workflow,
            &execution_id,
            json!({ "node": stage }),
        ));
    }
    instance.publish(events).await;

    let detail = instance
        .eventually(
            &format!("/api/v1/workflow-executions/{execution_id}"),
            |body| {
                body["nodes"]
                    .as_array()
                    .is_some_and(|nodes| nodes.len() == 3)
            },
        )
        .await;

    // The three stages of the declared graph, two of them run by two different
    // pods, and the third one nobody has reached — which is the whole reason
    // the topology rides the log rather than being inferred.
    let nodes: BTreeMap<&str, &str> = detail["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .map(|node| {
            (
                node["node_id"].as_str().expect("an id"),
                node["status"].as_str().expect("a status"),
            )
        })
        .collect();
    assert_eq!(nodes["acquire"], "succeeded");
    assert_eq!(nodes["normalize"], "succeeded");
    assert_eq!(
        nodes["persist"], "pending",
        "a stage nothing has started is drawn, not omitted"
    );
    assert_eq!(
        detail["summary"]["runs"].as_array().expect("runs").len(),
        3,
        "three runs, one execution: the join a runs list cannot express"
    );

    // And the same id reaches the executions list, which is what the panel's
    // Workflows tab lands on from the launch acknowledgement's link.
    let listed = instance
        .eventually("/api/v1/workflow-executions", |body| {
            body["executions"]
                .as_array()
                .is_some_and(|rows| !rows.is_empty())
        })
        .await;
    assert_eq!(listed["executions"][0]["workflow_run_id"], execution_id);

    instance.stop().await;
}

#[tokio::test]
async fn an_input_the_entity_does_not_declare_never_reaches_the_orchestrator() {
    // The typo case, end to end: the adapter reads the interface from the
    // control plane, refuses the launch, and the caller gets a 400 whose
    // message belongs beside a form field — not a 502 about a gateway.
    let admin = Admin::start().await;
    let instance = Instance::start(config(&admin)).await;

    let mut inputs = curation_inputs();
    inputs["agnets"] = json!(["planner"]);
    let (status, body) = instance
        .post(
            "/api/v1/engine/launches",
            json!({
                "workflow": "lp:planner:production:house_dataset_curation:v7",
                "inputs": inputs,
            }),
        )
        .await;

    assert_eq!(status, reqwest::StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "launch_refused");
    assert!(
        body["message"]
            .as_str()
            .expect("a message")
            .contains("agnets"),
        "{body}"
    );
    assert!(
        admin.executions().await.is_empty(),
        "a typo in a filter must not become a run over everything"
    );

    // A timestamp that will not parse fails the same way, and the message says
    // which field and what it wanted.
    let mut wrong = curation_inputs();
    wrong["since"] = json!("yesterday");
    let (status, body) = instance
        .post(
            "/api/v1/engine/launches",
            json!({
                "workflow": "lp:planner:production:house_dataset_curation:v7",
                "inputs": wrong,
            }),
        )
        .await;
    assert_eq!(status, reqwest::StatusCode::BAD_REQUEST, "{body}");
    let message = body["message"].as_str().expect("a message");
    assert!(
        message.contains("since") && message.contains("RFC 3339"),
        "{message}"
    );
    assert!(admin.executions().await.is_empty());

    instance.stop().await;
}

#[tokio::test]
async fn a_body_naming_its_own_endpoint_is_refused_by_the_running_server() {
    // `deny_unknown_fields` through a real HTTP request rather than through a
    // handler call: aiwatcher runs inside the cluster, so "POST this url" is a
    // request to reach that cluster's network on the caller's behalf.
    let admin = Admin::start().await;
    let instance = Instance::start(config(&admin)).await;

    let (status, _) = instance
        .post(
            "/api/v1/engine/launches",
            json!({
                "workflow": "lp:planner:production:house_dataset_curation:v7",
                "endpoint": "http://169.254.169.254/latest/meta-data/",
            }),
        )
        .await;
    assert_eq!(status, reqwest::StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        admin.executions().await.is_empty(),
        "nothing left the process"
    );

    instance.stop().await;
}

#[tokio::test]
async fn a_rerun_of_an_observed_workflow_is_dispatched_as_a_launch() {
    // `AIWATCHER_WORKFLOW_RUNNER=engine`: one adapter, both outbound ports. The
    // seam under test is the wiring, which hands the same instance to two
    // fields of `AppState` — and which no test inside either crate can reach.
    let admin = Admin::start().await;
    let instance = Instance::start(Config {
        workflow_runner: WorkflowRunnerKind::Engine,
        ..config(&admin)
    })
    .await;

    // A rerun is refused for a workflow nothing has ever heard of, so the
    // observed one has to exist first — which it does by being published.
    instance
        .publish(vec![event(
            "workflow.declared",
            "driver-run",
            "house_dataset_curation",
            "exec-1",
            json!({
                "name": "House import",
                "version": "sha256:f00d",
                "nodes": [{ "id": "acquire" }],
                "edges": [],
            }),
        )])
        .await;
    instance
        .eventually("/api/v1/workflows", |body| {
            body["workflows"]
                .as_array()
                .is_some_and(|rows| !rows.is_empty())
        })
        .await;

    let (status, accepted) = instance
        .post(
            "/api/v1/workflows/house_dataset_curation/rerun",
            json!({ "inputs": curation_inputs() }),
        )
        .await;
    assert_eq!(status, reqwest::StatusCode::ACCEPTED, "{accepted}");
    assert_eq!(
        accepted["reference"], "planner:production:a018f3a2b7c417b3e9d5",
        "the rerun came back with the engine's own execution reference"
    );

    let posted = admin.executions().await;
    assert_eq!(posted.len(), 1);
    // The workflow id on the log is a producer's name for its own graph; the
    // configured project and domain complete it into something Flyte holds.
    assert_eq!(
        posted[0]["spec"]["launch_plan"]["name"],
        "house_dataset_curation"
    );
    assert_eq!(posted[0]["spec"]["launch_plan"]["version"], "v7");

    instance.stop().await;
}

#[tokio::test]
async fn an_instance_with_no_orchestrator_says_which_variable_is_unset() {
    // Through the real wiring, not through a handler with a `None` field: the
    // failure this catches is `build` wiring an engine that configuration said
    // not to, which is exactly the mistake a stub cannot make.
    let admin = Admin::start().await;
    let instance = Instance::start(Config {
        engine: EngineKind::None,
        flyte_endpoint: None,
        ..config(&admin)
    })
    .await;

    for path in ["/api/v1/engine", "/api/v1/engine/workflows"] {
        let (status, body) = instance.get(path).await;
        assert_eq!(
            status,
            reqwest::StatusCode::NOT_IMPLEMENTED,
            "{path}: {body}"
        );
        assert_eq!(body["code"], "engine_disabled");
        assert!(
            body["message"]
                .as_str()
                .expect("a message")
                .contains("AIWATCHER_ENGINE"),
            "the message has to name the variable: {body}"
        );
    }

    let (status, _) = instance
        .post(
            "/api/v1/engine/launches",
            json!({ "workflow": "lp:p:d:n:v" }),
        )
        .await;
    assert_eq!(status, reqwest::StatusCode::NOT_IMPLEMENTED);
    assert!(
        admin.executions().await.is_empty(),
        "an unconfigured instance reaches nothing, even one that is listening"
    );

    instance.stop().await;
}
