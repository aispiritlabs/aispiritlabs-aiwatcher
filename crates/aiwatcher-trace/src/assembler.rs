//! Folds events into spans.
//!
//! ## The shape it produces
//!
//! ```text
//! run                             trace
//! └── agent execution             span
//!     ├── LLM call                span
//!     │   ├── first token         span event
//!     │   └── chunks              counted, never stored individually
//!     └── tool call               span
//! ```
//!
//! ## Parenting
//!
//! An explicit `parent_span_id` from the producer always wins, and an SDK that
//! tracks its own scope stack should always send one — it knows the nesting
//! exactly, and the backend can only infer it.
//!
//! Inference is the fallback, for producers that cannot. The parent is then
//! **the most recently opened still-open container span** in the same run,
//! where a run, an agent and a step are containers and an LLM or tool call is
//! not. That covers the two cases a naive stack gets wrong: two LLM calls
//! issued in parallel both parent onto their agent rather than onto each other,
//! and a sub-agent still nests inside the agent that spawned it.
//!
//! Steps are containers, so a retrieval that wraps an embedding nests
//! correctly. A leaf that wraps another leaf — a model calling a model — is the
//! shape inference cannot see; that one needs an explicit parent.
//!
//! ## Why spans are only written on an end event
//!
//! A span that is still open may still gain children and attributes. Writing it
//! early means either rewriting it later — which trace stores do not support —
//! or losing what came after. The cost is that a producer which crashes without
//! sending its end events leaves spans open; [`SpanAssembler::sweep`] closes
//! those, and marks them so the difference stays visible.

use std::collections::HashMap;

use time::OffsetDateTime;
use time::ext::NumericalDuration;

use aiwatcher_core::attrs::{aiwatcher as own, genai, messaging};
use aiwatcher_core::ports::{
    Attr, AttrValue, CompletedSpan, MetricKind, MetricSample, SpanEvent, SpanKind, SpanStatus, attr,
};
use aiwatcher_core::{EventType, Phase, RecordedEvent, SpanId, Subject, TraceId, catalog};

/// How a span was closed. Emitted as [`own::span::CLOSED_BY`] so a dashboard
/// can tell a real completion from one this code had to invent.
mod closed_by {
    pub const EVENT: &str = "event";
    /// The end event never arrived and the orphan sweep closed it.
    pub const TIMEOUT: &str = "timeout";
    /// Only an end event was ever seen; the start was inferred from it.
    pub const SYNTHESISED_START: &str = "synthesised_start";
}

#[derive(Clone, Debug)]
pub struct AssemblerConfig {
    /// How long a span may sit without any new event before the sweep closes
    /// it. Must comfortably exceed the slowest legitimate LLM call.
    pub orphan_timeout: time::Duration,
    /// Upper bound on spans held open at once. Past it, the oldest are swept
    /// early rather than growing memory without limit.
    ///
    /// Ten thousand is far more than any healthy workload holds open; reaching
    /// it means producers have stopped sending end events. Part of the 512 MB
    /// budget documented on `ReadModelConfig`.
    pub max_open_spans: usize,
    /// Record a span event for each `llm.first_token`. Cheap and the only place
    /// time-to-first-token is visible inside the waterfall.
    pub record_first_token_event: bool,
}

impl Default for AssemblerConfig {
    fn default() -> Self {
        Self {
            orphan_timeout: time::Duration::minutes(15),
            max_open_spans: 10_000,
            record_first_token_event: true,
        }
    }
}

/// What one ingested event produced.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Assembled {
    pub spans: Vec<CompletedSpan>,
    pub metrics: Vec<MetricSample>,
}

impl Assembled {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty() && self.metrics.is_empty()
    }
}

#[derive(Clone, Debug)]
struct OpenSpan {
    trace_id: TraceId,
    span_id: SpanId,
    parent_span_id: Option<SpanId>,
    subject: Subject,
    name: String,
    kind: SpanKind,
    start: OffsetDateTime,
    last_seen: OffsetDateTime,
    attributes: Vec<Attr>,
    events: Vec<SpanEvent>,
    chunk_count: u64,
    first_token_at: Option<OffsetDateTime>,
    run_id: String,
}

impl OpenSpan {
    /// Whether this span can hold children.
    ///
    /// LLM and tool calls are leaves: an LLM call does not contain a tool call,
    /// it precedes one. Steps do contain things — a retrieval wrapping an
    /// embedding is the ordinary case.
    fn is_container(&self) -> bool {
        matches!(self.subject, Subject::Run | Subject::Agent | Subject::Step)
    }

    fn close(
        mut self,
        end: OffsetDateTime,
        status: SpanStatus,
        closed_by: &str,
        extra: Vec<Attr>,
    ) -> CompletedSpan {
        self.attributes.extend(extra);
        self.attributes.push(attr(own::span::CLOSED_BY, closed_by));
        if self.chunk_count > 0 {
            self.attributes
                .push(attr(own::span::CHUNK_COUNT, self.chunk_count));
        }
        CompletedSpan {
            trace_id: self.trace_id,
            span_id: self.span_id,
            parent_span_id: self.parent_span_id,
            name: self.name,
            kind: self.kind,
            start: self.start,
            // A clock skew between producers must not produce a negative
            // duration; clamp instead.
            end: end.max(self.start),
            status,
            attributes: self.attributes,
            events: self.events,
            links: Vec::new(),
        }
    }
}

/// Per-run bookkeeping: which container spans are open, most recent last.
#[derive(Debug, Default)]
struct RunState {
    containers: Vec<SpanId>,
}

/// Folds a run's events into spans and metrics.
///
/// Not `Sync`: one assembler per consumer task, and Laser's partitioning
/// guarantees a run's events all reach the same task in order.
#[derive(Debug)]
pub struct SpanAssembler {
    config: AssemblerConfig,
    open: HashMap<(TraceId, SpanId), OpenSpan>,
    runs: HashMap<String, RunState>,
}

impl Default for SpanAssembler {
    fn default() -> Self {
        Self::new(AssemblerConfig::default())
    }
}

impl SpanAssembler {
    #[must_use]
    pub fn new(config: AssemblerConfig) -> Self {
        Self {
            config,
            open: HashMap::new(),
            runs: HashMap::new(),
        }
    }

    /// How many spans are currently open. Exported as a gauge; a number that
    /// only grows means producers are not sending end events.
    #[must_use]
    pub fn open_span_count(&self) -> usize {
        self.open.len()
    }

    /// Fold one event in.
    pub fn ingest(&mut self, event: &RecordedEvent) -> Assembled {
        if !event.event_type.forms_span() {
            // An evaluation report has a start and an end, and is still not a
            // trace: it is folded by the evaluation projection and written
            // nowhere near the trace store. See `EventType::forms_span`.
            return Assembled::default();
        }
        let Some(phase) = event.event_type.phase() else {
            // An event type this build does not know takes part in no span. It
            // was still published live and stored in the log.
            return Assembled::default();
        };

        match phase {
            Phase::Start => {
                self.open_span(event);
                Assembled::default()
            }
            Phase::Point => self.record_point(event),
            Phase::End { ok } => self.close_span(event, ok),
        }
    }

    /// Close spans that have gone quiet, and report them.
    ///
    /// Called on a timer by the projector. A swept span is marked
    /// `closed_by=timeout` and given an error status: a run whose LLM call
    /// simply vanished did not succeed, and showing it as `Ok` would be a lie.
    pub fn sweep(&mut self, now: OffsetDateTime) -> Assembled {
        let deadline = now - self.config.orphan_timeout;
        let mut stale: Vec<(TraceId, SpanId)> = self
            .open
            .iter()
            .filter(|(_, span)| span.last_seen <= deadline)
            .map(|(key, _)| *key)
            .collect();

        // Over the cap, shed the oldest regardless of the timeout — an
        // unbounded map is a slower outage than a few early-closed spans.
        if self.open.len().saturating_sub(stale.len()) > self.config.max_open_spans {
            let mut by_age: Vec<_> = self
                .open
                .iter()
                .filter(|(key, _)| !stale.contains(key))
                .map(|(key, span)| (*key, span.last_seen))
                .collect();
            by_age.sort_by_key(|(_, last_seen)| *last_seen);
            let excess = self.open.len() - stale.len() - self.config.max_open_spans;
            stale.extend(by_age.into_iter().take(excess).map(|(key, _)| key));
        }

        let mut assembled = Assembled::default();
        for key in stale {
            let Some(span) = self.open.remove(&key) else {
                continue;
            };
            self.pop_container(&span);
            let last_seen = span.last_seen;
            assembled.metrics.push(MetricSample {
                name: own::metrics::SPANS_ORPHANED.to_owned(),
                kind: MetricKind::Counter,
                value: 1.0,
                unit: None,
                at: now,
                attributes: vec![attr(own::event::TYPE, span.subject.as_str())],
            });
            assembled.spans.push(span.close(
                last_seen,
                SpanStatus::Error {
                    message: "no end event arrived before the orphan timeout".to_owned(),
                },
                closed_by::TIMEOUT,
                Vec::new(),
            ));
        }
        assembled
    }

    /// Close everything still open, e.g. on graceful shutdown, so a restart
    /// does not lose in-flight spans.
    pub fn drain(&mut self, now: OffsetDateTime) -> Assembled {
        let keys: Vec<_> = self.open.keys().copied().collect();
        let mut assembled = Assembled::default();
        for key in keys {
            let Some(span) = self.open.remove(&key) else {
                continue;
            };
            self.pop_container(&span);
            let last_seen = span.last_seen;
            assembled.spans.push(span.close(
                last_seen,
                SpanStatus::Error {
                    message: "projector shut down while the span was open".to_owned(),
                },
                closed_by::TIMEOUT,
                Vec::new(),
            ));
        }
        let _ = now;
        assembled
    }

    fn open_span(&mut self, event: &RecordedEvent) -> SpanId {
        let key = (event.metadata.trace_id, event.metadata.span_id);
        if let Some(existing) = self.open.get_mut(&key) {
            // A redelivered start. Refresh liveness, change nothing else.
            existing.last_seen = event.metadata.occurred_at;
            return existing.span_id;
        }

        let subject = event.event_type.subject();
        let parent = event
            .metadata
            .parent_span_id
            .or_else(|| self.infer_parent(&event.metadata.run_id, event.metadata.span_id));

        let span = OpenSpan {
            trace_id: event.metadata.trace_id,
            span_id: event.metadata.span_id,
            parent_span_id: parent,
            subject,
            name: event.event_type.span_name(span_target(event)),
            kind: span_kind(event),
            start: event.metadata.occurred_at,
            last_seen: event.metadata.occurred_at,
            attributes: base_attributes(event),
            events: Vec::new(),
            chunk_count: 0,
            first_token_at: None,
            run_id: event.metadata.run_id.clone(),
        };

        if span.is_container() {
            self.runs
                .entry(event.metadata.run_id.clone())
                .or_default()
                .containers
                .push(span.span_id);
        }
        self.open.insert(key, span);
        event.metadata.span_id
    }

    fn record_point(&mut self, event: &RecordedEvent) -> Assembled {
        let key = (event.metadata.trace_id, event.metadata.span_id);
        let Some(span) = self.open.get_mut(&key) else {
            // A chunk or first-token for a call we never saw start. Nothing to
            // attach it to; it already went out live.
            return Assembled::default();
        };
        span.last_seen = event.metadata.occurred_at;

        match event.event_type {
            EventType::LlmChunk => {
                span.chunk_count += 1;
            }
            EventType::LlmFirstToken => {
                span.first_token_at = Some(event.metadata.occurred_at);
                if self.config.record_first_token_event {
                    span.events.push(SpanEvent {
                        name: "gen_ai.first_token".to_owned(),
                        at: event.metadata.occurred_at,
                        attributes: Vec::new(),
                    });
                }
                let ttft = (event.metadata.occurred_at - span.start).as_seconds_f64();
                return Assembled {
                    spans: Vec::new(),
                    metrics: vec![MetricSample {
                        name: genai::metrics::TIME_TO_FIRST_TOKEN.to_owned(),
                        kind: MetricKind::Histogram,
                        value: ttft.max(0.0),
                        unit: Some("s".to_owned()),
                        at: event.metadata.occurred_at,
                        attributes: model_attributes(event),
                    }],
                };
            }
            _ => {}
        }
        Assembled::default()
    }

    fn close_span(&mut self, event: &RecordedEvent, ok: bool) -> Assembled {
        let key = (event.metadata.trace_id, event.metadata.span_id);
        let (span, closed_by) = match self.open.remove(&key) {
            Some(span) => (span, closed_by::EVENT),
            None => {
                // Only an end event was seen. Back-date the start from
                // `duration_ms` where the producer sent one, so the span still
                // has a plausible width instead of collapsing to a point.
                let duration = event
                    .data_f64("duration_ms")
                    .filter(|ms| ms.is_finite() && *ms >= 0.0)
                    .map_or(time::Duration::ZERO, |ms| ms.milliseconds());
                let subject = event.event_type.subject();
                (
                    OpenSpan {
                        trace_id: event.metadata.trace_id,
                        span_id: event.metadata.span_id,
                        parent_span_id: event.metadata.parent_span_id.or_else(|| {
                            self.infer_parent(&event.metadata.run_id, event.metadata.span_id)
                        }),
                        subject,
                        name: event.event_type.span_name(span_target(event)),
                        kind: span_kind(event),
                        start: event.metadata.occurred_at - duration,
                        last_seen: event.metadata.occurred_at,
                        attributes: base_attributes(event),
                        events: Vec::new(),
                        chunk_count: 0,
                        first_token_at: None,
                        run_id: event.metadata.run_id.clone(),
                    },
                    closed_by::SYNTHESISED_START,
                )
            }
        };
        self.pop_container(&span);

        let status = if ok {
            SpanStatus::Ok
        } else {
            SpanStatus::Error {
                message: event
                    .data_str("error")
                    .or_else(|| event.data_str("message"))
                    .unwrap_or("failed")
                    .to_owned(),
            }
        };
        let duration = (event.metadata.occurred_at - span.start)
            .as_seconds_f64()
            .max(0.0);
        let subject = span.subject;
        let metrics = end_metrics(event, subject, duration, ok);
        let completed = span.close(
            event.metadata.occurred_at,
            status,
            closed_by,
            payload_attributes(event),
        );

        Assembled {
            spans: vec![completed],
            metrics,
        }
    }

    /// The most recently opened still-open container span for this run.
    fn infer_parent(&self, run_id: &str, self_span: SpanId) -> Option<SpanId> {
        self.runs
            .get(run_id)?
            .containers
            .iter()
            .rev()
            .find(|candidate| **candidate != self_span)
            .copied()
    }

    fn pop_container(&mut self, span: &OpenSpan) {
        if !span.is_container() {
            return;
        }
        if let Some(state) = self.runs.get_mut(&span.run_id) {
            state.containers.retain(|open| *open != span.span_id);
            if state.containers.is_empty() {
                self.runs.remove(&span.run_id);
            }
        }
    }
}

fn span_kind(event: &RecordedEvent) -> SpanKind {
    match event.event_type.subject() {
        // An LLM or tool call leaves the process; the GenAI conventions call
        // these client spans.
        Subject::Llm | Subject::Tool => SpanKind::Client,
        // A step's kind depends on what it is. A retrieval waits on a vector
        // store; a parse does not. Getting this right is what lets a trace UI
        // separate "we waited on someone else" from "we were busy".
        Subject::Step => event
            .event_type
            .step_type(&event.data)
            .filter(|kind| catalog::step_type::is_remote(kind))
            .map_or(SpanKind::Internal, |_| SpanKind::Client),
        Subject::Run | Subject::Agent | Subject::Eval | Subject::Unknown => SpanKind::Internal,
    }
}

/// What goes after the operation name in the span title.
fn span_target(event: &RecordedEvent) -> Option<&str> {
    match event.event_type.subject() {
        Subject::Llm => event
            .data_str("model")
            .or_else(|| event.data_str("request_model")),
        Subject::Tool => event
            .data_str("tool_name")
            .or_else(|| event.data_str("name")),
        Subject::Agent => event.metadata.agent_id.as_deref(),
        // `retriever knowledge_base`, `guardrail pii`.
        Subject::Step => event
            .data_str("name")
            .or_else(|| event.event_type.step_type(&event.data)),
        Subject::Run | Subject::Eval | Subject::Unknown => None,
    }
}

/// Attributes every span carries, regardless of what it is.
fn base_attributes(event: &RecordedEvent) -> Vec<Attr> {
    let metadata = &event.metadata;
    let mut out = vec![
        attr(messaging::SYSTEM, messaging::SYSTEM_NAME),
        attr(messaging::MESSAGE_ID, metadata.message_id.as_str()),
        attr(messaging::CORRELATION_ID, metadata.correlation_id.as_str()),
        attr(messaging::CAUSATION_ID, metadata.causation_id.as_str()),
        attr(own::run::ID, metadata.run_id.as_str()),
        attr(own::stream::NAME, metadata.stream_name.to_string()),
        attr(own::stream::GLOBAL_POSITION, metadata.global_position),
        attr(
            own::event::SCHEMA_VERSION,
            i64::from(metadata.schema_version),
        ),
        attr(own::source::SERVICE, metadata.source.service.as_str()),
        attr(own::source::SDK, metadata.source.sdk.as_str()),
    ];
    if let Some(instance) = &metadata.source.instance {
        out.push(attr(own::source::INSTANCE, instance.as_str()));
    }
    if let Some(conversation) = &metadata.conversation_id {
        out.push(attr(messaging::CONVERSATION_ID, conversation.as_str()));
        out.push(attr(genai::CONVERSATION_ID, conversation.as_str()));
    }
    if let Some(agent) = &metadata.agent_id {
        out.push(attr(genai::AGENT_ID, agent.as_str()));
    }
    match event.event_type.subject() {
        Subject::Llm => out.push(attr(genai::OPERATION_NAME, genai::operation::CHAT)),
        Subject::Tool => out.push(attr(genai::OPERATION_NAME, genai::operation::EXECUTE_TOOL)),
        Subject::Agent => out.push(attr(genai::OPERATION_NAME, genai::operation::INVOKE_AGENT)),
        Subject::Step => out.push(attr(genai::OPERATION_NAME, "step")),
        Subject::Run | Subject::Eval | Subject::Unknown => {}
    }
    out
}

/// Attributes read off the end event's payload.
fn payload_attributes(event: &RecordedEvent) -> Vec<Attr> {
    let mut out = Vec::new();
    let mut push_str = |key: &str, value: Option<&str>| {
        if let Some(value) = value {
            out.push(attr(key, value));
        }
    };

    match event.event_type.subject() {
        Subject::Llm => {
            push_str(genai::PROVIDER_NAME, event.data_str("provider"));
            push_str(genai::SYSTEM, event.data_str("provider"));
            push_str(genai::REQUEST_MODEL, event.data_str("model"));
            push_str(genai::RESPONSE_MODEL, event.data_str("response_model"));
            push_str(genai::RESPONSE_ID, event.data_str("response_id"));
            if let Some(reason) = event.data_str("finish_reason") {
                out.push((
                    genai::RESPONSE_FINISH_REASONS.to_owned(),
                    AttrValue::StrList(vec![reason.to_owned()]),
                ));
            }
            for (key, semconv) in [
                ("prompt_tokens", genai::USAGE_INPUT_TOKENS),
                ("input_tokens", genai::USAGE_INPUT_TOKENS),
                ("completion_tokens", genai::USAGE_OUTPUT_TOKENS),
                ("output_tokens", genai::USAGE_OUTPUT_TOKENS),
            ] {
                if let Some(count) = event.data_i64(key) {
                    out.push(attr(semconv, count));
                }
            }
            if let Some(cached) = event.data_i64("cached_tokens") {
                out.push(attr("gen_ai.usage.cached_tokens", cached));
            }
            if let Some(temperature) = event.data_f64("temperature") {
                out.push(attr(genai::REQUEST_TEMPERATURE, temperature));
            }
            if let Some(max_tokens) = event.data_i64("max_tokens") {
                out.push(attr(genai::REQUEST_MAX_TOKENS, max_tokens));
            }
        }
        Subject::Tool => {
            push_str(genai::TOOL_NAME, event.data_str("tool_name"));
            push_str(genai::TOOL_CALL_ID, event.data_str("call_id"));
        }
        Subject::Agent => {
            push_str(genai::AGENT_NAME, event.data_str("agent_name"));
        }
        Subject::Run => {
            push_str(own::run::STATUS, event.data_str("status"));
        }
        Subject::Step => {
            push_str(
                own::span::STEP_TYPE,
                event.event_type.step_type(&event.data),
            );
            push_str(own::span::STEP_NAME, event.data_str("name"));
            // Retrieval is the step kind worth measuring: how many documents
            // came back, and out of how large a corpus, is what a bad answer
            // gets debugged against.
            for (key, attribute) in [
                ("document_count", own::step::DOCUMENT_COUNT),
                ("top_k", own::step::TOP_K),
                ("candidate_count", own::step::CANDIDATE_COUNT),
            ] {
                if let Some(count) = event.data_i64(key) {
                    out.push(attr(attribute, count));
                }
            }
            if let Some(score) = event.data_f64("score") {
                out.push(attr(own::step::SCORE, score));
            }
        }
        Subject::Eval | Subject::Unknown => {}
    }
    out
}

/// Model/provider labels shared by the metrics an end event produces. Kept
/// deliberately small — every label multiplies the series count.
fn model_attributes(event: &RecordedEvent) -> Vec<Attr> {
    let mut out = Vec::new();
    if let Some(provider) = event.data_str("provider") {
        out.push(attr(genai::PROVIDER_NAME, provider));
    }
    if let Some(model) = event.data_str("model") {
        out.push(attr(genai::REQUEST_MODEL, model));
    }
    if let Some(agent) = &event.metadata.agent_id {
        out.push(attr(genai::AGENT_ID, agent.as_str()));
    }
    out
}

fn end_metrics(
    event: &RecordedEvent,
    subject: Subject,
    duration_seconds: f64,
    ok: bool,
) -> Vec<MetricSample> {
    let at = event.metadata.occurred_at;
    let mut labels = model_attributes(event);
    labels.push(attr(genai::OPERATION_NAME, operation_for(subject)));
    labels.push(attr(
        own::processor::STATUS,
        if ok { "ok" } else { "error" },
    ));

    let mut out = vec![MetricSample {
        name: genai::metrics::OPERATION_DURATION.to_owned(),
        kind: MetricKind::Histogram,
        value: duration_seconds,
        unit: Some("s".to_owned()),
        at,
        attributes: labels.clone(),
    }];

    if subject == Subject::Llm {
        for (keys, token_type) in [
            (["prompt_tokens", "input_tokens"], genai::token_type::INPUT),
            (
                ["completion_tokens", "output_tokens"],
                genai::token_type::OUTPUT,
            ),
            (
                ["cached_tokens", "cached_tokens"],
                genai::token_type::CACHED,
            ),
        ] {
            let Some(count) = keys.iter().find_map(|key| event.data_i64(key)) else {
                continue;
            };
            let mut token_labels = model_attributes(event);
            token_labels.push(attr(genai::token_type::KEY, token_type));
            out.push(MetricSample {
                name: genai::metrics::TOKEN_USAGE.to_owned(),
                kind: MetricKind::Histogram,
                value: count as f64,
                unit: Some("{token}".to_owned()),
                at,
                attributes: token_labels,
            });
        }
    }

    out
}

fn operation_for(subject: Subject) -> &'static str {
    match subject {
        Subject::Llm => genai::operation::CHAT,
        Subject::Tool => genai::operation::EXECUTE_TOOL,
        Subject::Agent => genai::operation::INVOKE_AGENT,
        Subject::Step => "step",
        Subject::Run | Subject::Eval | Subject::Unknown => "run",
    }
}
