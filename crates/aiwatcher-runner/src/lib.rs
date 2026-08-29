//! Asking somebody else to run a workflow again.
//!
//! This is the one crate here that makes something happen rather than
//! recording that it did, and the whole design is about keeping that
//! difference visible.
//!
//! aiwatcher does not orchestrate. It knows the shape of a graph because a
//! producer declared it on the log — not because it can execute one. So a
//! rerun is a **dispatch**: one HTTP POST to one endpoint the operator
//! configured, carrying names the producer already chose. What comes back is
//! an acknowledgement, never a result; the evidence that the rerun happened is
//! the events it publishes, on the same log as everything else.
//!
//! ## The endpoint comes from configuration
//!
//! Never from an event, and this is the security boundary rather than a
//! stylistic preference. A `workflow.declared` naming its own callback URL
//! would be a request-forgery primitive posted by anything that can reach
//! ingest: aiwatcher runs inside the cluster, so "POST this url" is a request
//! to reach the cluster's internal network on the caller's behalf. The
//! declaration names a *workflow*; the operator names a *runner*.
//!
//! ## Why this is a crate of its own
//!
//! `reqwest` is deliberately absent from `core`, `projector`, `api` and
//! `server` — only `trace` and `prompts` carry it, each for one adapter. This
//! is the third, and putting it in `server` would make the wiring crate an
//! HTTP client.

use std::time::Duration;

use async_trait::async_trait;
use serde::Serialize;

use aiwatcher_core::ports::{PortError, PortResult, RerunAccepted, RerunRequest, WorkflowRunner};

const TARGET: &str = "workflow-runner";

/// How to reach the orchestrator.
#[derive(Clone)]
pub struct HttpRunnerConfig {
    /// The single endpoint every rerun is posted to.
    pub endpoint: String,
    /// Sent as `Authorization: Bearer …` when present.
    pub token: Option<String>,
    pub timeout: Duration,
}

impl Default for HttpRunnerConfig {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            token: None,
            // The same ten seconds the OTLP exporter and the object store use.
            // A rerun is queued, not awaited: an orchestrator that has not
            // acknowledged in ten seconds is one worth reporting as
            // unavailable rather than one worth waiting on.
            timeout: Duration::from_secs(10),
        }
    }
}

impl std::fmt::Debug for HttpRunnerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The token must not reach a log through a `#[derive(Debug)]` on some
        // struct three layers up that happens to hold this.
        f.debug_struct("HttpRunnerConfig")
            .field("endpoint", &self.endpoint)
            .field("token", &self.token.is_some())
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// What goes on the wire.
///
/// Flat and `snake_case`, like the event envelope, so a producer already
/// parsing aiwatcher events needs no second vocabulary to receive one of these.
#[derive(Debug, Serialize)]
struct RerunBody<'a> {
    workflow_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    workflow_run_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    from_node: Option<&'a str>,
    #[serde(skip_serializing_if = "serde_json::Value::is_null")]
    inputs: &'a serde_json::Value,
    /// Who asked. An orchestrator that logs its callers should be able to tell
    /// a rerun from a scheduled run without guessing.
    requested_by: &'static str,
}

/// Posts a rerun to one configured endpoint.
pub struct HttpRunner {
    http: reqwest::Client,
    config: HttpRunnerConfig,
}

impl std::fmt::Debug for HttpRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpRunner")
            .field("endpoint", &self.config.endpoint)
            .finish_non_exhaustive()
    }
}

impl HttpRunner {
    /// Fails only if the HTTP client cannot be built — a TLS backend problem,
    /// not a reachability one. The endpoint is not contacted here: a runner
    /// that refused to start because the orchestrator was down would take
    /// aiwatcher down with it, and observability that stops when the thing it
    /// observes stops is the wrong way round.
    pub fn new(config: HttpRunnerConfig) -> PortResult<Self> {
        let http = reqwest::Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(|error| PortError::Other {
                target: TARGET,
                source: Box::new(error),
            })?;
        Ok(Self { http, config })
    }
}

#[async_trait]
impl WorkflowRunner for HttpRunner {
    async fn rerun(&self, request: RerunRequest) -> PortResult<RerunAccepted> {
        let body = RerunBody {
            workflow_id: &request.workflow_id,
            workflow_run_id: request.workflow_run_id.as_deref(),
            from_node: request.from_node.as_deref(),
            inputs: &request.inputs,
            requested_by: "aiwatcher",
        };

        let mut post = self.http.post(&self.config.endpoint).json(&body);
        if let Some(token) = &self.config.token {
            post = post.bearer_auth(token);
        }

        let response = post.send().await.map_err(|source| PortError::Unavailable {
            target: TARGET,
            message: source.to_string(),
        })?;

        let status = response.status();
        if !status.is_success() {
            let detail = response.text().await.unwrap_or_default();
            return Err(classify(status, truncated(&detail, 500)));
        }

        // An orchestrator is free to acknowledge with an empty body. Reading a
        // reference out of one that does say something is a convenience, not a
        // contract, so a body that will not parse is not a failed rerun — the
        // work was accepted either way.
        let acknowledgement = response.text().await.unwrap_or_default();
        Ok(read_acknowledgement(&acknowledgement))
    }
}

/// 4xx means the request is wrong and will still be wrong next time; 5xx,
/// 429 and a transport failure are worth a retry. Callers read
/// `PortError::is_retryable` to decide.
fn classify(status: reqwest::StatusCode, message: String) -> PortError {
    let message = format!("{status}: {message}");
    if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        PortError::Unavailable {
            target: TARGET,
            message,
        }
    } else {
        PortError::Rejected {
            target: TARGET,
            message,
        }
    }
}

fn read_acknowledgement(body: &str) -> RerunAccepted {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return RerunAccepted {
            reference: None,
            url: None,
        };
    };
    let text = |keys: &[&str]| -> Option<String> {
        keys.iter()
            .find_map(|key| value.get(*key))
            .and_then(serde_json::Value::as_str)
            .filter(|found| !found.is_empty())
            .map(ToOwned::to_owned)
    };
    RerunAccepted {
        reference: text(&["reference", "run_id", "execution_id", "id", "name"]),
        url: text(&["url", "link", "console_url"]),
    }
}

/// An orchestrator's error page is not bounded by anything aiwatcher controls.
fn truncated(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    #[test]
    fn a_5xx_is_retryable_and_a_4xx_is_not() {
        // The pipeline and the API both branch on this: retryable becomes a
        // 503 the caller may repeat, rejected becomes a 502 they must fix.
        assert!(
            classify(reqwest::StatusCode::BAD_GATEWAY, "down".to_owned()).is_retryable(),
            "an orchestrator that is down is worth asking again"
        );
        assert!(
            classify(reqwest::StatusCode::TOO_MANY_REQUESTS, String::new()).is_retryable(),
            "and one that is busy is asking to be asked again"
        );
        assert!(
            !classify(
                reqwest::StatusCode::BAD_REQUEST,
                "no such workflow".to_owned()
            )
            .is_retryable(),
            "a workflow that does not exist will not exist next time either"
        );
    }

    #[test]
    fn an_empty_acknowledgement_is_still_an_acceptance() {
        // The work was queued. Reading a reference out of the body is a
        // convenience; refusing the rerun because the body was blank would
        // report a failure that did not happen.
        let accepted = read_acknowledgement("");
        assert!(accepted.reference.is_none());
        assert!(accepted.url.is_none());

        let malformed = read_acknowledgement("<html>202 Accepted</html>");
        assert!(malformed.reference.is_none());
    }

    #[test]
    fn a_reference_is_read_from_whatever_the_orchestrator_calls_it() {
        let flyte_ish = read_acknowledgement(r#"{"name":"a7f3","url":"https://flyte/x"}"#);
        assert_eq!(flyte_ish.reference.as_deref(), Some("a7f3"));
        assert_eq!(flyte_ish.url.as_deref(), Some("https://flyte/x"));

        let plain = read_acknowledgement(r#"{"run_id":"import-42"}"#);
        assert_eq!(plain.reference.as_deref(), Some("import-42"));
    }

    #[test]
    fn the_body_carries_only_names_the_producer_already_chose() {
        // Not a description of how to run anything. aiwatcher cannot execute a
        // graph and the wire format should not suggest otherwise.
        let request = RerunRequest {
            workflow_id: "house-import".to_owned(),
            workflow_run_id: Some("exec-7".to_owned()),
            from_node: Some("analyze".to_owned()),
            inputs: serde_json::json!({ "source_url": "https://example.test/plan.pdf" }),
        };
        let body = RerunBody {
            workflow_id: &request.workflow_id,
            workflow_run_id: request.workflow_run_id.as_deref(),
            from_node: request.from_node.as_deref(),
            inputs: &request.inputs,
            requested_by: "aiwatcher",
        };

        let json = serde_json::to_value(&body).expect("serializes");
        assert_eq!(json["workflow_id"], "house-import");
        assert_eq!(json["workflow_run_id"], "exec-7");
        assert_eq!(json["from_node"], "analyze");
        assert_eq!(json["requested_by"], "aiwatcher");
        assert_eq!(json.as_object().expect("object").len(), 5);
    }

    #[test]
    fn an_absent_execution_and_empty_inputs_stay_off_the_wire() {
        let inputs = serde_json::Value::Null;
        let body = RerunBody {
            workflow_id: "house-import",
            workflow_run_id: None,
            from_node: None,
            inputs: &inputs,
            requested_by: "aiwatcher",
        };
        let json = serde_json::to_value(&body).expect("serializes");
        assert!(json.get("workflow_run_id").is_none());
        assert!(json.get("from_node").is_none());
        assert!(json.get("inputs").is_none());
    }

    #[test]
    fn the_token_does_not_appear_in_a_debug_rendering() {
        // A secret reaches a log through a derived `Debug` three layers up, not
        // through somebody printing it on purpose.
        let config = HttpRunnerConfig {
            endpoint: "http://planner-import-api/reruns".to_owned(),
            token: Some("s3cr3t-value".to_owned()),
            ..HttpRunnerConfig::default()
        };
        let rendered = format!("{config:?}");
        assert!(!rendered.contains("s3cr3t"), "{rendered}");
        assert!(rendered.contains("token: true"), "{rendered}");

        let runner = HttpRunner::new(config).expect("builds");
        assert!(!format!("{runner:?}").contains("s3cr3t"));
    }

    #[test]
    fn an_orchestrators_error_page_is_bounded() {
        let long = "x".repeat(5_000);
        assert!(truncated(&long, 500).chars().count() <= 501);
        assert_eq!(truncated("short", 500), "short");
    }
}
