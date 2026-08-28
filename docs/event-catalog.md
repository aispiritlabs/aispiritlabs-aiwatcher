# Event catalog

The events aiwatcher understands, what each turns into, and which payload
fields it reads. The machine-readable contract is
[`contracts/envelope.schema.json`](../contracts/envelope.schema.json); the
authoritative list is `EventType` in `crates/aiwatcher-core/src/catalog.rs`.

An event type not in this list is **not rejected**. It is stored in the log and
streamed live; it simply takes part in no span. A producer running a newer SDK
than the backend keeps working.

## The taxonomy

| Event | Phase | Becomes |
|-------|-------|---------|
| `run.started` | start | opens the trace's root span |
| `run.completed` | end (ok) | closes it, status `Ok` |
| `run.failed` | end (error) | closes it, status `Error` |
| `agent.started` | start | opens an agent span |
| `agent.completed` | end (ok) | closes it |
| `agent.failed` | end (error) | closes it, status `Error` |
| `llm.started` | start | opens an LLM span (kind `client`) |
| `llm.first_token` | point | a `gen_ai.first_token` span event, and the TTFT metric |
| `llm.chunk` | point | live only — counted as `aiwatcher.span.chunk_count`, never stored per chunk |
| `llm.completed` | end (ok) | closes the LLM span, emits token and duration metrics |
| `llm.failed` | end (error) | closes it, status `Error` |
| `tool.started` | start | opens a tool span (kind `client`) |
| `tool.completed` | end (ok) | closes it |
| `tool.failed` | end (error) | closes it, status `Error` |
| `step.started` | start | opens a step span — kind from `data.step_type` |
| `step.completed` | end (ok) | closes it |
| `step.failed` | end (error) | closes it, status `Error` |
| `eval.started` | start | opens an evaluation report — **no span** |
| `eval.case` | point | one scored case — **no span** |
| `eval.completed` | end (ok) | closes the report, status `succeeded` |
| `eval.failed` | end (error) | closes it, status `failed` |

## Matching a start to its end

A start and its end must resolve to the same `span_id`. They do automatically
when both carry the same `agent_id` and the same `data.call_id`, because the
span id derives from those.

**Two concurrent LLM calls inside one agent with no `call_id` collapse into one
span.** Both SDKs generate a `call_id` by default; pass your provider's request
id where you have one, so the span joins up with the provider's own logs.

## Steps: everything else with a start and an end

A retrieval, an embedding, a rerank, a parse, a guardrail, a plain chain node.
One event type for all of them, with the specific kind in `data.step_type`:

```json
{ "event_type": "step.started",
  "data": { "call_id": "r1", "step_type": "retriever", "name": "knowledge_base",
            "top_k": 20, "document_count": 8 } }
```

**A `step_type` this build has never seen still produces a span**, named after
itself and nested correctly. That is the point of putting the kind in the
payload rather than in the event type: adding `guardrail` or `policy_check`
needs no backend release, the same reason an unknown `event_type` passes
through.

`data.span_type` is accepted as an alias, because that is what the `agentic`
tracer already calls the field.

### Kinds that get special treatment

| `step_type` | Span kind | Why |
|---|---|---|
| `retriever`, `embedding`, `reranker` | `client` | waits on something outside the process |
| anything else | `internal` | stays in the process |

That distinction is what lets a trace UI separate "we waited on someone else"
from "we were busy", so it is worth getting right for a new kind.

### Steps nest

A step is a **container**: a retrieval wrapping an embedding nests, and so does
anything else a producer opens inside one. LLM and tool calls are leaves — an
LLM call does not contain a tool call, it precedes one.

The one shape inference cannot see is a leaf inside a leaf: a model calling a
model. For that, send an explicit `parent_span_id`. A producer that tracks its
own scopes should send one always; the Python SDK's `agentic` integration does.

## Evaluations: a record, not a trace

`eval.*` events have phases and produce **no spans**. Nothing about an
evaluation reaches the trace store; it is folded into its own projection and
served from `/api/v1/evaluations`. See
[ADR_0010](ADR/ADR_0010_EVALUATION_REPORTS.md).

They are the third rule about what becomes a record, and the three are
different:

| | Stored in the log | Part of a span | Its own trace record |
|---|---|---|---|
| an unknown `event_type` | yes | no — we do not know what it is | no |
| `llm.chunk` | yes | yes, as a count | no — 2000 per call |
| `eval.*` | yes | **no — it is not a trace** | no |

An evaluation uses its own `run_id` — the evaluation id — so it gets its own
stream and its own partition. It never appears in the runs list; its raw events
are still readable through `GET /api/v1/runs/{id}/events`, which is where the
"is this what we actually recorded" question is answered.

```json
{ "event_type": "eval.completed",
  "run_id": "nightly-2026-08-28",
  "data": { "suite": "catalog-floor-plan", "dataset": "house-catalog@3",
            "variant": "floor-plan-v3",
            "params": { "model": "gpt-5-mini", "threshold": "0.90" },
            "metrics": { "mean_score": 0.88, "cost_usd": 0.42 },
            "report": { "scorer": "catalog-contract-v2" } } }
```

**`data.report` is not redacted.** The Collector strips prompts and completions
from spans, and an evaluation is not a span, so nothing strips this. Putting
model output in a report is a retention decision; make it deliberately.

## Payload fields the backend reads

Anything else in `data` is stored and displayed but not interpreted.

### `llm.*`

| Field | Becomes |
|-------|---------|
| `call_id` | part of the span key |
| `provider` | `gen_ai.provider.name`, `gen_ai.system` |
| `model` | `gen_ai.request.model`, and the span name (`chat <model>`) |
| `response_model` | `gen_ai.response.model` |
| `response_id` | `gen_ai.response.id` |
| `prompt_tokens` / `input_tokens` | `gen_ai.usage.input_tokens`, token metric |
| `completion_tokens` / `output_tokens` | `gen_ai.usage.output_tokens`, token metric |
| `cached_tokens` | `gen_ai.usage.cached_tokens`, token metric |
| `finish_reason` | `gen_ai.response.finish_reasons` |
| `temperature`, `max_tokens` | `gen_ai.request.*` |
| `duration_ms` | back-dates the span start when no start event was ever seen |
| `error` / `message` | the span's error status message |

### `tool.*`

| Field | Becomes |
|-------|---------|
| `call_id` | part of the span key; `gen_ai.tool.call.id` |
| `tool_name` | `gen_ai.tool.name`, and the span name (`execute_tool <name>`) |

### `step.*`

| Field | Becomes |
|-------|---------|
| `call_id` | part of the span key |
| `step_type` / `span_type` | `aiwatcher.span.step_type`, and the span kind |
| `name` | `aiwatcher.span.step_name`, and the span name |
| `document_count` | `aiwatcher.step.document_count` |
| `top_k` | `aiwatcher.step.top_k` |
| `candidate_count` | `aiwatcher.step.candidate_count` |
| `score` | `aiwatcher.step.score` |

The retrieval fields have no settled OpenTelemetry convention yet, so they sit
in the aiwatcher namespace rather than squatting on a `gen_ai.*` name that may
come to mean something else.

### `run.*`

| Field | Becomes |
|-------|---------|
| `status` | `aiwatcher.run.status` |
| `error` | the run's error, shown on the run page |

### `eval.*`

Read from every event of the evaluation, so a producer may send them on the
start, on the end, or split between the two. Later values win; `params` and
`metrics` merge.

| Field | Becomes |
|-------|---------|
| `suite` / `suite_name` / `run_name` | the suite, in that order of preference — `run_name` is MLflow's word |
| `dataset` / `dataset_id` / `dataset_version` | the dataset, and what pins a comparison |
| `variant` / `variant_id` | what was under test |
| `params` | the parameter map, values stringified |
| `metrics` | the metric map, non-numbers skipped |
| `report` | the document, dropped rather than cut when oversized |
| `cases_total` / `cases_passed` / `cases_failed` | the counts, for a producer that scores in a batch rather than sending a case each |
| `error` / `message` | the failure reason on `eval.failed` |

On `eval.case`:

| Field | Becomes |
|-------|---------|
| `case_id` / `case` / `id` | the case identity, and what a regression is matched on |
| `passed` / `success` | the pass count, and the regressed/fixed lists |
| `score` | the case score |
| `reason` / `rationale` | why the scorer said so, truncated at 500 characters |
| `duration_ms`, `error` | shown on the case row |

## What every span carries

Regardless of type:

- `messaging.system`, `messaging.message.id`,
  `messaging.message.correlation_id`, `messaging.message.causation_id` — the
  OpenTelemetry messaging conventions, the same ones Emmett's `almanac` uses.
- `aiwatcher.run.id`, `aiwatcher.stream.name`,
  `aiwatcher.stream.global_position`, `aiwatcher.event.schema_version`.
- `aiwatcher.source.service`, `aiwatcher.source.sdk`, and `instance` where sent.
- `aiwatcher.span.closed_by` — `event`, `timeout`, or `synthesised_start`. This
  is how a real completion is told apart from one the orphan sweeper had to
  invent.

## Metrics

| Metric | Kind | Labels |
|--------|------|--------|
| `gen_ai.client.operation.duration` | histogram (s) | provider, model, agent, operation, status |
| `gen_ai.client.token.usage` | histogram (tokens) | provider, model, agent, `gen_ai.token.type` |
| `gen_ai.server.time_to_first_token` | histogram (s) | provider, model, agent |
| `aiwatcher.events.ingested` | counter | event type |
| `aiwatcher.events.deduplicated` | counter | — |
| `aiwatcher.spans.orphaned` | counter | subject |
| `aiwatcher.spans.open` | gauge | processor |

`aiwatcher.spans.orphaned` is the one to alert on. A rising count means
producers are starting operations and never reporting their end — a crash loop,
a missing `finally`, or a network partition.

## Adding an event type

1. Add a row to the `event_catalog!` macro in
   `crates/aiwatcher-core/src/catalog.rs` with its subject and phase. The
   exhaustiveness tests will tell you if a subject is missing a start or an end.
2. If it carries fields worth putting on a span, extend `payload_attributes` in
   `crates/aiwatcher-trace/src/assembler.rs`.
3. Add it to this table and to `contracts/envelope.schema.json`.
4. Both SDKs pick it up for free — `emit` takes the type as a string.

Deployment order matters in one direction only: producers may emit a new type
before the backend knows it (it passes through as `Unknown`), but a backend that
expects a type no producer sends will just never see it.
