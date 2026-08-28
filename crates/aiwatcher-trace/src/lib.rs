//! Turning an event stream into traces and metrics.
//!
//! Two things live here:
//!
//! * [`assembler`] — the state machine that folds a run's events into a handful
//!   of spans. This is where "an event is not a span" is actually enforced.
//! * [`otlp`] — OTLP/JSON exporters for VictoriaTraces and VictoriaMetrics.
//!
//! The exporters hand-roll the OTLP payload rather than using the
//! OpenTelemetry SDK, and that is deliberate. The SDK times spans as they
//! happen and mints its own ids; a projector does the opposite — it writes
//! spans whose ids and timestamps were decided by a producer, possibly hours
//! ago, and must reproduce them exactly on a replay. Fighting the SDK into that
//! shape costs more than the ~200 lines of JSON in [`otlp`].

pub mod assembler;
pub mod otlp;

pub use assembler::{Assembled, AssemblerConfig, SpanAssembler};
pub use otlp::{OtlpMetricSink, OtlpTraceStore};
