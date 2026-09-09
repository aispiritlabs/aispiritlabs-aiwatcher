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

use aiwatcher_core::human_input::OnTimeout;
use aiwatcher_core::{ArtifactKind, ArtifactRef, MessageId};
use aiwatcher_datasets::{BlockPosition, BlockSpec, CurationPipeline, PipelineBlock, PipelineEdge};
use aiwatcher_execution::decide::ANSWERED_BY_TIMEOUT;
use aiwatcher_execution::handler::deadline_rows;
use aiwatcher_execution::hosted::TimerWrite;
use aiwatcher_execution::message::MessageMetadata;
use aiwatcher_execution::plan::{DefinitionKind, InputBinding, RuntimeBinding};
use aiwatcher_execution::state::ExecutionState;
use aiwatcher_execution::store::memory::MemoryWorkflowStore;
use aiwatcher_execution::{
    CompileOptions, ExecutionId, ExecutionMode, ExecutionOwner, ExecutionPlan, Now, RuntimeKind,
    StateType, WorkflowCommand, WorkflowEvent, WorkflowMessage, compile_curation, decide,
};
use aiwatcher_execution::{ExecutionHandler, WorkflowStore};
use time::{Duration, OffsetDateTime};

/// The instant every scenario here starts at.
const NOW: OffsetDateTime = OffsetDateTime::UNIX_EPOCH;

fn now() -> Now {
    Now::at(NOW)
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
                timeout_seconds: None,
                on_timeout: Default::default(),
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

/// The timer arriving on the question, which is what the tick sends.
fn lapsed() -> WorkflowMessage {
    WorkflowMessage::Command(WorkflowCommand::TimeoutInput {
        step_id: "sign-off".to_owned(),
        attempt: 1,
    })
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

/// The same chain, with a clock on the gate and a policy behind it.
fn gate_with_a_deadline(on_timeout: OnTimeout) -> CurationPipeline {
    let mut pipeline = gated_pipeline();
    if let BlockSpec::Approval {
        timeout_seconds,
        on_timeout: policy,
        ..
    } = &mut pipeline.blocks[1].spec
    {
        *timeout_seconds = Some(3600);
        *policy = on_timeout;
    }
    pipeline
}

/// Start a gated chain with a deadline and take it to the question.
fn waiting_with_a_deadline(on_timeout: OnTimeout) -> ExecutionState {
    let plan = compile_curation(&gate_with_a_deadline(on_timeout), CompileOptions::default())
        .expect("a gated plan");
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

/// The deadline the run is holding, and the timer row it produced.
fn deadline_of(state: &ExecutionState) -> (OffsetDateTime, Vec<TimerWrite>) {
    let plan = compile_curation(
        &gate_with_a_deadline(OnTimeout::Fail),
        CompileOptions::default(),
    )
    .expect("a gated plan");
    let mut fresh = aiwatcher_execution::initial_state();
    let started = decide(&fresh, &start(plan), &cause("start"), now()).expect("start");
    fresh = apply(fresh, &started);
    let outputs = decide(
        &fresh,
        &WorkflowMessage::Event(WorkflowEvent::StepCompleted {
            step_id: "read".to_owned(),
            attempt: 1,
            outputs: vec![rows()],
            result: None,
        }),
        &cause("read-done"),
        now(),
    )
    .expect("the query ran");
    let rows = deadline_rows(
        &ExecutionId::new("exec-1"),
        &apply(fresh.clone(), &outputs),
        &outputs,
    );
    let asked = state
        .active()
        .expect("an execution")
        .step("sign-off")
        .expect("the gate")
        .awaiting
        .as_ref()
        .expect("a question")
        .deadline
        .expect("a deadline");
    (asked, rows)
}

#[test]
fn a_deadline_is_resolved_when_the_question_is_asked_and_kept_as_one_row() {
    // Resolved from the clock that arrives in the input, so a replay reaches
    // the same instant — `TraceId::derive`'s rule for a moment rather than an
    // id. The row follows the fact rather than a decision of its own, which is
    // why `decide` needs no vocabulary for a timer.
    let state = waiting_with_a_deadline(OnTimeout::Fail);
    let (deadline, rows) = deadline_of(&state);

    assert_eq!(deadline, OffsetDateTime::UNIX_EPOCH + Duration::hours(1));
    let [TimerWrite::Schedule(timer)] = rows.as_slice() else {
        panic!("one timer, scheduled: {rows:?}");
    };
    // Names the attempt as well as the step: a retry asks the question again,
    // and the row the first attempt left must not fire on the second.
    assert_eq!(timer.timer_id, "input/sign-off/1");
    assert_eq!(timer.due_at, deadline);
}

#[test]
fn answering_in_time_retires_the_deadline_in_the_same_decision() {
    // Otherwise the clock is still coming for a question that is over. The
    // handler derives the retirement from the step ending, so nothing has to
    // remember to cancel it.
    let state = waiting_with_a_deadline(OnTimeout::Fail);
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
    .expect("an answer in time");

    let rows = deadline_rows(
        &ExecutionId::new("exec-1"),
        &apply(state.clone(), &outputs),
        &outputs,
    );
    assert!(
        rows.iter()
            .any(|row| matches!(row, TimerWrite::Cancel(id) if id == "input/sign-off/1")),
        "{rows:?}"
    );
    assert!(
        !rows
            .iter()
            .any(|row| matches!(row, TimerWrite::Schedule(_))),
        "{rows:?}"
    );
}

#[test]
fn a_deadline_that_runs_out_fails_the_run_when_that_is_what_the_gate_asked_for() {
    let state = waiting_with_a_deadline(OnTimeout::Fail);

    let outputs = decide(&state, &lapsed(), &cause("timeout"), now()).expect("the deadline");

    assert_eq!(
        names(&outputs),
        vec!["step_failed", "step_skipped", "execution_failed"]
    );
    let state = apply(state, &outputs);
    let execution = state.active().expect("an execution");
    assert_eq!(
        execution
            .step("sign-off")
            .expect("the gate")
            .state
            .state_type,
        StateType::Failed
    );
    // Everything after it is skipped, as for any failure: the safe reading of
    // "nobody said yes" is that what came next must not happen either.
    assert_eq!(
        execution.step("publish").expect("publish").state.state_type,
        StateType::Cancelled
    );
}

#[test]
fn a_deadline_that_runs_out_may_pass_the_step_over_instead() {
    let state = waiting_with_a_deadline(OnTimeout::Skip);

    let outputs = decide(&state, &lapsed(), &cause("timeout"), now()).expect("the deadline");

    // No `input_provided`: the history says the question was asked and never
    // answered, and the chain went on anyway. A run that recorded an answer
    // here would be claiming somebody made a decision.
    assert_eq!(
        names(&outputs),
        vec!["step_completed", "step_scheduled", "execute_step"]
    );
}

#[test]
fn a_deadline_may_answer_its_own_question_and_says_who_did() {
    let state = waiting_with_a_deadline(OnTimeout::Answer {
        response: serde_json::json!("reject"),
    });

    let outputs = decide(&state, &lapsed(), &cause("timeout"), now()).expect("the deadline");

    let answered = outputs
        .iter()
        .filter_map(|message| match message.message.event() {
            Some(WorkflowEvent::InputProvided {
                answered_by,
                response,
                ..
            }) => Some((answered_by.clone(), response.clone())),
            _ => None,
        })
        .next()
        .expect("an answer");
    // Attributed to the policy and not to a person: the one record of a human
    // decision has to say when there was not one.
    assert_eq!(answered.0, ANSWERED_BY_TIMEOUT);
    assert_eq!(answered.1, serde_json::json!("reject"));
    assert!(
        names(&outputs).contains(&"execute_step"),
        "the chain goes on"
    );
}

#[test]
fn a_deadline_that_arrives_after_the_answer_is_refused_rather_than_applied() {
    // The ordinary race, and the answer wins. The caller reads this refusal as
    // "retire the row", because a timer that outlived its question is not due
    // — it is over.
    let state = waiting_with_a_deadline(OnTimeout::Fail);
    let answered = decide(
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
    .expect("an answer in time");
    let state = apply(state, &answered);

    let refusal = decide(&state, &lapsed(), &cause("timeout"), now());

    assert!(refusal.is_err(), "the question is over");
}

/// Everything one tick needs: a store, a handler, and a run parked on a gate.
async fn parked_in_a_store(
    on_timeout: OnTimeout,
) -> ExecutionHandler<std::sync::Arc<MemoryWorkflowStore>> {
    let handler = ExecutionHandler::new(std::sync::Arc::new(MemoryWorkflowStore::default()));
    let execution = ExecutionId::new("exec-1");
    let plan = compile_curation(&gate_with_a_deadline(on_timeout), CompileOptions::default())
        .expect("a gated plan");
    for (index, input) in [
        start(plan),
        WorkflowMessage::Event(WorkflowEvent::StepCompleted {
            step_id: "read".to_owned(),
            attempt: 1,
            outputs: vec![rows()],
            result: None,
        }),
    ]
    .into_iter()
    .enumerate()
    {
        let id = MessageId::new(format!("in-{index}"));
        handler
            .handle(
                &execution,
                input,
                MessageMetadata::caused_by(&execution, &id, id.clone(), NOW),
                Now::at(NOW),
            )
            .await
            .expect("the run reaches its gate");
    }
    handler
}

#[tokio::test]
async fn the_tick_delivers_a_lapsed_deadline_to_the_run_that_is_waiting() {
    // The whole point of a row in a table: the browser is closed, the reactor
    // has nothing to claim, and the only thing that moves this run on is
    // something waking up and looking.
    let handler = parked_in_a_store(OnTimeout::Skip).await;
    let execution = ExecutionId::new("exec-1");

    // Nothing is due a minute in, and everything is due two hours in.
    assert!(
        handler
            .fire_due_timers(NOW + Duration::minutes(1), 10)
            .await
            .expect("a pass")
            .is_empty(),
        "a deadline an hour away is not due yet"
    );
    let fired = handler
        .fire_due_timers(NOW + Duration::hours(2), 10)
        .await
        .expect("a pass");

    assert_eq!(fired.len(), 1);
    assert!(fired[0].delivered, "the run was waiting for it");
    let run = handler
        .store()
        .projection(&execution)
        .await
        .expect("a store")
        .expect("a run");
    let gate = run
        .steps
        .iter()
        .find(|step| step.step_id == "sign-off")
        .expect("the gate");
    assert_eq!(gate.state.state_type, StateType::Completed);

    // And the row is gone with it: a timer that fires on every tick for ever
    // is the failure this is one transaction to avoid.
    assert!(
        handler
            .fire_due_timers(NOW + Duration::hours(3), 10)
            .await
            .expect("a pass")
            .is_empty(),
        "the row went with the decision that consumed it"
    );
}

#[tokio::test]
async fn a_deadline_the_answer_beat_is_retired_rather_than_left_due_for_ever() {
    let handler = parked_in_a_store(OnTimeout::Fail).await;
    let execution = ExecutionId::new("exec-1");
    let id = MessageId::new("answer");
    handler
        .handle(
            &execution,
            WorkflowMessage::Command(WorkflowCommand::ProvideInput {
                step_id: "sign-off".to_owned(),
                attempt: 1,
                answered_by: "somebody".to_owned(),
                response: serde_json::json!("approve"),
            }),
            MessageMetadata::caused_by(&execution, &id, id.clone(), NOW),
            Now::at(NOW),
        )
        .await
        .expect("an answer in time");

    // Cancelled by the decision that answered it, so the tick finds nothing.
    let fired = handler
        .fire_due_timers(NOW + Duration::hours(2), 10)
        .await
        .expect("a pass");

    assert!(fired.is_empty(), "{fired:?}");
    let run = handler
        .store()
        .projection(&execution)
        .await
        .expect("a store")
        .expect("a run");
    assert_eq!(
        run.steps
            .iter()
            .find(|step| step.step_id == "sign-off")
            .expect("the gate")
            .state
            .state_type,
        StateType::Completed
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
