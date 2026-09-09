//! The pure part: `initial_state`, `decide`, `evolve`.
//!
//! Emmett's Decider, and the three rules that make it worth the shape.
//!
//! **`decide` performs no I/O.** It reads no wall clock, opens no socket and
//! generates no random value. Time arrives in [`Now`]; ids are *derived* from
//! what they name, so two replays of one decision produce the same command id
//! and a redelivered dispatch lands on the attempt it already created rather
//! than beside it — the same rule as `TraceId::derive`, for the same reason.
//!
//! **`evolve` never fails.** It folds a fact that already happened. A fold that
//! could reject its input would be a second decision point, and the two would
//! disagree the first time somebody changed one.
//!
//! **Every output is caused by the input.** The stream holds both, so a
//! decision is explainable afterwards by reading in one direction.
//!
//! What this file therefore does *not* do: call Flow, upload anything, ask a
//! store whether a lease is still held, or decide that a run has died. The last
//! is the projector's rule kept here too — a step with no completion is running
//! until its lease expires, and an expired lease arrives as an input.

use std::collections::BTreeMap;

use aiwatcher_core::{CausationId, CorrelationId, MessageId};
use time::OffsetDateTime;

use crate::error::DecisionError;
use crate::message::{
    Direction, MessageMetadata, PendingMessage, WorkflowCommand, WorkflowEvent, WorkflowMessage,
};
use crate::plan::RuntimeBinding;
use crate::state::{
    AttemptRecord, Execution, ExecutionMode, ExecutionState, FailureClass, InputRequest, RunState,
    StateType, StepState,
};

/// Everything a decision needs that is not the state or the command.
///
/// One struct rather than three arguments, so that adding "and the resolved
/// policy" later is a field rather than a signature change in every caller —
/// and so that the list of things a decision is allowed to depend on is
/// readable in one place.
#[derive(Clone, Copy, Debug)]
pub struct Now {
    pub at: OffsetDateTime,
}

impl Now {
    #[must_use]
    pub const fn at(at: OffsetDateTime) -> Self {
        Self { at }
    }
}

/// The starting point: a stream nobody has written to.
#[must_use]
pub fn initial_state() -> ExecutionState {
    ExecutionState::Empty
}

/// Fold one fact into the state. Never fails, and ignores what it does not
/// recognise — a stream written by a newer build is missing information here,
/// not wrong.
#[must_use]
pub fn evolve(state: ExecutionState, event: &WorkflowEvent) -> ExecutionState {
    match (state, event) {
        (
            ExecutionState::Empty,
            WorkflowEvent::ExecutionRequested {
                execution_id,
                plan,
                owner,
                mode,
                requested_by,
                input,
            },
        ) => {
            let steps = plan
                .steps
                .iter()
                .map(|step| {
                    (
                        step.id.clone(),
                        StepState::fresh(step.id.clone(), step.runtime.kind()),
                    )
                })
                .collect();
            ExecutionState::Active(Box::new(Execution {
                execution_id: execution_id.clone(),
                plan: (**plan).clone(),
                owner: owner.clone(),
                mode: *mode,
                requested_by: requested_by.clone(),
                input: input.clone(),
                state: RunState::named(StateType::Scheduled, "Queued"),
                steps,
                cancelling: false,
                created_at: OffsetDateTime::UNIX_EPOCH,
            }))
        }
        // A second `ExecutionRequested`, or anything at all before the first,
        // is a stream that is not what it claims to be. The append's duplicate
        // check is what stops the first; the second is left alone rather than
        // guessed at.
        (state @ ExecutionState::Empty, _) => state,
        (ExecutionState::Active(mut execution), event) => {
            apply(&mut execution, event);
            ExecutionState::Active(execution)
        }
    }
}

fn apply(execution: &mut Execution, event: &WorkflowEvent) {
    match event {
        WorkflowEvent::ExecutionRequested { .. } => {}
        WorkflowEvent::ExecutionStarted => {
            execution.state = RunState::of(StateType::Running);
        }
        WorkflowEvent::StepScheduled {
            step_id,
            attempt,
            cache_key,
            ..
        } => {
            if let Some(step) = execution.step_mut(step_id) {
                step.state = RunState::of(StateType::Pending);
                step.current_attempt = *attempt;
                step.cache_key.clone_from(cache_key);
                step.attempts.push(AttemptRecord {
                    attempt: *attempt,
                    state: RunState::of(StateType::Pending),
                    error: None,
                    not_before: None,
                });
            }
        }
        WorkflowEvent::StepRetryScheduled {
            step_id,
            attempt,
            not_before,
        } => {
            if let Some(step) = execution.step_mut(step_id) {
                step.state = RunState::named(StateType::Pending, "AwaitingRetry");
                step.current_attempt = *attempt;
                step.attempts.push(AttemptRecord {
                    attempt: *attempt,
                    state: RunState::named(StateType::Pending, "AwaitingRetry"),
                    error: None,
                    not_before: Some(*not_before),
                });
            }
        }
        WorkflowEvent::StepStarted { step_id, attempt } => {
            if let Some(step) = execution.step_mut(step_id) {
                step.state = RunState::of(StateType::Running);
                if let Some(record) = step.attempts.iter_mut().find(|r| r.attempt == *attempt) {
                    record.state = RunState::of(StateType::Running);
                }
            }
            // The run's own state is not touched here. `ExecutionStarted` is
            // emitted unconditionally by the start and already set it, so the
            // only states this could reach are the ones it has moved to
            // *since* — `Cancelling` and `Paused` — and a step reporting
            // itself started is not news that either has stopped. It is news
            // that a reactor claimed something that was dispatched before the
            // cancel or the pause landed, which is ordinary.
        }
        WorkflowEvent::StepCompleted {
            step_id,
            attempt,
            outputs,
            ..
        } => {
            if let Some(step) = execution.step_mut(step_id) {
                step.state = RunState::of(StateType::Completed);
                step.outputs.clone_from(outputs);
                step.awaiting = None;
                if let Some(record) = step.attempts.iter_mut().find(|r| r.attempt == *attempt) {
                    record.state = RunState::of(StateType::Completed);
                }
            }
        }
        WorkflowEvent::StepCacheHit {
            step_id,
            attempt,
            cache_key,
            outputs,
        } => {
            if let Some(step) = execution.step_mut(step_id) {
                step.state = RunState::named(StateType::Completed, "Cached");
                step.cache_key = Some(cache_key.clone());
                step.outputs.clone_from(outputs);
                if let Some(record) = step.attempts.iter_mut().find(|r| r.attempt == *attempt) {
                    record.state = RunState::named(StateType::Completed, "Cached");
                }
            }
        }
        WorkflowEvent::StepFailed {
            step_id,
            attempt,
            error,
        } => {
            let attempt_state = error.class.attempt_state();
            if let Some(step) = execution.step_mut(step_id) {
                step.state = RunState::of(attempt_state);
                step.awaiting = None;
                if let Some(record) = step.attempts.iter_mut().find(|r| r.attempt == *attempt) {
                    record.state = RunState::of(attempt_state);
                    record.error = Some(error.clone());
                }
            }
        }
        WorkflowEvent::StepSkipped { step_id, reason } => {
            if let Some(step) = execution.step_mut(step_id) {
                step.state = RunState::named(StateType::Cancelled, reason);
            }
        }
        WorkflowEvent::InputRequested {
            step_id,
            attempt,
            request,
        } => {
            if let Some(step) = execution.step_mut(step_id) {
                step.state = RunState::of(StateType::AwaitingInput);
                step.awaiting = Some(request.clone());
                if let Some(record) = step.attempts.iter_mut().find(|r| r.attempt == *attempt) {
                    record.state = RunState::of(StateType::AwaitingInput);
                }
            }
        }
        WorkflowEvent::InputProvided { step_id, .. } => {
            if let Some(step) = execution.step_mut(step_id) {
                step.awaiting = None;
            }
        }
        WorkflowEvent::ExecutionPaused => {
            execution.state = RunState::of(StateType::Paused);
        }
        WorkflowEvent::ExecutionResumed => {
            execution.state = RunState::of(StateType::Running);
        }
        WorkflowEvent::ExecutionCancelling { reason } => {
            execution.cancelling = true;
            execution.state = RunState::named(StateType::Running, "Cancelling");
            let _ = reason;
        }
        WorkflowEvent::ExecutionCancelled => {
            execution.state = RunState::of(StateType::Cancelled);
        }
        WorkflowEvent::ExecutionCompleted => {
            execution.state = RunState::of(StateType::Completed);
        }
        WorkflowEvent::ExecutionFailed { reason } => {
            execution.state = RunState::named(StateType::Failed, reason);
        }
    }
}

/// Fold a whole stream. What a store's `load` hands the decider.
#[must_use]
pub fn replay<'a>(events: impl IntoIterator<Item = &'a WorkflowEvent>) -> ExecutionState {
    events.into_iter().fold(initial_state(), evolve)
}

/// What one input produced: the outputs, and nothing else.
///
/// A `Vec` rather than a struct with `events` and `commands` split, because the
/// order between them matters — `StepScheduled` before the `ExecuteStep` it
/// authorises — and two lists would lose it.
pub type Decision = Vec<PendingMessage>;

/// Decide what one input means. Pure.
///
/// # Errors
///
/// [`DecisionError`] when the command does not apply: an execution that has not
/// started, one that already finished, a step the plan does not have, a retry
/// budget that is spent, or an answer from the wrong role.
pub fn decide(
    state: &ExecutionState,
    input: &WorkflowMessage,
    cause: &MessageId,
    now: Now,
) -> Result<Decision, DecisionError> {
    match (state, input) {
        (
            ExecutionState::Empty,
            WorkflowMessage::Command(WorkflowCommand::StartExecution {
                execution_id,
                plan,
                owner,
                mode,
                requested_by,
                input,
            }),
        ) => {
            if plan.steps.is_empty() {
                return Err(DecisionError::EmptyPlan);
            }
            if !plan.is_acyclic() {
                return Err(DecisionError::CyclicPlan);
            }
            let mut emit = Emitter::new(execution_id.as_str(), cause, now);
            emit.event(WorkflowEvent::ExecutionRequested {
                execution_id: execution_id.clone(),
                plan: plan.clone(),
                owner: owner.clone(),
                mode: *mode,
                requested_by: requested_by.clone(),
                input: input.clone(),
            });
            emit.event(WorkflowEvent::ExecutionStarted);

            // The state the rest of this decision reasons against is the one
            // after the two facts it just produced, not the one it was handed.
            let started = evolve(
                evolve(
                    initial_state(),
                    &WorkflowEvent::ExecutionRequested {
                        execution_id: execution_id.clone(),
                        plan: plan.clone(),
                        owner: owner.clone(),
                        mode: *mode,
                        requested_by: requested_by.clone(),
                        input: input.clone(),
                    },
                ),
                &WorkflowEvent::ExecutionStarted,
            );
            if let Some(execution) = started.active() {
                dispatch_ready(execution, &mut emit, now);
            }
            Ok(emit.into_messages())
        }
        (ExecutionState::Empty, message) => Err(DecisionError::NotStarted {
            message: message.name().to_owned(),
        }),
        (ExecutionState::Active(execution), message) => {
            decide_active(execution, message, cause, now)
        }
    }
}

fn decide_active(
    execution: &Execution,
    input: &WorkflowMessage,
    cause: &MessageId,
    now: Now,
) -> Result<Decision, DecisionError> {
    let mut emit = Emitter::new(execution.execution_id.as_str(), cause, now);
    match input {
        WorkflowMessage::Command(WorkflowCommand::StartExecution { .. }) => {
            Err(DecisionError::AlreadyStarted)
        }

        WorkflowMessage::Command(WorkflowCommand::PauseExecution) => {
            if execution.state.state_type.is_terminal() {
                return Err(DecisionError::AlreadyFinished {
                    state: execution.state.state_type,
                });
            }
            if execution.state.state_type == StateType::Paused {
                return Ok(Vec::new());
            }
            emit.event(WorkflowEvent::ExecutionPaused);
            Ok(emit.into_messages())
        }

        WorkflowMessage::Command(WorkflowCommand::ResumeExecution) => {
            if execution.state.state_type != StateType::Paused {
                return Err(DecisionError::NotPaused);
            }
            emit.event(WorkflowEvent::ExecutionResumed);
            let mut resumed = execution.clone();
            resumed.state = RunState::of(StateType::Running);
            dispatch_ready(&resumed, &mut emit, now);
            Ok(emit.into_messages())
        }

        WorkflowMessage::Command(WorkflowCommand::CancelExecution { reason }) => {
            if execution.state.state_type.is_terminal() {
                return Err(DecisionError::AlreadyFinished {
                    state: execution.state.state_type,
                });
            }
            if execution.cancelling {
                return Ok(Vec::new());
            }
            emit.event(WorkflowEvent::ExecutionCancelling {
                reason: reason.clone(),
            });
            // Cooperative: what is already in flight is asked to stop, and
            // everything not yet started stops being scheduled at all. When
            // nothing is running the cancel completes in this one decision.
            let mut running = false;
            for step in execution.steps.values() {
                match step.state.state_type {
                    StateType::Running | StateType::AwaitingInput => running = true,
                    StateType::Scheduled | StateType::Pending => {
                        emit.event(WorkflowEvent::StepSkipped {
                            step_id: step.step_id.clone(),
                            reason: "the execution was cancelled".to_owned(),
                        });
                    }
                    _ => {}
                }
            }
            if !running {
                emit.event(WorkflowEvent::ExecutionCancelled);
            }
            Ok(emit.into_messages())
        }

        WorkflowMessage::Command(WorkflowCommand::RetryStep { step_id }) => {
            let step = step_of(execution, step_id)?;
            if !matches!(
                step.state.state_type,
                StateType::Failed | StateType::Crashed
            ) {
                return Err(DecisionError::NotRetryable {
                    step: step_id.clone(),
                    state: step.state.state_type,
                });
            }
            if execution.cancelling {
                return Err(DecisionError::Cancelling);
            }
            let attempt = step.current_attempt + 1;
            schedule_attempt(execution, step_id, attempt, &mut emit, now);
            Ok(emit.into_messages())
        }

        WorkflowMessage::Command(WorkflowCommand::ProvideInput {
            step_id,
            attempt,
            answered_by,
            response,
        }) => {
            let step = step_of(execution, step_id)?;
            let Some(request) = step.awaiting.as_ref() else {
                return Err(DecisionError::NotWaiting {
                    step: step_id.clone(),
                });
            };
            if step.current_attempt != *attempt {
                return Err(DecisionError::StaleAttempt {
                    step: step_id.clone(),
                    attempt: *attempt,
                    current: step.current_attempt,
                });
            }
            if let Some(deadline) = request.deadline
                && now.at > deadline
            {
                return Err(DecisionError::DeadlinePassed {
                    step: step_id.clone(),
                });
            }
            if !request.choices.is_empty()
                && !response
                    .as_str()
                    .is_some_and(|value| request.choices.iter().any(|choice| choice == value))
            {
                return Err(DecisionError::NotOneOfTheChoices {
                    step: step_id.clone(),
                    choices: request.choices.clone(),
                });
            }
            emit.about_step(step_id, *attempt);
            emit.event(WorkflowEvent::InputProvided {
                step_id: step_id.clone(),
                attempt: *attempt,
                answered_by: answered_by.clone(),
                response: response.clone(),
            });
            // Answering is what completes a `HumanInput` step; there is nothing
            // else for it to do.
            emit.event(WorkflowEvent::StepCompleted {
                step_id: step_id.clone(),
                attempt: *attempt,
                outputs: Vec::new(),
                result: Some(response.clone()),
            });
            continue_after(execution, step_id, &mut emit, now);
            Ok(emit.into_messages())
        }

        WorkflowMessage::Event(WorkflowEvent::StepStarted { step_id, attempt }) => {
            let step = step_of(execution, step_id)?;
            if step.current_attempt != *attempt {
                return Err(DecisionError::StaleAttempt {
                    step: step_id.clone(),
                    attempt: *attempt,
                    current: step.current_attempt,
                });
            }
            emit.about_step(step_id, *attempt);
            emit.event(WorkflowEvent::StepStarted {
                step_id: step_id.clone(),
                attempt: *attempt,
            });
            Ok(emit.into_messages())
        }

        // A runtime that stopped to ask. Distinct from a `HumanInput` step,
        // which the decider parks when it schedules it: this one was already
        // running, and what it is asking for arrived from the executor.
        //
        // What resumes it is the answer, which today completes the step —
        // right for a `HumanInput`, and the half of section 41 that is missing
        // for a turn that wants to *continue* after the answer.
        WorkflowMessage::Event(WorkflowEvent::InputRequested {
            step_id,
            attempt,
            request,
        }) => {
            let step = step_of(execution, step_id)?;
            if step.current_attempt != *attempt {
                return Err(DecisionError::StaleAttempt {
                    step: step_id.clone(),
                    attempt: *attempt,
                    current: step.current_attempt,
                });
            }
            if step.state.state_type.is_terminal() {
                return Err(DecisionError::NotRetryable {
                    step: step_id.clone(),
                    state: step.state.state_type,
                });
            }
            emit.about_step(step_id, *attempt);
            emit.event(WorkflowEvent::InputRequested {
                step_id: step_id.clone(),
                attempt: *attempt,
                request: request.clone(),
            });
            Ok(emit.into_messages())
        }

        // A reactor answered the attempt out of the artifact catalog instead
        // of running it. Handled beside the completion rather than folded into
        // it, because the two are different facts: a hit says the work was not
        // done, and a run that reports one has to stay explainable after the
        // index is dropped (section 18).
        WorkflowMessage::Event(WorkflowEvent::StepCacheHit {
            step_id,
            attempt,
            cache_key,
            outputs,
        }) => {
            let step = step_of(execution, step_id)?;
            if step.state.state_type == StateType::Completed {
                return Ok(Vec::new());
            }
            if step.current_attempt != *attempt {
                return Err(DecisionError::StaleAttempt {
                    step: step_id.clone(),
                    attempt: *attempt,
                    current: step.current_attempt,
                });
            }
            emit.about_step(step_id, *attempt);
            emit.event(WorkflowEvent::StepCacheHit {
                step_id: step_id.clone(),
                attempt: *attempt,
                cache_key: cache_key.clone(),
                outputs: outputs.clone(),
            });
            continue_after(execution, step_id, &mut emit, now);
            Ok(emit.into_messages())
        }

        WorkflowMessage::Event(WorkflowEvent::StepCompleted {
            step_id,
            attempt,
            outputs,
            result,
        }) => {
            let step = step_of(execution, step_id)?;
            if step.state.state_type == StateType::Completed {
                // A redelivered completion. Idempotent by design: the fact is
                // already folded, and re-emitting it would give the step two
                // completions and the log two `step.completed`.
                return Ok(Vec::new());
            }
            if step.current_attempt != *attempt {
                return Err(DecisionError::StaleAttempt {
                    step: step_id.clone(),
                    attempt: *attempt,
                    current: step.current_attempt,
                });
            }
            emit.about_step(step_id, *attempt);
            emit.event(WorkflowEvent::StepCompleted {
                step_id: step_id.clone(),
                attempt: *attempt,
                outputs: outputs.clone(),
                result: result.clone(),
            });
            continue_after(execution, step_id, &mut emit, now);
            Ok(emit.into_messages())
        }

        WorkflowMessage::Event(WorkflowEvent::StepFailed {
            step_id,
            attempt,
            error,
        }) => {
            let step = step_of(execution, step_id)?;
            if step.state.state_type.is_terminal() && step.current_attempt != *attempt {
                return Err(DecisionError::StaleAttempt {
                    step: step_id.clone(),
                    attempt: *attempt,
                    current: step.current_attempt,
                });
            }
            let plan_step =
                execution
                    .plan
                    .step(step_id)
                    .ok_or_else(|| DecisionError::NoSuchStep {
                        step: step_id.clone(),
                    })?;

            emit.about_step(step_id, *attempt);
            emit.event(WorkflowEvent::StepFailed {
                step_id: step_id.clone(),
                attempt: *attempt,
                error: error.clone(),
            });

            let next = attempt + 1;
            // Two budgets, counted by kind rather than by attempt number. A
            // step that waited out a restarting service and then failed on its
            // own terms has spent one of the three attempts at the *work*, not
            // six — and a run that only ever failed to reach its runtime is
            // bounded by the other number rather than by this one.
            let may_have_run = error.class.may_have_run();
            let spent = 1 + step
                .attempts
                .iter()
                .filter_map(|record| record.error.as_ref())
                .filter(|failed| failed.class.may_have_run() == may_have_run)
                .count() as u32;
            let budget = if may_have_run {
                plan_step.retry.max_attempts
            } else {
                plan_step.retry.max_unavailable_attempts
            };
            let may_retry = error.class.is_retryable() && spent < budget && !execution.cancelling;
            if may_retry {
                let delay = plan_step.retry.delay_before(next, !may_have_run);
                let not_before =
                    now.at + time::Duration::try_from(delay).unwrap_or(time::Duration::ZERO);
                emit.event(WorkflowEvent::StepRetryScheduled {
                    step_id: step_id.clone(),
                    attempt: next,
                    not_before,
                });
                emit.command(WorkflowCommand::ExecuteStep {
                    step_id: step_id.clone(),
                    attempt: next,
                    runtime: plan_step.runtime.kind(),
                    idempotency_key: idempotency_key(
                        execution.execution_id.as_str(),
                        step_id,
                        next,
                    ),
                });
                return Ok(emit.into_messages());
            }

            // No retry left. Everything downstream of this step will not run,
            // and the execution has failed — unless a cancel is already what is
            // stopping it, in which case that is the answer being given.
            let mut settled = execution.clone();
            settled.set_attempt_state(step_id, *attempt, RunState::of(error.class.attempt_state()));
            if let Some(step) = settled.step_mut(step_id) {
                step.state = RunState::of(error.class.attempt_state());
            }
            skip_downstream(&settled, step_id, &mut emit);
            if execution.cancelling {
                emit.event(WorkflowEvent::ExecutionCancelled);
            } else {
                emit.event(WorkflowEvent::ExecutionFailed {
                    reason: format!("{step_id} failed: {}", error.message),
                });
            }
            Ok(emit.into_messages())
        }

        // A reactor or the API sending anything else as an input is a caller
        // bug, and one worth naming rather than absorbing.
        other => Err(DecisionError::Unhandled {
            message: other.name().to_owned(),
        }),
    }
}

fn step_of<'a>(execution: &'a Execution, step_id: &str) -> Result<&'a StepState, DecisionError> {
    execution
        .step(step_id)
        .ok_or_else(|| DecisionError::NoSuchStep {
            step: step_id.to_owned(),
        })
}

/// Schedule every step whose parents have all completed and which has not been
/// scheduled yet.
fn dispatch_ready(execution: &Execution, emit: &mut Emitter, now: Now) {
    // A hosted run's plan is its *shape*, not its program
    // ([`ExecutionMode`]'s own words): the worker chooses the next node,
    // because an agent graph's conditions are decided by a model and its join
    // arity is discovered. Scheduling from the shape would put a claimable
    // attempt in front of every reactor for work the worker is also doing —
    // two parties executing one step, which is what section 40.3 splits the
    // decider from the history to prevent. Guarded here rather than at the
    // start, because a resume and a completion reach this too.
    if execution.mode == ExecutionMode::Hosted {
        return;
    }
    if execution.state.state_type == StateType::Paused || execution.cancelling {
        return;
    }
    for step in &execution.plan.steps {
        let Some(current) = execution.step(&step.id) else {
            continue;
        };
        if current.state.state_type != StateType::Scheduled {
            continue;
        }
        let ready = execution.plan.parents_of(&step.id).iter().all(|parent| {
            execution
                .step(parent)
                .is_some_and(|s| s.state.state_type == StateType::Completed)
        });
        if ready {
            schedule_attempt(execution, &step.id, 1, emit, now);
        }
    }
}

/// One step, one attempt: the fact, then the effect that carries it out.
fn schedule_attempt(
    execution: &Execution,
    step_id: &str,
    attempt: u32,
    emit: &mut Emitter,
    _now: Now,
) {
    let Some(plan_step) = execution.plan.step(step_id) else {
        return;
    };
    emit.about_step(step_id, attempt);
    emit.event(WorkflowEvent::StepScheduled {
        step_id: step_id.to_owned(),
        attempt,
        runtime: plan_step.runtime.kind(),
        // Computed here, where it can be: the key is a pure function of the
        // step and the artifacts its parents produced, and both are in the
        // state. `None` is the ordinary answer — caching is opt-in, and
        // `cache_key` refuses a key for anything whose inputs are not all
        // digest-addressed. What consults the index is the reactor, because
        // that is a read and `decide` performs none.
        cache_key: crate::cache_key(plan_step, &execution.resolved_inputs(step_id)),
    });

    // A wait is not dispatched anywhere: the question goes in front of
    // somebody and the answer comes back as a command.
    if let RuntimeBinding::HumanInput(spec) = &plan_step.runtime {
        emit.event(WorkflowEvent::InputRequested {
            step_id: step_id.to_owned(),
            attempt,
            request: InputRequest {
                prompt: spec.prompt.clone(),
                role: spec.role.clone(),
                choices: spec.choices.clone(),
                deadline: None,
            },
        });
        emit.command(WorkflowCommand::RequestInput {
            step_id: step_id.to_owned(),
            attempt,
        });
        return;
    }

    emit.command(WorkflowCommand::ExecuteStep {
        step_id: step_id.to_owned(),
        attempt,
        runtime: plan_step.runtime.kind(),
        idempotency_key: idempotency_key(execution.execution_id.as_str(), step_id, attempt),
    });
}

/// What follows a step completing: its children, or the end of the run.
fn continue_after(execution: &Execution, step_id: &str, emit: &mut Emitter, now: Now) {
    let mut after = execution.clone();
    if let Some(step) = after.step_mut(step_id) {
        step.state = RunState::of(StateType::Completed);
        step.awaiting = None;
    }
    if after.cancelling {
        if after
            .steps
            .values()
            .all(|step| step.state.state_type != StateType::Running)
        {
            emit.event(WorkflowEvent::ExecutionCancelled);
        }
        return;
    }
    dispatch_ready(&after, emit, now);
    if after
        .steps
        .values()
        .all(|step| step.state.state_type == StateType::Completed)
    {
        emit.event(WorkflowEvent::ExecutionCompleted);
    }
}

/// Everything reachable from `step_id` will not run, and says so once each.
fn skip_downstream(execution: &Execution, step_id: &str, emit: &mut Emitter) {
    let mut frontier = vec![step_id.to_owned()];
    let mut seen: BTreeMap<String, ()> = BTreeMap::new();
    while let Some(current) = frontier.pop() {
        for child in execution.plan.children_of(&current) {
            if seen.insert(child.to_owned(), ()).is_some() {
                continue;
            }
            if execution
                .step(child)
                .is_some_and(|step| !step.state.state_type.is_terminal())
            {
                emit.event(WorkflowEvent::StepSkipped {
                    step_id: child.to_owned(),
                    reason: format!("{current} did not succeed"),
                });
            }
            frontier.push(child.to_owned());
        }
    }
}

/// `<execution>/<step>/<attempt>`: the stable key a reactor asks a runtime by
/// before it retries a timeout, and the one a worker's completion carries.
#[must_use]
pub fn idempotency_key(execution_id: &str, step_id: &str, attempt: u32) -> String {
    format!("{execution_id}/{step_id}/{attempt}")
}

/// Collects a decision's outputs, giving each one metadata derived from what it
/// is rather than generated.
struct Emitter {
    execution_id: String,
    cause: MessageId,
    now: Now,
    step: Option<(String, u32)>,
    out: Vec<PendingMessage>,
}

impl Emitter {
    fn new(execution_id: &str, cause: &MessageId, now: Now) -> Self {
        Self {
            execution_id: execution_id.to_owned(),
            cause: cause.clone(),
            now,
            step: None,
            out: Vec::new(),
        }
    }

    fn about_step(&mut self, step_id: &str, attempt: u32) {
        self.step = Some((step_id.to_owned(), attempt));
    }

    fn event(&mut self, event: WorkflowEvent) {
        let name = event.name();
        let message = WorkflowMessage::Event(event);
        self.push(name, message);
    }

    fn command(&mut self, command: WorkflowCommand) {
        let name = command.name();
        let message = WorkflowMessage::Command(command);
        self.push(name, message);
    }

    fn push(&mut self, name: &str, message: WorkflowMessage) {
        // Derived, never generated: the same decision replayed produces the
        // same ids, so a redelivered dispatch lands on the attempt it already
        // created. `ordinal` is in the key because one decision may emit two
        // messages of one name — two `step_skipped`, for instance.
        let ordinal = self.out.len();
        let message_id = MessageId::new(crate::derive_uuid(&format!(
            "aiwatcher/execution/{}/{}/{}/{ordinal}",
            self.execution_id, self.cause, name
        )));
        let mut metadata = MessageMetadata {
            schema_version: crate::message::SCHEMA_VERSION,
            message_id,
            occurred_at: self.now.at,
            correlation_id: CorrelationId::new(&self.execution_id),
            causation_id: CausationId::new(self.cause.as_str()),
            trace_id: None,
            span_id: None,
            step_id: None,
            attempt: None,
        };
        if let Some((step_id, attempt)) = &self.step {
            metadata.step_id = Some(step_id.clone());
            metadata.attempt = Some(*attempt);
        }
        self.out.push(PendingMessage {
            direction: Direction::Output,
            message,
            metadata,
        });
    }

    fn into_messages(self) -> Decision {
        self.out
    }
}

/// The one failure class the decider itself produces, for a command it can see
/// is wrong without asking anybody.
#[must_use]
pub fn validation_error(message: impl Into<String>) -> crate::state::StepError {
    crate::state::StepError::new(FailureClass::Validation, message)
}
