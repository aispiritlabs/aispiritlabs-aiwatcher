# ADR_0003: An event is not a span

- **Status**: accepted
- **Date**: 2026-08-27

## Context

A streaming LLM call emits one `llm.started`, one `llm.first_token`, one
`llm.chunk` per token and one `llm.completed`. A 2000-token response is 2003
events for **one** operation.

Writing each event as a trace record would produce a waterfall with thousands of
one-microsecond bars, a trace store bill dominated by tokens, and a UI nobody
can read. The events themselves are still worth keeping: they are what makes the
live view live.

## Decision

Events fold into spans. The shape:

```
run                             trace
└── agent execution             span    agent.started  → agent.completed
    ├── LLM call                span    llm.started    → llm.completed
    │   ├── first token         span event
    │   └── chunks              counted, live only
    └── tool call               span    tool.started   → tool.completed
```

`EventType::phase()` drives it: a `Start` opens a span, an `End` closes it, a
`Point` becomes a span event or is dropped after the live fan-out. `llm.chunk`
is a `Point` and contributes only `aiwatcher.span.chunk_count`.

**A span is written only when its end event arrives.** A span still open may
gain children and attributes; writing it early means either rewriting it — which
trace stores do not support — or losing what came after.

**Parenting**: an explicit `parent_span_id` wins. Otherwise the parent is the
most recently opened still-open **container** span in the same run, where a run
and an agent are containers and an LLM or tool call is not. That single rule
handles the two cases a naive stack gets wrong: two LLM calls issued in parallel
both parent onto their agent rather than onto each other, and a sub-agent still
nests inside the agent that spawned it.

**Orphans**: `SpanAssembler::sweep` closes spans that have gone quiet for longer
than `orphan_timeout` (15 minutes by default), marks them
`aiwatcher.span.closed_by=timeout`, and gives them an error status. On shutdown
`drain` does the same for everything still open.

Attributes follow the OpenTelemetry GenAI conventions —
`gen_ai.usage.input_tokens`, `gen_ai.request.model`,
`gen_ai.response.finish_reasons` — so a generic OTel dashboard understands the
output without knowing aiwatcher exists.

## Alternatives considered

**One span per event.** Simple, and it produces an unreadable waterfall and a
storage bill that scales with tokens.

**Sample chunks — keep one in fifty.** Rejected: a sampled chunk stream is
neither a complete transcript nor a clean aggregate. The count and the
time-to-first-token carry the useful information; the text belongs on the live
channel and in the log.

**Write the span on `*.started` and patch it on `*.completed`.** Trace stores
are append-only; a patch means a second record that most UIs show as a duplicate.

**Let the producer send finished spans.** Moves the assembly problem into every
SDK in every language, and makes a producer that crashes mid-run invisible
rather than visibly incomplete.

## Consequences

- A run whose producer dies is visible: its spans close as `timeout` with an
  error status, and `aiwatcher.spans.orphaned` counts them. A rising count on
  that metric is the fastest signal that a producer stopped sending end events.
- Spans arrive in the trace store *after* the run finishes, so the live view has
  to come from somewhere else. That is why the projector keeps a read model —
  see [ADR_0004](ADR_0004_LIVE_STREAM_RESUME.md).
- The assembler holds open spans in memory. Bounded by `max_open_spans`, over
  which the oldest are swept early rather than growing without limit.
- `SpanAssembler` is not `Sync`: one per consumer task. Safe because Laser
  partitions by stream, so a run's events all reach the same task in order.

**What would make this wrong.** Inference has one blind spot it cannot close: a
leaf inside a leaf — a model calling a model, an embedding inside an LLM call —
looks identical from outside. That is now handled by believing an explicit
`parent_span_id`, and the Python SDK sends one for every scope. If a producer
that *cannot* track its own scopes needs that shape, inference would have to
take a depth hint rather than guess.
