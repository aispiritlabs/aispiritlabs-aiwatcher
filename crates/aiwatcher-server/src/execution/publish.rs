//! The last block of a curation: a dataset version, written where the ingress
//! is.
//!
//! The one binding that runs in the `serve` role, because it executes nothing —
//! no service to call, no script to run, no credential this role lacks. It
//! reads the rows the step before it stored and writes a content-addressed
//! version through the object store the panel already reads.
//!
//! **Identity is the script, the ordered rows, the source and the window**
//! (`aiwatcher_datasets::dataset_identity`). `produced_by` and `execution_id`
//! are provenance and stay out of that digest: a block dragged across the
//! canvas is a new pipeline revision and the same rows, and a version per
//! canvas tidy-up would be a version history about layout. `execution_id` joins
//! a published version back to the run, its waterfall and its facts.
//!
//! **The script comes from the plan.** `PublishDatasetSpec` names a dataset and
//! a revision, not a query; the reactor already loaded the plan
//! ([`ActivityContext::plan`]), so `plan_id` carries no second copy of it.

use std::sync::Arc;

use aiwatcher_api::state::AppState;
use aiwatcher_datasets::{PublishDatasetRequest, Registry as DatasetRegistry};
use aiwatcher_execution::{
    ActivityCommand, ActivityContext, ActivityError, ActivityExecutor, ActivityResult,
    ExecutionPlan, ExecutorRegistry, FailureClass, RuntimeBinding, RuntimeKind,
};
use async_trait::async_trait;
use serde_json::json;

use super::artifacts::Artifacts;

/// The publish executor, if this deployment has a dataset registry.
///
/// No registry means no executor, which means a `publish_dataset` attempt is
/// never claimed here — the same shape as the Flow executor's missing address,
/// and the same consequence: the work waits rather than failing.
#[must_use]
pub fn executors(state: &AppState, artifacts: Option<&Artifacts>) -> ExecutorRegistry {
    let registry = ExecutorRegistry::new();
    let (Some(datasets), Some(artifacts)) = (state.datasets.as_ref(), artifacts) else {
        return registry;
    };
    tracing::info!("the serve role publishes managed dataset versions");
    registry.with(Arc::new(PublishExecutor {
        datasets: Arc::clone(datasets),
        artifacts: artifacts.clone(),
    }))
}

#[derive(Debug)]
pub struct PublishExecutor {
    datasets: Arc<DatasetRegistry>,
    artifacts: Artifacts,
}

#[async_trait]
impl ActivityExecutor for PublishExecutor {
    fn runtime(&self) -> RuntimeKind {
        RuntimeKind::PublishDataset
    }

    async fn execute(
        &self,
        command: &ActivityCommand,
        context: &ActivityContext,
    ) -> Result<ActivityResult, ActivityError> {
        let RuntimeBinding::PublishDataset(spec) = &command.step.runtime else {
            return Err(ActivityError::user_code("this step does not publish"));
        };
        let Some(input) = command.inputs.first() else {
            // A view block with nothing upstream. The compiler wires the edge,
            // so reaching here means the plan and the state disagree about
            // what the step before it produced.
            return Err(ActivityError::user_code(format!(
                "{} has no rows to publish: the step before it produced no artifact",
                command.key.step_id
            )));
        };

        // Verified on the way out as well as in. This is the one corruption no
        // metric downstream detects: a dataset version whose rows are somebody
        // else's table.
        let rows = self.artifacts.read_rows(input).await?;
        let columns = rows
            .first()
            .map(|row| row.keys().cloned().collect())
            .unwrap_or_default();

        let published = self
            .datasets
            .publish(PublishDatasetRequest {
                name: spec.dataset.clone(),
                description: String::new(),
                recipe: None,
                pipeline: script_of(&context.plan, &command.key.step_id),
                // The only query binding `script_of` reads today is Flow's; the
                // other two engines' steps name theirs when they compile.
                engine: aiwatcher_datasets::QueryEngine::Flow,
                columns,
                items: rows,
                // Where the rows came from, in the words a reader of the
                // version needs: the digest is the whole claim, and the `uri`
                // is only resolvable through this deployment's object store.
                source: input.digest.clone(),
                window_seconds: None,
                produced_by: Some(spec.produced_by.clone()),
                // The join this phase adds. From a published version back to
                // the run that produced it — its waterfall, its `step.*` on the
                // log, and the plan it pinned.
                execution_id: Some(command.key.execution_id.to_string()),
            })
            .await
            .map_err(refusal)?;

        Ok(ActivityResult {
            outputs: Vec::new(),
            result: Some(json!({
                "dataset": published.dataset.name,
                "version": published.dataset.latest.version,
                "rows": published.dataset.latest.row_count,
                // False when this exact script and this exact ordered set of
                // rows already existed. A rerun that changed nothing is a fact
                // worth reporting, not a failure.
                "created": published.created,
            })),
            diagnostics: None,
            awaiting: None,
            ..ActivityResult::default()
        })
    }
}

/// The query that produced the rows this step publishes.
///
/// Walks back through the plan's edges to the nearest Flow step. In a
/// Flow-only chain there is exactly one and it is the step before; the walk
/// is what keeps that true once a notebook sits between them, where the script
/// alone no longer describes the execution and `produced_by` is what does.
fn script_of(plan: &ExecutionPlan, from: &str) -> String {
    let mut seen = std::collections::BTreeSet::new();
    let mut frontier = vec![from.to_owned()];
    while let Some(step_id) = frontier.pop() {
        if !seen.insert(step_id.clone()) {
            continue;
        }
        if let Some(RuntimeBinding::FlowPhp(spec)) = plan.step(&step_id).map(|step| &step.runtime) {
            return spec.script.clone();
        }
        frontier.extend(plan.parents_of(&step_id).iter().map(|id| (*id).to_owned()));
    }
    // A chain with no Flow step in it at all. The registry demands a non-empty
    // script, and saying which plan produced the rows is more use than a
    // refusal nobody can act on.
    format!("-- produced by the plan {}", plan.plan_id)
}

/// A registry refusal, as the class that decides whether to retry.
///
/// The same split the API renders as a status: an object store that is down is
/// worth coming back for, and a name the registry refused will be refused
/// identically forever.
fn refusal(error: aiwatcher_datasets::RegistryError) -> ActivityError {
    use aiwatcher_datasets::RegistryError;
    match error {
        RegistryError::Store(store) if store.is_retryable() => {
            ActivityError::transient(store.to_string())
        }
        RegistryError::Store(store) => {
            ActivityError::new(FailureClass::Infrastructure, store.to_string())
        }
        other => ActivityError::user_code(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use aiwatcher_execution::plan::{
        CachePolicy, DefinitionKind, DefinitionRevision, FlowSourceRef, FlowStepSpec, InputBinding,
        PlanEdge, PlanStep, PublishDatasetSpec, RetryPolicy,
    };

    use super::*;

    fn step(id: &str, runtime: RuntimeBinding, inputs: Vec<InputBinding>) -> PlanStep {
        PlanStep {
            id: id.to_owned(),
            runtime,
            inputs,
            outputs: Vec::new(),
            retry: RetryPolicy::default(),
            timeout_seconds: 60,
            cache: CachePolicy::Never,
        }
    }

    fn flow(script: &str) -> RuntimeBinding {
        RuntimeBinding::FlowPhp(FlowStepSpec {
            script: script.to_owned(),
            source: FlowSourceRef::default(),
            blocks: Vec::new(),
        })
    }

    fn publish() -> RuntimeBinding {
        RuntimeBinding::PublishDataset(PublishDatasetSpec {
            dataset: "clean".to_owned(),
            produced_by: "pii@ab".to_owned(),
            block: None,
        })
    }

    #[test]
    fn a_published_version_names_the_query_that_produced_its_rows() {
        let plan = ExecutionPlan::seal(
            DefinitionKind::CurationPipeline,
            "pii".to_owned(),
            DefinitionRevision("ab".repeat(32)),
            vec![
                step("read", flow("data_frame()->read(hub_rows)"), Vec::new()),
                step(
                    "write",
                    publish(),
                    vec![InputBinding::Step {
                        step: "read".to_owned(),
                        output: "rows".to_owned(),
                    }],
                ),
            ],
            vec![PlanEdge {
                from: "read".to_owned(),
                to: "write".to_owned(),
            }],
        );
        assert_eq!(script_of(&plan, "write"), "data_frame()->read(hub_rows)");
    }

    #[test]
    fn a_notebook_between_them_does_not_hide_the_query() {
        // Checked here because the walk is what keeps this true once a
        // notebook arrives. The script alone stops describing the execution at
        // that point, which is what `produced_by` is for.
        let plan = ExecutionPlan::seal(
            DefinitionKind::CurationPipeline,
            "pii".to_owned(),
            DefinitionRevision("ab".repeat(32)),
            vec![
                step("read", flow("data_frame()->read(hub_rows)"), Vec::new()),
                step(
                    "detect",
                    RuntimeBinding::Marimo(aiwatcher_execution::plan::MarimoStepSpec {
                        notebook: "pii_scan".to_owned(),
                        code_revision: "cd".repeat(32),
                        params: BTreeMap::new(),
                        block: None,
                    }),
                    Vec::new(),
                ),
                step("write", publish(), Vec::new()),
            ],
            vec![
                PlanEdge {
                    from: "read".to_owned(),
                    to: "detect".to_owned(),
                },
                PlanEdge {
                    from: "detect".to_owned(),
                    to: "write".to_owned(),
                },
            ],
        );
        assert_eq!(script_of(&plan, "write"), "data_frame()->read(hub_rows)");
    }

    #[test]
    fn a_chain_with_no_query_in_it_says_which_plan_produced_the_rows() {
        // The registry demands a non-empty script, and naming the plan is more
        // use than a refusal nobody can act on.
        let plan = ExecutionPlan::seal(
            DefinitionKind::CurationPipeline,
            "pii".to_owned(),
            DefinitionRevision("ab".repeat(32)),
            vec![step("write", publish(), Vec::new())],
            Vec::new(),
        );
        assert!(script_of(&plan, "write").contains(&plan.plan_id.to_string()));
    }
}
