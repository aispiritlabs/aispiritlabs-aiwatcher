# ADR_0005: VictoriaTraces stores spans; QuestDB is a projection to add later, if ever

- **Status**: accepted
- **Date**: 2026-08-27

## Context

Spans have to go somewhere queryable. The comparison that matters is
**VictoriaTraces + VictoriaMetrics** against **QuestDB** — VictoriaMetrics alone
is a metrics database and does not store spans, so comparing it to QuestDB
compares the wrong things.

| | VictoriaTraces + VictoriaMetrics | QuestDB |
|---|---|---|
| OTLP spans | native | no documented native OTLP span store |
| Waterfall UI | Grafana via the Jaeger API | build it yourself |
| Dynamic span attributes | indexed automatically | explicit schema; JSON as `VARCHAR` |
| Ad-hoc SQL over runs and cost | LogsQL, moderate | excellent |
| Rust client | standard OTLP exporter | official `questdb-rs` |
| Operational floor | single node, small | docs suggest ~8 GB RAM |
| Open-source HA | VictoriaTraces cluster | mostly Enterprise |
| Maturity | young (0.11.0, pre-release) | mature, but not a tracing engine |

## Decision

- **Spans** → VictoriaTraces, via OTLP.
- **Aggregates and alerts** → VictoriaMetrics, via OTLP.
- **Raw events** → the log is the source of truth; VictoriaLogs optional for
  search.
- **QuestDB** → not now. Added later as an independent projector if deep SQL
  analytics over runs becomes more valuable than the trace view.

Both exporters hand-roll OTLP/JSON rather than using the OpenTelemetry SDK. The
SDK times spans as they happen and mints its own ids; a projector writes spans
whose ids and timestamps were decided by a producer, possibly hours earlier, and
must reproduce them exactly on a replay. Bending the SDK into that shape costs
more than the roughly two hundred lines in `aiwatcher-trace::otlp`.

Deferring QuestDB costs nothing, because Laser keeps every event. A QuestDB
projection built in six months can be backfilled by replaying the log from the
beginning — which is the point of keeping the log as the source of truth rather
than the database.

## Alternatives considered

**QuestDB as the only store.** The right answer if the goal is primarily a
product-analytics surface: cost per customer per model, tokens per conversation,
prompt-version comparisons, response-time percentiles. The cost is building the
OTLP-to-table mapping, span hierarchy reconstruction, waterfall rendering,
attribute evolution and `SYMBOL`-versus-`VARCHAR` decisions by hand. That is a
lot of work to arrive at what a trace store gives for free.

**Jaeger or Tempo.** Both are proven and neither gives the metrics half of the
answer from the same family. VictoriaTraces speaks the Jaeger API anyway, so
Grafana's existing Jaeger data source reads it — the migration path is open in
both directions.

**ClickHouse.** Strong, and a larger operational commitment than this system
justifies at its current size.

## Consequences

- VictoriaTraces is young. Its advanced trace UI is still on the roadmap, so the
  waterfall comes from Grafana, from Jaeger UI, or from aiwatcher's own panel.
- Hand-rolled OTLP/JSON means the encoding is our responsibility. The three
  rules that produce a silent `200` with no data — hex ids not base64,
  nanosecond timestamps as strings, protobuf enum numbers for kind and status —
  are asserted in `otlp.rs`'s tests for exactly that reason.
- The metric exporter keeps **cumulative** state per series rather than sending
  one delta point per observation. Deltas looked simpler and lost data: two LLM
  calls in one flush share a series and a millisecond, and a database keyed by
  (series, timestamp) keeps one of them — so token totals silently reported the
  last call in each batch. Found by comparing a seeded run's known token counts
  against what VictoriaMetrics had stored, which is worth doing after any change
  to this file.
- The Collector sits in the path even though aiwatcher can post directly to
  VictoriaTraces. It is where sampling, redaction and a second destination get
  added, and having it there from the start makes those a config change.
- Prompt and completion text is deleted by the Collector before export. It is
  the highest-risk field in the system; enabling it should be a deliberate
  decision with a retention policy attached.

**What would make this wrong.** If most questions asked of this system turn out
to be SQL-shaped — cost attribution, prompt A/B comparisons, cohort analysis —
rather than "show me this trace", QuestDB should become the primary store and
VictoriaTraces the secondary. Watch which of the two UIs people actually open.
