//! Authored Python workflows. Definitions are data; only registered workers run code.

mod registry;
pub use registry::{DefinitionRegistry, SavedWorkflow};

use std::collections::{BTreeMap, BTreeSet};

use aiwatcher_core::ArtifactKind;
use aiwatcher_core::human_input::OnTimeout;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::plan::{
    CachePolicy, ContainerJobSpec, DefinitionKind, DefinitionRevision, ExecutionPlan,
    HumanInputSpec, InputBinding, OutputDeclaration, PlanEdge, PlanStep, PythonTaskSpec,
    RetryPolicy, RuntimeBinding, canonical,
};
use crate::pods::PodRequest;
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

/// One step of a workflow: work a worker runs, or a gate that waits.
///
/// Two shapes in one struct rather than a tagged union, because a definition is
/// content-addressed and stored: every revision saved before gates existed has
/// to keep parsing and keep hashing to the same revision. An internally tagged
/// enum has no default tag, so it would have refused all of them. `approval`
/// is what decides which shape this is, and the fields the other shape needs
/// are refused by name when it is set.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowTask {
    pub id: String,
    /// A registered function's name and pinned version. Never an import path to
    /// execute. Empty on a gate, which runs nothing.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub task_ref: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub queue: String,
    /// Set to run this step in a pod of its own, from an operator's template
    /// (ADR_0029). Absent from what a revision digests when unset, so every
    /// definition saved before pods existed keeps its revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pod: Option<PodRequest>,
    /// Set to make this step a gate: it waits for a person and runs nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<ApprovalGate>,
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
    /// How long an attempt may take. Absent on a gate: nothing dispatches a
    /// wait, so no timer is ever armed and a number here would be a deadline
    /// the code does not keep.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub timeout_seconds: u64,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

/// A question this step puts in front of a person before the graph goes on.
///
/// The same question a curation's `approval` block holds, and
/// [`aiwatcher_core::human_input`] owns what a valid one is — a workflow gate
/// and a canvas gate compile to one binding and are answered through one route.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ApprovalGate {
    /// What is being asked, in the words the person reads.
    pub prompt: String,
    /// The role that may answer.
    #[serde(default = "answerable_role")]
    pub role: String,
    /// The answers offered. Empty is a free-text answer; anything else is the
    /// whole set, and an answer outside it is refused.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
    /// How long the graph waits here. Absent waits as long as it takes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    /// What happens when it runs out. Authored with the deadline and refused
    /// without one.
    #[serde(default)]
    pub on_timeout: OnTimeout,
}

fn answerable_role() -> String {
    aiwatcher_core::human_input::ANSWERABLE_ROLE.to_owned()
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
            if !valid_name(&step.id) {
                problems.push(format!(
                    "{}: step id must be nonempty without outer whitespace",
                    step.id
                ));
            }
            if let Some(gate) = &step.approval {
                problems.extend(aiwatcher_core::human_input::question_problems(
                    &step.id,
                    &gate.prompt,
                    &gate.role,
                    &gate.choices,
                    gate.timeout_seconds,
                    &gate.on_timeout,
                ));
                // Everything a step that *runs* needs, refused here by name.
                // Silently ignoring them would leave a queue nobody claims on,
                // a timeout nothing arms and a retry budget nothing spends
                // sitting on a saved definition, read by whoever opens it next
                // as things this system does.
                for (field, set) in [
                    ("task_ref", !step.task_ref.is_empty()),
                    ("queue", !step.queue.is_empty()),
                    ("pod", step.pod.is_some()),
                    ("outputs", !step.outputs.is_empty()),
                    ("timeout_seconds", step.timeout_seconds != 0),
                    // Against the *default* rather than against `once()`: an
                    // omitted retry and an explicitly default one are the same
                    // bytes, so refusing that would refuse every gate that
                    // simply did not mention it. What is refused is a budget
                    // somebody chose.
                    ("retry", step.retry != RetryPolicy::default()),
                ] {
                    if set {
                        problems.push(format!(
                            "{}: an approval step names {field}, and it waits rather than running                              — nobody claims it, nothing times it out and answering it once is                              the whole of what it does",
                            step.id
                        ));
                    }
                }
            } else {
                if !valid_name(&step.queue) {
                    problems.push(format!(
                        "{}: queue must be nonempty without outer whitespace",
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
                // What a request says on its own. Whether this deployment has
                // the template, lists the image and allows the quantities is
                // registration's question, asked against the templates file.
                if let Some(pod) = &step.pod {
                    problems.extend(pod.problems(&step.id));
                }
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
                // One binding for both authored surfaces. A gate on a canvas
                // and a gate in a graph are the same wait, parked by the same
                // decision and released by the same answer.
                runtime: match &step.approval {
                    Some(gate) => RuntimeBinding::HumanInput(HumanInputSpec {
                        prompt: gate.prompt.clone(),
                        role: gate.role.clone(),
                        choices: gate.choices.clone(),
                        // No canvas here: a workflow's editor addresses steps
                        // by their own id, which is what `blocks()` answering
                        // `None` means.
                        block: None,
                        timeout_seconds: gate.timeout_seconds,
                        on_timeout: gate.on_timeout.clone(),
                    }),
                    None => match &step.pod {
                        Some(pod) => RuntimeBinding::ContainerJob(ContainerJobSpec {
                            task_ref: step.task_ref.clone(),
                            queue: step.queue.clone(),
                            params: step.params.clone(),
                            pod: pod.clone(),
                        }),
                        None => RuntimeBinding::PythonTask(PythonTaskSpec {
                            task_ref: step.task_ref.clone(),
                            queue: step.queue.clone(),
                            params: step.params.clone(),
                        }),
                    },
                },
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
                // A wait is taken once — a person answered, or nobody has
                // yet, and neither is a failure to take again — and nothing
                // dispatches it, so no attempt timer is ever armed. The
                // definition carries neither and the plan says what is true.
                retry: match &step.approval {
                    Some(_) => RetryPolicy::once(),
                    None => step.retry.clone(),
                },
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

    /// The same graph with a gate between the two tasks.
    fn gated() -> WorkflowSpec {
        serde_json::from_value(json!({"name":"house/import", "version":"1", "steps":[
            {"id":"acquire", "task_ref":"acquire@1", "queue":"local", "timeout_seconds":30,
             "outputs":["rows"]},
            {"id":"sign-off", "approval":{"prompt":"Import these houses?",
             "choices":["approve","reject"]}, "after":["acquire"]},
            {"id":"persist", "task_ref":"persist@1", "queue":"local", "timeout_seconds":30,
             "after":["sign-off"], "inputs":[{"step":"acquire", "output":"rows"}]}
        ]}))
        .expect("definition")
    }

    #[test]
    fn a_gate_between_two_tasks_compiles_to_the_wait_a_canvas_gate_compiles_to() {
        // One binding for both authored surfaces. Point this arm anywhere else
        // and the run has nothing to park on.
        let plan = gated().compile().expect("compile");

        let gate = plan.step("sign-off").expect("the gate");
        let RuntimeBinding::HumanInput(spec) = &gate.runtime else {
            panic!("a gate waits");
        };
        assert_eq!(spec.prompt, "Import these houses?");
        assert_eq!(spec.role, aiwatcher_core::human_input::ANSWERABLE_ROLE);
        assert_eq!(spec.choices, vec!["approve", "reject"]);
        assert!(gate.outputs.is_empty(), "answering produces nothing");
        // The graph runs through it: `persist` waits for the answer and still
        // reads the rows `acquire` produced. Both are the author's own edges —
        // a workflow states ordering and data separately, which is why a gate
        // needs no special case here and the curation compiler needed two
        // cursors to say the same thing about a chain.
        assert_eq!(plan.parents_of("persist"), ["acquire", "sign-off"]);
        assert_eq!(
            plan.step("persist").expect("persist").inputs,
            vec![InputBinding::Step {
                step: "acquire".to_owned(),
                output: "rows".to_owned()
            }]
        );
    }

    #[test]
    fn a_workflow_gate_is_addressed_by_its_step_id_because_there_is_no_canvas() {
        // `None` is not "no blocks" — it is *not drawn on a canvas*, and it is
        // what makes the step id the address. `Some(&[])` here would make the
        // step unreachable from both directions.
        let plan = gated().compile().expect("compile");

        let gate = plan.step("sign-off").expect("the gate");
        assert_eq!(gate.runtime.blocks(), None);
        assert_eq!(
            plan.step_for_block("sign-off").map(|step| step.id.as_str()),
            Some("sign-off")
        );
    }

    #[test]
    fn a_gate_that_also_names_what_a_running_step_needs_is_refused_field_by_field() {
        let mut spec = gated();
        spec.steps[1].task_ref = "approve@1".to_owned();
        spec.steps[1].queue = "local".to_owned();
        spec.steps[1].timeout_seconds = 30;
        spec.steps[1].outputs = vec!["verdict".to_owned()];

        let problems = spec.compile().expect_err("refused").problems().to_vec();

        for field in ["task_ref", "queue", "outputs", "timeout_seconds"] {
            assert!(
                problems
                    .iter()
                    .any(|problem| problem.contains(&format!("names {field},"))),
                "{field} not named: {problems:?}"
            );
        }
    }

    #[test]
    fn a_gate_asking_nothing_is_refused_by_the_rule_a_canvas_gate_is_refused_by() {
        let mut spec = gated();
        spec.steps[1].approval = Some(ApprovalGate {
            prompt: "  ".to_owned(),
            role: "supervisor".to_owned(),
            choices: vec!["yes".to_owned(), "yes".to_owned()],
            timeout_seconds: None,
            on_timeout: OnTimeout::Fail,
        });

        let problems = spec.compile().expect_err("refused").problems().to_vec();

        assert!(
            problems
                .iter()
                .any(|problem| problem.contains("does not say what it is asking")),
            "{problems:?}"
        );
        assert!(
            problems
                .iter()
                .any(|problem| problem.contains("asks for the 'supervisor' role")),
            "{problems:?}"
        );
        assert!(
            problems
                .iter()
                .any(|problem| problem.contains("the same answer twice")),
            "{problems:?}"
        );
    }

    #[test]
    fn a_definition_saved_before_gates_existed_reads_and_digests_as_it_did() {
        // A revision is a content address and the registry is keyed by it, so a
        // definition stored before gates existed has to keep parsing *and* keep
        // hashing to the same value. That is why `approval` is a field rather
        // than a tag on a union — an internally tagged enum has no default tag,
        // so every one of them would have stopped parsing — and why the fields
        // a gate omits are skipped rather than written empty.
        let spec: WorkflowSpec = serde_json::from_value(json!({
            "name":"house/import", "version":"1", "steps":[
                {"id":"acquire", "task_ref":"acquire@1", "queue":"local",
                 "timeout_seconds":30, "outputs":["rows"], "after":[], "inputs":[], "params":{},
                 "retry":{"max_attempts":3, "max_unavailable_attempts":10,
                          "delays_seconds":[1,5,30], "delays_seconds_unavailable":[5,15,30,60]}}
            ]
        }))
        .expect("an older definition");

        assert!(spec.steps[0].approval.is_none());
        let digested = canonical(&spec);
        for absent in ["approval", "\"pod\""] {
            assert!(
                !digested.contains(absent),
                "{absent} is absent from what a revision digests: {digested}"
            );
        }
        // And the three a gate may omit are still written by a step that runs,
        // so nothing that was in an older digest has left it.
        for present in ["\"task_ref\"", "\"queue\"", "\"timeout_seconds\""] {
            assert!(digested.contains(present), "{present} left the digest");
        }
    }

    /// `definition()` with `acquire` asking for a pod.
    fn podded(pod: Value) -> WorkflowSpec {
        let mut spec = definition();
        spec.steps[1].pod = Some(serde_json::from_value(pod).expect("a pod request"));
        spec
    }

    #[test]
    fn a_step_that_asks_for_a_pod_compiles_to_a_container_job_carrying_it() {
        let spec = podded(json!({"template": "planner-import",
            "image": "ghcr.io/planner/import:1.4", "memory": "2Gi"}));
        let plan = spec.compile().expect("compile");

        let acquire = plan.step("acquire").expect("acquire");
        let RuntimeBinding::ContainerJob(job) = &acquire.runtime else {
            panic!("a pod of its own: {:?}", acquire.runtime);
        };
        assert_eq!(job.task_ref, "acquire@1");
        assert_eq!(job.queue, "local");
        assert_eq!(job.pod.image, "ghcr.io/planner/import:1.4");
        assert_eq!(job.pod.memory.as_deref(), Some("2Gi"));
        // The other step is untouched: a pod is opt-in per step.
        assert!(matches!(
            plan.step("persist").map(|step| &step.runtime),
            Some(RuntimeBinding::PythonTask(_))
        ));
        // And the image is part of what the plan is.
        let other = podded(json!({"template": "planner-import",
            "image": "ghcr.io/planner/import:1.5", "memory": "2Gi"}));
        assert_ne!(plan.plan_id, other.compile().expect("compile").plan_id);
    }

    #[test]
    fn a_pod_request_is_refused_for_a_field_it_does_not_have() {
        // Where it runs, what it mounts and what it may hold are the
        // template's. Refused by name rather than stored and ignored.
        for field in ["namespace", "secret", "serviceAccount", "gpu"] {
            let mut pod = json!({"template": "planner-import", "image": "python:3.13"});
            pod[field] = json!("x");
            let error = serde_json::from_value::<PodRequest>(pod).expect_err(field);
            assert!(error.to_string().contains(field), "{error}");
        }
    }

    #[test]
    fn a_pod_request_that_is_not_one_is_refused_before_any_template_is_read() {
        let spec = podded(json!({"template": "Planner Import",
            "image": "ghcr.io/Planner/import", "cpu": "lots", "memory": "0"}));
        let problems = spec.compile().expect_err("refused").problems().to_vec();
        for named in [
            "pod.template",
            "pod.image",
            "pod.cpu 'lots'",
            "pod.memory '0'",
        ] {
            assert!(
                problems.iter().any(|problem| problem.contains(named)),
                "{named} not named: {problems:?}"
            );
        }
    }

    #[test]
    fn a_gate_that_asks_for_a_pod_is_refused_by_name() {
        let mut spec = gated();
        spec.steps[1].pod = Some(PodRequest {
            template: "planner-import".to_owned(),
            image: "python:3.13".to_owned(),
            cpu: None,
            memory: None,
        });
        let problems = spec.compile().expect_err("refused").problems().to_vec();
        assert!(
            problems
                .iter()
                .any(|problem| problem.contains("names pod,")),
            "{problems:?}"
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
