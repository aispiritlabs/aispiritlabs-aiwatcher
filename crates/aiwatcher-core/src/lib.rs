//! Domain model for aiwatcher.
//!
//! This crate holds everything that is true regardless of transport or
//! storage: what an event looks like on the wire, how the four correlation ids
//! relate to each other, which event types exist, and the ports the outer
//! layers implement. It has no knowledge of Laser, HTTP, OTLP or axum.
//!
//! The correlation model is lifted from Emmett's `RecordedMessageMetadata`
//! (`messageId` / `streamName` / `streamPosition` / `globalPosition` /
//! `checkpoint` / `correlationId` / `causationId` / `traceId` / `spanId`) and
//! from its scope resolution rule, see [`context`].

pub mod attrs;
pub mod catalog;
pub mod checkpoint;
pub mod context;
pub mod engine;
pub mod envelope;
pub mod error;
pub mod ids;
pub mod ports;
pub mod prompts;
pub mod stream;

pub use catalog::{EventType, Phase, Subject};
pub use checkpoint::Checkpoint;
pub use context::{ContextGenerator, ObservabilityContext, SeedContext, SystemContextGenerator};
pub use engine::{
    CatalogQuery, EngineCatalog, EngineDescription, EngineExecution, EngineParameter, EnginePhase,
    EngineRef, EngineWorkflow, EntityKind, LaunchAccepted, LaunchError, LaunchRequest,
    ParameterKind, PipelineStage, WorkflowEngine,
};
pub use envelope::{
    EventEnvelope, MessageKind, RecordedEvent, RecordedMetadata, SCHEMA_VERSION, Sdk, Source,
};
pub use error::{CoreError, Result};
pub use ids::{CausationId, CorrelationId, MessageId, SpanId, TraceId};
pub use prompts::{
    ObjectEntry, ObjectStore, OptimizationOutcome, OptimizationRecord, OptimizationSummary,
    PromptError, PromptHead, PromptName, PromptSummary, PromptVersion, PromptVersionId,
    PromptVersionSummary, RejectionReason, Score, Verdict, VersionOrigin,
};
pub use stream::{GlobalPosition, StreamName, StreamPosition};
