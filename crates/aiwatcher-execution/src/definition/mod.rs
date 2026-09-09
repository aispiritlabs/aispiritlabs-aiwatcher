//! Authored Python workflows. Definitions are data; only registered workers run code.

mod registry;
pub use registry::{DefinitionRegistry, SavedWorkflow};

use std::collections::{BTreeMap, BTreeSet};

use aiwatcher_core::ArtifactKind;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::plan::{
    CachePolicy, DefinitionKind, DefinitionRevision, ExecutionPlan, InputBinding,
    OutputDeclaration, PlanEdge, PlanStep, PythonTaskSpec, RetryPolicy, RuntimeBinding, canonical,
};
use crate::{CompileError, digest};

/// A workflow's executable definition, saved by content before it is run.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowSpec {
    pub name: String,
    /// The application's release label; the immutable identity is the revision digest.
    pub version: String,
    pub steps: Vec<WorkflowTask>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowTask {
    pub id: String,
    /// A registered function's name and pinned version. Never an import path to execute.
    pub task_ref: String,
    pub queue: String,
    #[serde(default)]
    pub after: Vec<String>,
    #[serde(default)]
    pub inputs: Vec<WorkflowInput>,
    /// Named row artifacts this step produces.
    #[serde(default)]
    pub outputs: Vec<String>,
    #[serde(default)]
    #[schema(value_type = Object)]
    pub params: BTreeMap<String, Value>,
    #[serde(default)]
    pub retry: RetryPolicy,
    pub timeout_seconds: u64,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowInput {
    pub step: String,
    pub output: String,
}

impl WorkflowSpec {
    #[must_use]
    pub fn revision(&self) -> DefinitionRevision {
        DefinitionRevision(digest(canonical(self).as_bytes()))
    }

    /// Validate every reference and resolve a deterministic topological order.
    ///
    /// # Errors
    /// All authored problems, before any definition or execution is written.
    pub fn compile(&self) -> Result<ExecutionPlan, CompileError> {
        let mut problems = Vec::new();
        for (field, value) in [("name", &self.name), ("version", &self.version)] {
            if !valid_name(value) || value.contains('@') {
                problems.push(format!(
                    "{field} must be nonempty, without @ or outer whitespace"
                ));
            }
        }
        if self.steps.is_empty() || self.steps.len() > 256 {
            problems.push("a workflow needs between 1 and 256 steps".to_owned());
        }
        let by_id: BTreeMap<_, _> = self
            .steps
            .iter()
            .map(|step| (step.id.as_str(), step))
            .collect();
        if by_id.len() != self.steps.len() {
            problems.push("step ids must be unique".to_owned());
        }
        let mut edges = BTreeSet::new();
        for step in &self.steps {
            if !valid_name(&step.id) || !valid_name(&step.queue) {
                problems.push(format!(
                    "{}: step id and queue must be nonempty without outer whitespace",
                    step.id
                ));
            }
            if !step
                .task_ref
                .split_once('@')
                .is_some_and(|(name, version)| {
                    valid_name(name) && valid_name(version) && !version.contains('@')
                })
            {
                problems.push(format!("{}: task_ref must be name@version", step.id));
            }
            if step.timeout_seconds == 0 || step.timeout_seconds > 604_800 {
                problems.push(format!(
                    "{}: timeout_seconds must be between 1 and 604800",
                    step.id
                ));
            }
            if step.retry.max_attempts == 0 || step.retry.max_unavailable_attempts == 0 {
                problems.push(format!("{}: retry budgets must be positive", step.id));
            }
            if step.outputs.iter().any(|output| !valid_name(output))
                || step.outputs.iter().collect::<BTreeSet<_>>().len() != step.outputs.len()
            {
                problems.push(format!("{}: outputs must be nonempty and unique", step.id));
            }
            let mut input_names = BTreeSet::new();
            for input in &step.inputs {
                if !by_id
                    .get(input.step.as_str())
                    .is_some_and(|source| source.outputs.contains(&input.output))
                {
                    problems.push(format!(
                        "{}: input {}.{} is not a declared output",
                        step.id, input.step, input.output
                    ));
                }
                if !input_names.insert(&input.output) {
                    problems.push(format!(
                        "{}: input name {} is ambiguous",
                        step.id, input.output
                    ));
                }
            }
            for parent in step
                .after
                .iter()
                .chain(step.inputs.iter().map(|input| &input.step))
            {
                if !by_id.contains_key(parent.as_str()) {
                    problems.push(format!("{}: unknown dependency {parent}", step.id));
                }
                edges.insert(PlanEdge {
                    from: parent.clone(),
                    to: step.id.clone(),
                });
            }
        }
        let mut remaining: BTreeSet<_> = by_id.keys().copied().collect();
        let mut ordered = Vec::new();
        while !remaining.is_empty() {
            let ready: Vec<_> = remaining
                .iter()
                .copied()
                .filter(|id| {
                    edges
                        .iter()
                        .filter(|edge| edge.to == **id)
                        .all(|edge| !remaining.contains(edge.from.as_str()))
                })
                .collect();
            if ready.is_empty() {
                problems.push("workflow dependencies contain a cycle".to_owned());
                break;
            }
            for id in ready {
                remaining.remove(id);
                if let Some(step) = by_id.get(id) {
                    ordered.push(*step);
                }
            }
        }
        if !problems.is_empty() {
            return Err(CompileError::Refused(problems));
        }
        let steps = ordered
            .into_iter()
            .map(|step| PlanStep {
                id: step.id.clone(),
                runtime: RuntimeBinding::PythonTask(PythonTaskSpec {
                    task_ref: step.task_ref.clone(),
                    queue: step.queue.clone(),
                    params: step.params.clone(),
                }),
                inputs: step
                    .inputs
                    .iter()
                    .map(|input| InputBinding::Step {
                        step: input.step.clone(),
                        output: input.output.clone(),
                    })
                    .collect(),
                outputs: step
                    .outputs
                    .iter()
                    .map(|name| OutputDeclaration {
                        name: name.clone(),
                        kind: ArtifactKind::Rows,
                        schema_ref: None,
                    })
                    .collect(),
                retry: step.retry.clone(),
                timeout_seconds: step.timeout_seconds,
                cache: CachePolicy::Never,
            })
            .collect();
        Ok(ExecutionPlan::seal(
            DefinitionKind::Workflow,
            self.name.clone(),
            self.revision(),
            steps,
            edges.into_iter().collect(),
        ))
    }
}

fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value.len() <= 512
        && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn definition() -> WorkflowSpec {
        serde_json::from_value(json!({"name":"house/import", "version":"1", "steps":[
            {"id":"persist", "task_ref":"persist@1", "queue":"local", "timeout_seconds":30,
             "inputs":[{"step":"acquire", "output":"rows"}]},
            {"id":"acquire", "task_ref":"acquire@1", "queue":"local", "timeout_seconds":30,
             "outputs":["rows"]}
        ]}))
        .expect("definition")
    }

    #[test]
    fn input_artifacts_establish_dependencies_and_steps_are_topologically_ordered() {
        let plan = definition().compile().expect("compile");
        assert_eq!(plan.steps[0].id, "acquire");
        assert_eq!(plan.parents_of("persist"), ["acquire"]);
        assert_eq!(plan.definition_kind, DefinitionKind::Workflow);
        assert_eq!(
            plan.steps[1].inputs,
            vec![InputBinding::Step {
                step: "acquire".to_owned(),
                output: "rows".to_owned()
            }]
        );
    }

    #[test]
    fn invalid_graphs_and_unpinned_tasks_are_refused_together() {
        let mut spec = definition();
        spec.steps[1].after = vec!["persist".to_owned()];
        spec.steps[0].task_ref = "persist".to_owned();
        spec.steps[0].inputs[0].output = "missing".to_owned();
        let error = spec.compile().expect_err("refused");
        assert_eq!(error.problems().len(), 3);
    }

    #[tokio::test]
    async fn registration_is_idempotent_and_new_heads_do_not_change_pinned_versions() {
        let store =
            std::sync::Arc::new(aiwatcher_prompts::adapters::memory::MemoryObjectStore::new());
        let registry = DefinitionRegistry::new(store);
        let spec = definition();
        let now = time::OffsetDateTime::now_utc();
        let one = registry
            .save(spec.clone(), "first".to_owned(), now)
            .await
            .expect("save");
        assert_eq!(
            registry
                .save(spec.clone(), "again".to_owned(), now)
                .await
                .expect("repeat"),
            one
        );
        let mut edited = spec;
        edited.steps[0].task_ref = "persist@2".to_owned();
        let two = registry
            .save(edited, "second".to_owned(), now)
            .await
            .expect("save");
        assert_ne!(one.revision, two.revision);
        assert_eq!(
            registry
                .get("house/import", Some(&one.revision.0))
                .await
                .expect("old"),
            Some(one)
        );
        assert_eq!(
            registry.get("house/import", None).await.expect("head"),
            Some(two)
        );
        assert_eq!(registry.list().await.expect("list").len(), 1);
    }
}
