//! One Flow PHP query, as a managed step.
//!
//! ```text
//!   execute   POST {flow}/flow/query  {pipeline, window_seconds?, execution_id}
//!             └─► rows ─► object store ─► ArtifactRef ─► receipt
//!   lookup    GET  {flow}/flow/executions/{execution_id}
//!             └─► running | done {digest, rows} | absent
//! ```
//!
//! The script is compiled in Rust
//! (`aiwatcher_execution::compile::flow_script`); the panel's direct calls to
//! the query service are the ad-hoc path and are labelled as such.
//!
//! The address is `AIWATCHER_FLOW_URL`. A `FlowStepSpec` carries a script and a
//! source, never a host — a plan that named its own endpoint would be a
//! request-forgery primitive posted by anything that can reach the API. A
//! process without the address registers no Flow executor and claims no
//! `flow_php` attempt.
//!
//! [`FlowExecutor::lookup`] asks **two** questions. The query service is the
//! only party that knows a query is *still executing*; the object store's
//! receipt says what a finished attempt produced, because the service keeps no
//! rows. `done` with no receipt is the honest gap: the query finished, the rows
//! never landed, and running it again is the only way to get them.

use std::sync::Arc;
use std::time::Duration;

use aiwatcher_execution::{
    ActivityCommand, ActivityContext, ActivityError, ActivityExecutor, ActivityResult,
    ExecutorRegistry, FailureClass, PriorAttempt, RuntimeBinding, RuntimeKind,
};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use super::artifacts::{Artifacts, Receipt, Rows, preview};
use crate::config::Config;

/// The Flow executor, if this process has an address for one.
///
/// An empty registry is a working state and says so at start-up: this process
/// then claims no `flow_php` attempt, and another one that does claims it
/// instead.
#[must_use]
pub fn executors(config: &Config, artifacts: Option<&Artifacts>) -> ExecutorRegistry {
    let registry = ExecutorRegistry::new();
    let (Some(endpoint), Some(artifacts)) = (config.flow_url.clone(), artifacts) else {
        return registry;
    };
    match FlowExecutor::new(endpoint.clone(), artifacts.clone()) {
        Ok(executor) => {
            tracing::info!(%endpoint, "the work role runs managed Flow steps");
            registry.with(Arc::new(executor))
        }
        Err(error) => {
            // Not fatal, and not silent. A client that will not build is a
            // misconfiguration, and the honest consequence is that this
            // process claims no Flow attempt rather than failing every one it
            // claims.
            tracing::error!(%endpoint, %error, "the Flow client could not be built; no Flow step will be claimed");
            registry
        }
    }
}

#[derive(Debug)]
pub struct FlowExecutor {
    endpoint: String,
    client: reqwest::Client,
    artifacts: Artifacts,
}

impl FlowExecutor {
    /// # Errors
    ///
    /// When the HTTP client cannot be built at all.
    pub fn new(endpoint: String, artifacts: Artifacts) -> Result<Self, reqwest::Error> {
        Ok(Self {
            endpoint: endpoint.trim_end_matches('/').to_owned(),
            // No client-wide timeout: the deadline is the *step's*, from the
            // plan, and it is applied per request. A number here would be a
            // second timeout nobody chose.
            client: reqwest::Client::builder().build()?,
            artifacts,
        })
    }

    /// The rows a finished attempt already stored, if it stored them.
    async fn stored(
        &self,
        key: &str,
        runtime_digest: &str,
    ) -> Result<Option<ActivityResult>, ActivityError> {
        let Some(receipt) = self.artifacts.receipt(key).await? else {
            return Ok(None);
        };
        if receipt.runtime_digest != runtime_digest {
            // The service ran this key and got something else. The stored
            // bytes are a different run's, so they are not this attempt's
            // answer — and the only way to get one is to run it.
            tracing::warn!(
                key,
                stored = %receipt.runtime_digest,
                reported = %runtime_digest,
                "the query service describes a different run of this key than the receipt does"
            );
            return Ok(None);
        }
        if !self.artifacts.holds(&receipt.artifact).await? {
            // A receipt pointing at nothing. Impossible in the ordering this
            // code keeps — data before receipt — and worth surviving anyway:
            // a bucket somebody pruned should cost a rerun, not a failure.
            return Ok(None);
        }
        Ok(Some(ActivityResult {
            outputs: vec![receipt.artifact],
            result: Some(json!({ "rows": receipt.rows, "reused": true })),
            diagnostics: None,
            awaiting: None,
            // A takeover's answer, not a fresh run: whether it may be cached
            // was decided when the attempt this receipt describes ran, and it
            // is not decided again here.
            cacheable: false,
        }))
    }
}

#[async_trait]
impl ActivityExecutor for FlowExecutor {
    fn runtime(&self) -> RuntimeKind {
        RuntimeKind::FlowPhp
    }

    async fn execute(
        &self,
        command: &ActivityCommand,
        context: &ActivityContext,
    ) -> Result<ActivityResult, ActivityError> {
        let RuntimeBinding::FlowPhp(spec) = &command.step.runtime else {
            // Unreachable through the registry, which routes by runtime. A
            // refusal rather than a guess: a Flow client handed a notebook step
            // would run the wrong thing successfully.
            return Err(ActivityError::user_code(
                "this step is not a Flow PHP query",
            ));
        };
        let key = command.idempotency_key();

        let answer: QueryAnswer = self
            .post("/flow/query", &request_of(spec, &key), context.timeout)
            .await?
            .decode()
            .await?;

        // What the plan asked for against what the service could do. A step
        // whose plan pinned a span and whose service narrowed it to a duration
        // read *correct rows for a different question*: the window drifted by
        // however long the dispatch took. The rows are still the step's answer;
        // what they may not become is the answer to the key, which claims the
        // span. Believed from the answer rather than assumed from the request,
        // because only the service knows what its sources accept — an older
        // build that never learnt `as_of` says so by omission.
        let honoured = worth_remembering(
            spec.source.window.is_some(),
            answer.window_applied.as_deref(),
            answer.deterministic,
        );
        if !answer.deterministic {
            tracing::debug!(
                key,
                "the query named a function whose value is the moment it ran; \
                 this result will not be cached"
            );
        }
        if !honoured {
            tracing::debug!(
                key,
                applied = answer.window_applied.as_deref().unwrap_or("duration"),
                "the query service narrowed a pinned span; this result will not be cached"
            );
        }

        let rows: Rows = answer.rows;
        if answer.truncated {
            // A cap the panel is entitled to hit and a managed run is not:
            // publishing the first thousand rows of something as a dataset
            // version is how somebody draws a conclusion from a slice they did
            // not know was a slice.
            return Err(ActivityError::user_code(format!(
                "the query service returned its {} row cap and stopped; \
                 narrow the query or raise the service's limit",
                rows.len()
            )));
        }

        let artifact = self.artifacts.put_rows("rows", &rows).await?;
        // Data first, then the pointer to it. `aiwatcher_jobs::ORDERING`, in
        // the fifth place it applies.
        self.artifacts
            .put_receipt(&Receipt {
                idempotency_key: key,
                artifact: artifact.clone(),
                rows: rows.len(),
                runtime_digest: answer.digest.clone().unwrap_or_default(),
                stored_at: time::OffsetDateTime::now_utc(),
            })
            .await?;

        Ok(ActivityResult {
            outputs: vec![artifact],
            result: Some(preview(&answer.columns, &rows, answer.took_ms)),
            diagnostics: None,
            awaiting: None,
            cacheable: honoured,
        })
    }

    async fn lookup(&self, command: &ActivityCommand) -> Result<PriorAttempt, ActivityError> {
        let key = command.idempotency_key();
        let response = self
            .get(&format!("/flow/executions/{key}"), Duration::from_secs(10))
            .await?;

        // A service that does not serve this route at all — an older build —
        // is a service that cannot be asked, which is what `Absent` means. The
        // cost is a retry that may duplicate work, which is exactly the state
        // section 15.4 exists to leave behind, and it is not a reason to fail
        // a step.
        if response.status == reqwest::StatusCode::NOT_FOUND {
            return Ok(PriorAttempt::Absent);
        }

        let seen: PriorAnswer = response.decode().await?;
        match seen.state.as_str() {
            "running" => Ok(PriorAttempt::Running),
            "done" => {
                let digest = seen.digest.unwrap_or_default();
                match self.stored(&key, &digest).await? {
                    Some(result) => Ok(PriorAttempt::Done(Box::new(result))),
                    // It ran and its rows never reached the store. Running it
                    // again is the only way to get them.
                    None => Ok(PriorAttempt::Absent),
                }
            }
            _ => Ok(PriorAttempt::Absent),
        }
    }
}

impl FlowExecutor {
    async fn post(
        &self,
        path: &str,
        body: &Value,
        timeout: Duration,
    ) -> Result<Answered, ActivityError> {
        self.send(
            self.client
                .post(format!("{}{path}", self.endpoint))
                .json(body)
                .timeout(timeout),
        )
        .await
    }

    async fn get(&self, path: &str, timeout: Duration) -> Result<Answered, ActivityError> {
        self.send(
            self.client
                .get(format!("{}{path}", self.endpoint))
                .timeout(timeout),
        )
        .await
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> Result<Answered, ActivityError> {
        let response = request.send().await.map_err(|error| {
            if error.is_timeout() {
                // Its own class, because it is the one failure that proves
                // nothing about the runtime — see the module docs.
                ActivityError::timed_out(format!("the query service did not answer: {error}"))
            } else {
                ActivityError::transient(format!("the query service could not be reached: {error}"))
            }
        })?;
        let status = response.status();
        let body = response.text().await.map_err(|error| {
            ActivityError::transient(format!("the query service's answer stopped short: {error}"))
        })?;

        if status.is_success() || status == reqwest::StatusCode::NOT_FOUND {
            return Ok(Answered { status, body });
        }
        // The service's own split, kept: a query it refused is a 422 carrying a
        // message and a column, and a 502 is aiwatcher being unreachable from
        // *there*. Only the first is the pipeline's fault.
        let class = if status.is_client_error() {
            FailureClass::UserCode
        } else {
            FailureClass::Transient
        };
        Err(ActivityError::new(class, refusal(status, &body)))
    }
}

/// The plan step, as the query service's request.
///
/// A named function rather than a literal inside `execute`, because this is the
/// seam: it is where a `FlowStepSpec` becomes somebody else's wire format, and
/// it is what a second engine over the same structured source would have a
/// sibling of. Pure, and tested, for the same reason the script generator is.
///
/// It sends the span the plan pinned **and** the duration that span is worth,
/// so a service that can honour bounds does, and one that cannot narrows it to
/// what it has always taken. Which of the two happened comes back on the answer
/// rather than being assumed here — see `window_applied`.
fn request_of(spec: &aiwatcher_execution::plan::FlowStepSpec, key: &str) -> Value {
    let mut body = json!({
        "pipeline": spec.script,
        "execution_id": key,
    });
    if let Some(window) = spec.source.window {
        body["window_from"] = json!(window.from);
        body["window_to"] = json!(window.to);
        body["window_seconds"] = json!(window.to.saturating_sub(window.from).max(0));
    }
    body
}

/// One answer, before it is decoded.
struct Answered {
    status: reqwest::StatusCode,
    body: String,
}

impl Answered {
    async fn decode<T: serde::de::DeserializeOwned>(self) -> Result<T, ActivityError> {
        serde_json::from_str(&self.body).map_err(|error| {
            // The shape being wrong is not transient: the same build of the
            // service will answer identically forever, and a retry would only
            // hide which of the two is out of date.
            ActivityError::user_code(format!(
                "the query service answered something this build cannot read: {error}"
            ))
        })
    }
}

/// What the service said went wrong, rather than its status alone.
fn refusal(status: reqwest::StatusCode, body: &str) -> String {
    #[derive(Deserialize)]
    struct Body {
        error: Detail,
    }
    #[derive(Deserialize)]
    struct Detail {
        message: String,
        #[serde(default)]
        column: usize,
    }
    match serde_json::from_str::<Body>(body) {
        Ok(refused) if refused.error.column > 0 => format!(
            "{} (at column {})",
            refused.error.message, refused.error.column
        ),
        Ok(refused) => refused.error.message,
        Err(_) => format!("the query service answered {status}"),
    }
}

/// `POST /flow/query`, as much of it as a managed step reads.
///
/// Deliberately not the panel's whole schema: the fields a *table on a screen*
/// needs — the grain, the dataset name, whether cells may be shortened — are
/// presentation, and a reactor that parsed them would have to keep up with
/// them.
#[derive(Debug, Deserialize)]
struct QueryAnswer {
    #[serde(default)]
    columns: Vec<String>,
    #[serde(default)]
    rows: Rows,
    #[serde(default)]
    truncated: bool,
    #[serde(default)]
    took_ms: Option<u64>,
    /// What the service says this result hashed to, in its own encoding.
    /// Absent from an older build, which costs the cross-check in `stored` and
    /// nothing else.
    #[serde(default)]
    digest: Option<String>,
    /// `span` when the service read the exact bounds the plan pinned,
    /// `duration` when it narrowed them to a width applied from its own now.
    ///
    /// Declared by the service rather than inferred here, because only it knows
    /// what its sources accept. Absent from an older build, which reads as
    /// `duration` — the behaviour that build has.
    #[serde(default)]
    window_applied: Option<String>,
    /// Whether running the query again would answer the same thing.
    ///
    /// False once it named `now()`, `uuid_v4()` or another function whose value
    /// is the moment it ran. Only the query service can know this — the script
    /// is text until it resolves it — which is the same reason
    /// `window_applied` is declared there rather than inferred here. Defaults
    /// to true so an older build, which never reported it, keeps the behaviour
    /// it had: those builds have no function that could make it false.
    #[serde(default = "yes")]
    deterministic: bool,
}

/// Serde's default for a field an older query service does not send.
fn yes() -> bool {
    true
}

/// Whether this result may be remembered under the step's cache key.
///
/// Both halves are the *service's* answer rather than the request's, for the
/// reason section 43.18 states: whether a key is well defined is `cache_key`'s
/// question, and whether the run that produced these rows happened under those
/// conditions is only knowable where the query resolved.
fn worth_remembering(pinned_a_span: bool, applied: Option<&str>, deterministic: bool) -> bool {
    deterministic && (!pinned_a_span || applied == Some("span"))
}

/// `GET /flow/executions/{execution_id}`.
#[derive(Debug, Deserialize)]
struct PriorAnswer {
    state: String,
    #[serde(default)]
    digest: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pinned_span_is_sent_as_bounds_and_as_the_width_it_is_worth() {
        // Both, so a service that can honour bounds does and one that cannot
        // reads the field it has always read. This is the seam: extending the
        // query service is a change there, not here.
        use aiwatcher_execution::plan::{FlowSourceRef, FlowStepSpec, ResolvedWindow};

        let spec = FlowStepSpec {
            script: "data_frame()->read(runs)".to_owned(),
            source: FlowSourceRef {
                dataset: "runs".to_owned(),
                window: Some(ResolvedWindow {
                    from: 1_700_000_000,
                    to: 1_700_003_600,
                }),
                ..FlowSourceRef::default()
            },
            blocks: Vec::new(),
        };
        let body = request_of(&spec, "exec-1/read/1");
        assert_eq!(body["window_from"], 1_700_000_000_i64);
        assert_eq!(body["window_to"], 1_700_003_600_i64);
        assert_eq!(body["window_seconds"], 3600);
        assert_eq!(body["execution_id"], "exec-1/read/1");

        // And a step with no window carries none of the three, rather than a
        // zero the service would read as "everything".
        let unwindowed = FlowStepSpec {
            source: FlowSourceRef::default(),
            ..spec
        };
        let body = request_of(&unwindowed, "exec-1/read/1");
        assert!(body.get("window_seconds").is_none());
        assert!(body.get("window_from").is_none());
    }

    #[test]
    fn a_result_is_remembered_only_when_the_service_says_both_halves_held() {
        // A pinned span the service honoured, over functions that answer the
        // same thing twice. Anything less is produced, reported and forgotten.
        assert!(worth_remembering(true, Some("span"), true));
        assert!(worth_remembering(false, None, true));

        // The service narrowed the span to a width applied from its own clock,
        // so a retry five minutes later reads different rows under one key.
        assert!(!worth_remembering(true, Some("duration"), true));

        // And the half the query language decides: `now()` is honest work and
        // a cache entry that would be wrong the second time it is read.
        assert!(!worth_remembering(true, Some("span"), false));
        assert!(!worth_remembering(false, None, false));
    }

    #[test]
    fn a_query_service_that_never_learnt_to_report_determinism_is_believed() {
        // Those builds have no function that could make it false, so the
        // absent field means "yes" rather than "unknown" — the same reasoning
        // that lets an older service omit `window_applied`.
        let answer: QueryAnswer = serde_json::from_str(r#"{"rows":[]}"#).expect("an empty answer");
        assert!(answer.deterministic);
    }

    #[test]
    fn a_refused_query_reports_what_the_service_said_and_where() {
        // The message is what a reader sees beside the step, and "the service
        // answered 422" sends them nowhere.
        let refused = refusal(
            reqwest::StatusCode::UNPROCESSABLE_ENTITY,
            r#"{"error":{"message":"Unknown function 'frobnicate'.","column":42}}"#,
        );
        assert_eq!(refused, "Unknown function 'frobnicate'. (at column 42)");

        // And a body that is not the service's shape still names the status
        // rather than saying nothing.
        assert!(
            refusal(reqwest::StatusCode::BAD_GATEWAY, "<html>").contains("502"),
            "a proxy's error page is still an answer worth reporting"
        );
    }
}
