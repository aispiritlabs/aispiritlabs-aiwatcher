// See the note in aiwatcher-bus/tests: `clippy.toml`'s test allowances do not
// reach files under `tests/`.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! What a realistic run turns into.
//!
//! These tests drive the assembler with the event sequence a Python agent
//! actually emits, and assert on the *trace* that comes out — the shape, the
//! parenting, and the fact that 2000 streamed chunks do not become 2000
//! records.

use serde_json::json;
use time::OffsetDateTime;
use time::macros::datetime;

use aiwatcher_core::ports::{AttrValue, CompletedSpan, SpanKind, SpanStatus};
use aiwatcher_core::{EventEnvelope, EventType, RecordedEvent, Sdk, Source};
use aiwatcher_trace::{Assembled, AssemblerConfig, SpanAssembler};

/// Builds the recorded stream for one run, assigning positions as a log would.
struct Run {
    run_id: String,
    position: u64,
    at: OffsetDateTime,
}

impl Run {
    fn new(run_id: &str) -> Self {
        Self {
            run_id: run_id.to_owned(),
            position: 0,
            at: datetime!(2026-08-27 18:20:00 UTC),
        }
    }

    fn after(&mut self, millis: i64) -> &mut Self {
        self.at += time::Duration::milliseconds(millis);
        self
    }

    fn emit(
        &mut self,
        event_type: EventType,
        agent_id: Option<&str>,
        data: serde_json::Value,
    ) -> RecordedEvent {
        self.position += 1;
        let mut envelope = EventEnvelope::new(
            event_type,
            &self.run_id,
            self.at,
            Source::new("python-agent-service", Sdk::Python),
        )
        .with_data(data);
        envelope.agent_id = agent_id.map(ToOwned::to_owned);
        envelope.sequence = Some(self.position);
        envelope.record(self.position, self.position, self.at, None)
    }
}

fn collect(assembler: &mut SpanAssembler, events: &[RecordedEvent]) -> Assembled {
    let mut all = Assembled::default();
    for event in events {
        let produced = assembler.ingest(event);
        all.spans.extend(produced.spans);
        all.metrics.extend(produced.metrics);
    }
    all
}

fn find<'a>(spans: &'a [CompletedSpan], name: &str) -> &'a CompletedSpan {
    spans
        .iter()
        .find(|span| span.name == name)
        .unwrap_or_else(|| panic!("no span named {name}; got {:?}", names(spans)))
}

fn names(spans: &[CompletedSpan]) -> Vec<&str> {
    spans.iter().map(|span| span.name.as_str()).collect()
}

fn string_attr<'a>(span: &'a CompletedSpan, key: &str) -> Option<&'a str> {
    span.attributes
        .iter()
        .find_map(|(name, value)| match value {
            AttrValue::Str(inner) if name == key => Some(inner.as_str()),
            _ => None,
        })
}

fn int_attr(span: &CompletedSpan, key: &str) -> Option<i64> {
    span.attributes
        .iter()
        .find_map(|(name, value)| match value {
            AttrValue::Int(inner) if name == key => Some(*inner),
            _ => None,
        })
}

/// A run with one agent, one streamed LLM call and one tool call.
fn realistic_run() -> Vec<RecordedEvent> {
    let mut run = Run::new("run-456");
    let mut events = vec![
        run.emit(EventType::RunStarted, None, json!({})),
        run.after(5)
            .emit(EventType::AgentStarted, Some("research-agent"), json!({})),
        run.after(10).emit(
            EventType::LlmStarted,
            Some("research-agent"),
            json!({ "call_id": "call-1", "provider": "anthropic", "model": "claude-opus-5" }),
        ),
        run.after(300).emit(
            EventType::LlmFirstToken,
            Some("research-agent"),
            json!({ "call_id": "call-1", "provider": "anthropic", "model": "claude-opus-5" }),
        ),
    ];
    // A streaming response: many chunks, one call.
    for _ in 0..500 {
        events.push(run.after(2).emit(
            EventType::LlmChunk,
            Some("research-agent"),
            json!({ "call_id": "call-1", "text": "…" }),
        ));
    }
    events.push(run.after(20).emit(
        EventType::LlmCompleted,
        Some("research-agent"),
        json!({
            "call_id": "call-1",
            "provider": "anthropic",
            "model": "claude-opus-5",
            "prompt_tokens": 812,
            "completion_tokens": 193,
            "cached_tokens": 400,
            "finish_reason": "stop"
        }),
    ));
    events.push(run.after(5).emit(
        EventType::ToolStarted,
        Some("research-agent"),
        json!({ "call_id": "tool-1", "tool_name": "web_search" }),
    ));
    events.push(run.after(120).emit(
        EventType::ToolCompleted,
        Some("research-agent"),
        json!({ "call_id": "tool-1", "tool_name": "web_search" }),
    ));
    events.push(
        run.after(5)
            .emit(EventType::AgentCompleted, Some("research-agent"), json!({})),
    );
    events.push(run.after(2).emit(
        EventType::RunCompleted,
        None,
        json!({ "status": "succeeded" }),
    ));
    events
}

#[test]
fn a_streamed_run_of_hundreds_of_events_produces_four_spans() {
    let events = realistic_run();
    assert!(events.len() > 500, "the input really is chunk-heavy");

    let mut assembler = SpanAssembler::default();
    let assembled = collect(&mut assembler, &events);

    assert_eq!(
        assembled.spans.len(),
        4,
        "run, agent, llm, tool — got {:?}",
        names(&assembled.spans)
    );
    assert_eq!(assembler.open_span_count(), 0, "nothing is left open");
}

#[test]
fn chunks_are_counted_on_the_llm_span_not_stored_individually() {
    let mut assembler = SpanAssembler::default();
    let assembled = collect(&mut assembler, &realistic_run());
    let llm = find(&assembled.spans, "chat claude-opus-5");

    assert_eq!(int_attr(llm, "aiwatcher.span.chunk_count"), Some(500));
    assert_eq!(
        llm.events.len(),
        1,
        "only the first token becomes a span event"
    );
    assert_eq!(llm.events[0].name, "gen_ai.first_token");
}

#[test]
fn the_waterfall_nests_llm_and_tool_under_the_agent_under_the_run() {
    let mut assembler = SpanAssembler::default();
    let assembled = collect(&mut assembler, &realistic_run());

    let run = find(&assembled.spans, "run");
    let agent = find(&assembled.spans, "invoke_agent research-agent");
    let llm = find(&assembled.spans, "chat claude-opus-5");
    let tool = find(&assembled.spans, "execute_tool web_search");

    assert_eq!(run.parent_span_id, None, "the run span is the trace root");
    assert_eq!(agent.parent_span_id, Some(run.span_id));
    assert_eq!(llm.parent_span_id, Some(agent.span_id));
    assert_eq!(tool.parent_span_id, Some(agent.span_id));

    for span in [run, agent, llm, tool] {
        assert_eq!(span.trace_id, run.trace_id, "one trace per run");
    }
}

#[test]
fn llm_and_tool_spans_are_client_spans_and_containers_are_internal() {
    let mut assembler = SpanAssembler::default();
    let assembled = collect(&mut assembler, &realistic_run());

    assert_eq!(
        find(&assembled.spans, "chat claude-opus-5").kind,
        SpanKind::Client
    );
    assert_eq!(
        find(&assembled.spans, "execute_tool web_search").kind,
        SpanKind::Client
    );
    assert_eq!(find(&assembled.spans, "run").kind, SpanKind::Internal);
    assert_eq!(
        find(&assembled.spans, "invoke_agent research-agent").kind,
        SpanKind::Internal
    );
}

#[test]
fn usage_lands_on_the_span_as_genai_semconv_attributes() {
    let mut assembler = SpanAssembler::default();
    let assembled = collect(&mut assembler, &realistic_run());
    let llm = find(&assembled.spans, "chat claude-opus-5");

    assert_eq!(string_attr(llm, "gen_ai.provider.name"), Some("anthropic"));
    assert_eq!(
        string_attr(llm, "gen_ai.request.model"),
        Some("claude-opus-5")
    );
    assert_eq!(int_attr(llm, "gen_ai.usage.input_tokens"), Some(812));
    assert_eq!(int_attr(llm, "gen_ai.usage.output_tokens"), Some(193));
    assert_eq!(int_attr(llm, "gen_ai.usage.cached_tokens"), Some(400));
    assert_eq!(string_attr(llm, "gen_ai.operation.name"), Some("chat"));
    assert_eq!(
        llm.attributes
            .iter()
            .find(|(key, _)| key == "gen_ai.response.finish_reasons")
            .map(|(_, value)| value.clone()),
        Some(AttrValue::StrList(vec!["stop".to_owned()]))
    );
}

#[test]
fn correlation_and_causation_ride_along_on_every_span() {
    let mut assembler = SpanAssembler::default();
    let assembled = collect(&mut assembler, &realistic_run());

    for span in &assembled.spans {
        assert!(
            string_attr(span, "messaging.message.correlation_id").is_some(),
            "{} has no correlation id",
            span.name
        );
        assert!(
            string_attr(span, "messaging.message.causation_id").is_some(),
            "{} has no causation id",
            span.name
        );
        assert_eq!(string_attr(span, "messaging.system"), Some("aiwatcher"));
        assert_eq!(string_attr(span, "aiwatcher.run.id"), Some("run-456"));
    }
}

#[test]
fn token_usage_and_latency_become_metrics() {
    let mut assembler = SpanAssembler::default();
    let assembled = collect(&mut assembler, &realistic_run());

    let token_samples: Vec<_> = assembled
        .metrics
        .iter()
        .filter(|sample| sample.name == "gen_ai.client.token.usage")
        .collect();
    assert_eq!(token_samples.len(), 3, "input, output and cached");
    assert!(token_samples.iter().any(|sample| sample.value == 812.0));
    assert!(token_samples.iter().any(|sample| sample.value == 400.0));

    let ttft: Vec<_> = assembled
        .metrics
        .iter()
        .filter(|sample| sample.name == "gen_ai.server.time_to_first_token")
        .collect();
    assert_eq!(ttft.len(), 1);
    assert!(
        (ttft[0].value - 0.300).abs() < 0.001,
        "measured from llm.started, not from the run or agent start: {}",
        ttft[0].value
    );

    let durations: Vec<_> = assembled
        .metrics
        .iter()
        .filter(|sample| sample.name == "gen_ai.client.operation.duration")
        .collect();
    assert_eq!(durations.len(), 4, "one per closed span");
}

#[test]
fn two_parallel_llm_calls_both_parent_onto_the_agent_not_onto_each_other() {
    let mut run = Run::new("run-parallel");
    let events = vec![
        run.emit(EventType::RunStarted, None, json!({})),
        run.after(1)
            .emit(EventType::AgentStarted, Some("a"), json!({})),
        run.after(1).emit(
            EventType::LlmStarted,
            Some("a"),
            json!({ "call_id": "one", "model": "m1" }),
        ),
        run.after(1).emit(
            EventType::LlmStarted,
            Some("a"),
            json!({ "call_id": "two", "model": "m2" }),
        ),
        run.after(50).emit(
            EventType::LlmCompleted,
            Some("a"),
            json!({ "call_id": "two", "model": "m2" }),
        ),
        run.after(50).emit(
            EventType::LlmCompleted,
            Some("a"),
            json!({ "call_id": "one", "model": "m1" }),
        ),
        run.after(1)
            .emit(EventType::AgentCompleted, Some("a"), json!({})),
        run.after(1).emit(EventType::RunCompleted, None, json!({})),
    ];

    let mut assembler = SpanAssembler::default();
    let assembled = collect(&mut assembler, &events);

    let agent = find(&assembled.spans, "invoke_agent a");
    let first = find(&assembled.spans, "chat m1");
    let second = find(&assembled.spans, "chat m2");

    assert_ne!(first.span_id, second.span_id, "two calls, two spans");
    assert_eq!(first.parent_span_id, Some(agent.span_id));
    assert_eq!(
        second.parent_span_id,
        Some(agent.span_id),
        "the second call must not nest inside the first"
    );
}

#[test]
fn a_sub_agent_nests_inside_the_agent_that_spawned_it() {
    let mut run = Run::new("run-nested");
    let events = vec![
        run.emit(EventType::RunStarted, None, json!({})),
        run.after(1)
            .emit(EventType::AgentStarted, Some("parent"), json!({})),
        run.after(1)
            .emit(EventType::AgentStarted, Some("child"), json!({})),
        run.after(1).emit(
            EventType::LlmStarted,
            Some("child"),
            json!({ "call_id": "c", "model": "m" }),
        ),
        run.after(10).emit(
            EventType::LlmCompleted,
            Some("child"),
            json!({ "call_id": "c", "model": "m" }),
        ),
        run.after(1)
            .emit(EventType::AgentCompleted, Some("child"), json!({})),
        run.after(1)
            .emit(EventType::AgentCompleted, Some("parent"), json!({})),
        run.after(1).emit(EventType::RunCompleted, None, json!({})),
    ];

    let mut assembler = SpanAssembler::default();
    let assembled = collect(&mut assembler, &events);

    let root = find(&assembled.spans, "run");
    let parent = find(&assembled.spans, "invoke_agent parent");
    let child = find(&assembled.spans, "invoke_agent child");
    let llm = find(&assembled.spans, "chat m");

    assert_eq!(parent.parent_span_id, Some(root.span_id));
    assert_eq!(child.parent_span_id, Some(parent.span_id));
    assert_eq!(llm.parent_span_id, Some(child.span_id));
}

#[test]
fn a_failure_closes_the_span_with_the_producers_message() {
    let mut run = Run::new("run-failed");
    let events = vec![
        run.emit(EventType::RunStarted, None, json!({})),
        run.after(1)
            .emit(EventType::AgentStarted, Some("a"), json!({})),
        run.after(1).emit(
            EventType::LlmStarted,
            Some("a"),
            json!({ "call_id": "c", "model": "m" }),
        ),
        run.after(80).emit(
            EventType::LlmFailed,
            Some("a"),
            json!({ "call_id": "c", "model": "m", "error": "429 rate limited" }),
        ),
    ];

    let mut assembler = SpanAssembler::default();
    let assembled = collect(&mut assembler, &events);
    let llm = find(&assembled.spans, "chat m");

    assert_eq!(
        llm.status,
        SpanStatus::Error {
            message: "429 rate limited".to_owned()
        }
    );
    assert_eq!(string_attr(llm, "aiwatcher.span.closed_by"), Some("event"));
}

#[test]
fn an_end_without_a_start_is_back_dated_from_its_duration() {
    let mut run = Run::new("run-endonly");
    let events = vec![run.after(500).emit(
        EventType::LlmCompleted,
        Some("a"),
        json!({ "call_id": "c", "model": "m", "duration_ms": 1240 }),
    )];

    let mut assembler = SpanAssembler::default();
    let assembled = collect(&mut assembler, &events);
    let llm = find(&assembled.spans, "chat m");

    assert_eq!(
        (llm.end - llm.start),
        time::Duration::milliseconds(1240),
        "the span keeps a plausible width instead of collapsing to a point"
    );
    assert_eq!(
        string_attr(llm, "aiwatcher.span.closed_by"),
        Some("synthesised_start"),
        "and says so, rather than passing as a normal completion"
    );
}

#[test]
fn a_run_that_stops_mid_flight_is_swept_and_marked() {
    let mut run = Run::new("run-abandoned");
    let events = vec![
        run.emit(EventType::RunStarted, None, json!({})),
        run.after(1)
            .emit(EventType::AgentStarted, Some("a"), json!({})),
        run.after(1).emit(
            EventType::LlmStarted,
            Some("a"),
            json!({ "call_id": "c", "model": "m" }),
        ),
    ];

    let mut assembler = SpanAssembler::new(AssemblerConfig {
        orphan_timeout: time::Duration::minutes(5),
        ..AssemblerConfig::default()
    });
    let assembled = collect(&mut assembler, &events);
    assert!(assembled.spans.is_empty(), "nothing has ended yet");
    assert_eq!(assembler.open_span_count(), 3);

    // Not yet stale.
    let early = assembler.sweep(datetime!(2026-08-27 18:22:00 UTC));
    assert!(early.spans.is_empty());
    assert_eq!(assembler.open_span_count(), 3);

    let swept = assembler.sweep(datetime!(2026-08-27 18:40:00 UTC));
    assert_eq!(swept.spans.len(), 3, "all three are closed");
    assert_eq!(assembler.open_span_count(), 0);
    for span in &swept.spans {
        assert!(matches!(span.status, SpanStatus::Error { .. }));
        assert_eq!(
            string_attr(span, "aiwatcher.span.closed_by"),
            Some("timeout")
        );
        assert!(span.end >= span.start, "a swept span keeps a sane duration");
    }
    assert_eq!(
        swept
            .metrics
            .iter()
            .filter(|m| m.name == "aiwatcher.spans.orphaned")
            .count(),
        3
    );
}

#[test]
fn redelivering_the_whole_run_produces_identical_spans() {
    let events = realistic_run();

    let mut first_pass = SpanAssembler::default();
    let first = collect(&mut first_pass, &events);
    let mut second_pass = SpanAssembler::default();
    let second = collect(&mut second_pass, &events);

    assert_eq!(
        first.spans, second.spans,
        "a replay must overwrite, not duplicate"
    );
}

#[test]
fn an_unknown_event_type_is_ignored_by_the_assembler_without_failing() {
    let mut run = Run::new("run-unknown");
    let events = vec![
        run.emit(EventType::RunStarted, None, json!({})),
        run.after(1).emit(
            EventType::parse("guardrail.tripped"),
            Some("a"),
            json!({ "rule": "pii" }),
        ),
        run.after(1).emit(EventType::RunCompleted, None, json!({})),
    ];

    let mut assembler = SpanAssembler::default();
    let assembled = collect(&mut assembler, &events);
    assert_eq!(assembled.spans.len(), 1, "just the run span");
    assert_eq!(assembler.open_span_count(), 0);
}

/// A retrieval-augmented turn: the shape a knowledge-base agent actually makes.
#[test]
fn steps_nest_arbitrarily_and_carry_their_kind() {
    let mut run = Run::new("run-rag");
    let events = vec![
        run.emit(EventType::RunStarted, None, json!({})),
        run.after(1)
            .emit(EventType::AgentStarted, Some("a"), json!({})),
        // A retrieval wrapping an embedding: a container inside a container.
        run.after(1).emit(
            EventType::StepStarted,
            Some("a"),
            json!({ "call_id": "r1", "step_type": "retriever", "name": "knowledge_base" }),
        ),
        run.after(2).emit(
            EventType::StepStarted,
            Some("a"),
            json!({ "call_id": "e1", "step_type": "embedding", "name": "bge-small" }),
        ),
        run.after(8).emit(
            EventType::StepCompleted,
            Some("a"),
            json!({ "call_id": "e1", "step_type": "embedding", "name": "bge-small" }),
        ),
        run.after(30).emit(
            EventType::StepCompleted,
            Some("a"),
            json!({
                "call_id": "r1", "step_type": "retriever", "name": "knowledge_base",
                "document_count": 8, "top_k": 20
            }),
        ),
        run.after(1).emit(
            EventType::LlmStarted,
            Some("a"),
            json!({ "call_id": "c1", "model": "m" }),
        ),
        run.after(40).emit(
            EventType::LlmCompleted,
            Some("a"),
            json!({ "call_id": "c1", "model": "m" }),
        ),
        run.after(1)
            .emit(EventType::AgentCompleted, Some("a"), json!({})),
        run.after(1).emit(EventType::RunCompleted, None, json!({})),
    ];

    let mut assembler = SpanAssembler::default();
    let assembled = collect(&mut assembler, &events);

    let agent = find(&assembled.spans, "invoke_agent a");
    let retriever = find(&assembled.spans, "knowledge_base");
    let embedding = find(&assembled.spans, "bge-small");
    let llm = find(&assembled.spans, "chat m");

    assert_eq!(retriever.parent_span_id, Some(agent.span_id));
    assert_eq!(
        embedding.parent_span_id,
        Some(retriever.span_id),
        "a step is a container, so the embedding nests inside the retrieval"
    );
    assert_eq!(
        llm.parent_span_id,
        Some(agent.span_id),
        "and the LLM call that follows is back on the agent, not inside the retrieval"
    );

    assert_eq!(
        string_attr(retriever, "aiwatcher.span.step_type"),
        Some("retriever")
    );
    assert_eq!(
        int_attr(retriever, "aiwatcher.step.document_count"),
        Some(8)
    );
    assert_eq!(int_attr(retriever, "aiwatcher.step.top_k"), Some(20));

    // Kind follows what the step does: a retrieval waits on a vector store, a
    // parse does not.
    assert_eq!(retriever.kind, SpanKind::Client);
    assert_eq!(embedding.kind, SpanKind::Client);
    assert_eq!(agent.kind, SpanKind::Internal);
    // run, agent, retriever, embedding, llm.
    assert_eq!(assembled.spans.len(), 5);
}

#[test]
fn a_local_step_is_an_internal_span_and_an_unknown_kind_still_produces_one() {
    let mut run = Run::new("run-steps");
    let events = vec![
        run.emit(EventType::RunStarted, None, json!({})),
        run.after(1).emit(
            EventType::StepStarted,
            None,
            json!({ "call_id": "p1", "step_type": "parser", "name": "json" }),
        ),
        run.after(3).emit(
            EventType::StepCompleted,
            None,
            json!({ "call_id": "p1", "step_type": "parser", "name": "json" }),
        ),
        run.after(1).emit(
            EventType::StepStarted,
            None,
            json!({ "call_id": "g1", "step_type": "policy_check", "name": "pii" }),
        ),
        run.after(2).emit(
            EventType::StepFailed,
            None,
            json!({ "call_id": "g1", "step_type": "policy_check", "name": "pii", "error": "blocked" }),
        ),
        run.after(1).emit(EventType::RunCompleted, None, json!({})),
    ];

    let mut assembler = SpanAssembler::default();
    let assembled = collect(&mut assembler, &events);

    let parser = find(&assembled.spans, "json");
    assert_eq!(parser.kind, SpanKind::Internal, "a parse stays in-process");

    // A kind this build has never heard of is still a span, still nested, still
    // named after itself — no backend release needed to add one.
    let novel = find(&assembled.spans, "pii");
    assert_eq!(
        string_attr(novel, "aiwatcher.span.step_type"),
        Some("policy_check")
    );
    assert_eq!(novel.kind, SpanKind::Internal);
    assert_eq!(
        novel.status,
        SpanStatus::Error {
            message: "blocked".to_owned()
        }
    );
}

#[test]
fn an_explicit_parent_overrides_inference_so_a_leaf_can_nest_in_a_leaf() {
    // The shape inference cannot see: a model calling a model. The producer
    // knows its own nesting, and an explicit parent_span_id is believed.
    let mut run = Run::new("run-nested-llm");
    let outer = run.emit(
        EventType::LlmStarted,
        Some("a"),
        json!({ "call_id": "outer", "model": "big" }),
    );
    let outer_span = outer.metadata.span_id;

    let mut inner_envelope = EventEnvelope::new(
        EventType::LlmStarted,
        "run-nested-llm",
        datetime!(2026-08-27 18:20:01 UTC),
        Source::new("test", Sdk::Python),
    )
    .with_data(json!({ "call_id": "inner", "model": "small" }));
    inner_envelope.agent_id = Some("a".to_owned());
    inner_envelope.parent_span_id = Some(outer_span);
    let inner = inner_envelope.record(2, 2, datetime!(2026-08-27 18:20:01 UTC), None);

    let mut close = EventEnvelope::new(
        EventType::LlmCompleted,
        "run-nested-llm",
        datetime!(2026-08-27 18:20:02 UTC),
        Source::new("test", Sdk::Python),
    )
    .with_data(json!({ "call_id": "inner", "model": "small" }));
    close.agent_id = Some("a".to_owned());
    let close = close.record(3, 3, datetime!(2026-08-27 18:20:02 UTC), None);

    let mut assembler = SpanAssembler::default();
    let assembled = collect(&mut assembler, &[outer, inner, close]);

    let small = find(&assembled.spans, "chat small");
    assert_eq!(
        small.parent_span_id,
        Some(outer_span),
        "an LLM call is a leaf to inference, but the producer said otherwise"
    );
}

#[test]
fn a_shutdown_drains_open_spans_so_a_restart_does_not_lose_them() {
    let mut run = Run::new("run-shutdown");
    let events = vec![
        run.emit(EventType::RunStarted, None, json!({})),
        run.after(1)
            .emit(EventType::AgentStarted, Some("a"), json!({})),
    ];

    let mut assembler = SpanAssembler::default();
    collect(&mut assembler, &events);
    let drained = assembler.drain(datetime!(2026-08-27 18:25:00 UTC));

    assert_eq!(drained.spans.len(), 2);
    assert_eq!(assembler.open_span_count(), 0);
}

/// An evaluation report rides the same log and produces no trace.
///
/// It has a start, an end and a duration — everything a span needs — and is
/// still the wrong thing to write to a trace store: its payload is a report,
/// not a request that happened. The evaluation projection folds it instead.
#[test]
fn an_evaluation_report_produces_no_spans_and_no_metrics() {
    let mut assembler = SpanAssembler::default();
    let mut run = Run::new("eval-2026-08-28");

    let events = vec![
        run.emit(
            EventType::EvalStarted,
            None,
            json!({ "suite": "catalog-floor-plan", "dataset": "house-catalog@3" }),
        ),
        run.after(400).emit(
            EventType::EvalCase,
            None,
            json!({ "case_id": "K-127", "score": 0.94, "passed": true }),
        ),
        run.after(400).emit(
            EventType::EvalCompleted,
            None,
            json!({ "metrics": { "mean_score": 0.94 }, "report": { "cases": 1 } }),
        ),
    ];

    let assembled = collect(&mut assembler, &events);

    assert!(
        assembled.spans.is_empty(),
        "an evaluation report is not a trace; got {:?}",
        names(&assembled.spans)
    );
    assert!(assembled.metrics.is_empty());
    assert_eq!(
        assembler.open_span_count(),
        0,
        "and nothing is left open for the sweeper to close"
    );
}
