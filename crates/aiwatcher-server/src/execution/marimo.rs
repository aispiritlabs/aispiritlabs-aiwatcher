//! One marimo notebook, as a managed step.
//!
//! ```text
//!   execute   GET  {runtime}/ml-pipeline/notebooks/{name}/revisions/{sha256}
//!             POST {runtime}/ml-pipeline/run  {notebook, code_revision, rows, …}
//!             └─► rows ─► object store ─► ArtifactRef ─► receipt
//!   lookup    the receipt, and only the receipt
//! ```
//!
//! The plan pins a notebook and the `sha256` of its source. This names that
//! revision and gets those bytes however far the editable head has moved. The
//! digest is checked twice — before anything runs, and against what the
//! subprocess imported — because a run whose provenance says one thing and
//! whose rows came from another is what content addressing exists to prevent.
//!
//! The source never travels in a request. The notebook file is what marimo
//! serves and what a test reads; a copy would be a second source of truth.
//!
//! `lookup` asks the object store's receipt, which answers only whether a
//! previous attempt got all the way through. The notebook runtime remembers
//! nothing, so a step whose notebook is still running in a process this one
//! cannot see may run twice.
//!
//! The address is `AIWATCHER_ML_PIPELINE_URL`. A process without it registers
//! no notebook executor and claims no `marimo` attempt.
//!
//! ADR_0024.

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

        // Before anything runs. A revision the runtime no longer holds costs
        // one GET here; found after the rows are read it costs the read, and
        // never found at all it costs the provenance. The runtime's own 404
        // arrives as `UserCode` carrying its message, which names the notebook
        // and says that saving it again pins what is there now.
        let served: NotebookAnswer = self
            .get(
                &format!(
                    "/ml-pipeline/notebooks/{}/revisions/{}",
                    spec.notebook, spec.code_revision
                ),
                Duration::from_secs(30),
            )
            .await?
            .decode()
            .await?;
        // Not a tautology: the runtime recomputes the digest from the stored
        // bytes, so this is what catches a revision file that is no longer
        // what it is named after.
        drifted(
            spec,
            &served.revision,
            "is what the runtime holds under that name",
        )?;

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

        // Two sources, and only one of them can answer the question that
        // matters after a timeout. The runtime knows whether it is *still
        // executing* this key — nothing else can. The object store's receipt
        // knows what a finished attempt produced. Asked in that order, because
        // running a notebook again beside one that is still going is the
        // failure this route exists to prevent.
        if let Some(seen) = self.seen(&key).await {
            if seen.state == "running" {
                return Ok(PriorAttempt::Running);
            }
            if seen.state == "done"
                && let Some(revision) = seen.revision.as_deref()
                && let Some(receipt) = self.artifacts.receipt(&key).await?
                && receipt.runtime_digest != revision
            {
                // The runtime ran this key under a different notebook than the
                // receipt describes. The stored rows are another run's, so they
                // are not this attempt's answer.
                tracing::warn!(
                    key,
                    stored = %receipt.runtime_digest,
                    reported = revision,
                    "the notebook runtime describes a different revision of this key than the receipt does"
                );
                return Ok(PriorAttempt::Absent);
            }
        }

        // The durable half, and the one that survives a restart the memory does
        // not: a receipt is written after the bytes and keyed by this exact
        // attempt, so its existence is proof on its own.
        let Some(receipt) = self.artifacts.receipt(&key).await? else {
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

/// What the runtime remembers about one key.
#[derive(Debug, serde::Deserialize)]
struct PriorAnswer {
    state: String,
    #[serde(default)]
    revision: Option<String>,
}

impl MarimoExecutor {
    /// Ask the runtime about a key, or `None` when it cannot be asked.
    ///
    /// Best effort on purpose, and this is where it departs from the query
    /// service's lookup. A runtime that is unreachable, or an older build with
    /// no such route, must not stop the caller reading the *receipt* — that one
    /// is durable and this one is a fifteen-minute window, so failing to reach
    /// the volatile source would hide the answer that outlives it.
    async fn seen(&self, key: &str) -> Option<PriorAnswer> {
        let answered = self
            .get(&format!("/ml-pipeline/executions/{key}"), LOOKUP_TIMEOUT)
            .await
            .inspect_err(|error| {
                tracing::debug!(key, %error, "the notebook runtime could not be asked about this key");
            })
            .ok()?;
        answered.decode().await.ok()
    }

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
/// is `<execution>/<step>/<attempt>`, which is what the runtime keys on.
fn request_of(
    spec: &aiwatcher_execution::plan::MarimoStepSpec,
    context: &ActivityContext,
    rows: &Rows,
) -> Value {
    json!({
        "notebook": spec.notebook,
        // The pin, not the name alone. Absent is the editor's request and gets
        // the head; a managed step always sends one, and the runtime refuses a
        // revision it does not hold rather than falling back.
        "code_revision": spec.code_revision,
        "rows": rows,
        "params": spec.params,
        "context": context.context_id,
    })
}

/// The one comparison this file exists to make.
///
/// It no longer fires when somebody edits a notebook — the run resolves its
/// pin, so the head is free to move. What is left is the case where the bytes
/// stored under a digest are not what that digest names, which is corruption
/// rather than an edit, and the message says so rather than sending somebody
/// to re-save a pipeline that is perfectly correct.
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
         The two disagree, so the source this run is named after is not the source it \
         would have executed",
        spec.notebook, spec.code_revision
    )))
}

/// How much of what a notebook printed rides the completion event.
///
/// The same reasoning as the row preview beside it: this is a control value
/// somebody reads next to a block, not a log. The tail rather than the head,
/// because a traceback ends with the line that mattered.
const DIAGNOSTICS_BYTES: usize = 4 * 1024;

/// How long to wait for the runtime to say whether it is still running a key.
///
/// Short, because this is asked on the takeover path and the answer is a dict
/// lookup: a runtime that cannot answer it in ten seconds is one whose event
/// loop is blocked, which is the state this question was added to make
/// visible rather than to wait through.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);

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
    fn a_managed_run_names_the_revision_it_pinned_rather_than_the_notebook_alone() {
        // The difference between this and the editor's own request, and the
        // whole of work 5's first half: with the pin the runtime resolves the
        // source this run is named after, and without it the head somebody is
        // in the middle of editing.
        let rows: Rows = vec![BTreeMap::from([("text".to_owned(), json!("a"))])];

        let body = request_of(&spec(), &context(), &rows);

        assert_eq!(body["code_revision"], "abc123");
    }

    #[test]
    fn a_revision_whose_stored_bytes_are_not_what_it_is_named_after_is_refused() {
        // No longer the edit case: the head is free to move, because the run
        // resolves its pin. What is left is corruption, and the message says
        // that rather than sending somebody to re-save a correct pipeline.
        let refused = drifted(
            &spec(),
            "def456",
            "is what the runtime holds under that name",
        )
        .expect_err("a different revision is a refusal");

        assert_eq!(refused.class, FailureClass::UserCode);
        let message = refused.to_string();
        // Both revisions, because one of them alone sends somebody looking at
        // the wrong thing.
        assert!(message.contains("abc123"), "{message}");
        assert!(message.contains("def456"), "{message}");
        assert!(
            message.contains("not the source it would have executed"),
            "{message}"
        );

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
