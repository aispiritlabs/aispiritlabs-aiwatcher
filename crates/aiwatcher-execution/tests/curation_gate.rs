#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! An authored approval, compiled and then actually run.
//!
//! The two halves of a gate are tested apart everywhere else: `compile.rs`
//! proves what a plan looks like, and `decisions.rs` proves what `decide` does
//! with a wait somebody wrote by hand. Nothing joined them, and the seam is
//! where the interesting claim lives — a gate is in the *chain* without being
//! in the *data*, so the step after it is bound to a step that is not its
//! parent. That resolves by id rather than by adjacency, and this is what says
//! so out loud.

use std::collections::BTreeMap;

use aiwatcher_core::{ArtifactKind, ArtifactRef, MessageId};
use aiwatcher_datasets::{BlockPosition, BlockSpec, CurationPipeline, PipelineBlock, PipelineEdge};
use aiwatcher_execution::plan::{DefinitionKind, InputBinding, RuntimeBinding};
use aiwatcher_execution::state::ExecutionState;
use aiwatcher_execution::{
    CompileOptions, ExecutionId, ExecutionMode, ExecutionOwner, ExecutionPlan, Now, RuntimeKind,
    StateType, WorkflowCommand, WorkflowEvent, WorkflowMessage, compile_curation, decide,
};
use time::OffsetDateTime;

fn now() -> Now {
    Now::at(OffsetDateTime::UNIX_EPOCH)
}

fn cause(name: &str) -> MessageId {
    MessageId::new(name)
}

fn block(id: &str, spec: BlockSpec) -> PipelineBlock {
    PipelineBlock {
        id: id.to_owned(),
        title: id.to_owned(),
        position: BlockPosition::default(),
        spec,
    }
}

/// `read → sign-off → publish`, as somebody would draw it on the canvas.
fn gated_pipeline() -> CurationPipeline {
    let blocks = vec![
        block(
            "read",
            BlockSpec::Source {
                dataset: "hub_rows".to_owned(),
                arguments: BTreeMap::from([("dataset".to_owned(), "ai4privacy/pii".to_owned())]),
            },
        ),
        block(
            "sign-off",
            BlockSpec::Approval {
                prompt: "Publish these rows?".to_owned(),
                role: "editor".to_owned(),
                choices: vec!["approve".to_owned(), "reject".to_owned()],
            },
        ),
        block(
            "publish",
            BlockSpec::View {
                dataset: Some("pii-clean".to_owned()),
            },
        ),
    ];
    let edges = blocks
        .windows(2)
        .map(|pair| PipelineEdge {
            from: pair[0].id.clone(),
            to: pair[1].id.clone(),
        })
        .collect();
    CurationPipeline {
        name: "curation/pii".to_owned(),
        description: String::new(),
        blocks,
        edges,
        revision: "ab".repeat(32),
        saved_at: OffsetDateTime::UNIX_EPOCH,
    }
}

fn start(plan: ExecutionPlan) -> WorkflowMessage {
    WorkflowMessage::Command(WorkflowCommand::StartExecution {
        execution_id: ExecutionId::new("exec-1"),
        plan: Box::new(plan),
        owner: ExecutionOwner::Local,
        mode: ExecutionMode::Compiled,
        payloads: Default::default(),
        requested_by: "somebody".to_owned(),
        input: BTreeMap::new(),
    })
}

fn apply(state: ExecutionState, outputs: &[aiwatcher_execution::PendingMessage]) -> ExecutionState {
    outputs
        .iter()
        .filter_map(|message| message.message.event())
        .fold(state, |state, event| {
            aiwatcher_execution::evolve(state, event)
        })
}

fn names(outputs: &[aiwatcher_execution::PendingMessage]) -> Vec<&str> {
    outputs
        .iter()
        .map(|message| message.message.name())
        .collect()
}

/// The rows the Flow step hands on, as a reactor would report them.
fn rows() -> ArtifactRef {
    ArtifactRef::new("rows", "s3://curation/exec-1/read/rows", "cd".repeat(32))
        .of_kind(ArtifactKind::Rows)
}

/// Start the gated chain and take it to the point where it is waiting.
fn waiting_at_the_gate() -> ExecutionState {
    let plan =
        compile_curation(&gated_pipeline(), CompileOptions::default()).expect("a gated plan");
    let mut state = aiwatcher_execution::initial_state();

    let outputs = decide(&state, &start(plan), &cause("start"), now()).expect("start");
    state = apply(state, &outputs);

    let read_done = WorkflowMessage::Event(WorkflowEvent::StepCompleted {
        step_id: "read".to_owned(),
        attempt: 1,
        outputs: vec![rows()],
        result: None,
    });
    let outputs = decide(&state, &read_done, &cause("read-done"), now()).expect("the query ran");
    apply(state, &outputs)
}

#[test]
fn an_authored_gate_stops_the_run_and_dispatches_nothing() {
    let state = waiting_at_the_gate();
    let execution = state.active().expect("an execution");
    let gate = execution.step("sign-off").expect("the gate");

    assert_eq!(gate.state.state_type, StateType::AwaitingInput);
    // The question somebody drew, reaching the person who has to answer it
    // without anything in between re-deciding what it says.
    let asked = gate.awaiting.as_ref().expect("a question");
    assert_eq!(asked.prompt, "Publish these rows?");
    assert_eq!(asked.role, "editor");
    assert_eq!(asked.choices, vec!["approve", "reject"]);
    // A gate waits, and nothing sets a clock on it: the answer is the only
    // thing that moves this run on.
    assert!(asked.deadline.is_none());
    // And the run has not gone on without it: `Scheduled` is a step nothing
    // has dispatched, which is what a canvas draws as waiting its turn.
    assert_eq!(
        execution
            .step("publish")
            .expect("the publish step")
            .state
            .state_type,
        StateType::Scheduled
    );
}

#[test]
fn the_step_after_a_gate_reads_the_rows_the_gate_never_produced() {
    // The claim this whole file exists for. `publish` is bound to `read`,
    // which is two hops back, so a resolution that walked parents rather than
    // naming the step would hand the publisher nothing — and a dataset version
    // over no rows is the one failure that looks like a success.
    let state = waiting_at_the_gate();
    let execution = state.active().expect("an execution");

    let publish = execution.plan.step("publish").expect("the publish step");
    assert_eq!(
        publish.inputs,
        vec![InputBinding::Step {
            step: "read".to_owned(),
            output: "rows".to_owned(),
        }]
    );
    assert_eq!(execution.resolved_inputs("publish"), vec![rows()]);
    // The gate itself names what is being decided about, which is the same
    // artifact: a question with no subject is a question nobody can answer.
    assert_eq!(execution.resolved_inputs("sign-off"), vec![rows()]);
}

#[test]
fn answering_the_gate_completes_it_and_the_chain_goes_on() {
    let state = waiting_at_the_gate();

    let outputs = decide(
        &state,
        &WorkflowMessage::Command(WorkflowCommand::ProvideInput {
            step_id: "sign-off".to_owned(),
            attempt: 1,
            answered_by: "somebody".to_owned(),
            response: serde_json::json!("approve"),
        }),
        &cause("answer"),
        now(),
    )
    .expect("an answer the gate offered");

    // Answering *is* the completion — there is nothing else for a wait to do —
    // and the step behind it is dispatched in the same decision.
    assert_eq!(
        names(&outputs),
        vec![
            "input_provided",
            "step_completed",
            "step_scheduled",
            "execute_step"
        ]
    );
    let state = apply(state, &outputs);
    let execution = state.active().expect("an execution");
    assert_eq!(
        execution
            .step("sign-off")
            .expect("the gate")
            .state
            .state_type,
        StateType::Completed
    );
    // `Pending` rather than `Scheduled`: the answer dispatched it, and what it
    // is waiting for now is a reactor rather than a person.
    let publish = execution.step("publish").expect("the publish step");
    assert_eq!(publish.state.state_type, StateType::Pending);
    assert!(
        matches!(
            execution
                .plan
                .step("publish")
                .map(|step| step.runtime.kind()),
            Some(RuntimeKind::PublishDataset)
        ),
        "the step the answer released is the one that publishes"
    );
}

#[test]
fn an_answer_the_gate_never_offered_is_refused_and_the_run_stays_where_it_was() {
    let state = waiting_at_the_gate();

    let refusal = decide(
        &state,
        &WorkflowMessage::Command(WorkflowCommand::ProvideInput {
            step_id: "sign-off".to_owned(),
            attempt: 1,
            answered_by: "somebody".to_owned(),
            response: serde_json::json!("approve with edits"),
        }),
        &cause("answer"),
        now(),
    );

    assert!(refusal.is_err(), "the choices are the whole answer");
    assert_eq!(
        state
            .active()
            .expect("an execution")
            .step("sign-off")
            .expect("the gate")
            .state
            .state_type,
        StateType::AwaitingInput
    );
}

#[test]
fn the_gate_is_the_authored_block_the_canvas_lights() {
    // The panel is told which authored block each step covers, from the pinned
    // plan and never from the draft on screen. A gate whose binding named no
    // block would leave that box dark for exactly the time it is the only
    // thing the run is waiting on.
    let plan =
        compile_curation(&gated_pipeline(), CompileOptions::default()).expect("a gated plan");

    assert_eq!(plan.definition_kind, DefinitionKind::CurationPipeline);
    let gate = plan.step("sign-off").expect("the gate");
    assert!(matches!(gate.runtime, RuntimeBinding::HumanInput(_)));
    assert_eq!(
        plan.blocks_by_step()
            .iter()
            .find(|entry| entry.step_id == "sign-off")
            .map(|entry| entry.blocks.as_slice()),
        Some(["sign-off".to_owned()].as_slice())
    );
}
