# aiwatcher and MLflow

`ai_spirit_agent` already traces through MLflow (`MlflowLLMTracer`, `data/mlflow.db`,
`make mlflow-ui`). The aiwatcher integration does not replace it — `create_tracer`
tees to both. This is the honest case for keeping both, and for what each is
actually good at.

## They are not the same kind of tool

MLflow is an **experiment tracker** that grew tracing. Its centre of gravity is
the offline loop: runs, parameters, metrics, artifacts, model registry,
evaluation. Tracing is how you inspect one execution after it happened.

aiwatcher is an **operational observability** system. Its centre of gravity is
the online loop: what is happening now, what did it cost, what broke. It has no
model registry, no artifact store and no evaluation harness — no scorers, no
judges, no way to *run* a suite — and is not trying to acquire them.

It does now **record** an evaluation: parameters, metrics, per-case scores and a
document, compared against the previous report on the same dataset. That is one
of MLflow's two jobs here, and the smaller one — four fields and a JSON blob.
See [ADR_0010](ADR/ADR_0010_EVALUATION_REPORTS.md), and the section below.

The repo uses MLflow for both jobs today, which is why the second one feels thin.

## Where each one lands

| | MLflow | aiwatcher |
|---|---|---|
| Live view of a run in flight | no — a trace appears when it is logged | SSE per run, with `Last-Event-ID` resume |
| Redelivery / at-least-once | not a concept; a re-log is a new trace | derived span ids, so a replay overwrites |
| Aggregates across many runs | per-run metrics; cross-run needs the UI or the API | a metrics fold: tokens, latency percentiles, per agent / model / tool |
| Navigating session → run → span → messages | trace list, then one trace at a time | one page, pivotable by session / agent / model / tool |
| Prompt & completion capture | yes, and it is the point | deliberately redacted before export |
| Params, metrics and a report per evaluation | yes | yes — `/api/v1/evaluations`, folded from the same log |
| Comparison against the previous run | yes, in the UI | yes, and only within one dataset |
| Artifacts, model registry, running a suite | yes (`registry`, `evaluation` packages depend on it) | no, and not planned |
| Standard trace format | MLflow's own, plus OTel export | OTLP native, so Grafana/Jaeger/Tempo read it |
| Backpressure under load | synchronous-ish logging in the request path | bounded queue, events dropped before the agent blocks |
| Footprint | Postgres + MinIO + server, or a local sqlite | one process, ~119 MB at full retention |
| Cost to the agent per run | ~6.5 ms, synchronous | ~0.1 ms, enqueue only |

## Measured: 14,000 runs through each

Same workload, same machine, driven through the `LLMTracer` surface `agentic`
actually calls — one workflow, two agents, two LLM calls and two tool calls per
run, so ~126k tracer calls. Reproduce with `just bench-mlflow`.

| | MLflow (sqlite) | aiwatcher |
|---|---|---|
| **Time the agent is blocked** | **90.5 s** | **1.5 s** |
| Throughput | 155 runs/s | 9,641 runs/s |
| Until the data is queryable | 90.5 s (synchronous) | 3.9 s |
| Python RSS after `import` | **181 MB** (+160 over baseline) | **28 MB** (+7) |
| Python peak RSS | 341 MB | 136 MB¹ |
| Backend process | in-process, or Postgres + MinIO + server | one process, 122 MB |
| On disk | 52 MB (7 spans/run) | 144 MB (18 events/run) |

¹ aiwatcher's peak is almost all queue: this run was configured with a 200k-event
queue so nothing was dropped. With the default it peaked at 40 MB.

**60× less time in the agent's path** is the number that matters. MLflow's
tracing is synchronous — the agent waits 6.5 ms per run while spans are written.
aiwatcher's SDK enqueues and returns; the cost moves off the request path into a
background thread and a separate process.

### Read this honestly

- **This is a firehose, not agent traffic.** A real agent produces events
  seconds apart, where MLflow's 6.5 ms per run is invisible. The gap matters for
  batch evaluation, replay, load tests and high-concurrency serving — not for
  one person chatting.
- **aiwatcher trades time for memory.** Non-blocking means a queue, and a queue
  can overflow. At this rate the old 10k default **silently discarded 119,481 of
  126,000 events** — 95%, with nothing said. The default is now 50k and an
  overflow logs to stderr on the first drop; a synthetic burst this size still
  needs an explicit `queue_size`. That was a real defect this benchmark found.
- **The disk numbers are not comparable.** MLflow stores 7 assembled spans per
  run; aiwatcher stores 18 raw events and assembles spans from them. Different
  data, not a compression result.
- **`import mlflow` costs 160 MB** before a single span is written. Against the
  512 MB budget aiwatcher is sized for, the client library alone is a third of
  it, and the production shape (`make mlflow`) is Postgres + MinIO + a server on
  top.

## What aiwatcher adds that MLflow does not

1. **A live channel.** The agent's own tracer has no notion of "this run is
   happening now". aiwatcher fans events out before storage, so a run in flight
   is visible while it runs.
2. **Correlation as a first-class field.** `correlation_id` / `causation_id`
   travel with every event and land on every span, so "what caused this" is
   answerable across process boundaries. MLflow spans nest, but they do not
   carry a message-flow identity.
3. **Cross-run aggregates without a query language.** Tokens per agent, p95 per
   model, tool failure rates — one endpoint, one page.
4. **OTLP as the storage format.** The spans are readable by anything that
   speaks OpenTelemetry. MLflow's traces are readable by MLflow.
5. **A footprint that fits a sidecar.** MLflow's production shape here is
   Postgres + MinIO + a server.

## What MLflow does that aiwatcher does not, and should not

1. **Completions.** aiwatcher's Collector deletes `gen_ai.prompt` and
   `gen_ai.completion` from spans before export by design. Debugging *what the
   model said* is MLflow's job. (The *prompt* is a different matter now — see
   below.)
2. **Artifacts and the model registry.** Storing a model, versioning it,
   promoting it. There is no artifact store here and none planned.
3. **Running an evaluation.** `deepeval`, the metric classes, the benchmark
   harness — aiwatcher takes the *result* of these and has no opinion about how
   it was produced. Scoring is the producer's job.

## The prompt registry

The second thing that moved, after the evaluation report, and for the same
reason: it is a small, well-shaped record that was living in the wrong place.

`planner-mlplatform` versions its floor-plan prompt with
`FLOOR_PLAN_PROMPT_VERSION` (a hand-incremented integer),
`FLOOR_PLAN_PROMPT_SHA256` (computed at import) and a file under `artifacts/`
for whatever `PromptOptimizer` last produced. MLflow 3 has a prompt registry
that would hold all three, and taking it means keeping the server, the Postgres
and the MinIO for a feature whose entire data model is "a string, its hash, and
what it scored".

aiwatcher's version is 200 lines over an object store, and it differs from
MLflow's in one way that is the point rather than a detail: **it has an
opinion**. MLflow's registry stores a prompt and its aliases. aiwatcher's
refuses to move `production` onto a candidate that did not improve a held-out
score, or that stopped interpolating a variable the baseline used. That is the
discipline `prompt_optimization.py` already implements in one Python file with
`DEV_CATALOG_IDS` and `TEST_CATALOG_IDS` — moved somewhere it applies to every
run, in every service, and survives the file being rewritten.

The hash matches: `PromptVersionId::of` is a plain `sha256` of the text, so
`FLOOR_PLAN_PROMPT_SHA256` and the registry's version id are the same string.
That is what lets planner publish its checked-in prompt on start-up without
inventing a second identity for it. See
[ADR_0011](ADR/ADR_0011_PROMPT_REGISTRY.md).

## What this integration cannot see

Two numbers stay empty and neither is aiwatcher's fault:

- **Cached tokens.** `agentic`'s `ModelResponse` has no field for them. The
  adapter reads three likely names speculatively, so it starts working the day
  the provider layer surfaces one.
- **Time to first token.** `LLMTracer::llm` wraps one call and returns when it
  completes; there is no first-token callback to timestamp. Adding one to that
  protocol would light up TTFT, which aiwatcher already records from an
  `llm.first_token` event.

## The evaluation report

The narrower question is not "which is the better experiment tracker" but "where
does one `mlflow.start_run` block go". In `planner-mlplatform` that block is all
there is:

```python
with mlflow.start_run(run_name=run_name) as run:
    mlflow.log_params({k: str(v)[:500] for k, v in parameters.items()})
    mlflow.log_metrics(metrics)
    mlflow.log_dict(report, "evaluation-report.json")
```

Four fields, once per suite run. Against that, MLflow's production shape is a
tracking server, a database and an object store, and its client is 160 MB at
`import`. The aiwatcher equivalent is one call on a client that is already
imported for tracing:

```python
client.record_evaluation(
    suite=run_name, params=parameters, metrics=metrics, report=report,
)
```

What that buys beyond the deletion is the join. The report knows the suite and
the parameters; the traces know the latency and the token cost; both are keyed
the same way and live in one place. Two systems that do not share ids cannot
answer "the variant that scores better — what does it cost".

What it does not buy: artifacts, a registry, or anything that runs a suite. If
those are load-bearing, MLflow stays for them and the report can still move.

## The recommendation

Keep both, teed, as `create_tracer` now does — but for a narrower reason than
before.

- **MLflow** for what it alone does: completions, model artifacts, the model
  registry, and the evaluation harness itself.
- **aiwatcher** for the online loop — is it running, what is it costing, what
  broke — for the **evaluation report**, which had to live beside the traces to
  be worth anything, and for the **prompt registry**, which had to live
  somewhere with an opinion about held-out scores.

The one thing worth deciding deliberately is **completion content**. MLflow
holds what the model said and aiwatcher does not. That is a defensible split —
one system with the sensitive data and a retention policy, one without — and it
is worth being the split you chose rather than the one that happened.

The prompt side of that split has now moved, and it moved with a caveat worth
repeating: **the registry does not redact.** Storing the prompt verbatim is the
whole point of it, so a prompt that embeds a key or a customer's data is in an
object store that nothing evicts. The Collector's redaction covers spans; it
does not cover this.
