//! What a block was, when it ran — or when it was saved.
//!
//! Every part of the answer is somewhere the browser is not: the pinned plan,
//! the artifacts a parent produced, the attempt a staging key is named after.
//! So the server answers it.
//!
//! [`ContextSnapshot::of_step`] is one step of one **run** — what it read, what
//! it produced, which attempt it is on. Exact, because a run pins an immutable
//! plan: opening last week's step shows last week's script over last week's
//! rows.
//!
//! [`ContextSnapshot::of_block`] is one block of one **revision** — what it
//! would run. No input artifacts and no state, said with empty fields rather
//! than borrowed from the newest run.
//!
//! The runtime is the plan's own `RuntimeBinding`, not a second description of
//! it, so a runtime gaining a field changes one place.
//!
//! `code_revision` is the `sha256` the pipeline pinned, never a copy of the
//! source — that file is what marimo serves and what a test reads. Whether it
//! still hashes to the pin is the notebook runtime's answer, and the one part
//! of a context the process holding the plan cannot know.

use std::collections::BTreeMap;

use aiwatcher_core::ArtifactRef;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::plan::{
    DefinitionKind, DefinitionRevision, ExecutionPlan, PlanId, RuntimeBinding, RuntimeKind,
};
use crate::state::{Execution, StateType, StepState};

/// Something a caller may do with a block, given what it is and where it got to.
///
/// Deliberately **no hrefs**. An action's address is either this API's own — which the generated
/// client already has — or an optional service's, and a service's address in a
/// response body is the one thing every route here refuses to carry. What only
/// the server knows is *which* actions apply, and that is what this is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContextAction {
    /// Check the query without running it. A Flow step, whose script the query
    /// service can parse and refuse.
    Validate,
    /// Run it ad hoc, against this context's own rows. Not durable, and the
    /// panel labels it so (15.1).
    Test,
    /// Open the notebook's live app on this context.
    OpenEditor,
    /// Take this step again, from the inputs and the code its plan pinned.
    /// Offered only where `RetryStep` would be accepted, which is why it
    /// depends on the step's state rather than on its runtime.
    Retry,
    /// Answer the question this step is waiting on.
    Answer,
}

/// Something a caller may do to a **run**, given where it got to.
///
/// The sibling of [`ContextAction`] and deliberately a separate enum: these are
/// done to an execution and those to a block, and one list holding both would
/// be a step's panel offering to stop the whole run.
///
/// It exists so a caller never re-derives a precondition. The rules are three
/// lines and a panel could hold them — and then there would be two copies of
/// `decide`'s mind, in two languages, and the day they disagree is the day
/// somebody trusts the wrong one. Same reasoning as `ContextAction::allowed`
/// and as the annotation canvas implementing no validation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunAction {
    /// Stop this run. Cooperative: what is in flight is asked to stop.
    Cancel,
    /// Schedule nothing further until it is resumed.
    Pause,
    /// Let a paused run schedule again.
    Resume,
}

/// Which run-level commands would be accepted right now.
///
/// Each is listed exactly where `decide` accepts it. `Pause` and `Cancel` are
/// refused only on a run that has finished; `Resume` only applies to one that
/// is paused. Both of the first two are *idempotent* rather than refused when
/// they have already been asked for — a second pause is `Ok` with no events —
/// so they stay listed while a run is pausing or cancelling, which is honest:
/// asking again is accepted and changes nothing.
#[must_use]
pub fn allowed_run_actions(state: StateType) -> Vec<RunAction> {
    if state.is_terminal() {
        return Vec::new();
    }
    if state == StateType::Paused {
        return vec![RunAction::Resume, RunAction::Cancel];
    }
    vec![RunAction::Pause, RunAction::Cancel]
}

/// Everything needed to reopen one block as it was.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct ContextSnapshot {
    /// Where this step's staged input, parameters and output live.
    ///
    /// `<execution>/<step>/<attempt>` for a run (16.2), and
    /// `<definition>@<revision>/<block>` for a revision. Keyed by context and
    /// never by the notebook's name, so two pipelines editing one notebook do
    /// not overwrite each other's rows.
    pub context_id: String,
    pub definition_kind: DefinitionKind,
    pub definition_name: String,
    /// The authored revision this was compiled from — what somebody saved.
    pub definition_revision: DefinitionRevision,
    /// The compiled plan. For a revision context this is what the definition
    /// compiles to *now*, which is not the `plan_id` of a run that pinned a
    /// window: a plan digests its resolved bounds, and a revision has none.
    pub plan_id: PlanId,
    pub step_id: String,
    /// The step exactly as the plan pinned it: the script beside its structured
    /// source, or the notebook beside the digest it was saved against.
    pub runtime: RuntimeBinding,
    /// Values bound when the execution was requested. Empty for a revision.
    #[serde(default)]
    #[schema(value_type = Object)]
    pub parameters: BTreeMap<String, Value>,
    /// What this step read, resolved to the artifacts its parents produced.
    /// Empty for a revision, because nothing has run.
    #[serde(default)]
    pub input_artifacts: Vec<ArtifactRef>,
    /// Where the step got to, across every attempt. `None` for a revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<StepState>,
    pub allowed: Vec<ContextAction>,
}

impl ContextSnapshot {
    /// One step of one run, as it actually was.
    ///
    /// `None` when the plan has no such step, which is a caller naming
    /// something that never existed rather than a state worth describing.
    #[must_use]
    pub fn of_step(execution: &Execution, step_id: &str) -> Option<Self> {
        let step = execution.plan.step(step_id)?;
        let state = execution.step(step_id).cloned();
        // The attempt a staging key is named after: the one in flight, or the
        // last one taken. `0` before any, which is a context for a step that
        // has been scheduled and not yet dispatched — and that is a real thing
        // to open, so it gets a key rather than nothing.
        let attempt = state.as_ref().map_or(0, |state| state.current_attempt);
        Some(Self {
            context_id: crate::decide::idempotency_key(
                execution.execution_id.as_str(),
                step_id,
                attempt,
            ),
            definition_kind: execution.plan.definition_kind,
            definition_name: execution.plan.definition_name.clone(),
            definition_revision: execution.plan.revision.clone(),
            plan_id: execution.plan.plan_id.clone(),
            step_id: step_id.to_owned(),
            runtime: step.runtime.clone(),
            parameters: execution.input.clone(),
            input_artifacts: execution.resolved_inputs(step_id),
            allowed: allowed_for(step.runtime.kind(), state.as_ref(), execution.cancelling),
            state,
        })
    }

    /// One authored block of one revision, as it would run.
    ///
    /// `None` when no step covers that block — a canvas id nobody compiled.
    #[must_use]
    pub fn of_block(plan: &ExecutionPlan, block_id: &str) -> Option<Self> {
        let step = plan.step_for_block(block_id)?;
        Some(Self {
            context_id: format!("{}@{}/{block_id}", plan.definition_name, plan.revision),
            definition_kind: plan.definition_kind,
            definition_name: plan.definition_name.clone(),
            definition_revision: plan.revision.clone(),
            plan_id: plan.plan_id.clone(),
            step_id: step.id.clone(),
            runtime: step.runtime.clone(),
            parameters: BTreeMap::new(),
            input_artifacts: Vec::new(),
            state: None,
            // A revision has no run, so nothing here is retryable or waiting:
            // both of those are facts about an attempt, and a block that has
            // never been compiled into one has neither.
            allowed: allowed_for(step.runtime.kind(), None, false),
        })
    }
}

/// What may be done with a step of this kind, in this state.
///
/// Two families, and they are decided by different things. The **ad-hoc** ones
/// — validate, test, open the editor — are what the runtime can be asked to do
/// beside the run, so they follow the runtime. The **state-dependent** ones
/// are commands, and each is listed exactly where `decide` would accept it:
/// `Retry` only from `Failed` or `Crashed` and not while the run is
/// cancelling, `Answer` only while a step is actually holding a question. An
/// action listed where the command would be refused is a button that returns a
/// 409, which is worse than an absent one.
///
/// Cancel, pause and resume are not here. They are done to a **run** and this
/// is a block's context; a step's panel offering to stop the whole execution
/// would be answering a question nobody asked from it.
fn allowed_for(
    runtime: RuntimeKind,
    state: Option<&StepState>,
    cancelling: bool,
) -> Vec<ContextAction> {
    if state.is_some_and(|state| state.state.state_type == StateType::Running) {
        return Vec::new();
    }

    let mut actions = match runtime {
        RuntimeKind::FlowPhp | RuntimeKind::DataFusion | RuntimeKind::DuckDb => {
            vec![ContextAction::Validate, ContextAction::Test]
        }
        RuntimeKind::Marimo => vec![ContextAction::Test, ContextAction::OpenEditor],
        // Publishing, a wait and a delegation have no ad-hoc form: the first
        // writes a dataset version and the other two are not this system's to
        // perform.
        RuntimeKind::PublishDataset
        | RuntimeKind::PythonTask
        | RuntimeKind::HumanInput
        | RuntimeKind::ExternalWorkflow => Vec::new(),
    };

    let Some(state) = state else {
        return actions;
    };

    if !cancelling
        && matches!(
            state.state.state_type,
            StateType::Failed | StateType::Crashed
        )
    {
        actions.push(ContextAction::Retry);
    }
    // The question itself rather than the state name: `awaiting` is what
    // `ProvideInput` reads, and the two agreeing by construction is the point.
    if state.awaiting.is_some() {
        actions.push(ContextAction::Answer);
    }

    actions
}

#[cfg(test)]
mod tests {
    use aiwatcher_core::ArtifactKind;

    use super::*;
    use crate::plan::{
        CachePolicy, FlowSourceRef, FlowStepSpec, InputBinding, MarimoStepSpec, OutputDeclaration,
        PlanEdge, PlanStep, PublishDatasetSpec, ResolvedWindow, RetryPolicy,
    };

    fn step(id: &str, runtime: RuntimeBinding, inputs: Vec<InputBinding>) -> PlanStep {
        PlanStep {
            id: id.to_owned(),
            runtime,
            inputs,
            outputs: vec![OutputDeclaration {
                name: "rows".to_owned(),
                kind: ArtifactKind::Rows,
                schema_ref: None,
            }],
            retry: RetryPolicy::default(),
            timeout_seconds: 60,
            cache: CachePolicy::Never,
        }
    }

    /// A source and two transforms folded into one Flow step, then a notebook,
    /// then a view — the shape a canvas produces.
    fn plan() -> ExecutionPlan {
        ExecutionPlan::seal(
            DefinitionKind::CurationPipeline,
            "pii".to_owned(),
            DefinitionRevision("ab".repeat(32)),
            vec![
                step(
                    "read",
                    RuntimeBinding::FlowPhp(FlowStepSpec {
                        script: "data_frame()->read(hub_rows)->limit(500)".to_owned(),
                        source: FlowSourceRef {
                            dataset: "hub_rows".to_owned(),
                            window: Some(ResolvedWindow {
                                from: 1_700_000_000,
                                to: 1_700_003_600,
                            }),
                            ..FlowSourceRef::default()
                        },
                        blocks: vec!["read".to_owned(), "clean".to_owned(), "trim".to_owned()],
                    }),
                    Vec::new(),
                ),
                step(
                    "detect",
                    RuntimeBinding::Marimo(MarimoStepSpec {
                        notebook: "pii_scan".to_owned(),
                        code_revision: "cd".repeat(32),
                        params: BTreeMap::from([("threshold".to_owned(), Value::from(0.8))]),
                        block: Some("detect".to_owned()),
                    }),
                    vec![InputBinding::Step {
                        step: "read".to_owned(),
                        output: "rows".to_owned(),
                    }],
                ),
                step(
                    "write",
                    RuntimeBinding::PublishDataset(PublishDatasetSpec {
                        dataset: "pii-clean".to_owned(),
                        produced_by: "pii@ab".to_owned(),
                        block: Some("write".to_owned()),
                    }),
                    Vec::new(),
                ),
            ],
            vec![PlanEdge {
                from: "read".to_owned(),
                to: "detect".to_owned(),
            }],
        )
    }

    fn running(plan: ExecutionPlan) -> Execution {
        let state =
            crate::decide::replay(std::iter::once(&crate::WorkflowEvent::ExecutionRequested {
                execution_id: crate::ExecutionId::new("exec-1"),
                plan: Box::new(plan),
                owner: crate::ExecutionOwner::Local,
                mode: crate::ExecutionMode::Compiled,
                payloads: Default::default(),
                requested_by: "mk".to_owned(),
                input: BTreeMap::from([("limit".to_owned(), Value::from(500))]),
            }));
        state.active().expect("an execution").clone()
    }

    #[test]
    fn any_of_the_three_boxes_one_flow_step_covers_opens_that_step() {
        // Flow executes one pipeline, so a source and its transforms are one
        // step. Three boxes on the canvas point at it, and opening any of them
        // has to reach it — a lookup by block id would find only the first.
        let plan = plan();
        for block in ["read", "clean", "trim"] {
            let context = ContextSnapshot::of_block(&plan, block).expect("a context");
            assert_eq!(context.step_id, "read", "opening {block}");
            assert_eq!(
                context.allowed,
                vec![ContextAction::Validate, ContextAction::Test]
            );
        }
        assert!(ContextSnapshot::of_block(&plan, "nothing").is_none());
    }

    #[test]
    fn a_revision_says_what_would_run_and_borrows_no_runs_rows() {
        // Nothing has happened, and empty fields say so. Borrowing the newest
        // execution's inputs would be a context that looks exact and describes
        // a different run.
        let context = ContextSnapshot::of_block(&plan(), "detect").expect("a context");
        assert!(context.input_artifacts.is_empty());
        assert!(context.state.is_none());
        assert!(context.parameters.is_empty());
        assert_eq!(
            context.context_id,
            format!("pii@{}/detect", "ab".repeat(32))
        );

        let RuntimeBinding::Marimo(spec) = &context.runtime else {
            panic!("the notebook block compiled to a notebook step");
        };
        // The digest the revision pinned, not the file's current one: what the
        // file hashes to now is the notebook runtime's answer, and this process
        // does not hold that file.
        assert_eq!(spec.code_revision, "cd".repeat(32));
        assert_eq!(
            context.allowed,
            vec![ContextAction::Test, ContextAction::OpenEditor]
        );
    }

    #[test]
    fn a_steps_context_is_keyed_by_the_attempt_its_staging_is_named_after() {
        // Staging is keyed by context and never by the notebook's name, so two pipelines editing one notebook stop overwriting each
        // other's rows and an old execution's editor shows that execution's
        // input.
        let execution = running(plan());
        let context = ContextSnapshot::of_step(&execution, "detect").expect("a context");
        assert_eq!(context.context_id, "exec-1/detect/0");
        assert_eq!(context.parameters["limit"], Value::from(500));
        assert!(ContextSnapshot::of_step(&execution, "absent").is_none());
    }

    #[test]
    fn a_step_that_is_running_is_not_something_to_re_run_underneath_itself() {
        let mut execution = running(plan());
        if let Some(step) = execution.steps.get_mut("read") {
            step.state = crate::RunState::of(StateType::Running);
        }
        let context = ContextSnapshot::of_step(&execution, "read").expect("a context");
        assert!(context.allowed.is_empty());
    }

    #[test]
    fn a_step_that_failed_may_be_retried_and_one_that_is_merely_waiting_may_not() {
        // The list is what a panel draws buttons from, so an action offered
        // where `decide` would refuse it is a button that returns a 409.
        let mut execution = running(plan());
        let step = execution
            .steps
            .get_mut("read")
            .expect("the plan's first step");
        step.state =
            crate::state::RunState::named(StateType::Failed, "the query service refused it");

        let context = ContextSnapshot::of_step(&execution, "read").expect("a step of this plan");
        assert!(context.allowed.contains(&ContextAction::Retry));

        // And a run on its way out offers none of it: the retry `decide` would
        // refuse is `DecisionError::Cancelling`, so listing it here would be
        // this file disagreeing with the state machine.
        execution.cancelling = true;
        let context = ContextSnapshot::of_step(&execution, "read").expect("a step of this plan");
        assert!(!context.allowed.contains(&ContextAction::Retry));
    }

    #[test]
    fn a_step_holding_a_question_is_the_only_one_that_offers_an_answer() {
        // Read from `awaiting` rather than from the state's name, because
        // `awaiting` is what `ProvideInput` reads — the two agreeing by
        // construction is the point.
        let execution = running(plan());
        let context = ContextSnapshot::of_step(&execution, "read").expect("a step of this plan");
        assert!(!context.allowed.contains(&ContextAction::Answer));
    }

    #[test]
    fn a_view_offers_nothing_ad_hoc_because_publishing_has_no_ad_hoc_form() {
        let context = ContextSnapshot::of_block(&plan(), "write").expect("a context");
        assert!(context.allowed.is_empty());
    }

    #[test]
    fn a_finished_run_is_offered_nothing_and_a_paused_one_is_offered_resume() {
        for state in StateType::TERMINAL {
            assert!(
                allowed_run_actions(state).is_empty(),
                "{} offered an action decide would refuse",
                state.as_str()
            );
        }
        assert_eq!(
            allowed_run_actions(StateType::Paused),
            vec![RunAction::Resume, RunAction::Cancel]
        );
        assert_eq!(
            allowed_run_actions(StateType::Running),
            vec![RunAction::Pause, RunAction::Cancel]
        );
        // Not `Resume`: `decide` answers `NotPaused`, and a button that 409s is
        // worse than an absent one.
        assert!(!allowed_run_actions(StateType::AwaitingInput).contains(&RunAction::Resume));
    }
}
