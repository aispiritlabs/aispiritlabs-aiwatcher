#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! An attempt that was already running when it stopped to ask.
//!
//! The other kind of gate is authored: a `HumanInput` step is the question and
//! nothing else, the decider parks it at the moment it schedules it, and it
//! reaches the claim table never. `curation_gate.rs` is that one.
//!
//! This is the other half — a worker holding a lease that stops in the
//! middle of its own work, because a tool call wants approving. Three things
//! separate it from the authored gate and each is a claim here: the row exists
//! and has to be released without being ended, the answer is one *input* to the
//! rest of the work rather than the work's result, and the policy for nobody
//! answering came from the question rather than from the plan.

use std::collections::BTreeMap;

use aiwatcher_core::MessageId;
use aiwatcher_core::human_input::OnTimeout;
use aiwatcher_execution::claim::{AttemptKey, AttemptWrite};
use aiwatcher_execution::handler::{attempt_rows, deadline_rows};
use aiwatcher_execution::hosted::TimerWrite;
use aiwatcher_execution::plan::{
    CachePolicy, DefinitionKind, DefinitionRevision, HumanInputSpec, PlanEdge, PlanStep,
    PythonTaskSpec, RetryPolicy, RuntimeBinding,
};
use aiwatcher_execution::state::{ExecutionState, InputRequest, StateType};
use aiwatcher_execution::{
    ExecutionId, ExecutionMode, ExecutionOwner, ExecutionPlan, Now, PendingMessage,
    WorkflowCommand, WorkflowEvent, WorkflowMessage, decide,
};
use time::{Duration, OffsetDateTime};

const NOW: OffsetDateTime = OffsetDateTime::UNIX_EPOCH;

fn now() -> Now {
    Now::at(NOW)
}

fn cause(name: &str) -> MessageId {
    MessageId::new(name)
}

fn execution() -> ExecutionId {
    ExecutionId::new("exec-1")
}

fn task(id: &str) -> PlanStep {
    PlanStep {
        id: id.to_owned(),
        runtime: RuntimeBinding::PythonTask(PythonTaskSpec {
            task_ref: "agent@1".to_owned(),
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

fn plan(steps: Vec<PlanStep>, edges: Vec<PlanEdge>) -> ExecutionPlan {
    ExecutionPlan::seal(
        DefinitionKind::Workflow,
        "agent".to_owned(),
        DefinitionRevision("ab".repeat(32)),
        steps,
        edges,
    )
}

fn start(plan: ExecutionPlan) -> WorkflowMessage {
    WorkflowMessage::Command(WorkflowCommand::StartExecution {
        execution_id: execution(),
        plan: Box::new(plan),
        owner: ExecutionOwner::Local,
        mode: ExecutionMode::Compiled,
        payloads: Default::default(),
        requested_by: "somebody".to_owned(),
        input: BTreeMap::new(),
    })
}

fn apply(state: ExecutionState, outputs: &[PendingMessage]) -> ExecutionState {
    outputs
        .iter()
        .filter_map(|message| message.message.event())
        .fold(state, |state, event| {
            aiwatcher_execution::evolve(state, event)
        })
}

fn names(outputs: &[PendingMessage]) -> Vec<&str> {
    outputs
        .iter()
        .map(|message| message.message.name())
        .collect()
}

/// The question a worker reports, as the result route turns it into a fact.
fn question(deadline: Option<OffsetDateTime>, on_timeout: OnTimeout) -> InputRequest {
    InputRequest {
        prompt: "May I send this email?".to_owned(),
        role: "editor".to_owned(),
        choices: vec!["approve".to_owned(), "reject".to_owned()],
        deadline,
        on_timeout: Some(on_timeout),
    }
}

/// A worker's park, arriving as the reactor's `settle` reports it.
fn parks(step: &str, attempt: u32, request: InputRequest) -> WorkflowMessage {
    WorkflowMessage::Event(WorkflowEvent::InputRequested {
        step_id: step.to_owned(),
        attempt,
        request,
    })
}

fn answers(step: &str, attempt: u32, response: &str) -> WorkflowMessage {
    WorkflowMessage::Command(WorkflowCommand::ProvideInput {
        step_id: step.to_owned(),
        attempt,
        answered_by: "mkubasz@gmail.com".to_owned(),
        response: serde_json::json!(response),
    })
}

/// One `agent` step, running, with its first attempt claimed.
fn running() -> ExecutionState {
    let mut state = aiwatcher_execution::initial_state();
    let outputs = decide(
        &state,
        &start(plan(vec![task("agent")], vec![])),
        &cause("start"),
        now(),
    )
    .expect("start");
    state = apply(state, &outputs);

    let started = WorkflowMessage::Event(WorkflowEvent::StepStarted {
        step_id: "agent".to_owned(),
        attempt: 1,
    });
    let outputs = decide(&state, &started, &cause("started"), now()).expect("a worker took it");
    apply(state, &outputs)
}

/// The same, taken to the point where the worker has asked.
fn parked() -> (ExecutionState, Vec<PendingMessage>) {
    let state = running();
    let outputs = decide(
        &state,
        &parks("agent", 1, question(None, OnTimeout::Fail)),
        &cause("asked"),
        now(),
    )
    .expect("a worker may stop to ask");
    let state = apply(state, &outputs);
    (state, outputs)
}

#[test]
fn a_worker_that_stopped_to_ask_releases_its_row_without_ending_it() {
    // The shape the claim table reserved and nothing ever wrote. A park is
    // neither of the other two: `Retire` would lose the attempt that asked, and
    // leaving the row dispatched would let the *next* claimant run the work
    // again five minutes later, having been told nothing about the question.
    let (state, outputs) = parked();
    let rows = attempt_rows(&execution(), &state, &outputs);

    assert_eq!(
        rows,
        vec![AttemptWrite::Park(AttemptKey::new(execution(), "agent", 1))],
        "the row stays and the lease goes"
    );
}

#[test]
fn an_authored_gate_writes_no_claim_row_because_it_never_had_one() {
    // The narrowing that keeps the park cheap. A `HumanInput` step is parked by
    // the decider when it schedules it and reaches the claim table never —
    // `schedule_attempt` emits `RequestInput`, and only `ExecuteStep` writes a
    // row. A park emitted for it would be a write per gate per transaction that
    // the `file` adapter pays for by rewriting its table.
    let gate = PlanStep {
        id: "sign-off".to_owned(),
        runtime: RuntimeBinding::HumanInput(HumanInputSpec {
            prompt: "Publish?".to_owned(),
            role: "editor".to_owned(),
            choices: vec!["approve".to_owned()],
            block: None,
            timeout_seconds: None,
            on_timeout: OnTimeout::Fail,
        }),
        inputs: Vec::new(),
        outputs: Vec::new(),
        retry: RetryPolicy::default(),
        timeout_seconds: 60,
        cache: CachePolicy::Never,
    };
    let state = aiwatcher_execution::initial_state();
    let outputs = decide(
        &state,
        &start(plan(vec![gate], vec![])),
        &cause("start"),
        now(),
    )
    .expect("start");
    let state = apply(state, &outputs);

    assert!(
        names(&outputs).contains(&"input_requested"),
        "the decider parks it at the moment it schedules it"
    );
    assert!(
        attempt_rows(&execution(), &state, &outputs).is_empty(),
        "and writes nothing to a table it was never in"
    );
}

#[test]
fn the_answer_starts_a_new_attempt_instead_of_completing_the_step() {
    // The difference from an authored gate. A `HumanInput` step
    // *is* the question, so answering completes it. This step stopped in the
    // middle of its own work: the answer is one input to the rest of it, so the
    // work goes on in attempt two and the attempt that asked stays immutable.
    let (state, _) = parked();
    let outputs = decide(
        &state,
        &answers("agent", 1, "approve"),
        &cause("answered"),
        now(),
    )
    .expect("an answer");

    assert_eq!(
        names(&outputs),
        vec!["input_provided", "step_scheduled", "execute_step"],
        "the answer is recorded and the work is dispatched again"
    );
    assert!(
        !names(&outputs).contains(&"step_completed"),
        "answering is not this step's result"
    );

    let state = apply(state, &outputs);
    let run = state.active().expect("an execution");
    let step = run.step("agent").expect("the step");
    assert_eq!(step.current_attempt, 2);
    assert_eq!(step.state.state_type, StateType::Pending);
    assert!(step.awaiting.is_none(), "the question has been answered");

    // The parked row is retired and the new attempt dispatched, in one
    // decision. A park keeps its row because a question is not an ending; an
    // answer *is* one, and leaving it behind would grow the claim table by a
    // row per park for ever — the table growing with the history, which is what
    // retiring a finished attempt exists to prevent.
    let rows = attempt_rows(&execution(), &state, &outputs);
    assert_eq!(
        rows,
        vec![
            AttemptWrite::Retire(AttemptKey::new(execution(), "agent", 1)),
            AttemptWrite::Dispatch(
                aiwatcher_execution::claim::AttemptRow::claimable(
                    AttemptKey::new(execution(), "agent", 2),
                    aiwatcher_execution::RuntimeKind::PythonTask,
                    rows.iter()
                        .find_map(|write| match write {
                            AttemptWrite::Dispatch(row) => Some(row.command_id.clone()),
                            AttemptWrite::Retire(_) | AttemptWrite::Park(_) => None,
                        })
                        .expect("a dispatch"),
                )
                .on_queue("default".to_owned(), "agent@1".to_owned())
            ),
        ]
    );

    // The attempt that asked is still readable, with what happened to it.
    let asked = step.attempt(1).expect("the attempt that asked");
    assert_eq!(asked.state.state_type, StateType::AwaitingInput);
}

#[test]
fn the_resumed_attempt_carries_the_answer_the_previous_one_asked_for() {
    // Why the answer is kept on the *step*. Attempt two re-runs the work from
    // the beginning, so it reaches the same question again — and without the
    // answer in front of it, it would park a second time for ever.
    let (state, _) = parked();
    let outputs = decide(
        &state,
        &answers("agent", 1, "approve"),
        &cause("answered"),
        now(),
    )
    .expect("an answer");
    let state = apply(state, &outputs);

    let step = state
        .active()
        .expect("a run")
        .step("agent")
        .expect("the step")
        .clone();
    assert_eq!(step.answers.len(), 1);
    assert_eq!(step.answers[0].attempt, 1, "whose question was answered");
    assert_eq!(step.answers[0].answered_by, "mkubasz@gmail.com");
    assert_eq!(step.answers[0].response, serde_json::json!("approve"));
}

#[test]
fn a_task_that_asks_twice_keeps_both_answers() {
    // The reason this is a list. Attempt two replays to the first question,
    // reads the answer it already has, goes on and asks a second. Attempt three
    // then needs *both*: given only the last it would park on the first
    // question again, and the run would never move.
    let (state, _) = parked();
    let outputs = decide(
        &state,
        &answers("agent", 1, "approve"),
        &cause("first"),
        now(),
    )
    .expect("an answer");
    let state = apply(state, &outputs);

    let outputs = decide(
        &state,
        &parks("agent", 2, question(None, OnTimeout::Fail)),
        &cause("asked-again"),
        now(),
    )
    .expect("the second question");
    let state = apply(state, &outputs);

    let outputs = decide(
        &state,
        &answers("agent", 2, "reject"),
        &cause("second"),
        now(),
    )
    .expect("an answer");
    let state = apply(state, &outputs);

    let step = state
        .active()
        .expect("a run")
        .step("agent")
        .expect("the step")
        .clone();
    assert_eq!(step.current_attempt, 3);
    assert_eq!(
        step.answers
            .iter()
            .map(|answer| (answer.attempt, answer.response.clone()))
            .collect::<Vec<_>>(),
        vec![
            (1, serde_json::json!("approve")),
            (2, serde_json::json!("reject")),
        ],
        "oldest first, which is the order a replay asks them in"
    );
}

#[test]
fn an_answered_step_is_not_cached() {
    // A human decision is addressed by nothing, so a step that has one no
    // longer has all its inputs addressed — the guardrail's own rule. Two runs
    // of one step answered differently would share a key, and the second would
    // be served the first one's rows without ever seeing its own answer.
    let cached = PlanStep {
        cache: CachePolicy::ByContent,
        ..task("agent")
    };
    let mut state = aiwatcher_execution::initial_state();
    let outputs = decide(
        &state,
        &start(plan(vec![cached], vec![])),
        &cause("start"),
        now(),
    )
    .expect("start");
    state = apply(state, &outputs);
    let first = state
        .active()
        .expect("a run")
        .step("agent")
        .expect("the step")
        .cache_key
        .clone();
    assert!(
        first.is_some(),
        "an unanswered step is cacheable as authored"
    );

    let started = WorkflowMessage::Event(WorkflowEvent::StepStarted {
        step_id: "agent".to_owned(),
        attempt: 1,
    });
    let outputs = decide(&state, &started, &cause("started"), now()).expect("claimed");
    state = apply(state, &outputs);
    let outputs = decide(
        &state,
        &parks("agent", 1, question(None, OnTimeout::Fail)),
        &cause("asked"),
        now(),
    )
    .expect("asked");
    state = apply(state, &outputs);
    let outputs = decide(
        &state,
        &answers("agent", 1, "approve"),
        &cause("answered"),
        now(),
    )
    .expect("answered");
    state = apply(state, &outputs);

    assert!(
        state
            .active()
            .expect("a run")
            .step("agent")
            .expect("the step")
            .cache_key
            .is_none(),
        "and an answered one is not"
    );
}

// ── The deadline, proved rather than assumed ─────────────────────────────────

#[test]
fn a_parked_attempt_with_a_deadline_gets_a_timer_row() {
    // The kickoff said this might follow for free. The scheduling half does:
    // the row is derived from `InputRequested` carrying a deadline, and a
    // worker's park emits the same fact.
    let state = running();
    let request = question(Some(NOW + Duration::hours(1)), OnTimeout::Fail);
    let outputs =
        decide(&state, &parks("agent", 1, request), &cause("asked"), now()).expect("asked");
    let state = apply(state, &outputs);

    let timers = deadline_rows(&execution(), &state, &outputs);
    assert!(
        matches!(
            timers.as_slice(),
            [TimerWrite::Schedule(timer)] if timer.due_at == NOW + Duration::hours(1)
        ),
        "one row, due when the question said: {timers:?}"
    );
}

#[test]
fn answering_a_parked_attempt_retires_its_deadline() {
    // The cancelling half did *not* follow for free, and this is what says so.
    // `has_deadline` asked the plan whether the step was an authored gate with
    // a clock — true of nothing a worker parks, because the worker authored the
    // question and the plan says nothing about it. The row would have been left
    // behind, still coming for a question that was over.
    let state = running();
    let request = question(Some(NOW + Duration::hours(1)), OnTimeout::Fail);
    let outputs =
        decide(&state, &parks("agent", 1, request), &cause("asked"), now()).expect("asked");
    let state = apply(state, &outputs);

    let outputs = decide(
        &state,
        &answers("agent", 1, "approve"),
        &cause("answered"),
        now(),
    )
    .expect("answered");
    let timers = deadline_rows(&execution(), &apply(state, &outputs), &outputs);

    assert!(
        matches!(timers.as_slice(), [TimerWrite::Cancel(_)]),
        "the question is over, so its clock is too: {timers:?}"
    );
}

#[test]
fn a_lapsed_deadline_reads_the_policy_the_question_carried() {
    // The plan pinned this step's *code*, and a `PythonTask` spec has no
    // `on_timeout` — there is no authored gate here to read one from. Before
    // the policy moved onto the question, this arm could not find one and
    // refused the timer as `NoSuchStep`, which left the run parked for ever
    // with a deadline that had already passed.
    let state = running();
    let request = question(Some(NOW + Duration::hours(1)), OnTimeout::Skip);
    let outputs =
        decide(&state, &parks("agent", 1, request), &cause("asked"), now()).expect("asked");
    let state = apply(state, &outputs);

    let lapsed = WorkflowMessage::Command(WorkflowCommand::TimeoutInput {
        step_id: "agent".to_owned(),
        attempt: 1,
    });
    let later = Now::at(NOW + Duration::hours(2));
    let outputs = decide(&state, &lapsed, &cause("lapsed"), later).expect("the timer is due");

    // `skip` completes the step with no `InputProvided`: the history says the
    // question was asked and never answered. A run that recorded an answer
    // there would be claiming somebody made a decision.
    assert!(names(&outputs).contains(&"step_completed"));
    assert!(
        !names(&outputs).contains(&"input_provided"),
        "nobody answered, so nobody is recorded as having"
    );

    let step = apply(state, &outputs)
        .active()
        .expect("a run")
        .step("agent")
        .expect("the step")
        .clone();
    assert_eq!(step.state.state_type, StateType::Completed);
    assert!(
        step.answers.is_empty(),
        "a timeout is not an answer somebody gave"
    );
}

#[test]
fn a_timeout_that_answers_a_parked_attempt_resumes_it_like_any_answer() {
    // `answer` records one, attributed to the timeout rather than to a person —
    // and for a step that was already running, the answer is still an input to
    // the rest of the work rather than its result. The same rule as a person's
    // answer, reached by the other door.
    let state = running();
    let request = question(
        Some(NOW + Duration::hours(1)),
        OnTimeout::Answer {
            response: serde_json::json!("reject"),
        },
    );
    let outputs =
        decide(&state, &parks("agent", 1, request), &cause("asked"), now()).expect("asked");
    let state = apply(state, &outputs);

    let lapsed = WorkflowMessage::Command(WorkflowCommand::TimeoutInput {
        step_id: "agent".to_owned(),
        attempt: 1,
    });
    let later = Now::at(NOW + Duration::hours(2));
    let outputs = decide(&state, &lapsed, &cause("lapsed"), later).expect("the timer is due");
    let step = apply(state, &outputs)
        .active()
        .expect("a run")
        .step("agent")
        .expect("the step")
        .clone();

    assert_eq!(
        step.answers
            .first()
            .map(|answer| answer.answered_by.as_str()),
        Some(aiwatcher_execution::decide::ANSWERED_BY_TIMEOUT),
        "the one record of a human decision says when there was not one"
    );

    // The half this test asserted only in its name. The timeout's `answer` arm
    // completed the step outright: attempt one, `Completed`, no outputs — the
    // work stopped where the question was and everything bound to its rows read
    // nothing. Both doors go through `answer_lands` now.
    assert_eq!(
        names(&outputs),
        vec!["input_provided", "step_scheduled", "execute_step"],
        "an answer nobody gave is still an answer, and resumes the work"
    );
    assert_eq!(step.current_attempt, 2);
    assert_eq!(step.state.state_type, StateType::Pending);
}

#[test]
fn a_lapsed_deadline_that_answers_an_authored_gate_still_completes_it() {
    // The other side of the same helper, and the reason it is a `match` on the
    // plan rather than one rule. A `HumanInput` step *is* the question: there is
    // no work behind it to resume, so an answer — a person's or a timeout's —
    // completes it and the chain goes on.
    let gate = PlanStep {
        id: "sign-off".to_owned(),
        runtime: RuntimeBinding::HumanInput(HumanInputSpec {
            prompt: "Publish?".to_owned(),
            role: "editor".to_owned(),
            choices: Vec::new(),
            block: None,
            timeout_seconds: Some(3600),
            on_timeout: OnTimeout::Answer {
                response: serde_json::json!("approve"),
            },
        }),
        inputs: Vec::new(),
        outputs: Vec::new(),
        retry: RetryPolicy::default(),
        timeout_seconds: 60,
        cache: CachePolicy::Never,
    };
    let state = aiwatcher_execution::initial_state();
    let outputs = decide(
        &state,
        &start(plan(vec![gate], vec![])),
        &cause("start"),
        now(),
    )
    .expect("start");
    let state = apply(state, &outputs);

    let lapsed = WorkflowMessage::Command(WorkflowCommand::TimeoutInput {
        step_id: "sign-off".to_owned(),
        attempt: 1,
    });
    let later = Now::at(NOW + Duration::hours(2));
    let outputs = decide(&state, &lapsed, &cause("lapsed"), later).expect("the timer is due");

    assert!(names(&outputs).contains(&"step_completed"));
    assert!(
        !names(&outputs).contains(&"step_scheduled"),
        "there is no work behind a gate to resume"
    );
    let step = apply(state, &outputs)
        .active()
        .expect("a run")
        .step("sign-off")
        .expect("the gate")
        .clone();
    assert_eq!(step.state.state_type, StateType::Completed);
    assert_eq!(step.current_attempt, 1);
}

#[test]
fn a_hosted_run_records_the_answer_and_schedules_nothing() {
    // A hosted run's worker chooses its own next node, so recording the answer
    // is the whole of this engine's part in it. Scheduling one here would be
    // the engine and the worker both deciding what runs next — what
    // `dispatch_ready` returns early to prevent, reached through a second door.
    let mut run = match parked().0 {
        ExecutionState::Active(execution) => *execution,
        ExecutionState::Empty => panic!("a run"),
    };
    run.mode = ExecutionMode::Hosted;
    let state = ExecutionState::Active(Box::new(run));

    let outputs = decide(
        &state,
        &answers("agent", 1, "approve"),
        &cause("answered"),
        now(),
    )
    .expect("an answer");

    assert_eq!(
        names(&outputs),
        vec!["input_provided"],
        "the history is kept and the next node is the worker's"
    );
}
