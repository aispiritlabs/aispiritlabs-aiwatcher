//! One marimo notebook, as a managed step.
//!
//! Section 16, and the other half of ADR_0024's notebook block. The panel may
//! still run a notebook from the browser for an editor test — that is ad-hoc
//! mode and is labelled as such. This is the managed one: the plan pinned a
//! notebook and the `sha256` of the source it was saved against, the rows come
//! from an artifact its parent produced, and what comes back becomes an
//! artifact of its own.
//!
//! ```text
//!   execute   GET  {runtime}/ml-pipeline/notebooks/{name}   the code that is there
//!             POST {runtime}/ml-pipeline/run  {notebook, rows, params, context}
//!             └─► rows ─► object store ─► ArtifactRef ─► receipt
//!   lookup    the receipt, and only the receipt — see below
//! ```
//!
//! ## The pinned revision is checked twice, and both are the same rule
//!
//! A managed run pins the code it ran, which is why the compiler refuses a
//! notebook block with no revision. So this asks the runtime what source it
//! holds *before* anything executes, and compares what actually ran *after*.
//! The first is what makes drift a refusal that cost nothing; the second is
//! what makes it impossible for a notebook edited between those two moments to
//! be recorded as the pinned one. Neither is a warning: a run whose provenance
//! says one thing and whose rows came from another is the failure the whole
//! content-addressing exists to prevent.
//!
//! Note what this does **not** do: it never sends the source. The notebook file
//! is what marimo serves, what `ml_pipeline.step` imports and what a test
//! reads, and a copy travelling in a request would be a second source of truth
//! for a file that has to stay runnable on its own.
//!
//! ## What a timeout means here, and why `lookup` can say so little
//!
//! Less than it does for a Flow query, and the difference is the runtime's
//! rather than this file's. The query service remembers that it ran a key
//! (section 15.4); the notebook runtime remembers nothing — a run is a
//! subprocess it holds open, and there is no route to ask "did you run this".
//!
//! So `lookup` can only ask the object store's receipt, which answers one
//! question honestly: whether a *previous attempt got all the way through*,
//! rows stored and receipt written. That covers a redelivered dispatch and a
//! reactor that died after storing. It does not cover a notebook still running
//! in a process this one cannot see, and the consequence is that such a step
//! may run twice. Recorded rather than hidden: giving the notebook runtime the
//! query service's execution memory is the fix, and it is a change to that
//! service.
//!
//! ## The address is configuration
//!
//! `AIWATCHER_ML_PIPELINE_URL`. A `MarimoStepSpec` names a notebook and a
//! revision, never a host — the rule every executor here keeps. A process with
//! no address registers no notebook executor and therefore claims no `marimo`
//! attempt.

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

/// The notebook executor, if this process has an address for one.
#[must_use]
pub fn executors(config: &Config, artifacts: Option<&Artifacts>) -> ExecutorRegistry {
    let registry = ExecutorRegistry::new();
    let (Some(endpoint), Some(artifacts)) = (config.ml_pipeline_url.clone(), artifacts) else {
        return registry;
    };
    match MarimoExecutor::new(endpoint.clone(), artifacts.clone()) {
        Ok(executor) => {
            tracing::info!(%endpoint, "the work role runs managed notebook steps");
            registry.with(Arc::new(executor))
        }
        Err(error) => {
            tracing::error!(
                %endpoint, %error,
                "the notebook client could not be built; no notebook step will be claimed"
            );
            registry
        }
    }
}

#[derive(Debug)]
pub struct MarimoExecutor {
    endpoint: String,
    client: reqwest::Client,
    artifacts: Artifacts,
}

impl MarimoExecutor {
    /// # Errors
    ///
    /// When the HTTP client cannot be built at all.
    pub fn new(endpoint: String, artifacts: Artifacts) -> Result<Self, reqwest::Error> {
        Ok(Self {
            endpoint: endpoint.trim_end_matches('/').to_owned(),
            // The deadline is the step's, from the plan, applied per request —
            // 900 seconds for a notebook, because it is a subprocess somebody
            // is halfway through writing.
            client: reqwest::Client::builder().build()?,
            artifacts,
        })
    }
}

#[async_trait]
impl ActivityExecutor for MarimoExecutor {
    fn runtime(&self) -> RuntimeKind {
        RuntimeKind::Marimo
    }

    async fn execute(
        &self,
        command: &ActivityCommand,
        context: &ActivityContext,
    ) -> Result<ActivityResult, ActivityError> {
        let RuntimeBinding::Marimo(spec) = &command.step.runtime else {
            return Err(ActivityError::user_code("this step is not a notebook"));
        };
        let key = command.idempotency_key();

        // Before anything runs. A refusal here costs one GET; a refusal after
        // the run costs the run, and a *missing* refusal costs the provenance.
        let served: NotebookAnswer = self
            .get(
                &format!("/ml-pipeline/notebooks/{}", spec.notebook),
                Duration::from_secs(30),
            )
            .await?
            .decode()
            .await?;
        drifted(spec, &served.revision, "is what the runtime holds")?;

        // Rows come from the artifact a parent produced, and the digest is
        // verified on the way in. A notebook with no upstream is a chain the
        // compiler would not have produced — `previous` is always non-empty
        // for a notebook block — so an absent input is a refusal rather than
        // an empty table somebody could mistake for a result.
        let Some(input) = command.inputs.first() else {
            return Err(ActivityError::user_code(
                "this notebook step has no rows to read; a notebook block follows a source",
            ));
        };
        let rows = self.artifacts.read_rows(input).await?;

        let answer: RunAnswer = self
            .post(
                "/ml-pipeline/run",
                &request_of(spec, context, &rows),
                context.timeout,
            )
            .await?
            .decode()
            .await?;

        // And after. Between the check above and this line a person may have
        // saved the notebook; `revision` is the digest of what the subprocess
        // actually imported.
        drifted(spec, &answer.revision, "is what ran")?;

        if answer.truncated {
            // The same refusal a Flow step makes, for the same reason: a
            // managed run publishing the first N rows of something is how
            // somebody draws a conclusion from a slice they did not know was
            // a slice.
            return Err(ActivityError::user_code(format!(
                "the notebook runtime returned its {} row cap and stopped; \
                 narrow the chain or raise the service's limit",
                answer.rows.len()
            )));
        }

        let artifact = self.artifacts.put_rows("rows", &answer.rows).await?;
        // Data first, then the pointer to it. `aiwatcher_jobs::ORDERING`.
        self.artifacts
            .put_receipt(&Receipt {
                idempotency_key: key,
                artifact: artifact.clone(),
                rows: answer.rows.len(),
                // The notebook runtime reports no digest of its own, so the
                // receipt is named by the code that produced the rows. Never
                // compared against the artifact's digest — those are two
                // encodings by two languages (the guardrail), and this one
                // answers "which run of this key is stored".
                runtime_digest: answer.revision.clone(),
                stored_at: time::OffsetDateTime::now_utc(),
            })
            .await?;

        let mut result = preview(&answer.columns, &answer.rows, answer.took_ms);
        result["app_url"] = json!(answer.app_url);

        Ok(ActivityResult {
            outputs: vec![artifact],
            result: Some(result),
            // What a notebook does is arbitrary Python, so nothing outside it
            // can tell a pure transform from one that reads the clock, draws a
            // random sample or asks a model. The notebook declares it —
            // `deterministic = False` beside its `output` — and this reports
            // what it said, exactly as the query service reports what its
            // query resolved to. Absent means true: a curation block normally
            // is one, and a default that turned caching off would make every
            // chain pay for the exceptions.
            cacheable: answer.deterministic,
            // What the notebook printed. Bounded here rather than by the
            // runtime, because a `print` in a loop over a corpus is a normal
            // thing to leave in and an unbounded field on a workflow message
            // is not.
            diagnostics: bounded(&answer.stdout),
            awaiting: None,
        })
    }

    async fn lookup(&self, command: &ActivityCommand) -> Result<PriorAttempt, ActivityError> {
        let key = command.idempotency_key();
        let Some(receipt) = self.artifacts.receipt(&key).await? else {
            // Not "it did not run" — "nothing here can say". See the module
            // docs: the notebook runtime keeps no memory of keys.
            return Ok(PriorAttempt::Absent);
        };
        if !self.artifacts.holds(&receipt.artifact).await? {
            return Ok(PriorAttempt::Absent);
        }
        Ok(PriorAttempt::Done(Box::new(ActivityResult {
            outputs: vec![receipt.artifact],
            result: Some(json!({ "rows": receipt.rows, "reused": true })),
            diagnostics: None,
            awaiting: None,
            // A takeover's answer rather than a fresh run.
            cacheable: false,
        })))
    }
}

impl MarimoExecutor {
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
                ActivityError::timed_out(format!("the notebook runtime did not answer: {error}"))
            } else {
                ActivityError::transient(format!(
                    "the notebook runtime could not be reached: {error}"
                ))
            }
        })?;
        let status = response.status();
        let body = response.text().await.map_err(|error| {
            ActivityError::transient(format!(
                "the notebook runtime's answer stopped short: {error}"
            ))
        })?;

        if status.is_success() {
            return Ok(Answered { body });
        }
        // The service's own split, kept. A notebook it refused or that raised
        // is a 422 carrying the message; a 5xx is the runtime itself.
        let class = if status.is_client_error() {
            FailureClass::UserCode
        } else {
            FailureClass::Transient
        };
        Err(ActivityError::new(class, refusal(status, &body)))
    }
}

/// The step, as the notebook runtime's request.
///
/// The named seam, as `request_of` is for a Flow query. What it carries beyond
/// the obvious is `context`: the notebook runtime stages the rows it was given
/// so the *live app* opens on them, and staging keyed by the notebook's name
/// means two pipelines using one notebook overwrite each other. The context id
/// is `<execution>/<step>/<attempt>`, which is what section 16.2 asks for.
fn request_of(
    spec: &aiwatcher_execution::plan::MarimoStepSpec,
    context: &ActivityContext,
    rows: &Rows,
) -> Value {
    json!({
        "notebook": spec.notebook,
        "rows": rows,
        "params": spec.params,
        "context": context.context_id,
    })
}

/// The one comparison this file exists to make.
///
/// A message rather than a boolean, because "the notebook changed" sends
/// somebody looking at the wrong thing: what they need is which revision was
/// pinned, which one is there, and that saving the pipeline again is the fix.
fn drifted(
    spec: &aiwatcher_execution::plan::MarimoStepSpec,
    found: &str,
    when: &str,
) -> Result<(), ActivityError> {
    if found == spec.code_revision {
        return Ok(());
    }
    Err(ActivityError::user_code(format!(
        "this run pinned '{}' at {}, and {found} {when}. \
         The notebook was edited after the pipeline was saved; save it again to pin the new code",
        spec.notebook, spec.code_revision
    )))
}

/// How much of what a notebook printed rides the completion event.
///
/// The same reasoning as the row preview beside it: this is a control value
/// somebody reads next to a block, not a log. The tail rather than the head,
/// because a traceback ends with the line that mattered.
const DIAGNOSTICS_BYTES: usize = 4 * 1024;

/// What a notebook printed, at a size a message can carry.
fn bounded(stdout: &str) -> Option<String> {
    let trimmed = stdout.trim_end();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.len() <= DIAGNOSTICS_BYTES {
        return Some(trimmed.to_owned());
    }
    // On a character boundary, or this panics on the first notebook that
    // prints anything outside ASCII.
    let mut cut = trimmed.len() - DIAGNOSTICS_BYTES;
    while cut < trimmed.len() && !trimmed.is_char_boundary(cut) {
        cut += 1;
    }
    Some(format!("…{}", &trimmed[cut..]))
}

/// One answer, before it is decoded.
struct Answered {
    body: String,
}

impl Answered {
    async fn decode<T: serde::de::DeserializeOwned>(self) -> Result<T, ActivityError> {
        serde_json::from_str(&self.body).map_err(|error| {
            ActivityError::user_code(format!(
                "the notebook runtime answered something this build cannot read: {error}"
            ))
        })
    }
}

/// What the runtime said went wrong, rather than its status alone.
fn refusal(status: reqwest::StatusCode, body: &str) -> String {
    #[derive(Deserialize)]
    struct Body {
        #[serde(default)]
        error: String,
        #[serde(default)]
        message: String,
    }
    match serde_json::from_str::<Body>(body) {
        Ok(refused) if !refused.error.is_empty() => refused.error,
        Ok(refused) if !refused.message.is_empty() => refused.message,
        _ => format!("the notebook runtime answered {status}"),
    }
}

/// `GET /ml-pipeline/notebooks/{name}`, as much of it as this reads.
#[derive(Debug, Deserialize)]
struct NotebookAnswer {
    #[serde(default)]
    revision: String,
}

/// `POST /ml-pipeline/run`, likewise. The source is deliberately not read:
/// this never holds a copy of the notebook.
#[derive(Debug, Deserialize)]
struct RunAnswer {
    #[serde(default)]
    revision: String,
    #[serde(default)]
    columns: Vec<String>,
    #[serde(default)]
    rows: Rows,
    #[serde(default)]
    truncated: bool,
    #[serde(default)]
    stdout: String,
    #[serde(default)]
    took_ms: Option<u64>,
    #[serde(default)]
    app_url: Option<String>,
    /// Whether running this notebook again would answer the same thing.
    ///
    /// Defaults to true so a runtime that never learnt to report it keeps the
    /// behaviour it had — those builds run notebooks that never said
    /// otherwise, because there was no way to.
    #[serde(default = "yes")]
    deterministic: bool,
}

/// Serde's default for a field an older notebook runtime does not send.
fn yes() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use aiwatcher_execution::plan::MarimoStepSpec;
    use std::collections::BTreeMap;

    use super::*;

    fn spec() -> MarimoStepSpec {
        MarimoStepSpec {
            notebook: "pii_detection".to_owned(),
            code_revision: "abc123".to_owned(),
            params: BTreeMap::from([("threshold".to_owned(), json!(0.8))]),
            block: Some("detect".to_owned()),
        }
    }

    fn context() -> ActivityContext {
        ActivityContext {
            owner: "host/1/work".to_owned(),
            timeout: Duration::from_secs(900),
            context_id: "exec-1/detect/2".to_owned(),
            plan: Arc::new(aiwatcher_execution::ExecutionPlan::seal(
                aiwatcher_execution::plan::DefinitionKind::CurationPipeline,
                "pii".to_owned(),
                aiwatcher_execution::plan::DefinitionRevision("ab".repeat(32)),
                Vec::new(),
                Vec::new(),
            )),
        }
    }

    #[test]
    fn a_request_carries_the_context_so_two_pipelines_stop_sharing_one_notebook() {
        // The notebook runtime stages the rows it is given so the *live app*
        // opens on them. Staged by the notebook's name, two pipelines using
        // one notebook overwrite each other; the context is what separates
        // them, and it is the attempt's own key.
        let rows: Rows = vec![BTreeMap::from([("text".to_owned(), json!("a"))])];
        let body = request_of(&spec(), &context(), &rows);

        assert_eq!(body["context"], "exec-1/detect/2");
        assert_eq!(body["notebook"], "pii_detection");
        assert_eq!(body["params"]["threshold"], 0.8);
        assert_eq!(body["rows"].as_array().expect("the rows").len(), 1);
        // And never the source. That file is what marimo serves and what a
        // test imports; a copy on the wire would be a second source of truth.
        assert!(body.get("source").is_none());
    }

    #[test]
    fn a_notebook_edited_after_the_pipeline_was_saved_is_refused_and_says_how_to_fix_it() {
        let refused = drifted(&spec(), "def456", "is what the runtime holds")
            .expect_err("a different revision is a refusal");

        assert_eq!(refused.class, FailureClass::UserCode);
        let message = refused.to_string();
        // Both revisions, because "the notebook changed" sends somebody
        // looking at the wrong thing.
        assert!(message.contains("abc123"), "{message}");
        assert!(message.contains("def456"), "{message}");
        assert!(message.contains("save it again"), "{message}");

        drifted(&spec(), "abc123", "is what ran").expect("the pinned revision is what runs");
    }

    #[test]
    fn what_a_notebook_printed_rides_the_event_at_a_size_a_message_can_carry() {
        assert_eq!(bounded("   \n"), None, "nothing worth keeping is None");
        assert_eq!(
            bounded("scanned 40 rows\n").as_deref(),
            Some("scanned 40 rows")
        );

        // The tail, because a traceback ends with the line that mattered.
        let long = format!("{}\nthe line that mattered", "noise\n".repeat(4000));
        let kept = bounded(&long).expect("something is kept");
        assert!(kept.ends_with("the line that mattered"), "{kept}");
        assert!(kept.len() <= DIAGNOSTICS_BYTES + 4);
    }

    #[test]
    fn a_multibyte_line_is_cut_on_a_character_and_not_in_the_middle_of_one() {
        // Cutting by byte offset alone panics on the first notebook that
        // prints anything outside ASCII, which is most of them.
        let long = "ł".repeat(DIAGNOSTICS_BYTES);
        assert!(bounded(&long).is_some());
    }

    #[test]
    fn a_notebook_that_raised_is_reported_by_what_it_said() {
        assert_eq!(
            refusal(
                reqwest::StatusCode::UNPROCESSABLE_ENTITY,
                r#"{"error":"demo ran but handed nothing on"}"#
            ),
            "demo ran but handed nothing on"
        );
        // And a body that is not the service's shape still names the status
        // rather than saying nothing.
        assert!(refusal(reqwest::StatusCode::BAD_GATEWAY, "<html>").contains("502"));
    }
}
