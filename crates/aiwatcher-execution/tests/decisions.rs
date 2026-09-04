#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Given some events, when a command, then these messages.
//!
//! Every scenario here runs without a store, a socket or a clock, which is the
//! whole reason `decide` is shaped the way it is. What each one is checking is
//! a rule somebody would otherwise have to discover in production: that a
//! redelivery does not double-schedule, that a retry budget is spent rather
//! than looped on, that a cancel does not become a failure, and that a step
//! nobody can run stops the run rather than hanging it.

use std::collections::BTreeMap;

use aiwatcher_core::MessageId;
use aiwatcher_execution::plan::{
    CachePolicy, DefinitionKind, DefinitionRevision, HumanInputSpec, PlanEdge, PlanStep,
    PythonTaskSpec, RetryPolicy, RuntimeBinding,
};
use aiwatcher_execution::state::ExecutionState;
use aiwatcher_execution::{
    DecisionError, ExecutionId, ExecutionMode, ExecutionOwner, ExecutionPlan, FailureClass, Now,
    RuntimeKind, StateType, StepError, WorkflowCommand, WorkflowEvent, WorkflowMessage, decide,
    replay,
};
use time::OffsetDateTime;

fn now() -> Now {
    Now::at(OffsetDateTime::UNIX_EPOCH)
}

fn task(id: &str) -> PlanStep {
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

fn plan(steps: Vec<PlanStep>, edges: Vec<PlanEdge>) -> ExecutionPlan {
    ExecutionPlan::seal(
        DefinitionKind::Workflow,
        "import".to_owned(),
        DefinitionRevision("ab".repeat(32)),
        steps,
        edges,
    )
}

fn chain_of_two() -> ExecutionPlan {
    plan(
        vec![task("extract"), task("load")],
        vec![PlanEdge {
            from: "extract".to_owned(),
            to: "load".to_owned(),
        }],
    )
}

fn start(plan: ExecutionPlan) -> WorkflowMessage {
    WorkflowMessage::Command(WorkflowCommand::StartExecution {
        execution_id: ExecutionId::new("exec-1"),
        plan: Box::new(plan),
        owner: ExecutionOwner::Local,
        mode: ExecutionMode::Compiled,
        requested_by: "somebody".to_owned(),
        input: BTreeMap::new(),
    })
}

/// Fold every event a decision produced into the state it produced them from.
fn apply(state: ExecutionState, outputs: &[aiwatcher_execution::PendingMessage]) -> ExecutionState {
    outputs
        .iter()
        .filter_map(|message| message.message.event())
        .fold(state, |state, event| {
            aiwatcher_execution::evolve(state, event)
        })
}

fn cause(name: &str) -> MessageId {
    MessageId::new(name)
}

fn names(outputs: &[aiwatcher_execution::PendingMessage]) -> Vec<&'static str> {
    outputs
        .iter()
        .map(|message| message.message.name())
        .collect()
}

/// Start, then run to the end. The state after each decision is what the next
/// one is given, which is how a real caller works.
fn run_to(plan: ExecutionPlan, inputs: Vec<WorkflowMessage>) -> ExecutionState {
    let mut state = aiwatcher_execution::initial_state();
    let outputs = decide(&state, &start(plan), &cause("start"), now()).expect("start");
    state = apply(state, &outputs);
    for (index, input) in inputs.into_iter().enumerate() {
        let outputs = decide(&state, &input, &cause(&format!("in-{index}")), now())
            .unwrap_or_else(|error| panic!("input {index}: {error}"));
        state = apply(state, &outputs);
    }
    state
}

#[test]
fn starting_a_chain_schedules_only_the_step_with_no_parent() {
    // The second step exists in the plan and is not dispatched: a DAG's whole
    // point is that a step waits for what feeds it, and the panel draws the
    // rest as `Pending` rather than as nothing.
    let outputs = decide(
        &aiwatcher_execution::initial_state(),
        &start(chain_of_two()),
        &cause("start"),
        now(),
    )
    .expect("a two-step chain");

    assert_eq!(
        names(&outputs),
        vec![
            "execution_requested",
            "execution_started",
            "step_scheduled",
            "execute_step"
        ]
    );
    let scheduled: Vec<&str> = outputs
        .iter()
        .filter_map(|message| match message.message.event() {
            Some(WorkflowEvent::StepScheduled { step_id, .. }) => Some(step_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(scheduled, vec!["extract"]);
}

#[test]
fn a_dispatch_carries_the_key_a_reactor_asks_the_runtime_by() {
    let outputs = decide(
        &aiwatcher_execution::initial_state(),
        &start(chain_of_two()),
        &cause("start"),
        now(),
    )
    .expect("a start");
    let dispatch = outputs
        .iter()
        .find_map(|message| message.message.command())
        .expect("one dispatch");
    let WorkflowCommand::ExecuteStep {
        idempotency_key,
        runtime,
        attempt,
        ..
    } = dispatch
    else {
        panic!("the command is a dispatch");
    };
    // `<execution>/<step>/<attempt>`: a timeout proves nothing about the
    // runtime, so this is what the reactor asks by before it retries.
    assert_eq!(idempotency_key, "exec-1/extract/1");
    assert_eq!(*runtime, RuntimeKind::PythonTask);
    assert_eq!(*attempt, 1);
}

#[test]
fn completing_the_first_step_dispatches_the_second_and_completing_that_ends_the_run() {
    let state = run_to(
        chain_of_two(),
        vec![
            WorkflowMessage::Event(WorkflowEvent::StepCompleted {
                step_id: "extract".to_owned(),
                attempt: 1,
                outputs: Vec::new(),
                result: None,
            }),
            WorkflowMessage::Event(WorkflowEvent::StepCompleted {
                step_id: "load".to_owned(),
                attempt: 1,
                outputs: Vec::new(),
                result: None,
            }),
        ],
    );
    let execution = state.active().expect("an execution");
    assert_eq!(execution.state.state_type, StateType::Completed);
}

#[test]
fn a_redelivered_completion_schedules_nothing_a_second_time() {
    // At-least-once delivery is the contract, so this is not an edge case: the
    // same completion arrives twice whenever a reactor's acknowledgement is
    // lost, and a second dispatch of the next step would run it twice.
    let mut state = aiwatcher_execution::initial_state();
    let outputs = decide(&state, &start(chain_of_two()), &cause("start"), now()).expect("start");
    state = apply(state, &outputs);

    let completion = WorkflowMessage::Event(WorkflowEvent::StepCompleted {
        step_id: "extract".to_owned(),
        attempt: 1,
        outputs: Vec::new(),
        result: None,
    });
    let first = decide(&state, &completion, &cause("done"), now()).expect("the first delivery");
    state = apply(state, &first);
    assert!(names(&first).contains(&"execute_step"));

    let again = decide(&state, &completion, &cause("done-again"), now())
        .expect("a redelivery is not an error");
    assert!(
        again.is_empty(),
        "a redelivery produced {:?}",
        names(&again)
    );
}

#[test]
fn a_transient_failure_is_retried_with_the_delay_the_policy_names() {
    let mut state = aiwatcher_execution::initial_state();
    let outputs = decide(&state, &start(chain_of_two()), &cause("start"), now()).expect("start");
    state = apply(state, &outputs);

    let failure = WorkflowMessage::Event(WorkflowEvent::StepFailed {
        step_id: "extract".to_owned(),
        attempt: 1,
        error: StepError::new(FailureClass::Transient, "connection reset"),
    });
    let outputs = decide(&state, &failure, &cause("fail"), now()).expect("a transient failure");
    assert_eq!(
        names(&outputs),
        vec!["step_failed", "step_retry_scheduled", "execute_step"]
    );

    let scheduled = outputs
        .iter()
        .find_map(|message| match message.message.event() {
            Some(WorkflowEvent::StepRetryScheduled { not_before, .. }) => Some(*not_before),
            _ => None,
        })
        .expect("a retry with a time on it");
    // Resolved to an instant here rather than left as a duration, so replay
    // reaches the same schedule instead of one relative to when it replayed.
    assert_eq!(
        scheduled,
        OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(1)
    );
}

#[test]
fn a_parse_error_is_not_retried_and_the_run_fails_saying_which_step() {
    // A deterministic user-code failure will be just as broken on the third
    // attempt, and spending the budget on it only delays the message.
    let mut state = aiwatcher_execution::initial_state();
    let outputs = decide(&state, &start(chain_of_two()), &cause("start"), now()).expect("start");
    state = apply(state, &outputs);

    let outputs = decide(
        &state,
        &WorkflowMessage::Event(WorkflowEvent::StepFailed {
            step_id: "extract".to_owned(),
            attempt: 1,
            error: StepError::new(FailureClass::UserCode, "unexpected token"),
        }),
        &cause("fail"),
        now(),
    )
    .expect("a user-code failure");

    assert_eq!(
        names(&outputs),
        vec!["step_failed", "step_skipped", "execution_failed"]
    );
    let reason = outputs
        .iter()
        .find_map(|message| match message.message.event() {
            Some(WorkflowEvent::ExecutionFailed { reason }) => Some(reason.clone()),
            _ => None,
        })
        .expect("a reason");
    assert!(reason.contains("extract"), "{reason}");
}

#[test]
fn the_retry_budget_runs_out_rather_than_looping() {
    let mut state = aiwatcher_execution::initial_state();
    let outputs = decide(&state, &start(chain_of_two()), &cause("start"), now()).expect("start");
    state = apply(state, &outputs);

    let mut last = Vec::new();
    for attempt in 1..=RetryPolicy::default().max_attempts {
        let outputs = decide(
            &state,
            &WorkflowMessage::Event(WorkflowEvent::StepFailed {
                step_id: "extract".to_owned(),
                attempt,
                error: StepError::new(FailureClass::Transient, "reset"),
            }),
            &cause(&format!("fail-{attempt}")),
            now(),
        )
        .expect("a transient failure");
        state = apply(state, &outputs);
        last = names(&outputs);
    }
    assert_eq!(
        last,
        vec!["step_failed", "step_skipped", "execution_failed"],
        "the third failure spends the budget rather than scheduling a fourth"
    );
}

#[test]
fn an_expired_lease_crashes_the_attempt_rather_than_failing_it() {
    // Nobody's code gave a wrong answer. A run whose only red step says
    // "failed" when a node was drained sends somebody to read a log that says
    // nothing.
    let state = run_to(
        chain_of_two(),
        vec![WorkflowMessage::Event(WorkflowEvent::StepFailed {
            step_id: "extract".to_owned(),
            attempt: 1,
            error: StepError::new(FailureClass::Infrastructure, "the lease expired"),
        })],
    );
    let execution = state.active().expect("an execution");
    let attempt = execution
        .step("extract")
        .and_then(|step| step.attempt(1))
        .expect("attempt 1");
    assert_eq!(attempt.state.state_type, StateType::Crashed);
    // And it is retried, because the work was lost rather than answered.
    assert_eq!(execution.step("extract").unwrap().current_attempt, 2);
}

#[test]
fn a_report_about_a_lost_attempt_is_refused_by_name() {
    // Two workers, one attempt: the one whose lease expired comes back with an
    // answer about work somebody else has taken over.
    let state = run_to(
        chain_of_two(),
        vec![WorkflowMessage::Event(WorkflowEvent::StepFailed {
            step_id: "extract".to_owned(),
            attempt: 1,
            error: StepError::new(FailureClass::Infrastructure, "the lease expired"),
        })],
    );
    let error = decide(
        &state,
        &WorkflowMessage::Event(WorkflowEvent::StepCompleted {
            step_id: "extract".to_owned(),
            attempt: 1,
            outputs: Vec::new(),
            result: None,
        }),
        &cause("late"),
        now(),
    )
    .expect_err("a completion from the worker that lost its lease");
    assert!(
        matches!(
            error,
            DecisionError::StaleAttempt {
                attempt: 1,
                current: 2,
                ..
            }
        ),
        "{error}"
    );
}

#[test]
fn cancelling_stops_what_is_queued_and_waits_for_what_is_running() {
    // Cooperative first: the running step is asked to stop, and the run does
    // not report itself cancelled until it has.
    let mut state = aiwatcher_execution::initial_state();
    let outputs = decide(&state, &start(chain_of_two()), &cause("start"), now()).expect("start");
    state = apply(state, &outputs);
    let outputs = decide(
        &state,
        &WorkflowMessage::Event(WorkflowEvent::StepStarted {
            step_id: "extract".to_owned(),
            attempt: 1,
        }),
        &cause("started"),
        now(),
    )
    .expect("a start report");
    state = apply(state, &outputs);

    let outputs = decide(
        &state,
        &WorkflowMessage::Command(WorkflowCommand::CancelExecution {
            reason: "somebody asked".to_owned(),
        }),
        &cause("cancel"),
        now(),
    )
    .expect("a cancel");
    assert_eq!(
        names(&outputs),
        vec!["execution_cancelling", "step_skipped"]
    );
    state = apply(state, &outputs);

    // The running step comes back; now there is nothing left to wait for.
    let outputs = decide(
        &state,
        &WorkflowMessage::Event(WorkflowEvent::StepCompleted {
            step_id: "extract".to_owned(),
            attempt: 1,
            outputs: Vec::new(),
            result: None,
        }),
        &cause("done"),
        now(),
    )
    .expect("the last report");
    assert!(names(&outputs).contains(&"execution_cancelled"));
    assert!(
        !names(&outputs).contains(&"execute_step"),
        "a cancelled run does not dispatch the next step"
    );
}

#[test]
fn a_cancelled_run_does_not_also_report_itself_failed() {
    // Two different answers to "why did this stop", and giving both would make
    // a deliberate stop look like a fault in every list that counts failures.
    let mut state = aiwatcher_execution::initial_state();
    let outputs = decide(&state, &start(chain_of_two()), &cause("start"), now()).expect("start");
    state = apply(state, &outputs);
    let outputs = decide(
        &state,
        &WorkflowMessage::Event(WorkflowEvent::StepStarted {
            step_id: "extract".to_owned(),
            attempt: 1,
        }),
        &cause("started"),
        now(),
    )
    .expect("a start report");
    state = apply(state, &outputs);
    let outputs = decide(
        &state,
        &WorkflowMessage::Command(WorkflowCommand::CancelExecution {
            reason: String::new(),
        }),
        &cause("cancel"),
        now(),
    )
    .expect("a cancel");
    state = apply(state, &outputs);

    let outputs = decide(
        &state,
        &WorkflowMessage::Event(WorkflowEvent::StepFailed {
            step_id: "extract".to_owned(),
            attempt: 1,
            error: StepError::new(FailureClass::Policy, "cancelled"),
        }),
        &cause("stopped"),
        now(),
    )
    .expect("the step stopping");
    assert!(names(&outputs).contains(&"execution_cancelled"));
    assert!(!names(&outputs).contains(&"execution_failed"));
}

#[test]
fn pausing_holds_the_next_dispatch_and_resuming_releases_it() {
    let mut state = aiwatcher_execution::initial_state();
    let outputs = decide(&state, &start(chain_of_two()), &cause("start"), now()).expect("start");
    state = apply(state, &outputs);

    let outputs = decide(
        &state,
        &WorkflowMessage::Command(WorkflowCommand::PauseExecution),
        &cause("pause"),
        now(),
    )
    .expect("a pause");
    state = apply(state, &outputs);

    let outputs = decide(
        &state,
        &WorkflowMessage::Event(WorkflowEvent::StepCompleted {
            step_id: "extract".to_owned(),
            attempt: 1,
            outputs: Vec::new(),
            result: None,
        }),
        &cause("done"),
        now(),
    )
    .expect("a completion while paused");
    assert!(
        !names(&outputs).contains(&"execute_step"),
        "a paused run does not dispatch"
    );
    state = apply(state, &outputs);

    let outputs = decide(
        &state,
        &WorkflowMessage::Command(WorkflowCommand::ResumeExecution),
        &cause("resume"),
        now(),
    )
    .expect("a resume");
    assert_eq!(
        names(&outputs),
        vec!["execution_resumed", "step_scheduled", "execute_step"]
    );
}

#[test]
fn a_human_step_waits_rather_than_being_dispatched_anywhere() {
    let plan = plan(
        vec![PlanStep {
            runtime: RuntimeBinding::HumanInput(HumanInputSpec {
                prompt: "promote this model?".to_owned(),
                role: "admin".to_owned(),
                choices: vec!["yes".to_owned(), "no".to_owned()],
            }),
            retry: RetryPolicy::once(),
            ..task("approve")
        }],
        Vec::new(),
    );
    let outputs = decide(
        &aiwatcher_execution::initial_state(),
        &start(plan),
        &cause("start"),
        now(),
    )
    .expect("a plan with one wait");
    assert_eq!(
        names(&outputs),
        vec![
            "execution_requested",
            "execution_started",
            "step_scheduled",
            "input_requested",
            "request_input"
        ]
    );
    assert!(
        !names(&outputs).contains(&"execute_step"),
        "nobody runs a wait"
    );
}

#[test]
fn an_answer_the_step_did_not_offer_is_refused_and_the_right_one_completes_it() {
    let plan = plan(
        vec![PlanStep {
            runtime: RuntimeBinding::HumanInput(HumanInputSpec {
                prompt: "promote?".to_owned(),
                role: "admin".to_owned(),
                choices: vec!["yes".to_owned(), "no".to_owned()],
            }),
            retry: RetryPolicy::once(),
            ..task("approve")
        }],
        Vec::new(),
    );
    let mut state = aiwatcher_execution::initial_state();
    let outputs = decide(&state, &start(plan), &cause("start"), now()).expect("start");
    state = apply(state, &outputs);

    let error = decide(
        &state,
        &WorkflowMessage::Command(WorkflowCommand::ProvideInput {
            step_id: "approve".to_owned(),
            attempt: 1,
            answered_by: "mk".to_owned(),
            response: serde_json::json!("maybe"),
        }),
        &cause("answer"),
        now(),
    )
    .expect_err("an answer that was not offered");
    assert!(
        matches!(error, DecisionError::NotOneOfTheChoices { .. }),
        "{error}"
    );

    let outputs = decide(
        &state,
        &WorkflowMessage::Command(WorkflowCommand::ProvideInput {
            step_id: "approve".to_owned(),
            attempt: 1,
            answered_by: "mk".to_owned(),
            response: serde_json::json!("yes"),
        }),
        &cause("answer"),
        now(),
    )
    .expect("the answer the step offered");
    assert_eq!(
        names(&outputs),
        vec!["input_provided", "step_completed", "execution_completed"]
    );
}

#[test]
fn a_command_addressed_at_an_execution_nobody_started_says_so() {
    let error = decide(
        &aiwatcher_execution::initial_state(),
        &WorkflowMessage::Command(WorkflowCommand::PauseExecution),
        &cause("pause"),
        now(),
    )
    .expect_err("a pause before a start");
    assert!(matches!(error, DecisionError::NotStarted { .. }), "{error}");
}

#[test]
fn starting_twice_is_refused_rather_than_giving_one_execution_two_plans() {
    let mut state = aiwatcher_execution::initial_state();
    let outputs = decide(&state, &start(chain_of_two()), &cause("start"), now()).expect("start");
    state = apply(state, &outputs);
    let error = decide(&state, &start(chain_of_two()), &cause("start-2"), now())
        .expect_err("a second start");
    assert_eq!(error, DecisionError::AlreadyStarted);
}

#[test]
fn an_empty_plan_and_a_cyclic_one_are_refused_before_anything_is_recorded() {
    let empty = plan(Vec::new(), Vec::new());
    assert_eq!(
        decide(
            &aiwatcher_execution::initial_state(),
            &start(empty),
            &cause("start"),
            now()
        )
        .expect_err("an empty plan"),
        DecisionError::EmptyPlan
    );

    let cyclic = plan(
        vec![task("one"), task("two")],
        vec![
            PlanEdge {
                from: "one".to_owned(),
                to: "two".to_owned(),
            },
            PlanEdge {
                from: "two".to_owned(),
                to: "one".to_owned(),
            },
        ],
    );
    assert_eq!(
        decide(
            &aiwatcher_execution::initial_state(),
            &start(cyclic),
            &cause("start"),
            now()
        )
        .expect_err("a cycle"),
        DecisionError::CyclicPlan
    );
}

#[test]
fn a_fan_out_dispatches_both_branches_and_the_join_waits_for_both() {
    // A chain is the first authoring model and the plan is a graph from the
    // start, so that an agent or ML workflow needs no second execution record.
    let plan = plan(
        vec![task("read"), task("left"), task("right"), task("merge")],
        vec![
            PlanEdge {
                from: "read".to_owned(),
                to: "left".to_owned(),
            },
            PlanEdge {
                from: "read".to_owned(),
                to: "right".to_owned(),
            },
            PlanEdge {
                from: "left".to_owned(),
                to: "merge".to_owned(),
            },
            PlanEdge {
                from: "right".to_owned(),
                to: "merge".to_owned(),
            },
        ],
    );
    let mut state = aiwatcher_execution::initial_state();
    let outputs = decide(&state, &start(plan), &cause("start"), now()).expect("start");
    state = apply(state, &outputs);

    let outputs = decide(
        &state,
        &WorkflowMessage::Event(WorkflowEvent::StepCompleted {
            step_id: "read".to_owned(),
            attempt: 1,
            outputs: Vec::new(),
            result: None,
        }),
        &cause("read-done"),
        now(),
    )
    .expect("the source completing");
    let dispatched: Vec<String> = outputs
        .iter()
        .filter_map(|message| match message.message.command() {
            Some(WorkflowCommand::ExecuteStep { step_id, .. }) => Some(step_id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(dispatched, vec!["left", "right"]);
    state = apply(state, &outputs);

    let outputs = decide(
        &state,
        &WorkflowMessage::Event(WorkflowEvent::StepCompleted {
            step_id: "left".to_owned(),
            attempt: 1,
            outputs: Vec::new(),
            result: None,
        }),
        &cause("left-done"),
        now(),
    )
    .expect("one branch completing");
    assert!(
        !names(&outputs).contains(&"execute_step"),
        "the join waits for the other branch"
    );
    state = apply(state, &outputs);

    let outputs = decide(
        &state,
        &WorkflowMessage::Event(WorkflowEvent::StepCompleted {
            step_id: "right".to_owned(),
            attempt: 1,
            outputs: Vec::new(),
            result: None,
        }),
        &cause("right-done"),
        now(),
    )
    .expect("the other branch completing");
    assert!(names(&outputs).contains(&"execute_step"));
}

#[test]
fn a_step_nobody_can_run_stops_the_branch_below_it_and_nothing_else() {
    let plan = plan(
        vec![task("read"), task("left"), task("right"), task("merge")],
        vec![
            PlanEdge {
                from: "read".to_owned(),
                to: "left".to_owned(),
            },
            PlanEdge {
                from: "read".to_owned(),
                to: "right".to_owned(),
            },
            PlanEdge {
                from: "left".to_owned(),
                to: "merge".to_owned(),
            },
        ],
    );
    let mut state = aiwatcher_execution::initial_state();
    let outputs = decide(&state, &start(plan), &cause("start"), now()).expect("start");
    state = apply(state, &outputs);

    let outputs = decide(
        &state,
        &WorkflowMessage::Event(WorkflowEvent::StepFailed {
            step_id: "read".to_owned(),
            attempt: 1,
            error: StepError::new(FailureClass::Validation, "no such dataset"),
        }),
        &cause("fail"),
        now(),
    )
    .expect("a validation failure");
    let skipped: Vec<String> = outputs
        .iter()
        .filter_map(|message| match message.message.event() {
            Some(WorkflowEvent::StepSkipped { step_id, .. }) => Some(step_id.clone()),
            _ => None,
        })
        .collect();
    // Each named once, however many paths reach it.
    assert_eq!(skipped, vec!["left", "right", "merge"]);
}

#[test]
fn replaying_a_stream_reaches_the_state_the_decisions_left_behind() {
    // The property every recovery depends on: the state is a fold of the
    // stream, so a restart mid-run resumes rather than restarting.
    let mut state = aiwatcher_execution::initial_state();
    let mut every_event = Vec::new();
    let outputs = decide(&state, &start(chain_of_two()), &cause("start"), now()).expect("start");
    every_event.extend(outputs.iter().filter_map(|m| m.message.event().cloned()));
    state = apply(state, &outputs);

    for step in ["extract", "load"] {
        let outputs = decide(
            &state,
            &WorkflowMessage::Event(WorkflowEvent::StepCompleted {
                step_id: step.to_owned(),
                attempt: 1,
                outputs: Vec::new(),
                result: None,
            }),
            &cause(step),
            now(),
        )
        .expect("a completion");
        every_event.extend(outputs.iter().filter_map(|m| m.message.event().cloned()));
        state = apply(state, &outputs);
    }

    let replayed = replay(every_event.iter());
    assert_eq!(replayed, state);
    assert_eq!(
        replayed.active().expect("an execution").state.state_type,
        StateType::Completed
    );
}
