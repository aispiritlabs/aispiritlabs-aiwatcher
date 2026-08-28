# ADR_0007: Every way of slicing runs is one fold, and every list is a cursor page

- **Status**: accepted
- **Date**: 2026-08-28

## Context

The panel grew a second product area, then a third and a fourth — evaluation,
datasets, experiments — and the header could no longer be a flat list of pages.
Splitting the navigation was the trigger; what it exposed in the explorer was
the real problem.

**Three code paths for what is one question.** The explorer offered four pivots.
Sessions came from `/conversations`, backed by `conversations::compute`; agent,
model and tool came from the *metrics* fold, reusing `by_agent` / `by_model` /
`by_tool` because they happened to be counted there. Two sources, two row
shapes, and a fifth pivot meant picking one of them to extend.

**Two grouping controls fighting.** The pivot grouped the tree; a second control
in the message pane re-grouped the already-narrowed messages by span, agent or
type. Having narrowed to one span it offered to group that span's messages by
span. It also collapsed the one view that has to stay chronological.

**Nothing was lazy.** `GET /runs/{id}/events` returned the entire stream, and
the WAL answered it by scanning **the whole log** and filtering — a cost that
grew with every unrelated run ever recorded. The browser then mounted a row per
event. The `q` search parameter had been declared in the route's schema and
never implemented, which is what a client-side search costs: you cannot filter
what you have not downloaded, so it was never built.

**Two dimensions people asked for did not exist.** `runtime` and `workflow` were
not fields anywhere.

## Decision

**One fold, parameterised by which key a run contributes.** `dimensions::compute`
takes a `DimensionKind` and returns the same `DimensionSummary` shape for
`session | agent | runtime | workflow | trace | model | tool`. One endpoint,
`GET /api/v1/dimensions/{kind}`, and one row renderer in the panel.
`conversations::compute` is now a thin rename over it, so the `/conversations`
contract is unchanged and the duplicate fold is gone. A run with no key for the
chosen dimension counts in `ungrouped_runs` rather than vanishing — a tree that
silently holds fewer runs than the runs list is a tree nobody trusts.

**`runtime` is derived; `workflow` is promoted.** `runtime` is
`source.service`, already on every record — no wire change. `workflow` becomes
an optional `workflow_id` on the envelope, **read from `data.workflow` when
absent**, because the `agentic` integration has been sending the name there
since before there was a field for it. The fallback is what lets the dimension
light up for producers that never ship an SDK update, and what keeps a replay of
an older log producing the same rows.

**One grouping control.** The pivot decides the shape. Below it the message list
is flat and chronological, narrowed by the tree selection and by search. The
second control is gone.

**Spans get a flat index.** `GET /api/v1/spans` returns one row per span with
the run id attached (a `CompletedSpan` does not know its own run; the read
model's map key does) and the filtering attributes lifted into fields. It is a
pass over what the read model already holds — no second store, no new retention
— and it answers "every tool call slower than two seconds", which the waterfall
cannot.

**Paging is a port method, not an adapter's optimisation.**
`MessageSource::read_stream_page(stream, after, limit)` has a default
implementation that slices `read_stream`, so `broker` and `laser` keep working
untouched, and the shared contract test runs against all three adapters. The
WAL overrides it with a per-stream offset index built in the same replay pass
that already builds `stream_positions`. Reading a page now seeks; measured, a
50-event page takes 4.9 ms from a 200-event log and 4.8 ms from a 12,200-event
one.

**Search is server-side, on the page that was read.** `q` filters
`event_type`, `agent_id`, `span_key`, `workflow_id` and the serialised payload.
The response carries `scanned` alongside `events`, because without it a page
where nothing matched is indistinguishable from the end of the run. The cursor
comes from what was *read*, never from what survived the filter, so paging past
an empty page still advances.

**The live path stays in Rust; the analytical path is batch.** Evaluation,
datasets and experiments are ETL over exported runs, not requests against the
live read model: a suite scores thousands of cases and a scorer is itself an LLM
call. That work goes to a separate Flow PHP service reading Parquet the
projector exports, writing artifacts the Rust API serves. The contract between
them is a Parquet directory and a job description, not a synchronous call. The
explorer never crosses that boundary — it needs keystroke latency, which a batch
engine behind a network hop cannot give.

## Alternatives considered

**Add `runtime` and `workflow` as two more special cases.** Cheapest change, and
it would have made a fifth code path in the tree and a third row shape. The
pivots differ by one line — which key a run contributes — so anything that made
them differ by more was encoding an accident.

**Keep the message-pane grouping and only add pivots.** Rejected: the two
controls compose into states that mean nothing (narrow to a span, group by
span), and the cost of removing it is one chronological list, which is what the
audit view should have been.

**Client-side search and filtering.** What the unimplemented `q` was originally
sketched as. It requires having downloaded the whole run first, which is the
exact thing the paging work removes.

**Flow PHP in front of every read, including the explorer.** It is a batch
framework in a language nothing else here speaks; putting it on the live path
adds a process and a network hop to every keystroke. Rejected on latency, not
on taste — the batch half is where it earns its place.

**DataFusion or Polars instead of Flow PHP.** Same DataFrame semantics, in
process, no new runtime in the deployment. Rejected because the batch workload
is genuinely separate and the choice of engine for it is not this decision's to
make; the boundary drawn here (Parquet in, artifacts out) leaves it swappable.

**A per-stream index in the WAL, deferred.** The existing comment said to add
one "when this shows up in a profile, not before". It showed up: the explorer
pages events, and a full-log scan per page is a cost that grows with unrelated
traffic.

## Consequences

The WAL carries one more `u64` per event — the same order as the existing
`offsets` index. Measured under `just load-test` at 5000 runs: 124 MB RSS
(debug build), unchanged in shape against the 512 MB container limit.

`workflow_id` is optional and `schema_version` does not move, so an older
producer stays valid and a newer backend reads an older log unchanged. The
`data.workflow` fallback must not be removed while logs written before this
change are still being replayed.

`GET /runs/{run_id}/events` returns `EventPage` rather than a bare array — a
breaking response-shape change for any direct consumer of that route. The panel
is generated from the contract, so it broke at compile time, which is the point.

`total_known` on the span list and `total` on a dimension page are counts within
the retention window, not counts of everything that ever happened. Retention is
already stated on the metrics page; these inherit the same caveat.

The dimension fold clones the run summaries under the read lock, as
`conversations` and `metrics` already do. At the current caps that is a pass
over in-memory data with no I/O; it is not free at ten times the retention.

**What would make this wrong.** If a dimension appears that is not a function
of a run — one that needs its own index, or that groups something other than
runs — the single fold stops paying and becomes a `match` with seven unrelated
arms. If the dimension fold shows up in a profile as retention grows, the answer
is an incrementally maintained index, not a bigger pass. And if the batch side
ever needs to answer a question interactively, the live/batch line drawn here is
the thing to reopen, not to work around.
