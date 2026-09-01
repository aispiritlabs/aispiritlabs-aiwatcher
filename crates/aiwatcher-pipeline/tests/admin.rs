// See the note in aiwatcher-bus/tests: the clippy.toml allowances only reach
// `#[cfg(test)]` modules, and this is a separate crate.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! The adapter against a control plane that is really there.
//!
//! The unit tests in this crate check the pieces — the literal encoder, the
//! reference parser, the phase mapping. None of them proves that the pieces
//! fit, and everything that goes wrong in an adapter like this is a seam: a
//! version resolved and then not sent, a label built and never attached, an
//! interface read from one launch plan and bound against another.
//!
//! So this stands up a Flyte admin on a loopback socket — named entities,
//! launch plans with real `expected_inputs`, an execution endpoint that
//! records exactly what it was posted — and drives [`FlyteEngine`] through it
//! the way the API's routes do.

use std::collections::BTreeMap;
use std::sync::Arc;

use aiwatcher_core::engine::{
    CatalogQuery, EnginePhase, EngineRef, LaunchRequest, PipelineStage, WorkflowEngine,
};
use aiwatcher_core::ports::{RerunRequest, WorkflowRunner};
use aiwatcher_pipeline::{FlyteConfig, FlyteEngine, RUN_ID_LABEL};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

/// One request the admin was sent.
#[derive(Clone, Debug)]
struct Seen {
    method: String,
    path: String,
    query: String,
    body: Value,
    authorization: Option<String>,
}

#[derive(Default)]
struct State {
    seen: Vec<Seen>,
    /// Answer the next admin call with a 401, once. The expired-token case.
    reject_once: bool,
    tokens_minted: usize,
}

struct Admin {
    endpoint: String,
    state: Arc<Mutex<State>>,
}

impl Admin {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
        let port = listener.local_addr().expect("an address").port();
        let endpoint = format!("http://127.0.0.1:{port}");
        let state = Arc::new(Mutex::new(State::default()));

        let served = Arc::clone(&state);
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                serve(stream, Arc::clone(&served)).await;
            }
        });

        Self { endpoint, state }
    }

    fn engine(&self) -> FlyteEngine {
        FlyteEngine::new(FlyteConfig {
            endpoint: self.endpoint.clone(),
            project: "planner".to_owned(),
            domain: "production".to_owned(),
            console_url: Some("https://flyte.example".to_owned()),
            ..FlyteConfig::default()
        })
        .expect("builds")
    }

    fn authenticated_engine(&self) -> FlyteEngine {
        FlyteEngine::new(FlyteConfig {
            endpoint: self.endpoint.clone(),
            project: "planner".to_owned(),
            domain: "production".to_owned(),
            client_id: Some("aiwatcher".to_owned()),
            client_secret: Some("a-secret".to_owned()),
            token_url: Some(format!("{}/oauth2/token", self.endpoint)),
            ..FlyteConfig::default()
        })
        .expect("builds")
    }

    async fn seen(&self) -> Vec<Seen> {
        self.state.lock().await.seen.clone()
    }

    async fn posted(&self, path: &str) -> Vec<Seen> {
        self.seen()
            .await
            .into_iter()
            .filter(|request| request.method == "POST" && request.path == path)
            .collect()
    }
}

async fn serve(mut stream: tokio::net::TcpStream, state: Arc<Mutex<State>>) {
    let mut raw = Vec::new();
    let mut buffer = vec![0_u8; 8192];
    // Headers first, then whatever `content-length` promised.
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
    let (path, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
    let authorization = headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("authorization")
            .then(|| value.trim().to_owned())
    });

    let (status, response) = answer(
        &method,
        path,
        query,
        body,
        &authorization,
        Arc::clone(&state),
    )
    .await;

    state.lock().await.seen.push(Seen {
        method,
        path: path.to_owned(),
        query: query.to_owned(),
        body: serde_json::from_str(body).unwrap_or(Value::Null),
        authorization,
    });

    let body = response.to_string();
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.flush().await;
}

async fn answer(
    method: &str,
    path: &str,
    query: &str,
    body: &str,
    authorization: &Option<String>,
    state: Arc<Mutex<State>>,
) -> (&'static str, Value) {
    if path == "/oauth2/token" {
        state.lock().await.tokens_minted += 1;
        return (
            "200 OK",
            json!({ "access_token": "minted-token", "token_type": "Bearer", "expires_in": 3600 }),
        );
    }
    {
        let mut state = state.lock().await;
        if state.reject_once {
            state.reject_once = false;
            return ("401 Unauthorized", json!({ "message": "token expired" }));
        }
    }
    let _ = (body, authorization);

    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
    match (method, segments.as_slice()) {
        (
            "GET",
            [
                "api",
                "v1",
                "named_entities",
                "LAUNCH_PLAN",
                _project,
                _domain,
            ],
        ) => (
            "200 OK",
            json!({
                "entities": [
                    { "id": { "resourceType": "LAUNCH_PLAN", "project": "planner", "domain": "production", "name": "house_dataset_curation" },
                      "metadata": { "description": "Curate the house corpus into a versioned dataset", "state": "NAMED_ENTITY_ACTIVE" } },
                    { "id": { "resourceType": "LAUNCH_PLAN", "project": "planner", "domain": "production", "name": "llama_finetune" },
                      "metadata": { "description": "Fine-tune on a curated dataset", "state": "NAMED_ENTITY_ACTIVE" } },
                    { "id": { "resourceType": "LAUNCH_PLAN", "project": "planner", "domain": "production", "name": "retired_import" },
                      "metadata": { "description": "Superseded", "state": "NAMED_ENTITY_ARCHIVED" } }
                ],
                "token": "page-2"
            }),
        ),
        ("GET", ["api", "v1", "launch_plans", project, domain, name]) => {
            assert!(
                query.contains("limit=1"),
                "the newest version is one row: {query}"
            );
            assert!(query.contains("DESCENDING"), "newest first: {query}");
            (
                "200 OK",
                json!({ "launch_plans": [plan(project, domain, name, "v7")] }),
            )
        }
        ("GET", ["api", "v1", "launch_plans", project, domain, name, version]) => {
            if *version == "v0" {
                return ("404 Not Found", json!({ "message": "not found" }));
            }
            ("200 OK", plan(project, domain, name, version))
        }
        ("POST", ["api", "v1", "executions"]) => (
            "201 Created",
            json!({ "id": { "project": "planner", "domain": "production", "name": "a018f3a2b7c417b3e9d5" } }),
        ),
        ("GET", ["api", "v1", "executions", project, domain, name]) => (
            "200 OK",
            json!({
                "id": { "project": project, "domain": domain, "name": name },
                "spec": {
                    "launch_plan": { "project": project, "domain": domain, "name": "house_dataset_curation", "version": "v7" },
                    "labels": { "values": { RUN_ID_LABEL: "018f3a2b7c417b3e9d552f6a1c0b8e77" } }
                },
                "closure": { "phase": "RUNNING", "started_at": "2026-08-31T09:00:00Z" }
            }),
        ),
        _ => ("404 Not Found", json!({ "message": "no such route" })),
    }
}

/// A launch plan with the interface a curation workflow really has: what,
/// where, a filter and a range.
fn plan(project: &str, domain: &str, name: &str, version: &str) -> Value {
    json!({
        "id": { "resourceType": "LAUNCH_PLAN", "project": project, "domain": domain, "name": name, "version": version },
        "spec": { "entity_metadata": { "description": "Curate the house corpus into a versioned dataset" } },
        "closure": {
            "state": "ACTIVE",
            "createdAt": "2026-08-20T08:00:00Z",
            "updatedAt": "2026-08-30T10:00:00Z",
            "expectedInputs": { "parameters": {
                "dataset": { "var": { "type": { "simple": "STRING" }, "description": "Where the rows land" }, "required": true },
                "since": { "var": { "type": { "simple": "DATETIME" } }, "required": true },
                "until": { "var": { "type": { "simple": "DATETIME" } }, "required": true },
                "agents": { "var": { "type": { "collectionType": { "simple": "STRING" } } }, "required": false },
                "limit": { "var": { "type": { "simple": "INTEGER" } }, "default": { "scalar": { "primitive": { "integer": "1000" } } } },
                "aiwatcher_workflow_run_id": { "var": { "type": { "simple": "STRING" } }, "required": false }
            } }
        }
    })
}

fn curation_inputs() -> BTreeMap<String, Value> {
    BTreeMap::from([
        (
            "dataset".to_owned(),
            json!("evaluation/production-sessions"),
        ),
        ("since".to_owned(), json!("2026-08-30T00:00:00Z")),
        ("until".to_owned(), json!("2026-08-31T00:00:00Z")),
        ("agents".to_owned(), json!(["planner", "importer"])),
    ])
}

#[tokio::test]
async fn the_catalog_lists_one_row_per_launch_plan_with_the_inputs_a_form_needs() {
    let admin = Admin::start().await;
    let catalog = admin
        .engine()
        .catalog(&CatalogQuery {
            limit: 20,
            ..CatalogQuery::default()
        })
        .await
        .expect("a catalog");

    // Two, not three: an archived entity is one somebody deliberately took out
    // of circulation, and listing it would be offering to start it.
    assert_eq!(catalog.workflows.len(), 2);
    let curation = &catalog.workflows[0];
    assert_eq!(
        curation.id,
        "lp:planner:production:house_dataset_curation:v7"
    );
    assert_eq!(curation.stage_hint, Some(PipelineStage::Curation));
    assert_eq!(
        catalog.workflows[1].stage_hint,
        Some(PipelineStage::Training)
    );
    assert!(curation.active);
    assert_eq!(
        curation.url.as_deref(),
        Some(
            "https://flyte.example/console/projects/planner/domains/production/launch_plans/house_dataset_curation"
        )
    );

    // What, where, the filter and the range — every one of them typed, so the
    // panel renders controls rather than a JSON box.
    let named: BTreeMap<_, _> = curation
        .parameters
        .iter()
        .map(|parameter| (parameter.name.as_str(), parameter))
        .collect();
    assert!(named["dataset"].required);
    assert_eq!(
        named["since"].kind,
        aiwatcher_core::engine::ParameterKind::Datetime
    );
    assert_eq!(
        named["agents"].kind,
        aiwatcher_core::engine::ParameterKind::Collection
    );
    assert_eq!(named["limit"].default, Some(json!(1000)));
    assert!(
        !named["limit"].required,
        "a default makes an input optional"
    );

    // The engine's own paging token, passed back untouched.
    assert_eq!(catalog.next_token.as_deref(), Some("page-2"));
}

#[tokio::test]
async fn a_search_is_narrowed_by_the_engine_and_then_again_here() {
    let admin = Admin::start().await;
    let catalog = admin
        .engine()
        .catalog(&CatalogQuery {
            search: Some("finetune".to_owned()),
            limit: 20,
            ..CatalogQuery::default()
        })
        .await
        .expect("a catalog");

    let listing = admin
        .seen()
        .await
        .into_iter()
        .find(|request| request.path.contains("named_entities"))
        .expect("the catalog listing");
    assert!(
        listing.query.contains("contains%28name%2Cfinetune%29"),
        "the search should reach Flyte as a filter: {}",
        listing.query
    );
    // And applied again here, because Flyte's filter matches a name and the
    // description is worth searching too.
    assert_eq!(catalog.workflows.len(), 1);
    assert_eq!(catalog.workflows[0].name, "llama_finetune");
}

#[tokio::test]
async fn a_stage_filter_keeps_only_what_the_hint_matched() {
    let admin = Admin::start().await;
    let catalog = admin
        .engine()
        .catalog(&CatalogQuery {
            stage: Some(PipelineStage::Training),
            limit: 20,
            ..CatalogQuery::default()
        })
        .await
        .expect("a catalog");
    assert_eq!(catalog.workflows.len(), 1);
    assert_eq!(catalog.workflows[0].name, "llama_finetune");
}

#[tokio::test]
async fn a_launch_pins_a_version_binds_the_inputs_and_carries_the_correlation_id() {
    let admin = Admin::start().await;
    let accepted = admin
        .engine()
        .launch(LaunchRequest {
            // No version: the caller picked a name, and what ran still has to
            // be answerable afterwards.
            workflow: "lp:planner:production:house_dataset_curation".to_owned(),
            inputs: curation_inputs(),
            workflow_run_id: Some("018f3a2b7c417b3e9d552f6a1c0b8e77".to_owned()),
            requested_by: "alice@example.test".to_owned(),
        })
        .await
        .expect("a launch");

    assert_eq!(
        accepted.reference,
        "planner:production:a018f3a2b7c417b3e9d5"
    );
    assert_eq!(
        accepted.url.as_deref(),
        Some(
            "https://flyte.example/console/projects/planner/domains/production/executions/a018f3a2b7c417b3e9d5"
        )
    );

    let posted = admin.posted("/api/v1/executions").await;
    assert_eq!(posted.len(), 1);
    let body = &posted[0].body;
    assert_eq!(
        body["spec"]["launch_plan"]["version"], "v7",
        "a launch is always pinned"
    );
    assert_eq!(body["spec"]["launch_plan"]["resource_type"], "LAUNCH_PLAN");

    let literals = &body["inputs"]["literals"];
    assert_eq!(
        literals["dataset"]["scalar"]["primitive"]["string_value"],
        "evaluation/production-sessions"
    );
    assert_eq!(
        literals["since"]["scalar"]["primitive"]["datetime"],
        "2026-08-30T00:00:00Z"
    );
    assert_eq!(
        literals["agents"]["collection"]["literals"][0]["scalar"]["primitive"]["string_value"],
        "planner"
    );
    // Untouched: `limit` has a default, and sending nothing is what lets the
    // launch plan's own value survive.
    assert!(literals.get("limit").is_none());

    // The join. The label is how an execution started here is found again,
    // and the declared input is filled in because this entity asked for it.
    assert_eq!(
        body["spec"]["labels"]["values"][RUN_ID_LABEL],
        "018f3a2b7c417b3e9d552f6a1c0b8e77"
    );
    assert_eq!(
        literals["aiwatcher_workflow_run_id"]["scalar"]["primitive"]["string_value"],
        "018f3a2b7c417b3e9d552f6a1c0b8e77"
    );
    // And the execution's own name shares the correlation id's prefix, so the
    // join is one a human can check in Flyte's console.
    assert_eq!(body["name"], "a018f3a2b7c417b3e9d5");
}

#[tokio::test]
async fn a_launch_naming_an_input_the_entity_does_not_declare_never_reaches_flyte() {
    let admin = Admin::start().await;
    let mut inputs = curation_inputs();
    inputs.insert("agnets".to_owned(), json!(["planner"]));

    let error = admin
        .engine()
        .launch(LaunchRequest {
            workflow: "lp:planner:production:house_dataset_curation:v7".to_owned(),
            inputs,
            workflow_run_id: None,
            requested_by: "alice@example.test".to_owned(),
        })
        .await
        .expect_err("refused");

    assert!(error.to_string().contains("agnets"), "{error}");
    assert!(
        !error.is_retryable(),
        "a typo will still be a typo next time"
    );
    assert!(
        admin.posted("/api/v1/executions").await.is_empty(),
        "a typo in a filter must not become a run over everything"
    );
}

#[tokio::test]
async fn a_launch_missing_a_required_input_never_reaches_flyte() {
    let admin = Admin::start().await;
    let mut inputs = curation_inputs();
    inputs.remove("until");

    let error = admin
        .engine()
        .launch(LaunchRequest {
            workflow: "lp:planner:production:house_dataset_curation:v7".to_owned(),
            inputs,
            workflow_run_id: None,
            requested_by: String::new(),
        })
        .await
        .expect_err("refused");
    assert!(error.to_string().contains("until"), "{error}");
    assert!(admin.posted("/api/v1/executions").await.is_empty());
}

#[tokio::test]
async fn a_version_that_was_never_registered_is_absent_rather_than_broken() {
    let admin = Admin::start().await;
    let missing = admin
        .engine()
        .workflow(
            &"lp:planner:production:house_dataset_curation:v0"
                .parse::<EngineRef>()
                .unwrap(),
        )
        .await
        .expect("a well-formed question");
    assert!(
        missing.is_none(),
        "a 404 from Flyte is an answer, not a fault"
    );
}

#[tokio::test]
async fn an_execution_is_read_back_with_its_phase_and_its_correlation_id() {
    let admin = Admin::start().await;
    let execution = admin
        .engine()
        .execution("planner:production:a018f3a2b7c417b3e9d5")
        .await
        .expect("a lookup")
        .expect("an execution");

    assert_eq!(execution.phase, EnginePhase::Running);
    assert_eq!(
        execution.workflow.as_deref(),
        Some("lp:planner:production:house_dataset_curation:v7")
    );
    assert_eq!(
        execution.workflow_run_id.as_deref(),
        Some("018f3a2b7c417b3e9d552f6a1c0b8e77")
    );
    assert!(execution.started_at.is_some());
}

#[tokio::test]
async fn a_token_is_minted_once_and_presented_on_every_call() {
    let admin = Admin::start().await;
    let engine = admin.authenticated_engine();
    engine
        .catalog(&CatalogQuery {
            limit: 5,
            ..CatalogQuery::default()
        })
        .await
        .expect("a catalog");

    assert_eq!(
        admin.state.lock().await.tokens_minted,
        1,
        "one mint covers the whole page; a token per request is a token endpoint under load"
    );
    let admin_calls: Vec<_> = admin
        .seen()
        .await
        .into_iter()
        .filter(|request| request.path.starts_with("/api/v1"))
        .collect();
    assert!(!admin_calls.is_empty());
    assert!(
        admin_calls
            .iter()
            .all(|request| request.authorization.as_deref() == Some("Bearer minted-token")),
        "every call carries the bearer"
    );
}

#[tokio::test]
async fn a_token_that_expired_between_two_calls_is_replaced_and_the_call_retried() {
    let admin = Admin::start().await;
    let engine = admin.authenticated_engine();
    // Mint one, then make the next admin call answer 401 exactly as an
    // expired token does.
    engine
        .execution("planner:production:a018f3a2b7c417b3e9d5")
        .await
        .expect("a lookup");
    admin.state.lock().await.reject_once = true;

    let execution = engine
        .execution("planner:production:a018f3a2b7c417b3e9d5")
        .await
        .expect("the retry succeeds")
        .expect("an execution");
    assert_eq!(execution.phase, EnginePhase::Running);
    assert_eq!(
        admin.state.lock().await.tokens_minted,
        2,
        "the second mint is the point: a cached token that has expired is not a broken credential"
    );
}

#[tokio::test]
async fn a_rerun_of_an_observed_workflow_becomes_a_launch_of_the_launch_plan_with_that_name() {
    let admin = Admin::start().await;
    // What arrives from the log is a producer's own name for its graph, not an
    // engine reference. The configured project and domain complete it.
    let accepted = admin
        .engine()
        .rerun(RerunRequest {
            workflow_id: "house_dataset_curation".to_owned(),
            workflow_run_id: Some("018f3a2b7c417b3e9d552f6a1c0b8e77".to_owned()),
            from_node: Some("normalize".to_owned()),
            inputs: json!({
                "dataset": "evaluation/production-sessions",
                "since": "2026-08-30T00:00:00Z",
                "until": "2026-08-31T00:00:00Z"
            }),
        })
        .await
        .expect("a rerun");

    assert_eq!(
        accepted.reference.as_deref(),
        Some("planner:production:a018f3a2b7c417b3e9d5")
    );
    let posted = admin.posted("/api/v1/executions").await;
    assert_eq!(posted.len(), 1);
    assert_eq!(
        posted[0].body["spec"]["launch_plan"]["name"],
        "house_dataset_curation"
    );
}

#[tokio::test]
async fn a_rerun_whose_inputs_are_not_an_object_is_refused_rather_than_guessed_at() {
    let admin = Admin::start().await;
    let error = admin
        .engine()
        .rerun(RerunRequest {
            workflow_id: "house_dataset_curation".to_owned(),
            workflow_run_id: None,
            from_node: None,
            inputs: json!([1, 2, 3]),
        })
        .await
        .expect_err("refused");
    assert!(!error.is_retryable());
    assert!(admin.posted("/api/v1/executions").await.is_empty());
}
