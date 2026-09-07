//! Commands are intentions; events are facts.
//!
//! One enum each, with imperative and past-tense names, because the difference
//! is the thing that keeps a decider honest: a command may be refused, an event
//! has already happened, and code that folds a command or refuses an event is
//! wrong in a way a reader can see.
//!
//! Both kinds live in the workflow stream and share its version counter, so a
//! decision is explainable afterwards: the input that caused it sits beside the
//! outputs it produced, at the version it was accepted at. Only the **facts**
//! reach the event log, and only through the outbox after commit (ADR_0026).
//!
//! ## What a message may not carry
//!
//! Rows, notebook source, secrets, a prompt, a completion, or an agent's
//! inter-node text. A step hands its data on as an [`ArtifactRef`] and its
//! answer as a bounded inline value. The last of those is conversation content
//! and belongs in the archive with a retention clock, never in a workflow row —
//! same rule, and same reason, as ADR_0021's.

use std::collections::BTreeMap;

use aiwatcher_core::{ArtifactRef, CausationId, CorrelationId, MessageId, SpanId, TraceId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;
use utoipa::ToSchema;

use crate::plan::{ExecutionPlan, RuntimeKind};
use crate::state::{ExecutionId, ExecutionMode, ExecutionOwner, InputRequest, RunState, StepError};

/// The largest a command or event payload may be, in bytes.
///
/// Section 5.4. Beyond this a step hands on an artifact, because a message that
/// grows with the data is one that eventually cannot be stored, replayed, sent
/// over SSE or shown.
pub const MAX_PAYLOAD_BYTES: usize = 256 * 1024;

/// The largest result that may stay inline rather than becoming an artifact.
pub const MAX_INLINE_RESULT_BYTES: usize = 64 * 1024;

/// Everything about a message that is not what it says.
///
/// The four correlation ids are `aiwatcher-core`'s, unchanged: an execution's
/// `correlation_id` is normally its own id, and `causation_id` names the input
/// message that produced this output. A reactor's completion event is caused by
/// the command that dispatched it, which is what makes a stream readable as a
/// chain of causes rather than a list.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
pub struct MessageMetadata {
    pub schema_version: u16,
    pub message_id: MessageId,
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
    pub correlation_id: CorrelationId,
    pub causation_id: CausationId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<TraceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span_id: Option<SpanId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u32>,
}

/// The schema version this build writes. Bumped when a message changes shape in
/// a way an older reader would misread rather than merely miss.
pub const SCHEMA_VERSION: u16 = 1;

impl MessageMetadata {
    /// Metadata for a message caused by `cause`, inside `execution`.
    #[must_use]
    pub fn caused_by(
        execution: &ExecutionId,
        cause: &MessageId,
        message_id: MessageId,
        occurred_at: OffsetDateTime,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            message_id,
            occurred_at,
            correlation_id: CorrelationId::new(execution.as_str()),
            causation_id: CausationId::new(cause.as_str()),
            trace_id: None,
            span_id: None,
            step_id: None,
            attempt: None,
        }
    }

    #[must_use]
    pub fn about_step(mut self, step_id: &str, attempt: u32) -> Self {
        self.step_id = Some(step_id.to_owned());
        self.attempt = Some(attempt);
        self
    }
}

/// What somebody wants to happen.
///
/// Two families in one enum, and the split is by who sends it. The first six
/// are **intentions** the API accepts from a caller. `ExecuteStep` and
/// `RequestInput` are **effects** the decider emits for a reactor or a worker
/// to pick up; they never reach the event log, because ADR_0026 keeps the
/// reasoning in the store and puts only facts about work on the log.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, ToSchema)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum WorkflowCommand {
    StartExecution {
        execution_id: ExecutionId,
        plan: Box<ExecutionPlan>,
        owner: ExecutionOwner,
        mode: ExecutionMode,
        requested_by: String,
        #[serde(default)]
        #[schema(value_type = Object)]
        input: BTreeMap<String, Value>,
    },
    /// Take a failed or crashed step again, from its pinned inputs and code.
    /// Historical attempts are preserved; this is a new one.
    RetryStep {
        step_id: String,
    },
    CancelExecution {
        #[serde(default)]
        reason: String,
    },
    PauseExecution,
    ResumeExecution,
    /// The answer a `HumanInput` step asked for.
    ProvideInput {
        step_id: String,
        attempt: u32,
        answered_by: String,
        #[schema(value_type = Object)]
        response: Value,
    },

    /// Dispatch one attempt. The effect command a reactor deduplicates by
    /// `command_id`, and the row a worker claims.
    ExecuteStep {
        step_id: String,
        attempt: u32,
        runtime: RuntimeKind,
        /// The stable idempotency key: `<execution>/<step>/<attempt>`. What a
        /// reactor asks a runtime by before it retries a timeout.
        idempotency_key: String,
    },
    /// Put a question in front of somebody. Not dispatched to anything: the
    /// answer arrives as `ProvideInput`.
    RequestInput {
        step_id: String,
        attempt: u32,
    },
}

impl WorkflowCommand {
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::StartExecution { .. } => "start_execution",
            Self::RetryStep { .. } => "retry_step",
            Self::CancelExecution { .. } => "cancel_execution",
            Self::PauseExecution => "pause_execution",
            Self::ResumeExecution => "resume_execution",
            Self::ProvideInput { .. } => "provide_input",
            Self::ExecuteStep { .. } => "execute_step",
            Self::RequestInput { .. } => "request_input",
        }
    }

    /// Whether this is an effect for a reactor or a worker rather than an
    /// intention from a caller. The API refuses to accept one of these.
    #[must_use]
    pub const fn is_effect(&self) -> bool {
        matches!(self, Self::ExecuteStep { .. } | Self::RequestInput { .. })
    }
}

/// What happened.
///
/// The ones a reactor or a worker reports — `StepStarted`, `StepCompleted`,
/// `StepFailed` — arrive as *inputs* to the decider, because the decider did
/// not know them: something outside performed the effect and came back. Every
/// other variant is an output the decider produced.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, ToSchema)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum WorkflowEvent {
    ExecutionRequested {
        execution_id: ExecutionId,
        plan: Box<ExecutionPlan>,
        owner: ExecutionOwner,
        mode: ExecutionMode,
        requested_by: String,
        #[serde(default)]
        #[schema(value_type = Object)]
        input: BTreeMap<String, Value>,
    },
    ExecutionStarted,
    StepScheduled {
        step_id: String,
        attempt: u32,
        runtime: RuntimeKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_key: Option<String>,
    },
    /// The delay before a retry may be dispatched, resolved to an instant here
    /// so that replay reaches the same schedule.
    StepRetryScheduled {
        step_id: String,
        attempt: u32,
        #[serde(with = "time::serde::rfc3339")]
        not_before: OffsetDateTime,
    },
    StepStarted {
        step_id: String,
        attempt: u32,
    },
    StepCompleted {
        step_id: String,
        attempt: u32,
        #[serde(default)]
        outputs: Vec<ArtifactRef>,
        /// A bounded control value. Rows go in an artifact.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schema(value_type = Object)]
        result: Option<Value>,
    },
    StepFailed {
        step_id: String,
        attempt: u32,
        error: StepError,
    },
    /// The step was answered from an earlier identical one, and this records
    /// which artifacts were reused. A hit is an execution state, not a silence.
    ///
    /// It names the **attempt** it answered, because that attempt was claimed
    /// and dispatched like any other: something has to settle its row, or it
    /// stays claimable behind a lease nobody releases.
    StepCacheHit {
        step_id: String,
        attempt: u32,
        cache_key: String,
        #[serde(default)]
        outputs: Vec<ArtifactRef>,
    },
    /// The step will not run: something it needed did not succeed.
    StepSkipped {
        step_id: String,
        reason: String,
    },
    InputRequested {
        step_id: String,
        attempt: u32,
        request: InputRequest,
    },
    InputProvided {
        step_id: String,
        attempt: u32,
        answered_by: String,
        #[schema(value_type = Object)]
        response: Value,
    },
    ExecutionPaused,
    ExecutionResumed,
    /// A cancel was accepted. Cooperative: running steps are asked to stop, and
    /// `ExecutionCancelled` follows when they have.
    ExecutionCancelling {
        #[serde(default)]
        reason: String,
    },
    ExecutionCancelled,
    ExecutionCompleted,
    ExecutionFailed {
        reason: String,
    },
}

impl WorkflowEvent {
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::ExecutionRequested { .. } => "execution_requested",
            Self::ExecutionStarted => "execution_started",
            Self::StepScheduled { .. } => "step_scheduled",
            Self::StepRetryScheduled { .. } => "step_retry_scheduled",
            Self::StepStarted { .. } => "step_started",
            Self::StepCompleted { .. } => "step_completed",
            Self::StepFailed { .. } => "step_failed",
            Self::StepCacheHit { .. } => "step_cache_hit",
            Self::StepSkipped { .. } => "step_skipped",
            Self::InputRequested { .. } => "input_requested",
            Self::InputProvided { .. } => "input_provided",
            Self::ExecutionPaused => "execution_paused",
            Self::ExecutionResumed => "execution_resumed",
            Self::ExecutionCancelling { .. } => "execution_cancelling",
            Self::ExecutionCancelled => "execution_cancelled",
            Self::ExecutionCompleted => "execution_completed",
            Self::ExecutionFailed { .. } => "execution_failed",
        }
    }
}

/// One thing in the stream, whichever kind it is.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkflowMessage {
    Command(WorkflowCommand),
    Event(WorkflowEvent),
}

impl WorkflowMessage {
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Command(command) => command.name(),
            Self::Event(event) => event.name(),
        }
    }

    #[must_use]
    pub const fn is_event(&self) -> bool {
        matches!(self, Self::Event(_))
    }

    #[must_use]
    pub fn event(&self) -> Option<&WorkflowEvent> {
        match self {
            Self::Event(event) => Some(event),
            Self::Command(_) => None,
        }
    }

    #[must_use]
    pub fn command(&self) -> Option<&WorkflowCommand> {
        match self {
            Self::Command(command) => Some(command),
            Self::Event(_) => None,
        }
    }
}

/// Which side of a decision a message was on.
///
/// Recording both in one stream is what makes a decision explainable: the input
/// that caused it and the outputs it produced sit together, at the version it
/// was accepted at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Input,
    Output,
}

/// A message as the stream holds it.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, ToSchema)]
pub struct RecordedMessage {
    pub stream_version: u64,
    pub direction: Direction,
    pub message: WorkflowMessage,
    pub metadata: MessageMetadata,
    #[serde(with = "time::serde::rfc3339")]
    pub recorded_at: OffsetDateTime,
}

/// A message on its way to the store, before it has a version.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct PendingMessage {
    pub direction: Direction,
    pub message: WorkflowMessage,
    pub metadata: MessageMetadata,
}

impl PendingMessage {
    #[must_use]
    pub fn input(message: WorkflowMessage, metadata: MessageMetadata) -> Self {
        Self {
            direction: Direction::Input,
            message,
            metadata,
        }
    }

    #[must_use]
    pub fn output(message: WorkflowMessage, metadata: MessageMetadata) -> Self {
        Self {
            direction: Direction::Output,
            message,
            metadata,
        }
    }

    /// Whether this fits inside [`MAX_PAYLOAD_BYTES`].
    ///
    /// Checked before an append rather than trusted, because the payload that
    /// breaks it is somebody inlining a result on the day the corpus grew.
    #[must_use]
    pub fn payload_size(&self) -> usize {
        serde_json::to_vec(&self.message).map_or(usize::MAX, |bytes| bytes.len())
    }
}

/// What an outbox row carries to the event log after the decision committed.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct OutboxMessage {
    pub message_id: MessageId,
    /// The event type from `aiwatcher_core::catalog` this publishes as.
    pub event_type: String,
    /// `workflow:<execution_id>`, per section 11.2.
    pub partition_key: String,
    pub payload: Value,
    #[serde(with = "time::serde::rfc3339")]
    pub available_at: OffsetDateTime,
    pub attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "time::serde::rfc3339::option")]
    pub published_at: Option<OffsetDateTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// The run projection a store keeps beside the stream.
///
/// Rebuildable from the stream, and updated in the same transaction that
/// appends to it. It exists to *accept the next command* without folding a
/// whole history, and for the run's own page — never as the source of a list the
/// event log's own folds already serve (ADR_0026).
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, ToSchema)]
pub struct RunProjection {
    pub execution_id: ExecutionId,
    pub plan_id: String,
    pub definition_name: String,
    pub owner: ExecutionOwner,
    pub mode: ExecutionMode,
    pub state: RunState,
    pub requested_by: String,
    pub steps: Vec<crate::state::StepState>,
    pub last_message_version: u64,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "time::serde::rfc3339::option")]
    pub started_at: Option<OffsetDateTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "time::serde::rfc3339::option")]
    pub ended_at: Option<OffsetDateTime>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_effect_command_is_told_apart_from_an_intention_a_caller_may_send() {
        // The API accepts the second kind and never the first: `ExecuteStep`
        // is what the decider emits, and a caller that could post one would be
        // scheduling work behind the state machine's back.
        assert!(
            WorkflowCommand::ExecuteStep {
                step_id: "one".to_owned(),
                attempt: 1,
                runtime: RuntimeKind::FlowPhp,
                idempotency_key: "e/one/1".to_owned(),
            }
            .is_effect()
        );
        assert!(!WorkflowCommand::PauseExecution.is_effect());
        assert!(
            !WorkflowCommand::RetryStep {
                step_id: "one".to_owned()
            }
            .is_effect()
        );
    }

    #[test]
    fn a_message_says_which_kind_it_is_and_which_one_of_that_kind() {
        // Two tags rather than one flattened union: `kind` is what the store's
        // check constraint and the dead-letter rule read, and the second is
        // what a reader and a `match` both understand.
        let json = serde_json::to_value(WorkflowMessage::Event(WorkflowEvent::ExecutionStarted))
            .expect("serialising an event");
        assert_eq!(json["kind"], "event");
        assert_eq!(json["event"], "execution_started");

        let json = serde_json::to_value(WorkflowMessage::Command(WorkflowCommand::PauseExecution))
            .expect("serialising a command");
        assert_eq!(json["kind"], "command");
        assert_eq!(json["command"], "pause_execution");
    }

    #[test]
    fn a_result_big_enough_to_be_rows_is_visible_before_it_is_appended() {
        // The limit is not a formatting preference: a message that grows with
        // the data is one that eventually cannot be stored, replayed or shown.
        let metadata = MessageMetadata::caused_by(
            &ExecutionId::new("e"),
            &MessageId::new("cause"),
            MessageId::new("m"),
            OffsetDateTime::UNIX_EPOCH,
        );
        let rows = Value::String("x".repeat(MAX_PAYLOAD_BYTES));
        let message = PendingMessage::output(
            WorkflowMessage::Event(WorkflowEvent::StepCompleted {
                step_id: "one".to_owned(),
                attempt: 1,
                outputs: Vec::new(),
                result: Some(rows),
            }),
            metadata,
        );
        assert!(message.payload_size() > MAX_PAYLOAD_BYTES);
    }
}
