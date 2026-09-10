# Curation engines over one corpus

The data-curation area runs its transforms in Flow PHP and its notebook blocks
in Python. This directory measures what that costs at size, and what the
alternatives cost: the same curation questions in Flow, Polars, DataFusion,
Daft, PyArrow and marrow, over the same synthetic corpus, with every answer
checked against another engine's before a time is reported. A faster engine
that computed something else has not been measured at all.

```bash
just bench-curation-generate 10GB        # the corpus: CSV of that size, the same rows as Parquet
just bench-curation 1GB                  # four queries, Flow and Polars, results/1GB.md
just bench-curation 10GB --engines polars polars-1t
just bench-curation-serve 1GB            # the comparison inside aiwatcher (below)
just bench-curation-run                  # …started from the shell, times read back from aiwatcher
```

| file | what it answers |
|---|---|
| `generate.py`, `corpus.py` | the corpus, deterministic from a seed |
| `bench.py`, `polars_bench.py`, `flow_bench.php` | four queries in Flow and Polars, each in its own process, answers compared |
| `profile_flow.php` | where Flow's time goes, one stage at a time |
| `compare_5gb.py` | one query in a Rust process, a fresh Python, a warm interpreter and a marimo step (run from `services/ml_pipeline`) |
| `faster_python.py` | the same query over CSV, Parquet, Arrow IPC, categoricals, a cached frame, a fork server |
| `arrow_exchange.py` | PyArrow as an engine, the format a step hands on, and the Polars ↔ PyArrow exchange |
| `engines_5gb.py` | Polars, DataFusion, DuckDB and Daft, CSV and Parquet — each through its Python API, no SQL |
| `rust/`, `rust-datafusion/` | the same query on the `polars` and `datafusion` crates, standalone Rust projects |
| `mojo/q2.mojo`, `mojo/marrow_q2.py` | the same query in marrow: compiled Mojo, and marrow's Python frontend |

## The corpus

A synthetic `spans` corpus at the grain of the Flow service's `spans` dataset:
190 bytes of CSV a row, 12 spans a run. 10 GB is 56,437,497 rows in 57 parts.
The one-file comparisons below use 5 GB merged into **one file** — 5.37 GB,
28,219,243 rows — and its Parquet copy, 414 MiB written by Polars.

Every number here comes from one machine: Apple M1 Max (8P + 2E cores, 64 GB),
macOS 26, PHP 8.5.10 with the tracing JIT, Flow 0.43.0, Polars 1.44.2 (the
`polars` crate 0.55.2), DataFusion 54.0.0 (the crate 55.0.0), Daft 0.7.24,
PyArrow 25.0.1, marrow at its repository's head on Mojo 1.1.0.dev2026090305.
Warm page cache unless said otherwise; a first run after a large write is slower
because the file has left the cache, and those are not the numbers compared.

## One question, every way to run it

q2 — the LLM spans per model, their token sums and mean latency — over the one
5 GB file. Warm runs, the engine's own time, peak resident memory of the
process that did the work. **Every row returned the same answer.**

| engine | CSV | peak | Parquet | peak |
|---|---|---|---|---|
| Flow PHP, one process, JIT | 548.8 s | 59 MiB | — | — |
| `polars` crate in a Rust process | 11.0 s | 5.1 GiB | — | — |
| marrow, Python frontend | — | — | 11.8 s | 541 MiB |
| marrow, compiled Mojo | — | — | 3.6 s | 177 MiB |
| Daft | 2.6–2.9 s | 1.5–2.4 GiB | 1.19–1.26 s | 1.85 GiB |
| PyArrow, computed in Acero | 2.05 s | 2.19 GiB | 0.855 s | 3.22 GiB |
| PyArrow reads, Polars computes | 1.92 s | 4.15 GiB | — | — |
| Polars (Python) | 1.93–2.14 s | 5.4–5.5 GiB | 0.09–0.19 s | 0.4–0.5 GiB |
| DuckDB, relational Python API | 2.02 s | 677 MiB | 0.215 s | **227 MiB** |
| DataFusion, Python DataFrame API | **1.68 s** | 280 MiB | 0.161 s | 310 MiB |
| **DataFusion in a Rust process** | **1.8–2.0 s** | **93 MiB** | **0.148 s** | **98 MiB** |
| Polars over a frame a warm worker keeps | — | — | 0.045 s | 1.53 GiB |

Every Python row is written through the engine's own API — Polars' and
DataFusion's DataFrame expressions, DuckDB's relational API (`ColumnExpression`,
`FunctionExpression`, `.filter()`, `.aggregate()`) — with no SQL string anywhere.
Where one engine was measured in more than one session the cell is a range.

What the table says:

- **Flow is 250–300× behind on the same file**, and it is the execution model —
  one PHP process evaluating an expression tree over a row of objects at a time —
  rather than memory, as the next section shows.
- **DataFusion matches Polars' speed in a twentieth of the memory** — and in a
  Rust process, a fiftieth: 93 MiB for the 5 GB CSV, about five cores busy.
  Polars maps the whole CSV (5.5 GiB for a 5 GB file); DataFusion streams it in
  batches. A worker that runs DataFusion in its own process stays inside the
  512 MB the read model is budgeted for, which Polars over a CSV does not.
- **DuckDB is in the same league without SQL**: 2.0 s over the CSV at 677 MiB,
  0.215 s over Parquet at 227 MiB — the leanest on Parquet. Its relational API
  is Python all the way down, and aiwatcher already builds DuckDB into the Rust
  workflow store behind its `duckdb` feature, so it has a Rust path as well.
- **The format matters more than the engine.** Every engine is 10–20× faster on
  Parquet, and the file is 13× smaller.
- **marrow is correct and not yet fast.** Compiled from Mojo, the query is a
  9.0 MB binary that answers in 3.6 s on about 1.6 cores at 177 MiB; through its
  Python frontend, 11.8 s. Both agree with every other engine to the last digit.
  It is alpha, says so, and is validated against DuckDB — worth watching for its
  compiled-query model, not a candidate engine today.
- **The `polars` crate built with stable Rust is 5× behind the Python wheel** on
  the same engine. CPU accounting shows why: 15.9 s of CPU over 10.5 s wall is
  about 1.5 cores, where the wheel used about 4.9. Fat LTO and mimalloc changed
  nothing (11.04 s against 11.17 s); the wheel's own build flags are the likely
  difference, not verified.
- **Daft and PyArrow's Acero are slower here** — Daft is built for multimodal and
  distributed work, and a plain aggregation is not its case.

## How a Polars query is run

The same Polars query over the CSV, run the ways an engine could run someone's
code (`compare_5gb.py`, `faster_python.py`):

| runner | wall | on top of the query |
|---|---|---|
| marimo step (`run_notebook`) | 3.21 s | 0.88 s |
| a script file, `python tmp_query.py` | 2.44 s | 0.15 s |
| a warm interpreter, first query | 2.15 s | 0.11 s |
| a warm interpreter, later queries | 2.17 s | ≈ 0 |

A fresh, isolated process costs 124 ms as a new interpreter importing Polars,
and **8 ms from a fork server** that has Polars preloaded. So each query can have
a process of its own — its own ceilings, nothing left behind — for almost
nothing; marimo's 0.9 s is the notebook runtime and belongs to notebook blocks.

## Formats

Polars, the same query over the same rows (`faster_python.py`):

| format | warm run | peak | file |
|---|---|---|---|
| CSV, streaming | 1.92 s | 5.47 GiB | 5.0 GiB |
| CSV, in-memory | 2.06 s | 6.35 GiB | |
| **Parquet (zstd), streaming** | **0.105 s** | **464 MiB** | **414 MiB** |
| Parquet, in-memory | 0.154 s | 1012 MiB | |
| Parquet with categoricals | 0.22 s | 328 MiB | 414 MiB |
| Arrow IPC, uncompressed | 0.38 s | 10.9 GiB | 10.7 GiB |

Converting the CSV to Parquet once took 5.9 s. Uncompressed Arrow IPC is larger
than the CSV it came from; categoricals trade speed for memory in this version.

## Handing a step's output on

What it costs to write 2 M rows × 5 columns and read them back, as the next step
would (`arrow_exchange.py`):

| format | write + read | bytes |
|---|---|---|
| JSON rows — what a step hands on today | 1.33 s | 215 MiB |
| Python dicts through `json` — the notebook path | ≈ 3.6 s | |
| CSV | 0.095 s | 83 MiB |
| Arrow IPC stream | 0.19 s | 137 MiB |
| **Arrow IPC file, memory-mapped** | **0.048 s**, reading ≈ 0 | 137 MiB |
| **Parquet (zstd)** | **0.051 s** | **15 MiB** |

Between Polars and PyArrow the exchange is free: 12.7 M rows cross either way in
0.06–0.10 ms through the PyCapsule interface, and five exports held at once grew
the peak by 0 MiB. Polars reads and writes Parquet and Arrow IPC natively, so an
engine needs no PyArrow to use them.

## Four queries, Flow and Polars

The first matrix, with CSV input:

| query | Polars, 10 cores | Polars, 1 thread | Flow, 100 MB | Flow, 1 GB | **Flow, 10 GB** | Flow ÷ Polars |
|---|---|---|---|---|---|---|
| q1 filter + count | 2.92 s · 2.0 GiB | 14.1 s · 271 MiB | 6.9 s · 52 MiB | 74 s · 52 MiB | *≈ 12 min* | ≈ 250× |
| q2 group by model | 3.17 s · 2.1 GiB | 16.8 s · 273 MiB | 10.0 s · 59 MiB | 102 s · 59 MiB | *≈ 17 min* | ≈ 320× |
| q3 group by run (4.7 M groups) | 4.12 s · 3.2 GiB | 18.8 s · 2.6 GiB | 12.3 s · 63 MiB | — | *≈ 20 min* | ≈ 300× |
| q4 filter + join + derive | 5.53 s · 2.4 GiB | 28.5 s · 297 MiB | 19.3 s · 69 MiB | — | *≈ 32 min* | ≈ 350× |

Polars is measured at 10 GB; Flow at 100 MB and 1 GB and scaled, in italics — it
streams, and q1 took 10.8× as long on ten times the data, q2 10.2×. The one-file
run above measured Flow's q2 at 5 GB directly: 548.8 s, in line with the scaling.

## Where Flow's time goes

`php profile_flow.php .data/100MB`, 551 210 rows, tracing JIT. Each line adds
one thing to the one above it:

| stage | time | |
|---|---|---|
| PHP `fgets`, every line | 0.05 s | reading the file costs nothing |
| PHP `fgetcsv`, every field | 1.72 s | PHP's CSV parser: the floor for anything written in PHP |
| PHP `fgetcsv` + q1's predicate, by hand | 1.72 s | the predicate itself is free |
| Flow `from_csv`, typed by schema, `count()` | 5.60 s | **Flow's rows: 3.9 s of object work per 100 MB** |
| Flow `from_csv`, untyped | 8.40 s | without a schema every cell's type is guessed: +50% |
| Flow q1 | 6.41 s | the filter: +0.8 s |

**Memory is not the bottleneck.** A batch of 10 000 rows used 94 MiB and ran
slower than Flow's default 1 000; 100 000 used 826 MiB and was slower still. The
cost is per cell: every value becomes an `Entry` object with a type, and every
expression is an object graph evaluated row by row. The query text matters less
than it looks: in q4 the join (+5.7 s) and the BigDecimal `plus`/`multiply`
(+3.9 s) are the largest clauses, and the best rewrite — lookups by
`match_cases()`, native arithmetic through `call()`, Parquet with the ten
columns it reads — took q4 from 20.4 s to 15.1 s over the same rows.

| lever, measured | effect on Flow |
|---|---|
| Parquet with the columns a query reads | 1.5–3.4× |
| tracing JIT (`opcache.jit=tracing`, buffer ≤ 128M on AArch64) | 1.3× |
| a schema on every read | 1.5× over untyped |
| native arithmetic instead of `plus`/`multiply` | 1.2× on q4 |
| batch size, memory limit, the collector | none, or slower |

## Inside aiwatcher

The same comparison runs as a managed curation pipeline, `curation/flow-vs-polars`
(`examples/flow-vs-polars`, seeded at start-up):

```
Benchmark corpus → Flow PHP: per model → Polars: the same query → View
   corpus_spans      the q2 aggregation      flow_vs_polars.py      curation/flow-vs-polars
```

Measured on the 1 GB corpus (5,643,870 rows) with `just bench-curation-serve 1GB`
and `just bench-curation-run`, as aiwatcher's workflow fold reports it:

| step | engine | took |
|---|---|---|
| `corpus` (+ `flow`) | Flow PHP: one query over the CSV | **95.2 s** |
| `polars` | the notebook: Polars over the same files, then the comparison | **1.17 s** — Polars' own query 0.35 s |
| `result` | publish the comparison as a dataset version | 0.03 s |

All eight models agreed. The first run drew every step as 0 ms in the waterfall:
the in-process reactor stamped a step's outcome with the clock its pass began on.
`Reactor::poll_once` now stamps it with when the work ended, and the lease is
still re-checked against the pass's clock, because nothing renews it while a
step runs. `a_step_that_took_time_reports_its_outcome_when_it_finished_rather_than_when_it_began`
holds it.

Three limits had to move for a corpus on disk, and each is configuration:

| limit | default | set by |
|---|---|---|
| a managed Flow step's timeout | 300 s | `AIWATCHER_FLOW_STEP_TIMEOUT_SECONDS` |
| the Flow service's own ceiling (CPU time) | 30 s | `AIWATCHER_FLOW_TIMEOUT_SECONDS` — above the step's: PHP stopping first is a 500, which a reactor retries |
| a notebook subprocess | 120 s | `AIWATCHER_ML_PIPELINE_TIMEOUT` |

A managed Flow step still returns at most 1 000 rows, so q3 and q4 cannot go
through one; and a reactor renews no lease while it waits, so beside a second
reactor a Flow step longer than five minutes can be taken over.

## What this decided

Recorded in [AW-3](../../docs/specs/AW-3-datafusion-and-duckdb-as-query-and-curation-engines/02-spec.md):
the query engine is chosen per deployment — `flow`, `datafusion`, or `duckdb` as
the addition — behind one contract. The two beside Flow are written as ordinary
Python, each through its own API and none through SQL, and run in a warm worker
behind a fork server. Polars is not one of them: as fast as DataFusion, but it
maps the whole CSV — 5.4–5.5 GiB for a 5 GB file against DataFusion's 280 MiB —
and a query engine's peak is what a deployment has to size for.
Rows at size are Parquet at rest and Arrow IPC between processes, never JSON.
marimo stays the notebook runtime rather than the query engine.
