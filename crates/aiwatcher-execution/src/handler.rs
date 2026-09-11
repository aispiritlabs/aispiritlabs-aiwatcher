//! The one place `decide` and the store meet.
//!
//! Every workflow input is handled the same way, whether it came from the API,
//! from a reactor's completion or from a worker:
//!
//! ```text
//! BEGIN
//!   load the stream at the expected version
//!   if this message_id is already recorded: return what it produced
//!   fold the events, run decide()
//!   append the input, then every output
//!   update the run and step projection
//!   insert the outbox rows
//!   advance the processor checkpoint, when the input came from the log
//! COMMIT
//! ```
//!
//! The load–decide–append is **optimistic**. A conflict means somebody appended
//! between the read and the write, and the answer is to re-read and decide
//! again — a bounded retry around a pure function, which is cheap precisely
//! because `decide` does no I/O. A pessimistic lock would serialise every
//! command on an execution for the duration of a decision that never blocks.
//!
//! Two things this deliberately does not do. It never retries a *decision
//! error*: a command that does not apply will not apply on the second read
//! either, and looping on it would turn a 409 into a hang. And it never
//! publishes: the outbox rows go down in the same transaction and a separate
//! publisher drains them **after commit**, which is ADR_0026's one strict
//! ordering and `aiwatcher_jobs::ORDERING` in the fourth place it applies.

use aiwatcher_core::Checkpoint;
use time::OffsetDateTime;

use crate::claim::{AttemptKey, AttemptRow, AttemptWrite};
use crate::decide::{Now, decide, replay};
use crate::error::{DecisionError, StoreError};
use crate::facts::{FactContext, envelopes_for, outbox_rows};
use crate::hosted::{DEADLINE_TIMER, Timer, TimerWrite};
use crate::message::WorkflowCommand;
use crate::message::{
    MessageMetadata, OutboxMessage, PendingMessage, RunProjection, WorkflowEvent, WorkflowMessage,
};
use crate::plan::{ExecutionPlan, RuntimeBinding};
use crate::state::{ExecutionId, ExecutionMode, ExecutionState, RunState, StateType};
use crate::store::{AppendOutcome, AppendRequest, ExpectedVersion, WorkflowStore};

/// How many times a conflict is re-read before the caller is told.
///
/// Three, and the reason is the same as `aiwatcher_jobs::MAX_ATTEMPTS`': the
/// contention worth retrying is a decider and a reactor arriving together,
/// which resolves in one round. A fourth conflict is a queue, and the caller
/// deciding what to do about that is better than a loop that hides it.
pub const MAX_CONFLICT_RETRIES: u32 = 3;

/// What handling one input did.
#[derive(Clone, Debug)]
pub struct Handled {
    pub outcome: AppendOutcome,
    /// The state after the decision, folded from the stream this call wrote.
    pub projection: RunProjection,
    /// The facts on their way to the log. Not yet published — that is the
    /// publisher's job, after this transaction committed.
    pub outbox: Vec<OutboxMessage>,
    /// Whether this input had already been handled. A redelivery, not a race.
    pub duplicate: bool,
}

/// Why an input was not handled.
#[derive(Debug, thiserror::Error)]
pub enum HandleError {
    /// The command does not apply. Not retried, and not a race: it will not
    /// apply on the second read either.
    #[error(transparent)]
    Decision(#[from] DecisionError),

    #[error(transparent)]
    Store(#[from] StoreError),

    #[error(
        "{retries} callers appended to this execution while the decision was being made. \
         That is contention rather than a conflict; try again"
    )]
    Contended { retries: u32 },

    /// The plan names work this deployment has nowhere to run.
    ///
    /// Answered before the run is accepted rather than when nothing claims the
    /// step, because the second failure is invisible: a dispatched attempt
    /// nobody can take looks exactly like a worker that is busy.
    #[error(
        "the `file` workflow store holds one process, and {what} needs more than one. \
         Set AIWATCHER_WORKFLOW_STORE=postgres"
    )]
    NeedsMultiProcess { what: String },
}

/// Handles workflow inputs against one store.
#[derive(Debug)]
pub struct ExecutionHandler<S> {
    store: S,
}

impl<S: WorkflowStore> ExecutionHandler<S> {
    #[must_use]
    pub const fn new(store: S) -> Self {
        Self { store }
    }

    #[must_use]
    pub const fn store(&self) -> &S {
        &self.store
    }

    /// Handle one input, re-reading and deciding again on a conflict.
    ///
    /// # Errors
    ///
    /// [`HandleError::Decision`] when the command does not apply,
    /// [`HandleError::Contended`] when the retries ran out, and whatever the
    /// store could not do.
    pub async fn handle(
        &self,
        execution: &ExecutionId,
        input: WorkflowMessage,
        metadata: MessageMetadata,
        now: Now,
    ) -> Result<Handled, HandleError> {
        self.handle_with_checkpoint(execution, input, metadata, now, None)
            .await
    }

    /// Handle an input that arrived from the event log, advancing that
    /// processor's cursor.
    ///
    /// The cursor moves *with* the decision, in its transaction — advancing it
    /// first would skip an input nothing decided on. The one case that cannot
    /// be atomic is an input the inbox says was already handled: that decision
    /// committed long ago, so the cursor is advanced on its own, afterwards,
    /// which is the right way round by [`aiwatcher_jobs::ORDERING`].
    ///
    /// # Errors
    ///
    /// As [`Self::handle`].
    pub async fn handle_from_log(
        &self,
        execution: &ExecutionId,
        input: WorkflowMessage,
        metadata: MessageMetadata,
        now: Now,
        processor: &str,
        checkpoint: Checkpoint,
    ) -> Result<Handled, HandleError> {
        let handled = self
            .handle_with_checkpoint(
                execution,
                input,
                metadata,
                now,
                Some((processor.to_owned(), checkpoint.clone())),
            )
            .await?;
        if handled.duplicate {
            // Nothing was appended, so nothing carried the cursor. Leaving it
            // behind would re-read this message forever.
            self.store.advance_checkpoint(processor, checkpoint).await?;
        }
        Ok(handled)
    }

    /// One message this engine produced for itself, decided like any other.
    ///
    /// The only door past the effect-command guard, and it is deliberately
    /// narrow: the guard is about a *caller* posting work behind the state
    /// machine's back, and there is exactly one thing on this side of it — a
    /// deadline the timer table found. Everything about the decision that
    /// follows is the same, including the transaction and the derived rows.
    ///
    /// # Errors
    ///
    /// As [`Self::handle`].
    pub(crate) async fn deliver(
        &self,
        execution: &ExecutionId,
        input: WorkflowMessage,
        metadata: MessageMetadata,
        now: Now,
    ) -> Result<Handled, HandleError> {
        self.decide_and_append(execution, input, metadata, now, None)
            .await
    }

    async fn handle_with_checkpoint(
        &self,
        execution: &ExecutionId,
        input: WorkflowMessage,
        metadata: MessageMetadata,
        now: Now,
        checkpoint: Option<(String, Checkpoint)>,
    ) -> Result<Handled, HandleError> {
        // An effect command is what the decider emits for a reactor to run. A
        // caller that could post one would be scheduling work behind the state
        // machine's back. The rule is about *who is asking*: this engine's own
        // deliveries — a deadline that ran out — go through
        // [`Self::deliver`], which is not reachable from a route.
        if let Some(command) = input.command()
            && command.is_effect()
        {
            return Err(HandleError::Decision(DecisionError::Unhandled {
                message: input.name().to_owned(),
            }));
        }
        // Before the run, not after. Here rather than in the API, so that
        // every caller of `StartExecution` — a route, a schedule, a test —
        // meets the same refusal.
        if let Some(WorkflowCommand::StartExecution { plan, mode, .. }) = input.command() {
            self.check_capacity(plan, *mode)?;
        }
        self.decide_and_append(execution, input, metadata, now, checkpoint)
            .await
    }

    /// Decide, and write the whole of it once, retrying only a lost race.
    async fn decide_and_append(
        &self,
        execution: &ExecutionId,
        input: WorkflowMessage,
        metadata: MessageMetadata,
        now: Now,
        checkpoint: Option<(String, Checkpoint)>,
    ) -> Result<Handled, HandleError> {
        for attempt in 0..=MAX_CONFLICT_RETRIES {
            let slice = self.store.load(execution).await?;

            // The inbox before the decision: a redelivery is not a race, and
            // deciding again on one would give the reactors a second dispatch.
            if let Some(seen) = slice
                .messages
                .iter()
                .find(|recorded| recorded.metadata.message_id == metadata.message_id)
            {
                let state = replay(slice.events());
                return Ok(Handled {
                    outcome: AppendOutcome::Duplicate {
                        version: seen.stream_version,
                    },
                    projection: projection_of(execution, &state, slice.version),
                    outbox: Vec::new(),
                    duplicate: true,
                });
            }

            let state = replay(slice.events());
            let outputs = decide(&state, &input, &metadata.message_id, now)?;

            // The state the projection describes is the one after this
            // decision, which is what makes the projection safe to accept the
            // *next* command from without folding the stream again.
            let after = outputs
                .iter()
                .filter_map(|message| message.message.event())
                .fold(state, crate::decide::evolve);

            let outbox = self.facts(execution, &after, &outputs, &metadata);
            let attempts = attempt_rows(execution, &after, &outputs);
            let timers = deadline_rows(execution, &after, &outputs);
            let projection =
                projection_of(execution, &after, slice.version + 1 + outputs.len() as u64);

            let expected = if slice.is_empty() {
                // Two API replicas, one click: `NoStream` is what makes the
                // second a conflict rather than a second plan on one stream.
                ExpectedVersion::NoStream
            } else {
                ExpectedVersion::Exact(slice.version)
            };

            let request = AppendRequest {
                expected_version: expected,
                input: PendingMessage::input(input.clone(), metadata.clone()),
                outputs,
                projection: projection.clone(),
                outbox: outbox.clone(),
                checkpoint: checkpoint.clone(),
                timers: timers.clone(),
                attempts,
            };

            match self.store.append(execution, request).await {
                Ok(outcome) => {
                    return Ok(Handled {
                        duplicate: outcome.is_duplicate(),
                        outbox: if outcome.is_duplicate() {
                            Vec::new()
                        } else {
                            outbox
                        },
                        outcome,
                        projection,
                    });
                }
                // Not surfaced, even on the last pass: a version number the
                // caller did not read is not something it can act on, and
                // "somebody else is appending, try again" is.
                Err(StoreError::VersionConflict { .. }) => {
                    let _ = attempt;
                }
                Err(error) => return Err(error.into()),
            }
        }
        Err(HandleError::Contended {
            retries: MAX_CONFLICT_RETRIES,
        })
    }

    /// Whether this store can carry out that plan at all.
    ///
    /// [`StoreCapabilities::multi_process`] is the whole check
    /// ([`crate::store::StoreCapabilities`]), and `file` is the adapter that
    /// answers `false`. ADR_0025: a development store must not become a
    /// production one by omission.
    fn check_capacity(&self, plan: &ExecutionPlan, mode: ExecutionMode) -> Result<(), HandleError> {
        if self.store.capabilities().multi_process {
            return Ok(());
        }
        // A hosted run's *decider* is the second process, whatever its plan
        // holds. The check below reads the plan's steps, and a hosted agent
        // graph's steps are its shape rather than its program — so a graph made
        // only of Flow blocks would have passed it while the worker that
        // decides the run sat in another process appending to a store that
        // holds one. That is the same hole the step check exists to close,
        // reached from the side the step check cannot see.
        if mode == ExecutionMode::Hosted {
            return Err(HandleError::NeedsMultiProcess {
                what: "a hosted execution — the worker that decides it is another process"
                    .to_owned(),
            });
        }
        let pulled = plan.steps_needing_another_process();
        if pulled.is_empty() {
            return Ok(());
        }
        Err(HandleError::NeedsMultiProcess {
            what: format!(
                "{} — a worker in another process claims {}",
                pulled
                    .iter()
                    .map(|step| format!("'{step}'"))
                    .collect::<Vec<_>>()
                    .join(", "),
                if pulled.len() == 1 { "it" } else { "them" }
            ),
        })
    }

    fn facts(
        &self,
        execution: &ExecutionId,
        after: &ExecutionState,
        outputs: &[PendingMessage],
        metadata: &MessageMetadata,
    ) -> Vec<OutboxMessage> {
        let Some(run) = after.active() else {
            return Vec::new();
        };
        let context = FactContext {
            execution,
            plan: &run.plan,
            owner: &run.owner,
            occurred_at: metadata.occurred_at,
            cause: &metadata.message_id,
        };
        let mut envelopes = Vec::new();
        for (ordinal, event) in outputs
            .iter()
            .filter_map(|message| message.message.event())
            .enumerate()
        {
            // Each output's facts are derived from the causing message plus its
            // own position, so two `step.skipped`-adjacent facts in one
            // decision do not collide.
            let cause = aiwatcher_core::MessageId::new(format!(
                "{}#{ordinal}",
                metadata.message_id.as_str()
            ));
            envelopes.extend(envelopes_for(
                event,
                &FactContext {
                    cause: &cause,
                    ..context
                },
            ));
        }
        outbox_rows(envelopes, execution)
    }
}

/// The inline projection for a folded state.
///
/// Derived rather than accumulated, so it cannot drift from the stream: the
/// store rebuilds it from the same fold on every append.
#[must_use]
pub fn projection_of(
    execution: &ExecutionId,
    state: &ExecutionState,
    version: u64,
) -> RunProjection {
    match state.active() {
        Some(run) => RunProjection {
            execution_id: execution.clone(),
            plan_id: run.plan.plan_id.to_string(),
            definition_name: run.plan.definition_name.clone(),
            owner: run.owner.clone(),
            mode: run.mode,
            payloads: run.payloads,
            state: run.state.clone(),
            requested_by: run.requested_by.clone(),
            steps: run.steps.values().cloned().collect(),
            last_message_version: version,
            created_at: run.created_at,
        },
        // A stream with no `ExecutionRequested` in it. Reachable only through
        // the checkpoint-only append above, and a placeholder rather than a
        // guess: nothing has named a plan yet.
        None => RunProjection {
            execution_id: execution.clone(),
            plan_id: String::new(),
            definition_name: String::new(),
            owner: crate::state::ExecutionOwner::Local,
            mode: crate::state::ExecutionMode::Compiled,
            payloads: crate::message::PayloadPolicy::External,
            state: RunState::of(StateType::Scheduled),
            requested_by: String::new(),
            steps: Vec::new(),
            last_message_version: version,
            created_at: OffsetDateTime::UNIX_EPOCH,
        },
    }
}

/// What the outputs of one decision would publish, without a store.
///
/// For a caller previewing an execution — and for the tests that assert the
/// mapping without going through an append.
#[must_use]
pub fn facts_of(
    execution: &ExecutionId,
    plan: &ExecutionPlan,
    owner: &crate::state::ExecutionOwner,
    events: &[WorkflowEvent],
    cause: &aiwatcher_core::MessageId,
    occurred_at: OffsetDateTime,
) -> Vec<OutboxMessage> {
    let mut envelopes = Vec::new();
    for (ordinal, event) in events.iter().enumerate() {
        let cause = aiwatcher_core::MessageId::new(format!("{}#{ordinal}", cause.as_str()));
        envelopes.extend(envelopes_for(
            event,
            &FactContext {
                execution,
                plan,
                owner,
                occurred_at,
                cause: &cause,
            },
        ));
    }
    outbox_rows(envelopes, execution)
}

/// The timer id a question's deadline is kept under.
///
/// Names the step *and* the attempt, because a retry asks the question again
/// and the row the first attempt left must not fire on the second. The
/// execution is the other half of the key and the store supplies it.
#[must_use]
pub fn deadline_timer_id(step_id: &str, attempt: u32) -> String {
    format!("input/{step_id}/{attempt}")
}

/// The timer table's share of one decision, derived from the facts it emitted.
///
/// `decide` gains no vocabulary for a timer, which is the point: a deadline is
/// a *consequence* of a question having been asked, so the row follows the
/// fact. `InputRequested` with a deadline schedules one; anything that ends
/// that step retires it, and `Cancel` is a no-op where there was none — so a
/// question nobody put a clock on costs an empty vector.
///
/// Written in the same transaction as the decision, for [`AttemptWrite`]'s
/// reason: a timer scheduled outside it is work nobody decided on, and one left
/// behind by a step that has ended is a deadline still coming for a question
/// that is over.
#[must_use]
pub fn deadline_rows(
    execution: &ExecutionId,
    after: &ExecutionState,
    outputs: &[PendingMessage],
) -> Vec<TimerWrite> {
    let Some(run) = after.active() else {
        return Vec::new();
    };
    // Only a step that can have a row is retired. Cancelling for every step
    // that ends would be a write per step per transaction — a no-op the `file`
    // adapter still pays for by rewriting its table.
    //
    // Two ways a step comes to have one, and asking only the plan was wrong for
    // the second. A `HumanInput` step's clock is authored, so the plan answers.
    // A running attempt that stopped to ask brought its own, and the plan that
    // pinned its code says nothing about it — which is why a *dispatched* step
    // is included whenever this decision saw it asking or being asked about.
    // Left to the plan alone, a parked worker attempt that completed would have
    // left its deadline behind, still coming for a question that was over.
    // Read from this decision's own facts rather than from `after`, which is
    // the state the decision *reached*: `evolve` clears `awaiting` on the very
    // events a cancel follows, so by then the question that had the clock is
    // already gone. A step this decision saw asking or being answered was
    // waiting a moment ago, whatever authored the question.
    let waiting_here: std::collections::BTreeSet<&str> = outputs
        .iter()
        .filter_map(|message| message.message.event())
        .filter_map(|event| match event {
            WorkflowEvent::InputRequested { step_id, .. }
            | WorkflowEvent::InputProvided { step_id, .. } => Some(step_id.as_str()),
            _ => None,
        })
        .collect();
    let has_deadline = |step_id: &str| {
        waiting_here.contains(step_id)
            || matches!(
                run.plan.step(step_id).map(|step| &step.runtime),
                Some(RuntimeBinding::HumanInput(spec)) if spec.timeout_seconds.is_some()
            )
    };
    let mut rows = Vec::new();
    for message in outputs {
        let Some(event) = message.message.event() else {
            continue;
        };
        match event {
            WorkflowEvent::InputRequested {
                step_id,
                attempt,
                request,
            } => {
                let Some(due_at) = request.deadline else {
                    continue;
                };
                rows.push(TimerWrite::Schedule(Timer {
                    execution: execution.clone(),
                    timer_id: deadline_timer_id(step_id, *attempt),
                    due_at,
                    // Nothing stored to hand back: what this timer means is
                    // read from the plan when it fires, because the plan is
                    // what pinned the policy. A hosted timer carries a
                    // worker's message because the engine composes none for
                    // it; here the engine is the decider.
                    message: crate::message::HostedMessage {
                        message_type: DEADLINE_TIMER.to_owned(),
                        metadata: serde_json::json!({
                            "step_id": step_id,
                            "attempt": attempt,
                        }),
                        payload: None,
                    },
                }));
            }
            WorkflowEvent::InputProvided {
                step_id, attempt, ..
            }
            | WorkflowEvent::StepCompleted {
                step_id, attempt, ..
            }
            | WorkflowEvent::StepFailed {
                step_id, attempt, ..
            } if has_deadline(step_id) => {
                rows.push(TimerWrite::Cancel(deadline_timer_id(step_id, *attempt)));
            }
            _ => {}
        }
    }
    rows
}

/// The claim table's side of one decision.
///
/// Derived from the decision's own outputs rather than accumulated, for the
/// same reason the projection is: a row inserted by anything other than the
/// decision that authorised it would be work nobody decided on. A dispatch
/// makes a row claimable; a completion, a failure or a skip settles it.
#[must_use]
pub fn attempt_rows(
    execution: &ExecutionId,
    after: &ExecutionState,
    outputs: &[PendingMessage],
) -> Vec<AttemptWrite> {
    let Some(run) = after.active() else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    for message in outputs {
        match &message.message {
            WorkflowMessage::Command(WorkflowCommand::ExecuteStep {
                step_id,
                attempt,
                runtime,
                ..
            }) => {
                let key = AttemptKey::new(execution.clone(), step_id, *attempt);
                let mut row =
                    AttemptRow::claimable(key, *runtime, message.metadata.message_id.clone());
                // A pulled attempt names the queue it is claimable on and the
                // pinned code a worker must match. Everything else is a
                // reactor's, claimed by runtime.
                //
                // A pod's attempt is pulled too, on the queue its template's
                // token holds. What keeps a long-lived worker on that queue
                // off it is the key-only rule in `ClaimFilter::matches`
                // (ADR_0029): only the pod started for it names its key.
                match run.plan.step(step_id).map(|step| &step.runtime) {
                    Some(RuntimeBinding::PythonTask(spec)) => {
                        row = row.on_queue(spec.queue.clone(), spec.task_ref.clone());
                    }
                    Some(RuntimeBinding::ContainerJob(spec)) => {
                        row = row.on_queue(spec.queue.clone(), spec.task_ref.clone());
                    }
                    _ => {}
                }
                if let Some(not_before) = retry_delay_of(outputs, step_id, *attempt) {
                    row = row.not_before(not_before);
                }
                rows.push(AttemptWrite::Dispatch(row));
            }
            // A runtime that stopped to ask. The row stays — a question is not
            // an ending — and the lease goes, which is what stops the next
            // claimant running the work again five minutes later having been
            // told nothing about the question.
            //
            // Only for a step that *has* a row. A `HumanInput` step is parked
            // by the decider when it schedules it and reaches the claim table
            // never: `schedule_attempt` emits `RequestInput`, not
            // `ExecuteStep`, and only `ExecuteStep` dispatches one. Emitting a
            // park for it would be a write per gate per transaction that the
            // `file` adapter pays for by rewriting its table — `deadline_rows`
            // narrows for the same reason.
            WorkflowMessage::Event(WorkflowEvent::InputRequested {
                step_id, attempt, ..
            }) if dispatched(run, step_id) => {
                rows.push(AttemptWrite::Park(AttemptKey::new(
                    execution.clone(),
                    step_id,
                    *attempt,
                )));
            }
            // The question is over, so the row it parked is too. A park keeps
            // its row because a question is not an ending; an *answer* is one,
            // and the attempt that asked is read back from the stream and the
            // projection rather than from here — 43.34's rule, which the resume
            // would otherwise walk straight past. Left behind, the claim table
            // grows by one row per park for ever, which is the table growing
            // with the history that rule exists to prevent.
            //
            // The row the answer dispatches is attempt *n+1* under its own key,
            // so this retires the old one without touching the new.
            WorkflowMessage::Event(WorkflowEvent::InputProvided {
                step_id, attempt, ..
            }) if dispatched(run, step_id) => {
                rows.push(AttemptWrite::Retire(AttemptKey::new(
                    execution.clone(),
                    step_id,
                    *attempt,
                )));
            }
            // Nothing ran, so nothing is claimable: retiring the row is what
            // takes a dispatched attempt out of every claimant's view when a
            // cancel or an upstream failure overtook it.
            //
            // The number comes from the run, because `StepSkipped` carries
            // none — it is a fact about a step rather than about an attempt.
            // Read as attempt `0` it retired a key that had never existed, and
            // left the row claimable for ever: a reactor took a cancelled
            // run's pending attempt and did the work, and a pod's row sat in
            // the claim table being read by every launcher pass. Found while
            // building AW-4's 2.4.
            WorkflowMessage::Event(WorkflowEvent::StepSkipped { step_id, .. }) => {
                if let Some(attempt) = dispatched_attempt(run, step_id) {
                    rows.push(AttemptWrite::Retire(AttemptKey::new(
                        execution.clone(),
                        step_id,
                        attempt,
                    )));
                }
            }
            WorkflowMessage::Event(event) => {
                if let Some((step_id, attempt)) = settled(event) {
                    rows.push(AttemptWrite::Retire(AttemptKey::new(
                        execution.clone(),
                        step_id,
                        attempt,
                    )));
                }
            }
            // A hosted decider's message authorises nothing here: the worker
            // schedules its own next node, and this engine keeps the history.
            // A claim row derived from one would be work nobody in this process
            // decided on.
            WorkflowMessage::Command(_) | WorkflowMessage::Hosted(_) => {}
        }
    }
    rows
}

/// The instant a retry in this same decision resolved to, if there was one.
///
/// The delay lives on `StepRetryScheduled` because that is the fact; the claim
/// row reads it so a retry is not taken before its time. Resolved by the
/// decider, so a replay reaches the same schedule.
fn retry_delay_of(
    outputs: &[PendingMessage],
    step_id: &str,
    attempt: u32,
) -> Option<OffsetDateTime> {
    outputs
        .iter()
        .filter_map(|message| message.message.event())
        .find_map(|event| match event {
            WorkflowEvent::StepRetryScheduled {
                step_id: id,
                attempt: number,
                not_before,
            } if id == step_id && *number == attempt => Some(*not_before),
            _ => None,
        })
}

/// Whether this step's attempts reach the claim table at all.
///
/// The discriminator between the two ways a step comes to be waiting for a
/// person, and it is a fact about the plan rather than about the question. A
/// `HumanInput` step was never dispatched — `schedule_attempt` emits
/// `RequestInput` for it and `ExecuteStep` for everything else, and only the
/// second writes a row. Anything else that asks was already running, holding a
/// lease something has to release.
fn dispatched(run: &crate::state::Execution, step_id: &str) -> bool {
    !matches!(
        run.plan.step(step_id).map(|step| &step.runtime),
        Some(RuntimeBinding::HumanInput(_)) | None
    )
}

/// Which attempt this fact ends.
///
/// Only *how* it ended is missing, and deliberately: the outcome is on the
/// event that carries it and in the run's own state, and the claim table's one
/// question is what may still be taken. Every arm here is terminal — an
/// attempt waiting for a person is `awaiting_input`, which is not an ending
/// and keeps its row.
fn settled(event: &WorkflowEvent) -> Option<(&str, u32)> {
    match event {
        WorkflowEvent::StepCompleted {
            step_id, attempt, ..
        } => Some((step_id, *attempt)),
        // A hit settles the row it answered. Without this the attempt stays
        // claimable behind a lease nobody releases — the work was never done,
        // so nothing else would ever settle it.
        WorkflowEvent::StepCacheHit {
            step_id, attempt, ..
        } => Some((step_id, *attempt)),
        WorkflowEvent::StepFailed {
            step_id, attempt, ..
        } => Some((step_id, *attempt)),
        // `StepSkipped` is not here: it names no attempt, so its row is
        // resolved from the run in [`attempt_rows`] instead.
        _ => None,
    }
}

/// The attempt of this step that has a row in the claim table, if any.
///
/// Two ways not to have one, and both are ordinary: a `HumanInput` step is
/// never dispatched at all ([`dispatched`]), and a step the plan scheduled but
/// nothing has dispatched yet is at attempt `0`. A retire for either would be
/// a write per skipped step per transaction, which the `file` adapter pays for
/// by rewriting its table.
fn dispatched_attempt(run: &crate::state::Execution, step_id: &str) -> Option<u32> {
    if !dispatched(run, step_id) {
        return None;
    }
    run.step(step_id)
        .map(|step| step.current_attempt)
        .filter(|attempt| *attempt > 0)
}
