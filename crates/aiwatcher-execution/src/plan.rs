//! What the server compiles a definition into, and runs.
//!
//! Three records, and they are deliberately different things (ADR_0025). A
//! definition is editable. A [`DefinitionRevision`] is immutable and
//! content-addressed — a curation pipeline's `revision` digests the whole
//! authored request, canvas positions included, because that is what a person
//! saved and what `produced_by` names. An [`ExecutionPlan`] is derived from
//! one, addressed by `plan_id`, and digests the *executable* fields only.
//!
//! That second digest is the whole reason this type exists rather than running
//! the blocks directly. Two pipelines differing only in layout compile to one
//! `plan_id`, so dragging a block across the canvas invalidates no cache and
//! starts no different run — while the authored revision still names the
//! picture somebody drew.
//!
//! The plan is a **graph**, even though the first authoring model is a chain.
//! An agent or ML workflow needs a DAG and must not need a second execution
//! record to get one; what a chain contributes is that its compiler produces a
//! graph with one edge per block.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use aiwatcher_core::ArtifactKind;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::digest;

/// The immutable, content-addressed version of an authored definition.
///
/// A newtype over the digest so that a function taking both a revision and a
/// `plan_id` cannot be called with them the wrong way round — they are both
/// 64 hex characters and they mean different things.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize, ToSchema)]
#[serde(transparent)]
pub struct DefinitionRevision(pub String);

impl std::fmt::Display for DefinitionRevision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// `sha256` of the canonical compiled plan.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize, ToSchema)]
#[serde(transparent)]
pub struct PlanId(pub String);

impl std::fmt::Display for PlanId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What kind of definition a plan was compiled from.
///
/// Part of a plan's identity, so a curation pipeline and an agent graph that
/// happened to compile to identical steps stay two plans. They have different
/// editors, different permissions and different provenance.
/// `Ord` so a [`crate::SlotKey`] is, which is what lets the memory adapter keep
/// slots in a `BTreeMap` and answer "this definition's, newest first" without
/// a scan of every definition's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DefinitionKind {
    /// ADR_0024's source/transform/notebook/view chain.
    CurationPipeline,
    /// A declared agent, search or ML workflow.
    Workflow,
}

impl DefinitionKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CurationPipeline => "curation_pipeline",
            Self::Workflow => "workflow",
        }
    }
}

/// Where a step runs, and everything that runtime needs.
///
/// Where each runtime executes, who owns its retries, and what it may carry.
/// Every variant names a *binding* and its parameters and never a host — an
/// executor's address is configuration, for ADR_0012's and ADR_0016's reason,
/// unchanged. A plan naming its own endpoint would be a request-forgery
/// primitive posted by anything that can reach the API.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, ToSchema)]
#[serde(tag = "runtime", rename_all = "snake_case")]
pub enum RuntimeBinding {
    /// One Flow PHP query. Its `blocks` is what makes three boxes on the canvas
    /// light up together: Flow executes one pipeline, so a source and its
    /// transforms compile to one step.
    FlowPhp(FlowStepSpec),
    /// One marimo notebook over the rows the step before it produced.
    Marimo(MarimoStepSpec),
    /// A dataset version, written by the serve role. The one binding that runs
    /// in that process, and it executes nothing: it writes a
    /// content-addressed version.
    PublishDataset(PublishDatasetSpec),
    /// A registered function a worker pulls and runs.
    PythonTask(PythonTaskSpec),
    /// Nobody runs it. It waits for somebody to answer.
    HumanInput(HumanInputSpec),
    /// Handed whole to an external engine through `core::engine::WorkflowEngine`.
    /// That engine owns its internal retries; this one records the reference.
    ExternalWorkflow(ExternalWorkflowSpec),
}

/// The word a reactor routes on, without loading the plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeKind {
    FlowPhp,
    Marimo,
    PublishDataset,
    PythonTask,
    HumanInput,
    ExternalWorkflow,
}

impl RuntimeKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FlowPhp => "flow_php",
            Self::Marimo => "marimo",
            Self::PublishDataset => "publish_dataset",
            Self::PythonTask => "python_task",
            Self::HumanInput => "human_input",
            Self::ExternalWorkflow => "external_workflow",
        }
    }

    /// Whether a worker claims this rather than a reactor in the work role.
    #[must_use]
    pub const fn is_pulled(self) -> bool {
        matches!(self, Self::PythonTask)
    }

    /// Whether performing this needs a process the server does not run.
    ///
    /// A pulled attempt is claimed by a worker — a process somebody else
    /// operates, against the same store. Everything else here is a reactor in
    /// one of this binary's two roles, or a wait, or a delegation to a system
    /// that never touches the store at all.
    #[must_use]
    pub const fn needs_another_process(self) -> bool {
        self.is_pulled()
    }

    /// Whether a step of this kind may be answered from a cache.
    ///
    /// Never for a wait or a delegation: a `HumanInput` cache hit would be a
    /// decision somebody made about a different run, and an `ExternalWorkflow`
    /// result is the engine's to reuse or not.
    #[must_use]
    pub const fn is_cacheable(self) -> bool {
        matches!(self, Self::FlowPhp | Self::Marimo | Self::PythonTask)
    }
}

impl RuntimeBinding {
    #[must_use]
    pub const fn kind(&self) -> RuntimeKind {
        match self {
            Self::FlowPhp(_) => RuntimeKind::FlowPhp,
            Self::Marimo(_) => RuntimeKind::Marimo,
            Self::PublishDataset(_) => RuntimeKind::PublishDataset,
            Self::PythonTask(_) => RuntimeKind::PythonTask,
            Self::HumanInput(_) => RuntimeKind::HumanInput,
            Self::ExternalWorkflow(_) => RuntimeKind::ExternalWorkflow,
        }
    }

    /// The authored canvas blocks this binding came from, in order.
    ///
    /// `None` is not "no blocks" — it is *this kind is not drawn on a canvas*,
    /// and the two answers are used differently: a block id is matched against
    /// the list when there is one, and against the step's own id when there is
    /// not, because a plan carrying one of those came from a
    /// `WorkflowDefinition`, whose editor addresses steps by their own id.
    ///
    /// One place that knows which specs carry a block, so the forward lookup
    /// and the reverse map cannot come to disagree about it.
    #[must_use]
    pub fn blocks(&self) -> Option<&[String]> {
        match self {
            // Many, and that is the interesting one: the compiler folds a
            // source and every transform behind it into a single query.
            Self::FlowPhp(spec) => Some(&spec.blocks),
            // `from_ref` rather than `as_slice`: a spec with no block answers
            // `None` — *not drawn on a canvas* — instead of an empty list. The
            // two are used differently and a gate is where the difference
            // finally bites, because both compilers produce one: a curation's
            // names its block, a workflow's has no canvas and is addressed by
            // its step id.
            Self::Marimo(spec) => spec.block.as_ref().map(std::slice::from_ref),
            Self::PublishDataset(spec) => spec.block.as_ref().map(std::slice::from_ref),
            Self::HumanInput(spec) => spec.block.as_ref().map(std::slice::from_ref),
            Self::PythonTask(_) | Self::ExternalWorkflow(_) => None,
        }
    }
}

/// One plan step, and the blocks somebody drew that became it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct StepBlocks {
    pub step_id: String,
    pub runtime: RuntimeKind,
    /// The authored block ids, in order. Empty for a step no canvas block
    /// compiled to, where the step id is the address.
    pub blocks: Vec<String>,
}

/// Where the rows come from, kept structured beside the generated script.
///
/// A historical editor regenerates the query from *this*, never from the
/// pipeline's current head — which is the whole point: opening a step of a run
/// from last week has to show what that run read.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct FlowSourceRef {
    pub dataset: String,
    #[serde(default)]
    pub arguments: BTreeMap<String, String>,
    /// The dataset version this read resolved to, when it resolved to one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_revision: Option<String>,
    /// A moving window is not cacheable; a resolved one is. This is where the
    /// difference is recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<ResolvedWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Exact bounds, in seconds since the epoch. A relative window resolved once,
/// at compile time, so that a retry three hours later reads the same rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct ResolvedWindow {
    pub from: i64,
    pub to: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct FlowStepSpec {
    /// The complete query, compiled here rather than in the browser.
    pub script: String,
    pub source: FlowSourceRef,
    /// The authored block ids this one step covers, in order. The panel lights
    /// them together from one `step.started`.
    #[serde(default)]
    pub blocks: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, ToSchema)]
pub struct MarimoStepSpec {
    pub notebook: String,
    /// `sha256` of the notebook source this plan pins. A managed run pins code
    /// first; an editor test may use unsaved code and is marked ad hoc.
    pub code_revision: String,
    #[serde(default)]
    #[schema(value_type = Object)]
    pub params: BTreeMap<String, Value>,
    /// The authored block this step came from, for the editor link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct PublishDatasetSpec {
    pub dataset: String,
    /// `<pipeline name>@<revision>`, the authored revision — provenance, and
    /// never part of a dataset version's identity.
    pub produced_by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, ToSchema)]
pub struct PythonTaskSpec {
    /// `name@version`: the registered name and the pinned code version a worker
    /// must match before it may claim an attempt.
    pub task_ref: String,
    /// Which worker queue this is claimable on.
    pub queue: String,
    #[serde(default)]
    #[schema(value_type = Object)]
    pub params: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct HumanInputSpec {
    /// What is being asked, in the words the person reads.
    pub prompt: String,
    /// The role that may answer. Checked when the answer arrives, never here.
    pub role: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
    /// The authored block this step came from, for the canvas box that lights
    /// up while it waits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct ExternalWorkflowSpec {
    /// The engine this is delegated to, as configuration names it.
    pub engine: String,
    /// Always version-pinned. An execution recorded against "whatever was
    /// current" is not something anybody can repeat.
    pub entity: String,
    #[serde(default)]
    pub inputs: BTreeMap<String, String>,
}

/// Where one of a step's inputs comes from.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(tag = "from", rename_all = "snake_case")]
pub enum InputBinding {
    /// An output of an earlier step, by name.
    Step { step: String, output: String },
    /// A value bound when the execution was requested.
    Parameter { name: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct OutputDeclaration {
    pub name: String,
    pub kind: ArtifactKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_ref: Option<String>,
}

/// How many times, and how far apart.
///
/// The defaults are three attempts, 1 s / 5 s / 30 s. The delays
/// are a list rather than a base and a multiplier so that a policy can be read
/// off the plan without arithmetic, and so that a runtime whose useful backoff
/// is not exponential can say so.
///
/// # Two budgets, because there are two kinds of failure
///
/// `max_attempts` counts attempts that **produced an answer**: the code ran and
/// was wrong, the query would not parse, the graph does not bind. Three is
/// right for those, because the fourth will be just as wrong.
///
/// `max_unavailable_attempts` counts attempts where nobody answered — the
/// service was down, the connection reset, the caller stopped waiting. Those
/// say nothing about the work, and spending the work's budget on them means a
/// forty-second outage kills a run that would have succeeded a minute later.
/// Measured, not supposed: restarting the process with the query service down
/// burned all three attempts and failed a chain whose Flow step was fine.
///
/// The two are counted separately off the attempt records the state already
/// keeps, so a run alternating between the two kinds is bounded by both.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct RetryPolicy {
    /// Attempts that got an answer and the answer was wrong.
    pub max_attempts: u32,
    /// Attempts where the runtime never answered. Its own budget — see above.
    #[serde(default = "default_unavailable_attempts")]
    pub max_unavailable_attempts: u32,
    /// Seconds before attempt 2, 3, … . The last entry repeats.
    pub delays_seconds: Vec<u64>,
    /// The same, for an attempt nobody answered. Longer, because what it is
    /// waiting for is a service coming back rather than a flake passing.
    #[serde(default = "default_unavailable_delays")]
    pub delays_seconds_unavailable: Vec<u64>,
}

/// Ten, which with the delays below tolerates roughly ten minutes of a runtime
/// being down — long enough to cover a rolling restart of the service a step
/// talks to, and short enough that a run does not sit `pending` for an hour
/// against something nobody is going to bring back.
fn default_unavailable_attempts() -> u32 {
    10
}

/// 5 s / 15 s / 30 s / 60 s, the last repeating. A service that is down comes
/// back on its own schedule, and asking it four times a second while it does
/// is a way to make its recovery slower.
fn default_unavailable_delays() -> Vec<u64> {
    vec![5, 15, 30, 60]
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: aiwatcher_jobs::MAX_ATTEMPTS,
            max_unavailable_attempts: default_unavailable_attempts(),
            delays_seconds: vec![1, 5, 30],
            delays_seconds_unavailable: default_unavailable_delays(),
        }
    }
}

impl RetryPolicy {
    /// Nothing is retried automatically. What a `HumanInput` and a
    /// deterministic user-code failure get.
    #[must_use]
    pub fn once() -> Self {
        Self {
            max_attempts: 1,
            max_unavailable_attempts: 1,
            delays_seconds: Vec::new(),
            delays_seconds_unavailable: Vec::new(),
        }
    }

    /// How long to wait before `attempt`, which is 1-based.
    ///
    /// Deterministic: jitter is the dispatcher's, applied when it schedules,
    /// because a decider that reached for a random number would stop being
    /// replayable.
    ///
    /// `unavailable` picks the second list. Which failure a delay follows is
    /// what decides how long it should be: a flake passes in a second, and a
    /// service that is down comes back on its own schedule.
    #[must_use]
    pub fn delay_before(&self, attempt: u32, unavailable: bool) -> Duration {
        let delays = if unavailable {
            &self.delays_seconds_unavailable
        } else {
            &self.delays_seconds
        };
        // Attempt 1 is not a retry, so it waits for nothing. Saturating
        // subtraction alone would give it the first delay.
        let Some(index) = attempt.checked_sub(2).map(|index| index as usize) else {
            return Duration::ZERO;
        };
        let seconds = delays
            .get(index)
            .or_else(|| delays.last())
            .copied()
            .unwrap_or(0);
        Duration::from_secs(seconds)
    }
}

/// Whether a step may be answered from an earlier identical one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CachePolicy {
    /// The default everywhere. Opting in is a claim that the step is a pure
    /// function of things that are all digest-addressed, and that claim is
    /// wrong often enough to be worth making out loud.
    #[default]
    Never,
    /// Reuse a completed result with the same cache key.
    ByContent,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, ToSchema)]
pub struct PlanStep {
    pub id: String,
    pub runtime: RuntimeBinding,
    #[serde(default)]
    pub inputs: Vec<InputBinding>,
    #[serde(default)]
    pub outputs: Vec<OutputDeclaration>,
    #[serde(default)]
    pub retry: RetryPolicy,
    /// Seconds. Per-runtime and explicit — never a global default, because the
    /// number that is generous for a Flow query is a hang for a notebook.
    pub timeout_seconds: u64,
    #[serde(default)]
    pub cache: CachePolicy,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize, ToSchema)]
pub struct PlanEdge {
    pub from: String,
    pub to: String,
}

/// The runtime-neutral graph an execution runs.
///
/// Immutable. A run pins exactly one of these, so editing the definition while
/// a run is active creates a new revision and a new plan and changes nothing
/// about what is already running.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, ToSchema)]
pub struct ExecutionPlan {
    pub plan_id: PlanId,
    pub definition_kind: DefinitionKind,
    pub definition_name: String,
    pub revision: DefinitionRevision,
    pub steps: Vec<PlanStep>,
    #[serde(default)]
    pub edges: Vec<PlanEdge>,
}

impl ExecutionPlan {
    /// Seal a compiled graph by giving it its content address.
    ///
    /// The one constructor: a plan with a `plan_id` that is not a digest of its
    /// own contents would be a cache key that means nothing.
    #[must_use]
    pub fn seal(
        definition_kind: DefinitionKind,
        definition_name: String,
        revision: DefinitionRevision,
        steps: Vec<PlanStep>,
        mut edges: Vec<PlanEdge>,
    ) -> Self {
        // Sorted so that two compilers emitting the same graph in a different
        // order reach the same address. The steps are *not* sorted: their order
        // is the topological one the compiler resolved, and it is information.
        edges.sort();
        edges.dedup();
        let mut plan = Self {
            plan_id: PlanId(String::new()),
            definition_kind,
            definition_name,
            revision,
            steps,
            edges,
        };
        plan.plan_id = PlanId(digest(canonical(&plan).as_bytes()));
        plan
    }

    #[must_use]
    pub fn step(&self, id: &str) -> Option<&PlanStep> {
        self.steps.iter().find(|step| step.id == id)
    }

    /// The steps `id` must wait for.
    #[must_use]
    pub fn parents_of(&self, id: &str) -> Vec<&str> {
        self.edges
            .iter()
            .filter(|edge| edge.to == id)
            .map(|edge| edge.from.as_str())
            .collect()
    }

    /// The steps waiting on `id`.
    #[must_use]
    pub fn children_of(&self, id: &str) -> Vec<&str> {
        self.edges
            .iter()
            .filter(|edge| edge.from == id)
            .map(|edge| edge.to.as_str())
            .collect()
    }

    /// The step an authored block compiled into.
    ///
    /// Not one-to-one, which is the whole reason this is a search rather than a
    /// lookup: Flow executes one pipeline, so a source and every transform
    /// after it fold into a single step whose `blocks` lists all of them. Three
    /// boxes on a canvas point at one step, and opening any of them has to
    /// reach it.
    #[must_use]
    pub fn step_for_block(&self, block_id: &str) -> Option<&PlanStep> {
        self.steps.iter().find(|step| match step.runtime.blocks() {
            Some(blocks) => blocks.iter().any(|id| id == block_id),
            None => step.id == block_id,
        })
    }

    /// Which authored blocks each step covers, in the plan's own order.
    ///
    /// The inverse of [`Self::step_for_block`], and the answer the canvas needs:
    /// a run reports `step.started` for a step, and the blocks a person drew
    /// are what they are looking at. Three source blocks folded into one Flow
    /// query light together, which is the truth about how they ran.
    ///
    /// Derived from the pinned plan and never from a draft for a step's
    /// context, at the grain of a whole run. A browser working this out would
    /// work it out from the canvas on screen, which is the one thing that is
    /// certainly not what the run compiled.
    #[must_use]
    pub fn blocks_by_step(&self) -> Vec<StepBlocks> {
        self.steps
            .iter()
            .map(|step| StepBlocks {
                step_id: step.id.clone(),
                runtime: step.runtime.kind(),
                blocks: step.runtime.blocks().unwrap_or_default().to_vec(),
            })
            .collect()
    }

    /// The steps that cannot be performed by this process alone.
    ///
    /// Read against [`StoreCapabilities::multi_process`] *before* a run is
    /// accepted (ADR_0025). Discovering it later is worse than it sounds: the
    /// run starts, the decider dispatches the step, and nothing claims it —
    /// which looks exactly like a worker that is busy, forever, with nothing
    /// in any log saying that no worker can ever exist here.
    ///
    /// [`StoreCapabilities::multi_process`]: crate::store::StoreCapabilities::multi_process
    #[must_use]
    pub fn steps_needing_another_process(&self) -> Vec<&str> {
        self.steps
            .iter()
            .filter(|step| step.runtime.kind().needs_another_process())
            .map(|step| step.id.as_str())
            .collect()
    }

    /// Whether every step is reachable and no edge closes a cycle.
    ///
    /// The compiler already refused both; this is what a plan loaded from a
    /// store is checked against before it is run, because a plan is a record
    /// that outlives the compiler that wrote it.
    #[must_use]
    pub fn is_acyclic(&self) -> bool {
        let mut remaining: BTreeSet<&str> = self.steps.iter().map(|s| s.id.as_str()).collect();
        loop {
            let ready: Vec<&str> = remaining
                .iter()
                .copied()
                .filter(|id| {
                    self.parents_of(id)
                        .iter()
                        .all(|parent| !remaining.contains(parent))
                })
                .collect();
            if ready.is_empty() {
                return remaining.is_empty();
            }
            for id in ready {
                remaining.remove(id);
            }
        }
    }
}

/// JSON with every object key sorted, at every depth.
///
/// `serde_json`'s own map is a `BTreeMap` here, so going through
/// [`serde_json::Value`] is what sorts the keys — serialising a struct directly
/// writes them in declaration order, and then reordering two fields in a Rust
/// file would silently change every plan's address.
pub(crate) fn canonical<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .and_then(|value| serde_json::to_string(&value))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(id: &str) -> PlanStep {
        PlanStep {
            id: id.to_owned(),
            runtime: RuntimeBinding::PythonTask(PythonTaskSpec {
                task_ref: "stage@1".to_owned(),
                queue: "default".to_owned(),
                params: BTreeMap::new(),
            }),
            inputs: Vec::new(),
            outputs: Vec::new(),
            retry: RetryPolicy::default(),
            timeout_seconds: 60,
            cache: CachePolicy::Never,
        }
    }

    fn plan(edges: Vec<PlanEdge>) -> ExecutionPlan {
        ExecutionPlan::seal(
            DefinitionKind::Workflow,
            "import".to_owned(),
            DefinitionRevision("ab".repeat(32)),
            vec![step("one"), step("two")],
            edges,
        )
    }

    #[test]
    fn the_order_two_edges_were_written_in_is_not_part_of_a_plans_identity() {
        let forward = plan(vec![PlanEdge {
            from: "one".to_owned(),
            to: "two".to_owned(),
        }]);
        let same = plan(vec![
            PlanEdge {
                from: "one".to_owned(),
                to: "two".to_owned(),
            },
            PlanEdge {
                from: "one".to_owned(),
                to: "two".to_owned(),
            },
        ]);
        assert_eq!(forward.plan_id, same.plan_id);
    }

    #[test]
    fn a_plan_id_is_a_digest_of_the_plan_and_not_of_its_name_alone() {
        let one = plan(Vec::new());
        let mut other_steps = vec![step("one"), step("two")];
        other_steps[0].timeout_seconds = 61;
        let other = ExecutionPlan::seal(
            DefinitionKind::Workflow,
            "import".to_owned(),
            DefinitionRevision("ab".repeat(32)),
            other_steps,
            Vec::new(),
        );
        assert_ne!(one.plan_id, other.plan_id);
    }

    #[test]
    fn a_cycle_is_visible_in_a_plan_loaded_from_a_store() {
        let cyclic = plan(vec![
            PlanEdge {
                from: "one".to_owned(),
                to: "two".to_owned(),
            },
            PlanEdge {
                from: "two".to_owned(),
                to: "one".to_owned(),
            },
        ]);
        assert!(!cyclic.is_acyclic());
        assert!(plan(Vec::new()).is_acyclic());
    }

    #[test]
    fn the_retry_delays_are_one_five_and_thirty_and_the_last_one_repeats() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.delay_before(2, false), Duration::from_secs(1));
        assert_eq!(policy.delay_before(3, false), Duration::from_secs(5));
        assert_eq!(policy.delay_before(4, false), Duration::from_secs(30));
        assert_eq!(policy.delay_before(9, false), Duration::from_secs(30));
        // Attempt 1 is not a retry, so it waits for nothing.
        assert_eq!(policy.delay_before(1, false), Duration::ZERO);
        assert_eq!(RetryPolicy::once().delay_before(2, false), Duration::ZERO);
    }

    #[test]
    fn waiting_for_a_runtime_to_come_back_waits_longer_than_waiting_out_a_flake() {
        // A flake passes in a second; a service that is down comes back on its
        // own schedule, and asking it four times a second while it does is a
        // way to make its recovery slower.
        let policy = RetryPolicy::default();
        assert_eq!(policy.delay_before(2, true), Duration::from_secs(5));
        assert_eq!(policy.delay_before(5, true), Duration::from_secs(60));
        assert_eq!(policy.delay_before(20, true), Duration::from_secs(60));

        // And it is the budget that makes an outage survivable: ten attempts
        // over those delays is about ten minutes, against the thirty-six
        // seconds one budget of three used to allow.
        let tolerated: u64 = (2..=policy.max_unavailable_attempts)
            .map(|attempt| policy.delay_before(attempt, true).as_secs())
            .sum();
        assert!(tolerated >= 300, "an outage of {tolerated}s is survivable");
    }

    #[test]
    fn a_plan_says_which_of_its_steps_this_process_cannot_perform_alone() {
        // Read before the run is accepted. A `PythonTask` on a store that
        // holds one process is a step that is dispatched and then claimed by
        // nobody, which looks exactly like a busy worker — forever.
        let pulled = ExecutionPlan::seal(
            DefinitionKind::Workflow,
            "import".to_owned(),
            DefinitionRevision("ab".repeat(32)),
            vec![step("stage")],
            Vec::new(),
        );
        assert_eq!(pulled.steps_needing_another_process(), vec!["stage"]);

        let mut local = step("publish");
        local.runtime = RuntimeBinding::PublishDataset(PublishDatasetSpec {
            dataset: "clean".to_owned(),
            produced_by: "pii@ab".to_owned(),
            block: None,
        });
        let here = ExecutionPlan::seal(
            DefinitionKind::CurationPipeline,
            "pii".to_owned(),
            DefinitionRevision("ab".repeat(32)),
            vec![local],
            Vec::new(),
        );
        assert!(here.steps_needing_another_process().is_empty());
    }

    #[test]
    fn a_wait_and_a_delegation_are_never_answered_from_a_cache() {
        // A `HumanInput` hit would be a decision somebody made about another
        // run; an `ExternalWorkflow` result is the engine's to reuse.
        assert!(!RuntimeKind::HumanInput.is_cacheable());
        assert!(!RuntimeKind::ExternalWorkflow.is_cacheable());
        assert!(RuntimeKind::FlowPhp.is_cacheable());
    }
}
