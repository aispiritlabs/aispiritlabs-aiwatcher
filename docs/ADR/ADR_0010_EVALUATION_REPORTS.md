# ADR_0010: An evaluation report rides the event log and forms no span

- **Status**: accepted
- **Date**: 2026-08-28

## Context

[ADR_0009](ADR_0009_INSTALL_BY_DETECTION.md) put it plainly: planner's k3s "runs
MLflow for the job aiwatcher is meant to take over". Reading what
`planner-mlplatform` actually does with it, that is two jobs, not one:

1. `mlflow.pydantic_ai.autolog(log_traces=True)` in `app/observability.py` —
   tracing. aiwatcher takes this over; it is the thing aiwatcher is.
2. `log_evaluation_run(run_name, parameters, metrics, report)` — an
   `mlflow.start_run` block with `log_params`, `log_metrics` and `log_dict`,
   called from `app/evaluation/house_catalog.py` after a DeepEval suite scores
   the floor-plan cases.

The second is **not a trace**. It is a report: four things measured once, plus a
document. And it had nowhere to go. `docs/mlflow-comparison.md` said aiwatcher
has "no experiment comparison, no evaluation harness, and is not trying to
acquire them", and the panel agreed — Evaluation, Datasets and Experiments were
all `AreaPlaceholder`.

That position was right about the **harness** and wrong about the **report**.
The harness is scorers, judges, dataset curation and a batch runner; rebuilding
it would be rebuilding MLflow badly, and planner already has one in `deepeval`
and `catalog_cases`. The report is a suite name, a string map, a number map and
a JSON document — and it is the only thing standing between
`planner-mlplatform` and deleting a dependency that costs 160 MB at `import` and
a Postgres, MinIO and server in its production shape.

The awkward part is that a report *looks* traceable. It has a start, an end, a
duration and an identity. Everything about the shape says "span", and following
that would put a document into a trace store.

## Decision

**An evaluation report is a first-class record that travels on the same log as
everything else, and forms no span.**

Concretely:

1. Four event types — `eval.started`, `eval.case`, `eval.completed`,
   `eval.failed` — under a new `Subject::Eval`. They carry phases, so a report
   is running, succeeded or failed the same way a run is.

2. `EventType::forms_span()` is false for all of them, and `SpanAssembler::ingest`
   returns immediately. Nothing about an evaluation reaches VictoriaTraces.
   This is a *third* rule alongside the two already in
   [ADR_0003](ADR_0003_SPAN_ASSEMBLY.md): an unknown type takes part in no span
   because we do not know what it is; `llm.chunk` takes part in a span but is
   never a record of its own; an `eval.*` event is known, complete, and still
   not a trace.

3. An evaluation uses its own `run_id` — the evaluation id — so it gets its own
   stream and its own partition, and the ordering guarantee that comes with
   them. `ReadModel::apply` routes `Subject::Eval` to a separate projection
   rather than into `RunSummary`.

4. **A comparison is pinned to a dataset.** The baseline for a report is the
   previous finished report of the same suite *on the same dataset*. Where the
   dataset differs there is no comparison, and the panel shows none.

5. aiwatcher **records** evaluations; it does not run them. No scorers, no
   judges, no dataset ETL, no suite runner. The producer scores and reports.

The SDK surface is one call for the batch case — `record_evaluation(...)`, a
direct swap for the `start_run`/`log_params`/`log_metrics`/`log_dict` block —
and a scope for the streaming case, where cases are published as they are
scored and the suite is watchable while it runs.

## Alternatives considered

**Keep MLflow for the report half.** The honest option, and it was the standing
recommendation. It loses because the two halves stop sharing anything: the
report knows the suite and the parameters, the traces know the latency and the
token cost, and nothing joins them. It also means planner keeps a tracking
server, a database and a 160 MB client import for four fields and a document.
Keeping MLflow for the *harness* — DeepEval, the metric classes, the registry —
remains right, and is unaffected: those never talked to aiwatcher.

**A separate write path: `POST /api/v1/evaluations` into its own store.** The
obvious REST shape, and it re-implements durability, deduplication, replay and
live fan-out — the expensive, already-built part. A report published twice by a
retrying client would double-count without the deduplicator. Riding the log
costs one event type and inherits all of it.

**Make an evaluation a span.** It has a start and an end; the assembler would
need no changes at all. It loses on what a trace store is for: a twenty-minute
batch job is noise in a waterfall, a report document is not a span attribute,
and VictoriaTraces' retention is sized for spans that answer "what happened to
this request".

**Fold it into the runs list.** One less projection. It puts a row with no
agents, no LLM calls and no tokens into the view people scan for what their
agents did, and makes "run" mean two things.

**Build the evaluation harness.** Suites, scorers, an LLM judge, dataset
versioning. That is the thing the placeholder said was not worth building, and
it still is not — it is a batch problem that belongs on the Parquet side of
[ADR_0008](ADR_0008_FLOW_QUERY_SURFACE.md), and planner's harness already works.

## Consequences

- **A fifth subject.** `Subject::Eval` has to be answered for in every match in
  the assembler, even though it never reaches them. That is the exhaustiveness
  working as intended: a new subject should have to say what it is.

- **A second memory contract.** `EvaluationConfig` caps evaluations, cases per
  evaluation, cases in total, report size and reports in total — because
  `max_evaluations × max_cases_per_evaluation` is an exposure, not a bound, the
  same trap `max_spans_total` exists to close. At the defaults the projection
  holds roughly 25 MB at saturation. Under pressure an old evaluation gives up
  its cases and its document and keeps its metrics, which is the same trade
  `shed_spans` makes for an old run's waterfall.

- **The projection is a cache; the log is the record.** A shed report is still
  in the log and is readable through `GET /api/v1/runs/{id}/events`, which is
  also how "is this what we actually recorded" stays answerable. Nothing
  reconstructs it into the projection except a replay.

- **Report content is not redacted.** The Collector's `attributes/redact`
  processor removes `gen_ai.prompt` and `gen_ai.completion` *from spans*. An
  evaluation forms no span, so it never passes through the Collector. A
  producer that puts model output into `data.report` — and an evaluation report
  is exactly where someone would — is putting it in the durable log and in
  aiwatcher's memory. That is a retention decision the producer makes, and it
  should be made deliberately rather than discovered.

- **This does not make aiwatcher an experiment tracker.** There is still no
  model registry, no artifact store and no run-a-suite button, and the Datasets
  and Experiments areas remain placeholders that now say more precisely what is
  missing.

**What would make this wrong.** Two observations:

- If reports need to be queried over months rather than looked at over days —
  "every report of this suite in Q3" — the bounded in-memory projection is the
  wrong home, and this moves to the Parquet/Flow side of ADR_0008 with the
  read model keeping only the recent window.
- If per-case volume grows past a few thousand per report — a suite over a
  hundred thousand cases — the event log stops being the right transport for
  the case stream, and cases become a batch artifact with only the aggregate on
  the log.
