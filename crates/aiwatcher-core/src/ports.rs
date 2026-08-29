//! The seams.
//!
//! Everything the domain needs from the outside world is a trait here. There
//! is exactly one implementation of each in production today, and that is not
//! the point — the point is that swapping VictoriaTraces for QuestDB, or Laser
//! for something else, is an adapter change and never a domain change.
//!
//! The span and metric types are deliberately *not* OpenTelemetry SDK types.
//! A projector replays history: it writes spans whose ids and timestamps were
//! decided elsewhere, sometimes hours ago. The OTel SDK is built to time spans
//! as they happen and mints its own ids, which is the wrong shape for this.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::OffsetDateTime;

use crate::catalog::EventType;
use crate::checkpoint::Checkpoint;
use crate::envelope::RecordedEvent;
use crate::ids::{SpanId, TraceId};

#[derive(Debug, Error)]
pub enum PortError {
    /// The backend is unreachable or rejected the batch, but the batch itself
    /// is fine — the caller should retry rather than dead-letter.
    #[error("{target} is unavailable: {message}")]
    Unavailable {
        target: &'static str,
        message: String,
    },

    /// The backend understood the batch and refused it. Retrying will not
    /// help; this is dead-letter territory.
    #[error("{target} rejected the batch: {message}")]
    Rejected {
        target: &'static str,
        message: String,
    },

    #[error("{target} failed: {source}")]
    Other {
        target: &'static str,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl PortError {
    /// Whether the pipeline should retry rather than park the batch.
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        matches!(self, Self::Unavailable { .. } | Self::Other { .. })
    }
}

pub type PortResult<T> = std::result::Result<T, PortError>;

/// A value on a span or a metric sample.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AttrValue {
    Bool(bool),
    Int(i64),
    Double(f64),
    Str(String),
    StrList(Vec<String>),
}

impl From<&str> for AttrValue {
    fn from(value: &str) -> Self {
        Self::Str(value.to_owned())
    }
}

impl From<String> for AttrValue {
    fn from(value: String) -> Self {
        Self::Str(value)
    }
}

impl From<i64> for AttrValue {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}

impl From<u64> for AttrValue {
    fn from(value: u64) -> Self {
        // OTLP has no unsigned integer; saturating keeps the value readable
        // instead of wrapping it negative.
        Self::Int(i64::try_from(value).unwrap_or(i64::MAX))
    }
}

impl From<f64> for AttrValue {
    fn from(value: f64) -> Self {
        Self::Double(value)
    }
}

impl From<bool> for AttrValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

/// A key/value pair on a span.
pub type Attr = (String, AttrValue);

/// Helper for building attribute lists without repeating `.to_owned()`.
#[must_use]
pub fn attr(key: &str, value: impl Into<AttrValue>) -> Attr {
    (key.to_owned(), value.into())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SpanKind {
    Internal,
    Server,
    Client,
    Producer,
    Consumer,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum SpanStatus {
    Unset,
    Ok,
    Error { message: String },
}

/// A point in time inside a span. `llm.first_token` becomes one of these.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpanEvent {
    pub name: String,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    pub attributes: Vec<Attr>,
}

/// A pointer to a span in another trace. Used where a causal edge crosses a
/// run boundary and a parent/child edge would be a lie.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpanLink {
    pub trace_id: TraceId,
    pub span_id: SpanId,
    pub attributes: Vec<Attr>,
}

/// A span that is finished and ready to be written.
///
/// Only ever produced by the assembler, and only from an end event or the
/// orphan sweep — nothing writes a span that might still receive children.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompletedSpan {
    pub trace_id: TraceId,
    pub span_id: SpanId,
    pub parent_span_id: Option<SpanId>,
    pub name: String,
    pub kind: SpanKind,
    #[serde(with = "time::serde::rfc3339")]
    pub start: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub end: OffsetDateTime,
    pub status: SpanStatus,
    pub attributes: Vec<Attr>,
    pub events: Vec<SpanEvent>,
    pub links: Vec<SpanLink>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricKind {
    /// Monotonic count. Written as an OTLP delta sum.
    Counter,
    /// Distribution. Written as an OTLP histogram with one observation.
    Histogram,
    /// Last known value.
    Gauge,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MetricSample {
    pub name: String,
    pub kind: MetricKind,
    pub value: f64,
    /// Only meaningful for [`MetricKind::Histogram`]; the OTLP unit string.
    pub unit: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    pub attributes: Vec<Attr>,
}

/// What a browser receives over SSE or WebSocket.
///
/// Kept small on purpose: the panel gets the event as it happened, not a
/// re-derived projection, and `checkpoint` is what a reconnecting client sends
/// back as `Last-Event-ID`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LiveEvent {
    pub checkpoint: Checkpoint,
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    /// Carried for the same reason `conversation_id` is: the live channel is
    /// filtered server-side, and a subscriber watching one workflow execution
    /// cannot be served by resolving it to a set of run ids at subscribe time —
    /// a stage that starts *after* the browser connected would be filtered out
    /// by the set it was given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_run_id: Option<String>,
    pub trace_id: TraceId,
    pub span_id: SpanId,
    pub event_type: EventType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
    #[schema(value_type = Object)]
    pub data: serde_json::Value,
}

impl From<&RecordedEvent> for LiveEvent {
    fn from(event: &RecordedEvent) -> Self {
        Self {
            checkpoint: event.metadata.checkpoint.clone(),
            run_id: event.metadata.run_id.clone(),
            conversation_id: event.metadata.conversation_id.clone(),
            workflow_id: event.metadata.workflow_id.clone(),
            workflow_run_id: event.metadata.workflow_run_id.clone(),
            trace_id: event.metadata.trace_id,
            span_id: event.metadata.span_id,
            event_type: event.event_type.clone(),
            sequence: event.metadata.sequence,
            occurred_at: event.metadata.occurred_at,
            data: event.data.clone(),
        }
    }
}

/// An event the pipeline could not process, with why.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeadLetter {
    pub checkpoint: Checkpoint,
    /// The bytes as received. Kept raw so a malformed event is still
    /// inspectable — a parsed representation would have lost the problem.
    pub raw: String,
    pub reason: String,
    pub attempts: u32,
    #[serde(with = "time::serde::rfc3339")]
    pub parked_at: OffsetDateTime,
}

/// Where finished spans go. VictoriaTraces in production, via OTLP.
#[async_trait]
pub trait TraceStore: Send + Sync + std::fmt::Debug {
    async fn write_spans(&self, spans: Vec<CompletedSpan>) -> PortResult<()>;
}

/// Where aggregates go. VictoriaMetrics in production, via OTLP.
#[async_trait]
pub trait MetricSink: Send + Sync + std::fmt::Debug {
    async fn record(&self, samples: Vec<MetricSample>) -> PortResult<()>;
}

/// The push side of the live channel. Implemented by the in-process hub the
/// SSE and WebSocket handlers read from.
#[async_trait]
pub trait LivePublisher: Send + Sync + std::fmt::Debug {
    async fn publish(&self, event: LiveEvent) -> PortResult<()>;
}

/// Where events that cannot be processed are parked.
#[async_trait]
pub trait DeadLetterSink: Send + Sync + std::fmt::Debug {
    async fn park(&self, letter: DeadLetter) -> PortResult<()>;
}

/// Asking for a workflow to be run again.
///
/// aiwatcher observes; it does not orchestrate. Every other port here writes
/// something aiwatcher derived — this one asks somebody else to do work, which
/// is why it is the only port whose absence is a 501 rather than a no-op: a
/// null runner would report success for a rerun that never happened.
///
/// The adapter's target comes from configuration and never from an event. A
/// declaration that named its own callback URL would be a request-forgery
/// primitive posted by anything that can reach ingest.
#[async_trait]
pub trait WorkflowRunner: Send + Sync + std::fmt::Debug {
    async fn rerun(&self, request: RerunRequest) -> PortResult<RerunAccepted>;
}

/// What is asked of the orchestrator.
///
/// Deliberately thin, and deliberately not a description of *how* to run
/// anything: aiwatcher knows the shape of a graph because a producer declared
/// it, not because it can execute one. Everything here is a name the producer
/// already chose.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RerunRequest {
    /// The orchestration to run. Always present.
    pub workflow_id: String,
    /// The execution being repeated, when there is one. `None` asks for a
    /// fresh execution rather than a repeat of a particular traversal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_run_id: Option<String>,
    /// Resume from this node rather than from the start.
    ///
    /// Advisory: whether an orchestrator can resume mid-graph is its business,
    /// and one that cannot is free to start over. aiwatcher cannot verify the
    /// difference and does not claim to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_node: Option<String>,
    /// Passed through untouched.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    #[schema(value_type = Object)]
    pub inputs: serde_json::Value,
}

/// What came back. Not a result — the work has not happened yet.
///
/// A rerun is accepted, not completed: the evidence that it ran is the events
/// it publishes, on the same log as everything else. `reference` is whatever
/// the orchestrator calls the thing it just queued, so a caller can find it in
/// that orchestrator's own console.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RerunAccepted {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// Where to watch it, if the orchestrator said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_is_retryable_and_rejected_is_not() {
        let unavailable = PortError::Unavailable {
            target: "victoriatraces",
            message: "connection refused".to_owned(),
        };
        let rejected = PortError::Rejected {
            target: "victoriatraces",
            message: "400 bad request".to_owned(),
        };
        assert!(unavailable.is_retryable());
        assert!(!rejected.is_retryable());
    }

    #[test]
    fn a_u64_attribute_saturates_rather_than_wrapping_negative() {
        assert_eq!(AttrValue::from(u64::MAX), AttrValue::Int(i64::MAX));
        assert_eq!(AttrValue::from(7u64), AttrValue::Int(7));
    }

    #[test]
    fn attributes_serialise_as_bare_json_values() {
        let attributes = vec![
            attr("a", "text"),
            attr("b", 7i64),
            attr("c", true),
            attr("d", 1.5f64),
        ];
        let json = serde_json::to_value(&attributes).expect("serializes");
        assert_eq!(json[0][1], "text");
        assert_eq!(json[1][1], 7);
        assert_eq!(json[2][1], true);
        assert_eq!(json[3][1], 1.5);
    }
}
