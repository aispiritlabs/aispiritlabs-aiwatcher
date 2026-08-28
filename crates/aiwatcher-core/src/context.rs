//! How the four ids resolve.
//!
//! Emmett's scope resolves them like this (`almanac/src/scopes/scope.ts`):
//!
//! ```text
//! traceId       = span.traceId || inherited.traceId || generate()
//! spanId        = span.spanId                          // a child always mints its own
//! correlationId = inherited.correlationId ?? generate()
//! causationId   = inherited.causationId   ?? correlationId
//! ```
//!
//! The last line is the one worth keeping: an event that nothing explicitly
//! caused roots its causation on the correlation, so `causation_id` is never
//! empty and "what caused this" always has an answer, even at the root of a
//! flow.
//!
//! aiwatcher applies the same rule with one change forced by at-least-once
//! delivery: where Emmett *generates* a trace or span id, we *derive* it (see
//! [`crate::ids`]). Generation is only used for a correlation id, which no
//! amount of replay can reconstruct if the producer never sent one.

use serde::{Deserialize, Serialize};

use crate::ids::{CausationId, CorrelationId, MessageId, SpanId, TraceId};

/// What a producer sent, before resolution. Every field is optional — that is
/// the point: this is the partial context, [`ObservabilityContext`] is the
/// complete one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeedContext {
    pub trace_id: Option<TraceId>,
    pub span_id: Option<SpanId>,
    pub parent_span_id: Option<SpanId>,
    pub correlation_id: Option<CorrelationId>,
    pub causation_id: Option<CausationId>,
}

impl SeedContext {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// The resolved context an event carries. Children inherit it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ObservabilityContext {
    pub trace_id: TraceId,
    pub span_id: SpanId,
    pub parent_span_id: Option<SpanId>,
    pub correlation_id: CorrelationId,
    pub causation_id: CausationId,
}

/// The inputs resolution needs beyond the seed: what the event is *about*.
///
/// `run_id` is what a missing trace id derives from and `span_key` is what a
/// missing span id derives from, so both must be stable across redeliveries.
#[derive(Clone, Copy, Debug)]
pub struct ResolveTarget<'a> {
    pub run_id: &'a str,
    pub span_key: &'a str,
}

impl ObservabilityContext {
    /// Fill in whatever the producer left out.
    ///
    /// Precedence is always: what the producer sent, then what the parent
    /// context carries, then what can be derived from the run. Nothing is
    /// invented that a replay could not reproduce, except a correlation id —
    /// and that one falls back to the message id so it is at least stable for
    /// this event.
    #[must_use]
    pub fn resolve(
        seed: &SeedContext,
        inherited: Option<&ObservabilityContext>,
        target: ResolveTarget<'_>,
        message_id: &MessageId,
    ) -> Self {
        let trace_id = seed
            .trace_id
            .or_else(|| inherited.map(|ctx| ctx.trace_id))
            .unwrap_or_else(|| TraceId::derive(target.run_id));

        // A child always mints its own span id — inheriting one would collapse
        // the child into its parent in the waterfall.
        let span_id = seed
            .span_id
            .unwrap_or_else(|| SpanId::derive(trace_id, target.span_key));

        let parent_span_id = seed
            .parent_span_id
            .or_else(|| inherited.map(|ctx| ctx.span_id))
            .filter(|parent| *parent != span_id);

        let correlation_id = seed
            .correlation_id
            .clone()
            .or_else(|| inherited.map(|ctx| ctx.correlation_id.clone()))
            .unwrap_or_else(|| CorrelationId::new(message_id.as_str()));

        // Emmett's rule: an unseeded causation roots itself on the correlation.
        let causation_id = seed
            .causation_id
            .clone()
            .or_else(|| inherited.map(|ctx| ctx.causation_id.clone()))
            .unwrap_or_else(|| CausationId::new(correlation_id.as_str()));

        Self {
            trace_id,
            span_id,
            parent_span_id,
            correlation_id,
            causation_id,
        }
    }
}

/// Where fresh ids come from. Injected so tests can pin them.
pub trait ContextGenerator: Send + Sync + std::fmt::Debug {
    fn generate_message_id(&self) -> MessageId;
    fn generate_correlation_id(&self) -> CorrelationId;
}

/// The production generator: UUIDv7, so message ids sort by creation time.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemContextGenerator;

impl ContextGenerator for SystemContextGenerator {
    fn generate_message_id(&self) -> MessageId {
        MessageId::generate()
    }

    fn generate_correlation_id(&self) -> CorrelationId {
        CorrelationId::new(uuid::Uuid::now_v7().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target<'a>() -> ResolveTarget<'a> {
        ResolveTarget {
            run_id: "run-1",
            span_key: "llm:call-1",
        }
    }

    #[test]
    fn an_unseeded_causation_roots_on_the_correlation() {
        let seed = SeedContext {
            correlation_id: Some(CorrelationId::new("corr-1")),
            ..SeedContext::default()
        };
        let ctx = ObservabilityContext::resolve(&seed, None, target(), &MessageId::new("m-1"));
        assert_eq!(ctx.causation_id.as_str(), "corr-1");
    }

    #[test]
    fn a_correlation_falls_back_to_the_message_id() {
        let ctx = ObservabilityContext::resolve(
            &SeedContext::default(),
            None,
            target(),
            &MessageId::new("m-1"),
        );
        assert_eq!(ctx.correlation_id.as_str(), "m-1");
        assert_eq!(ctx.causation_id.as_str(), "m-1");
    }

    #[test]
    fn resolution_is_deterministic_so_a_redelivery_lands_on_the_same_span() {
        let first = ObservabilityContext::resolve(
            &SeedContext::default(),
            None,
            target(),
            &MessageId::new("m-1"),
        );
        let second = ObservabilityContext::resolve(
            &SeedContext::default(),
            None,
            target(),
            &MessageId::new("m-1"),
        );
        assert_eq!(first, second);
        assert_eq!(first.trace_id, TraceId::derive("run-1"));
    }

    #[test]
    fn what_the_producer_sends_wins_over_what_could_be_derived() {
        let trace = TraceId::derive("some-other-run");
        let span = SpanId::derive(trace, "explicit");
        let seed = SeedContext {
            trace_id: Some(trace),
            span_id: Some(span),
            ..SeedContext::default()
        };
        let ctx = ObservabilityContext::resolve(&seed, None, target(), &MessageId::new("m-1"));
        assert_eq!(ctx.trace_id, trace);
        assert_eq!(ctx.span_id, span);
    }

    #[test]
    fn a_child_mints_its_own_span_and_points_at_its_parent() {
        let parent = ObservabilityContext::resolve(
            &SeedContext::default(),
            None,
            ResolveTarget {
                run_id: "run-1",
                span_key: "agent:researcher",
            },
            &MessageId::new("m-1"),
        );
        let child = ObservabilityContext::resolve(
            &SeedContext::default(),
            Some(&parent),
            target(),
            &MessageId::new("m-2"),
        );

        assert_eq!(child.trace_id, parent.trace_id);
        assert_ne!(child.span_id, parent.span_id);
        assert_eq!(child.parent_span_id, Some(parent.span_id));
        assert_eq!(child.correlation_id, parent.correlation_id);
    }

    #[test]
    fn a_span_never_becomes_its_own_parent() {
        let span = SpanId::derive(TraceId::derive("run-1"), "llm:call-1");
        let seed = SeedContext {
            span_id: Some(span),
            parent_span_id: Some(span),
            ..SeedContext::default()
        };
        let ctx = ObservabilityContext::resolve(&seed, None, target(), &MessageId::new("m-1"));
        assert_eq!(ctx.parent_span_id, None);
    }
}
