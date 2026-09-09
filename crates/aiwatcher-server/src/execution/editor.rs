//! Opening a block's editor on what one step actually read.
//! [`core::ports::EditorHost`], implemented.
//!
//! ```text
//!   object store ──► the rows that step read
//!                    └─► POST {runtime}/ml-pipeline/staging  {notebook, rows, params, context}
//!                        └─► app_url, for an iframe
//! ```
//!
//! **No token.** The notebook runtime has no authentication at all — it binds
//! to localhost and is a development surface — so a token it cannot validate
//! would be ceremony rather than a boundary. The gate is the route that calls
//! this: aiwatcher decides who may open which run's rows and reads them itself.
//!
//! **It stages and stops.** Running the notebook to fill its editor would
//! execute somebody's code because they clicked "open", and would overwrite the
//! output of the run being looked at.
//!
//! **The live app serves the notebook's head**, because marimo turns the
//! notebook root into apps and the revision history is kept out of it. So a
//! session on an old run's rows shows those rows under today's code; the
//! session names the revision that ran, so the two are never conflated.
//!
//! The address is `AIWATCHER_ML_PIPELINE_URL`. Without it this registers no
//! editor host and the route answers 501 naming the variable — a process that
//! cannot do the work does not offer it.

use std::sync::Arc;
use std::time::Duration;

use aiwatcher_core::ports::{EditorHost, EditorRequest, EditorSession, PortError, PortResult};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use super::artifacts::{Artifacts, Rows};
use crate::config::Config;

/// How long staging may take. Generous for a large table, and far short of a
/// run: nothing here executes anything.
const STAGE_TIMEOUT: Duration = Duration::from_secs(60);

/// What a failure is reported against. The runtime, not the object store: a
/// caller reading "the notebook runtime is unavailable" knows which process to
/// look at, and the artifact read is reported the same way because from the
/// panel's side the editor is one thing that did or did not open.
const TARGET: &str = "the notebook runtime";

/// The editor host, if this process has an address and an object store.
#[must_use]
pub fn host(config: &Config, artifacts: Option<&Artifacts>) -> Option<Arc<dyn EditorHost>> {
    let (Some(endpoint), Some(artifacts)) = (config.ml_pipeline_url.clone(), artifacts) else {
        return None;
    };
    match NotebookEditor::new(endpoint.clone(), artifacts.clone()) {
        Ok(editor) => {
            tracing::info!(%endpoint, "this role opens a block's editor on a step's own rows");
            Some(Arc::new(editor))
        }
        Err(error) => {
            tracing::error!(
                %endpoint, %error,
                "the notebook client could not be built; the editor route will answer 501"
            );
            None
        }
    }
}

#[derive(Debug)]
pub struct NotebookEditor {
    endpoint: String,
    client: reqwest::Client,
    artifacts: Artifacts,
}

impl NotebookEditor {
    /// # Errors
    ///
    /// When the HTTP client cannot be built at all.
    pub fn new(endpoint: String, artifacts: Artifacts) -> Result<Self, reqwest::Error> {
        Ok(Self {
            endpoint: endpoint.trim_end_matches('/').to_owned(),
            client: reqwest::Client::builder().build()?,
            artifacts,
        })
    }
}

#[async_trait]
impl EditorHost for NotebookEditor {
    async fn open(&self, request: EditorRequest) -> PortResult<EditorSession> {
        // A step with no upstream opens on nothing rather than on whatever the
        // last run left there. An empty table is the honest answer; somebody
        // else's table is the failure the context id exists to prevent.
        let rows: Rows = match &request.input {
            Some(artifact) => self.artifacts.read_rows(artifact).await.map_err(|error| {
                PortError::Unavailable {
                    target: TARGET,
                    message: error.to_string(),
                }
            })?,
            None => Vec::new(),
        };
        let staged = rows.len();

        let answer: StagedAnswer = self
            .post(json!({
                "notebook": request.notebook,
                "rows": rows,
                "params": request.params,
                "context": request.context_id,
            }))
            .await?;

        Ok(EditorSession {
            app_url: answer.app_url,
            context_id: request.context_id,
            notebook: request.notebook,
            code_revision: request.code_revision,
            rows: staged,
        })
    }
}

#[derive(Debug, Deserialize)]
struct StagedAnswer {
    app_url: String,
}

impl NotebookEditor {
    async fn post(&self, body: serde_json::Value) -> PortResult<StagedAnswer> {
        let response = self
            .client
            .post(format!("{}/ml-pipeline/staging", self.endpoint))
            .json(&body)
            .timeout(STAGE_TIMEOUT)
            .send()
            .await
            .map_err(|error| PortError::Unavailable {
                target: TARGET,
                message: format!("the notebook runtime could not be reached: {error}"),
            })?;

        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|error| PortError::Unavailable {
                target: TARGET,
                message: format!("the notebook runtime's answer stopped short: {error}"),
            })?;

        if !status.is_success() {
            // The runtime's own split, kept: it refuses a notebook it does not
            // hold with a 4xx, which says the same thing on every retry.
            let message = refusal(status, &text);
            return Err(if status.is_client_error() {
                PortError::Rejected {
                    target: TARGET,
                    message,
                }
            } else {
                PortError::Unavailable {
                    target: TARGET,
                    message,
                }
            });
        }

        serde_json::from_str(&text).map_err(|error| PortError::Unavailable {
            target: TARGET,
            message: format!("the runtime answered oddly: {error}"),
        })
    }
}

/// The runtime's own message, when it sent one.
fn refusal(status: reqwest::StatusCode, body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .and_then(|message| message.as_str().map(str::to_owned))
        })
        .unwrap_or_else(|| format!("the notebook runtime answered {status}"))
}
