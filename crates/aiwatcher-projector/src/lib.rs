//! The consumer pipeline.
//!
//! ```text
//! MessageSource
//!     │
//!     ├─ deduplicate        by message id, bounded
//!     ├─ publish live       LiveHub → SSE / WebSocket
//!     ├─ update read model  what the panel lists
//!     ├─ assemble           SpanAssembler
//!     ├─ flush              TraceStore + MetricSink, with retry
//!     └─ commit checkpoint  only after the flush succeeded
//! ```
//!
//! The last line is the whole at-least-once contract. Committing before the
//! write turns a crash into silent data loss; committing after turns it into a
//! redelivery, which the deterministic span ids make harmless.

pub mod conversations;
pub mod deadletter;
pub mod dedup;
pub mod dimensions;
pub mod evaluations;
pub mod live;
pub mod metrics;
pub mod pipeline;
pub mod readmodel;
pub mod retry;
pub mod spans;

pub use conversations::{ConversationFilter, ConversationPage, ConversationSummary};
pub use deadletter::{FileDeadLetters, InMemoryDeadLetters};
pub use dedup::Deduplicator;
pub use dimensions::{DimensionFilter, DimensionKind, DimensionPage, DimensionSummary};
pub use evaluations::{
    EvaluationCase, EvaluationComparison, EvaluationDetail, EvaluationFilter, EvaluationPage,
    EvaluationStatus, EvaluationSummary, SuitePage, SuiteSummary,
};
pub use live::{LiveHub, ReplayGap};
pub use metrics::{MetricsFilter, MetricsSummary};
pub use pipeline::{Projector, ProjectorConfig};
pub use readmodel::{ReadModel, RunDetail, RunFilter, RunPage, RunStatus, RunSummary};
pub use spans::{SpanFilter, SpanOutcome, SpanPage, SpanRow};
