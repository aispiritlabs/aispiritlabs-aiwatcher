//! Where an execution is, and where each of its steps is.
//!
//! Two things are separated deliberately, following Prefect. `StateType` is
//! stable and drives orchestration — a scheduler reads it and nothing else.
//! `name` is a user-facing refinement of the same state: `Validating`,
//! `AwaitingRetry`, `Cached`, `Cancelling`, `TimedOut`. A run that is
//! `pending / AwaitingRetry` and one that is `pending / Cancelling` are
//! scheduled identically and read very differently, and collapsing them would
//! cost either the reader or the scheduler.
//!
//! [`StateType::AwaitingInput`] is a type of its own rather than a name for
//! `Paused`, because the two resume differently. A paused run resumes on
//! `resume`, from whoever may pause. A waiting step resumes on the *input it
//! asked for*, from the role the step declared, and its lease is released while
//! it waits — a worker that parked an attempt to ask a question does not hold a
//! pod for the answer.

use std::collections::BTreeMap;

use aiwatcher_core::ArtifactRef;
use aiwatcher_core::human_input::OnTimeout;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;
use utoipa::ToSchema;

use crate::plan::{ExecutionPlan, PlanId, RuntimeKind};

/// The execution's own identity. `workflow_run_id` on every event it publishes.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize, ToSchema)]
#[serde(transparent)]
pub struct ExecutionId(pub String);

impl ExecutionId {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ExecutionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What drives orchestration. Nine, and no more without a reason written down.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StateType {
    /// Accepted, nothing dispatched yet.
    Scheduled,
    /// Waiting for something this system controls: a parent step, a retry
    /// delay, a free worker.
    Pending,
    Running,
    /// Waiting for a person. See the module docs for why this is not `Paused`.
    AwaitingInput,
    Completed,
    /// The work ran and did not succeed.
    Failed,
    /// The work was lost rather than answered — an expired lease, a killed pod.
    Crashed,
    Cancelled,
    /// Stopped on request, resumable on request.
    Paused,
}

impl StateType {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Scheduled => "scheduled",
            Self::Pending => "pending",
            Self::Running => "running",
            Self::AwaitingInput => "awaiting_input",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Crashed => "crashed",
            Self::Cancelled => "cancelled",
            Self::Paused => "paused",
        }
    }

    /// Whether nothing more will happen here without a new command.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Crashed | Self::Cancelled
        )
    }

    /// Every state [`Self::is_terminal`] answers true for, as a list.
    ///
    /// So a store can ask a database the question the fold asks in memory
    /// without writing a second list in SQL. The schema already spells one out
    /// for `step_attempts`, and that one is about a *step*; this is about a
    /// run, and a copy of it in a query is a copy that drifts the day a state
    /// is added. `terminal_states_are_the_ones_is_terminal_answers_for` is what
    /// keeps the two halves the same.
    pub const TERMINAL: [Self; 4] = [
        Self::Completed,
        Self::Failed,
        Self::Crashed,
        Self::Cancelled,
    ];

    /// Every variant, for the tests that have to be exhaustive about them.
    #[cfg(test)]
    const EVERY: [Self; 9] = [
        Self::Scheduled,
        Self::Pending,
        Self::Running,
        Self::AwaitingInput,
        Self::Completed,
        Self::Failed,
        Self::Crashed,
        Self::Cancelled,
        Self::Paused,
    ];
}

/// A stable type, and the word a person reads.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct RunState {
    pub state_type: StateType,
    /// Empty means "call it by its type".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
}

impl RunState {
    #[must_use]
    pub fn of(state_type: StateType) -> Self {
        Self {
            state_type,
            name: String::new(),
        }
    }

    #[must_use]
    pub fn named(state_type: StateType, name: &str) -> Self {
        Self {
            state_type,
            name: name.to_owned(),
        }
    }

    /// What the panel shows.
    #[must_use]
    pub fn label(&self) -> &str {
        if self.name.is_empty() {
            self.state_type.as_str()
        } else {
            &self.name
        }
    }
}

/// Who decides for this execution, and therefore who owns its retries.
///
/// The `ExecutionBackend` trait of the plan's first revision collapsed into
/// this field. A trait would have said the three are interchangeable
/// implementations of one thing; they are not — they differ in *who thinks*,
/// which is a property of the run rather than a strategy the run holds.
#[derive(Clone, Debug, PartialEq, Eq, ToSchema)]
pub enum ExecutionOwner {
    /// The Rust decider schedules the steps and owns their retries.
    Local,
    /// A worker runs `decide` and this system keeps the history and the lease.
    Worker,
    /// An owner this build does not know, kept exactly as it was written. It
    /// is shown and never scheduled or decided for. `engine:flyte`, from the
    /// builds that had a pipeline engine (AW-4), reads back as this, and so
    /// does anything a newer build names.
    Unknown(String),
}

impl ExecutionOwner {
    #[must_use]
    pub fn as_string(&self) -> String {
        match self {
            Self::Local => "local".to_owned(),
            Self::Worker => "worker".to_owned(),
            Self::Unknown(text) => text.clone(),
        }
    }

    /// `local` or `worker`. Any other text is kept as [`Self::Unknown`] rather
    /// than refused, so a record written by another build still reads.
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value {
            "local" => Self::Local,
            "worker" => Self::Worker,
            other => Self::Unknown(other.to_owned()),
        }
    }
}

impl std::fmt::Display for ExecutionOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.as_string())
    }
}

impl Serialize for ExecutionOwner {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(&self.as_string())
    }
}

impl<'de> Deserialize<'de> for ExecutionOwner {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        Ok(Self::parse(&String::deserialize(de)?))
    }
}

/// Whether the plan is the program, or only its shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    /// A static plan the Rust decider schedules.
    #[default]
    Compiled,
    /// A worker runs `decide`; this system keeps the history. What an agent
    /// graph needs, because its next node depends on what the last one said.
    Hosted,
}

/// Why an attempt did not succeed, and therefore whether to try again.
///
/// The classification is the caller's claim about the error, exactly as
/// `aiwatcher_jobs::after_failure` takes `retryable` rather than deciding it:
/// an unreachable notebook runtime is worth coming back for and a Flow parse
/// error will not parse on the third attempt either.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    /// An invalid graph, an unknown source, a parameter that does not bind.
    Validation,
    /// The code ran and was wrong.
    UserCode,
    /// A connection reset, a 503, no worker.
    Transient,
    /// The caller stopped waiting. Proves nothing about the runtime, which is
    /// why a reactor asks by idempotency key before it retries.
    Timeout,
    /// The work was lost: an expired lease, a killed pod.
    Infrastructure,
    /// Cancelled, permission revoked, an artifact quarantined.
    Policy,
}

impl FailureClass {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Validation => "validation",
            Self::UserCode => "user_code",
            Self::Transient => "transient",
            Self::Timeout => "timeout",
            Self::Infrastructure => "infrastructure",
            Self::Policy => "policy",
        }
    }

    /// Whether trying again could plausibly produce a different answer.
    #[must_use]
    pub const fn is_retryable(self) -> bool {
        matches!(self, Self::Transient | Self::Timeout | Self::Infrastructure)
    }

    /// Whether the runtime may have done the work despite the failure.
    ///
    /// This is what decides which retry budget an attempt spends, and the
    /// distinction is real. A `Transient` failure is the runtime *declining* —
    /// a refused connection, a 503, no worker — so nothing ran, nothing has a
    /// side effect, and running it again costs one call. A `Timeout` proves
    /// only that the caller stopped waiting, and an `Infrastructure` loss means
    /// a pod died holding work that may already be half done; both may repeat
    /// something, so both are spent from the tighter budget.
    ///
    /// The consequence, measured: with one budget of three, a forty-second
    /// outage of the query service killed a run whose step was fine. See
    /// [`RetryPolicy`](crate::plan::RetryPolicy).
    #[must_use]
    pub const fn may_have_run(self) -> bool {
        !matches!(self, Self::Transient)
    }

    /// The state an attempt that failed this way is left in.
    ///
    /// Infrastructure loss is `Crashed` rather than `Failed`: nobody's code
    /// gave a wrong answer, and a run whose only red step says "failed" when a
    /// node was drained sends somebody to read a log that says nothing.
    #[must_use]
    pub const fn attempt_state(self) -> StateType {
        match self {
            Self::Infrastructure => StateType::Crashed,
            _ => StateType::Failed,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct StepError {
    pub class: FailureClass,
    pub message: String,
}

impl StepError {
    #[must_use]
    pub fn new(class: FailureClass, message: impl Into<String>) -> Self {
        Self {
            class,
            message: message.into(),
        }
    }
}

/// What a step is waiting for somebody to answer.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct InputRequest {
    pub prompt: String,
    /// The role that may answer. Checked when the answer arrives.
    pub role: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "time::serde::rfc3339::option")]
    pub deadline: Option<OffsetDateTime>,
    /// What happens to this step if nobody answers in time.
    ///
    /// On the question rather than only on the plan, because a question has two
    /// authors and only one of them is the plan. A `HumanInput` step's policy is
    /// [`HumanInputSpec::on_timeout`](crate::plan::HumanInputSpec::on_timeout)
    /// and is copied here when the step is scheduled; a running attempt that
    /// stopped to ask supplies its own, and the plan that pinned its code has
    /// nothing to say about a question it did not author.
    ///
    /// `None` is a question written before this field existed. Those are all
    /// `HumanInput` steps, so the plan is read for them instead — the fallback
    /// exists so a replay of an old stream reaches the policy it was decided
    /// under rather than this field's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_timeout: Option<OnTimeout>,
}

/// One answer somebody gave a step, and which attempt asked for it.
///
/// Kept on the *step* rather than only on the attempt that asked, because the
/// answer's reader is the attempt that comes *after*. A parked attempt stays
/// immutable and the answer schedules attempt `n+1`, which re-runs the work
/// from the beginning — so the question it asks first is the one already
/// answered here, and without this it would ask it again and park again.
///
/// A list rather than one, because a task may ask twice. Attempt 1 asks, the
/// answer schedules attempt 2, which replays to the first question, reads this
/// answer instead of parking, goes on and asks a *second*. Attempt 3 then needs
/// both: given only the last it would park on the first question for ever. They
/// are consumed in the order they were given, which is the same order a replay
/// asks them in.
///
/// Each `response` is bounded — the answer route takes a control value, not
/// rows — and the list is as long as the number of questions the task asks.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, ToSchema)]
pub struct InputAnswer {
    /// The attempt that asked. Not the attempt that reads it, which is a later
    /// one — this says whose question was answered.
    pub attempt: u32,
    /// From the session that answered, never from a request body.
    pub answered_by: String,
    #[schema(value_type = Value)]
    pub response: Value,
}

/// One physical attempt to perform a step. Immutable once terminal.
///
/// **When it started and ended is not here.** An attempt's `step.*` events form
/// a span (ADR_0003), so the waterfall already times it — from the trace store,
/// with the shape a duration is actually read in. The two fields were here,
/// written by nothing and read by nothing (43.33).
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, ToSchema)]
pub struct AttemptRecord {
    pub attempt: u32,
    pub state: RunState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<StepError>,
    /// When a retry may be dispatched. `None` means "now".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "time::serde::rfc3339::option")]
    pub not_before: Option<OffsetDateTime>,
}

/// The logical state of one plan step, across every attempt it took.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, ToSchema)]
pub struct StepState {
    pub step_id: String,
    pub runtime: RuntimeKind,
    pub state: RunState,
    /// The attempt number in flight, or the last one taken. `0` before any.
    pub current_attempt: u32,
    #[serde(default)]
    pub attempts: Vec<AttemptRecord>,
    #[serde(default)]
    pub outputs: Vec<ArtifactRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub awaiting: Option<InputRequest>,
    /// Every answer this step has been given, oldest first.
    ///
    /// What a resumed attempt reads. See [`InputAnswer`] for why it is a list
    /// and why it lives on the step rather than on the attempt that asked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub answers: Vec<InputAnswer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_key: Option<String>,
}

impl StepState {
    #[must_use]
    pub fn fresh(step_id: String, runtime: RuntimeKind) -> Self {
        Self {
            step_id,
            runtime,
            state: RunState::of(StateType::Scheduled),
            current_attempt: 0,
            attempts: Vec::new(),
            outputs: Vec::new(),
            awaiting: None,
            answers: Vec::new(),
            cache_key: None,
        }
    }

    #[must_use]
    pub fn attempt(&self, attempt: u32) -> Option<&AttemptRecord> {
        self.attempts
            .iter()
            .find(|record| record.attempt == attempt)
    }

    fn attempt_mut(&mut self, attempt: u32) -> Option<&mut AttemptRecord> {
        self.attempts
            .iter_mut()
            .find(|record| record.attempt == attempt)
    }
}

/// Everything the decider knows, folded from the stream.
///
/// `Empty` is [`initial_state`](crate::decide::initial_state): a stream that
/// has never been written to. Only `StartExecution` is accepted there, and
/// every other command is refused by name rather than silently ignored — a
/// retry addressed at an execution nobody started is a bug in the caller and
/// worth saying so.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum ExecutionState {
    #[default]
    Empty,
    Active(Box<Execution>),
}

impl ExecutionState {
    #[must_use]
    pub fn active(&self) -> Option<&Execution> {
        match self {
            Self::Empty => None,
            Self::Active(execution) => Some(execution),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Execution {
    pub execution_id: ExecutionId,
    pub plan: ExecutionPlan,
    pub owner: ExecutionOwner,
    pub mode: ExecutionMode,
    /// Where this run's words live. `external` unless the run said otherwise.
    pub payloads: crate::message::PayloadPolicy,
    pub requested_by: String,
    pub input: BTreeMap<String, Value>,
    pub state: RunState,
    /// One entry per plan step, in plan order.
    pub steps: BTreeMap<String, StepState>,
    /// A cancel was accepted and the running steps have not all stopped.
    pub cancelling: bool,
    pub created_at: OffsetDateTime,
}

impl Execution {
    #[must_use]
    pub fn plan_id(&self) -> &PlanId {
        &self.plan.plan_id
    }

    #[must_use]
    pub fn step(&self, step_id: &str) -> Option<&StepState> {
        self.steps.get(step_id)
    }

    pub(crate) fn step_mut(&mut self, step_id: &str) -> Option<&mut StepState> {
        self.steps.get_mut(step_id)
    }

    pub(crate) fn set_attempt_state(&mut self, step_id: &str, attempt: u32, state: RunState) {
        if let Some(step) = self.steps.get_mut(step_id)
            && let Some(record) = step.attempt_mut(attempt)
        {
            record.state = state;
        }
    }

    /// What `step_id` reads, resolved to the artifacts its parents produced.
    ///
    /// The plan's [`InputBinding`](crate::plan::InputBinding) names a step and
    /// an output; the state holds what that step actually produced. Reading it
    /// from the state rather than from a catalog is what makes a retry reuse
    /// the *pinned* artifacts of its own context rather than whatever is
    /// newest — and it is what lets [`cache_key`](crate::cache_key) be computed
    /// inside `decide`, which may not perform I/O.
    #[must_use]
    pub fn resolved_inputs(&self, step_id: &str) -> Vec<ArtifactRef> {
        let Some(step) = self.plan.step(step_id) else {
            return Vec::new();
        };
        step.inputs
            .iter()
            .filter_map(|binding| match binding {
                crate::plan::InputBinding::Step { step, output } => {
                    let produced = self.step(step)?;
                    produced
                        .outputs
                        .iter()
                        .find(|artifact| &artifact.name == output)
                        .cloned()
                }
                crate::plan::InputBinding::Parameter { .. } => None,
            })
            .collect()
    }

    /// Whether every step has stopped, one way or another.
    #[must_use]
    pub fn is_settled(&self) -> bool {
        self.steps
            .values()
            .all(|step| step.state.state_type.is_terminal())
    }

    /// Whether any step ran and did not succeed.
    #[must_use]
    pub fn has_failure(&self) -> bool {
        self.steps.values().any(|step| {
            matches!(
                step.state.state_type,
                StateType::Failed | StateType::Crashed
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_states_are_the_ones_is_terminal_answers_for() {
        // The retention sweep asks a database `state_type = any(TERMINAL)` and
        // every other reader asks `is_terminal`. A state added to one and not
        // the other is a run that is finished everywhere except where it is
        // forgotten, which nothing would report.
        for state in StateType::EVERY {
            assert_eq!(
                state.is_terminal(),
                StateType::TERMINAL.contains(&state),
                "{}",
                state.as_str()
            );
        }
    }

    #[test]
    fn an_owner_round_trips_through_the_string_a_row_holds() {
        for owner in [
            ExecutionOwner::Local,
            ExecutionOwner::Worker,
            ExecutionOwner::Unknown("engine:flyte".to_owned()),
        ] {
            assert_eq!(ExecutionOwner::parse(&owner.as_string()), owner);
        }
    }

    #[test]
    fn a_run_the_removed_engine_owned_reads_back_as_unknown_and_unchanged() {
        // A row written while the Flyte engine existed. It is not an engine
        // nobody configured — there is no such thing any more — it is text
        // this build does not know, shown exactly as it was written.
        let owner = ExecutionOwner::parse("engine:flyte");
        assert_eq!(owner, ExecutionOwner::Unknown("engine:flyte".to_owned()));
        assert_eq!(owner.to_string(), "engine:flyte");

        let read: ExecutionOwner =
            serde_json::from_value(serde_json::json!("prefect")).expect("reads");
        assert_eq!(read, ExecutionOwner::Unknown("prefect".to_owned()));
        assert_eq!(serde_json::to_value(&read).expect("writes"), "prefect");
    }

    #[test]
    fn a_drained_node_crashed_an_attempt_and_a_parse_error_failed_it() {
        assert_eq!(
            FailureClass::Infrastructure.attempt_state(),
            StateType::Crashed
        );
        assert_eq!(FailureClass::UserCode.attempt_state(), StateType::Failed);
        assert!(FailureClass::Transient.is_retryable());
        assert!(!FailureClass::Validation.is_retryable());
        // A timeout proves nothing about the runtime, so it is worth asking
        // again — after the reactor has looked it up by idempotency key.
        assert!(FailureClass::Timeout.is_retryable());
    }

    #[test]
    fn waiting_for_a_person_is_not_a_terminal_state_and_not_a_pause() {
        assert!(!StateType::AwaitingInput.is_terminal());
        assert_ne!(StateType::AwaitingInput, StateType::Paused);
        assert!(StateType::Crashed.is_terminal());
    }

    #[test]
    fn a_state_shows_its_name_when_it_has_one_and_its_type_when_it_does_not() {
        assert_eq!(
            RunState::named(StateType::Pending, "AwaitingRetry").label(),
            "AwaitingRetry"
        );
        assert_eq!(RunState::of(StateType::Pending).label(), "pending");
    }
}
