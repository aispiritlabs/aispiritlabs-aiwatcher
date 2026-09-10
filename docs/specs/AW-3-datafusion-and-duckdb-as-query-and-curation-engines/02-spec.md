---
id: AW-3
step: spec
status: doing
branch: feat/agent-sdk-merge
repo: aiwatcher
created: 2026-09-10
updated: 2026-09-10
tags: [spec/AW-3, step/spec, branch/feat-agent-sdk-merge, status/doing]
---

`#spec/AW-3` · `#step/spec` · `#branch/feat-agent-sdk-merge` · repo `aiwatcher`

# ② Spec — AW-3

## Proposal

**Intent:** a deployment chooses its query engine — Flow DSL, DataFusion or DuckDB —
with one setting, and everything that queries or curates (the Query tab, recipes,
pipeline transforms, managed steps) talks to *the query engine* rather than to
Flow. The two beside Flow are written as ordinary Python, each through its own API;
no engine takes SQL. The benchmark measured why: over one 5 GB file Flow took
548.8 s; DataFusion 1.68 s over the CSV at 280 MiB and 0.161 s over Parquet; DuckDB
2.02 s over the CSV at 677 MiB and 0.215 s over Parquet at 227 MiB, the leanest
there. Polars was as fast and is not one of them: it maps the whole CSV, and peaked
at 5.4–5.5 GiB for a 5 GB file. Every answer was the same.

**Decisions settled from the investigation** (the user's, 2026-09-10):

| question | settled |
|---|---|
| engines beside Flow | two — DataFusion, and DuckDB as the addition, delivered after it — both measured by `engines_5gb.py` giving Flow's answers in under a gigabyte |
| Polars | measured and not taken: as fast as DataFusion, with a peak of 5.4–5.5 GiB over the 5 GB CSV against DataFusion's 280 MiB — a query engine's peak is what a deployment has to size for |
| language for DataFusion | ordinary Python through DataFusion's own DataFrame API — `col`, `lit`, `functions` — not SQL |
| language for DuckDB | ordinary Python through DuckDB's relational API — `ColumnExpression`, `ConstantExpression`, `FunctionExpression`, `.filter()`, `.aggregate()` — no SQL string and no `SQLExpression` |
| SQL | offered by no engine: a query is Python in DataFusion and DuckDB, Flow DSL in Flow |
| how an engine is chosen | at deployment: `AIWATCHER_QUERY_ENGINE=flow\|datafusion\|duckdb`, one per deployment |
| layout | generalised: `services/query/` holds the contract and the engines — `flow`, `datafusion`, `duckdb`; Rust has `execution/flow.rs`, `execution/datafusion.rs` and `execution/duckdb.rs` side by side |
| running Python a person typed | an `open` default — a subprocess with ceilings, as a notebook runs — and an opt-in `strict` mode that admits only the engine's own vocabulary |
| reading the read model | the Python engines page the API exactly as Flow does; no bulk export route |

**In scope:**
- One query contract (`/query/*`), implemented by every engine and proved by one
  conformance suite.
- The DataFusion engine: queries, pipeline transforms and managed steps, through its
  Python DataFrame API, over the same catalog Flow serves, under the same answer
  contract (1 000 rows, `truncated`, `deterministic`, `window_applied`, `digest`).
- The DuckDB engine, the same way, through its relational Python API — the
  addition, delivered after DataFusion.
- Deployment selection in env, the justfile, Docker Compose and Helm; the
  generalised names, with the Flow-specific ones kept as aliases for one release.
- Blocks, recipes and examples that name the engine they were written for.
- The panel following the deployed engine.
- ADR_0028, amending ADR_0008, ADR_0014 and ADR_0024.

**Out of scope:**
- Polars as an engine. It stays in `benchmarks/curation` as a measurement; a
  notebook block may still use it, because a notebook is its own code.
- SQL, in any engine, and translating a query between engines. A query is written
  for one engine and runs on that engine.
- Two engines active in one deployment.
- Results over 1 000 rows — Parquet step outputs and sharded dataset versions are
  the next spec; so are hub corpora staged as Parquet. This spec keeps every
  engine inside today's answer contract so that switching changes nothing else.
- A bulk export route for runs and spans.
- Running an engine inside the aiwatcher process. DataFusion in a Rust process held
  93 MiB over the 5 GB CSV and DuckDB already builds into the workflow store, so
  it is worth weighing — in the job phase, not as a requirement here.

**Approach:** generalise before adding. The Flow service's contract, its client in
the panel, its executor in Rust and its deployment values become the *query
engine's*, with Flow as the first implementation; DataFusion is the second and
DuckDB the third, Python services speaking the same contract. DuckDB comes last:
it is the addition, and nothing in the first two depends on it. A transform's text
belongs to the engine it names, so a deployment runs what was written for it and
refuses the rest by name.

**Success criteria:**
- [ ] With no new setting, every existing test, recipe, pipeline and deployment
      behaves exactly as before.
- [ ] One conformance suite passes against all three engines: the benchmark's
      q1–q4, each written in the engine's language, return the same rows.
- [ ] `AIWATCHER_QUERY_ENGINE=datafusion just bench-curation-serve 1GB` runs the
      DataFusion variant of the benchmark pipeline as a managed pipeline, with its
      step times in the Workflows waterfall — and `duckdb` runs its own.
- [ ] Over the 5 GB conformance corpus, neither Python engine's peak resident
      memory passes 1 GiB, on the CSV or on Parquet.
- [ ] Switching a Helm release between engines is one value.
- [ ] In `strict` mode, a query that reads a file, imports a module, passes a
      function or reaches for SQL is refused naming what it tried.

## Spec (delta)

### ADDED Requirements

#### Requirement: A deployment chooses one query engine
The server SHALL read `AIWATCHER_QUERY_ENGINE`, taking `flow` (the default),
`datafusion` or `duckdb`, and SHALL register the executor for that engine and no
other. It MUST refuse to start on any other value, naming the variable and the
values it takes.

##### Scenario: nothing set is today's deployment
- GIVEN no `AIWATCHER_QUERY_ENGINE`
- WHEN the server starts with `AIWATCHER_QUERY_URL` set
- THEN it registers the Flow executor, and claims `flow_php` attempts only

##### Scenario: a DataFusion deployment claims only DataFusion work
- GIVEN `AIWATCHER_QUERY_ENGINE=datafusion` and `AIWATCHER_QUERY_URL` set
- WHEN the reactor builds its claim filter
- THEN it holds the DataFusion executor and no other, and a `flow_php` attempt is
  never claimed by it

##### Scenario: an engine that is not offered
- GIVEN `AIWATCHER_QUERY_ENGINE=polars`
- WHEN the server starts
- THEN it refuses to, naming `AIWATCHER_QUERY_ENGINE` and
  `flow, datafusion, duckdb`

#### Requirement: One query contract, proved against every engine
Every engine SHALL serve `/query/healthz`, `/query/datasets`, `/query/check`,
`/query/query`, `/query/simulate` and `/query/executions/{id}`, with the request and
answer fields the Flow service serves today. `/query/healthz` and `/query/datasets`
SHALL also carry `engine` and `language`. A conformance suite under
`services/query/contract` SHALL run the same questions against any engine and
compare the rows.

##### Scenario: the panel learns which engine it is talking to
- GIVEN a DataFusion deployment
- WHEN the panel reads `/query/healthz`
- THEN the answer names `engine: datafusion` and `language: datafusion-python`

##### Scenario: the same question, the same rows
- GIVEN the conformance corpus
- WHEN q1, q2, q3 and q4 run against Flow in Flow DSL, against DataFusion in
  DataFusion Python and against DuckDB in DuckDB Python
- THEN all three return the same rows — counts and sums exactly, means within the
  rounding Flow applies

##### Scenario: an answer over the cap is reported, not cut silently
- GIVEN a DataFusion query that would return 1 001 rows
- WHEN it runs through `/query/query`
- THEN the answer carries 1 000 rows and `truncated: true`, as Flow's does

#### Requirement: A DataFusion query is ordinary DataFusion Python
The DataFusion engine SHALL evaluate query text as Python in which `col`, `lit` and
`f` (DataFusion's `functions`) are DataFusion's, and `read(name, **arguments)` opens
a catalog dataset as a DataFusion `DataFrame`. The value of the last expression
SHALL be the result, a DataFusion `DataFrame`, collected into the answer. In a
pipeline transform, `df` SHALL be bound to the rows the chain has so far. There is
no `SessionContext` in a query's namespace and no SQL: a dataset is reached through
`read()`, as in every engine.

##### Scenario: the Query tab's first question, in DataFusion
- GIVEN the text `read("spans").filter(col("kind") == lit("llm"))
  .aggregate([col("model")], [f.count(col("model")).alias("spans")])`
- WHEN it runs on a DataFusion deployment
- THEN the answer has one row per model, with the columns `model` and `spans`

##### Scenario: a transform reads what came before it
- GIVEN a pipeline whose source reads `corpus_spans` and whose transform is
  `df.filter(col("status") == lit("error"))`
- WHEN the chain runs on a DataFusion deployment
- THEN the transform's rows are exactly the source's rows with `status = error`

##### Scenario: a result that is not a frame
- GIVEN a query whose last expression is `42`
- WHEN it runs
- THEN it is refused with a 422 saying a query ends in a DataFusion `DataFrame`,
  and naming what it ended in

##### Scenario: a syntax error is located
- GIVEN text that does not parse as Python
- WHEN `/query/check` reads it
- THEN the diagnostic carries the line and column Python's parser reported

##### Scenario: a corpus larger than the process stays small
- GIVEN the 5 GB conformance corpus as CSV
- WHEN the per-model aggregation runs on DataFusion
- THEN the engine's peak resident memory SHOULD stay under 512 MiB — measured at
  280 MiB through the Python binding and 93 MiB in a Rust process, against
  Polars' 5.4–5.5 GiB on the same file

#### Requirement: A DuckDB query is ordinary DuckDB Python
The DuckDB engine SHALL evaluate query text as Python in which `ColumnExpression`,
`ConstantExpression`, `FunctionExpression`, `CaseExpression` and `StarExpression`
are DuckDB's, and `read(name, **arguments)` opens a catalog dataset as a DuckDB
relation. The value of the last expression SHALL be the result, a relation,
collected into the answer. In a pipeline transform, `df` SHALL be bound to the rows
the chain has so far. There is no connection in a query's namespace, no `sql()` and
no `SQLExpression`: a dataset is reached through `read()`, as in every engine.

##### Scenario: the first question, in DuckDB Python
- GIVEN the text `read("spans").filter(ColumnExpression("kind") ==
  ConstantExpression("llm")).aggregate([ColumnExpression("model"),
  FunctionExpression("count", ColumnExpression("model")).alias("spans")], "model")`
- WHEN it runs on a DuckDB deployment
- THEN the answer has one row per model, and in the conformance suite it is the
  same answer the other engines give

##### Scenario: a transform reads what came before it, in DuckDB
- GIVEN a pipeline whose transform is
  `df.filter(ColumnExpression("status") == ConstantExpression("error"))`
- WHEN the chain runs on a DuckDB deployment
- THEN the transform's rows are exactly the previous rows with `status = error`

##### Scenario: an expression is not a relation
- GIVEN a DuckDB query whose last expression is `ColumnExpression("model")`
- WHEN it runs
- THEN it is refused with a 422 saying a query ends in a relation, and naming
  what it ended in

##### Scenario: a sum wider than 64 bits comes back as an integer
- GIVEN a query that sums a `BIGINT` column, which DuckDB returns as a 128-bit
  `HUGEINT`
- WHEN it answers
- THEN the value is an integer in the answer, as every other engine's is, and a
  sum that does not fit 64 bits is reported rather than rounded

##### Scenario: DuckDB over Parquet stays the leanest
- GIVEN the 5 GB conformance corpus as Parquet
- WHEN the per-model aggregation runs on DuckDB
- THEN the engine's peak resident memory SHOULD stay under 512 MiB — measured at
  227 MiB, and at 677 MiB over the same corpus as CSV

#### Requirement: Running typed Python has a stated boundary
The DataFusion and DuckDB engines SHALL read `AIWATCHER_QUERY_ADMISSION`: `open`
(the default) or `strict`. In `open` mode a query SHALL run in a child process with
a CPU-time ceiling (`AIWATCHER_QUERY_TIMEOUT_SECONDS`), a memory ceiling, a scratch
working directory and an environment holding no `AIWATCHER_*` credential. In
`strict` mode the text SHALL be parsed and admitted before it runs: the engine's
own constructors and methods only, refusing anything that reads or writes a file or
a URL, takes a callable, imports a module, runs SQL — including a string DuckDB
would parse as SQL, anywhere but a bare column name as a group key — or names an
attribute beginning with `_`. The service MUST bind to `127.0.0.1` by default, as
Flow and the notebook runtime do.

##### Scenario: strict mode refuses a file read by name
- GIVEN `strict` admission on a DuckDB deployment
- WHEN a query calls `duckdb.read_csv("/etc/passwd")`
- THEN it is refused before anything runs, naming `read_csv` and saying a query
  reads datasets through `read()`

##### Scenario: strict mode refuses a function
- GIVEN `strict` admission on a DataFusion deployment
- WHEN a query calls `udf(lambda array: array, …)`
- THEN it is refused naming `udf`

##### Scenario: strict mode refuses SQL in DataFusion
- GIVEN `strict` admission on a DataFusion deployment
- WHEN a query calls `SessionContext().sql("SELECT 1")` or `register_csv(…)`
- THEN it is refused before anything runs, naming what it called and saying a
  query reads datasets through `read()`

##### Scenario: strict mode refuses a SQL string in DuckDB
- GIVEN `strict` admission on a DuckDB deployment
- WHEN a query calls `read("spans").filter("kind = 'llm'")`, `SQLExpression(…)` or
  `duckdb.sql(…)`
- THEN it is refused before anything runs, naming the string or the call, and
  saying a filter is written with `ColumnExpression` and `ConstantExpression`

##### Scenario: strict mode admits the benchmark
- GIVEN `strict` admission
- WHEN the conformance suite's four queries are checked, in DataFusion and in
  DuckDB
- THEN all eight are admitted and run

##### Scenario: open mode stops a query at its ceiling
- GIVEN `open` admission and `AIWATCHER_QUERY_TIMEOUT_SECONDS=2`
- WHEN a query spins for longer than that
- THEN the child is stopped and the answer names the ceiling, while the service
  keeps answering other requests

##### Scenario: open mode holds no credentials
- GIVEN `open` admission and a service started with `AIWATCHER_AUTH_INGEST_TOKENS`
  set
- WHEN a query reads `os.environ`
- THEN no `AIWATCHER_*` variable is present in the child

#### Requirement: The Python engines' catalog is Flow's catalog
The DataFusion and DuckDB engines SHALL serve the dataset names, declared columns,
parameters, windows and `deterministic` answers Flow serves, reading API datasets
through the same routes with the same `window_seconds` and `as_of`, and
`corpus_spans` from `AIWATCHER_CORPUS_DIR`.

##### Scenario: a window reaches the API
- GIVEN `read("spans", period="1h")`
- WHEN it runs on either Python engine
- THEN every page it requests carries `window_seconds=3600`

##### Scenario: a corpus read is never remembered
- GIVEN a query over `corpus_spans`
- WHEN it answers
- THEN `deterministic` is `false`

#### Requirement: A managed step on the deployed engine
A chain whose transforms name `datafusion` or `duckdb` SHALL compile to a step of
that engine, run by `execution/datafusion.rs` or `execution/duckdb.rs` against the
query engine, with the step timeout from `AIWATCHER_QUERY_STEP_TIMEOUT_SECONDS` and
caching by `deterministic`, as a Flow step is. A plan that needs an engine this
deployment does not run MUST be refused when it is started, naming the block, the
engine it was written for and the engine this deployment runs.

##### Scenario: the benchmark pipeline, on DataFusion
- GIVEN a DataFusion deployment and the DataFusion variant of the benchmark
  pipeline
- WHEN it is started on the server
- THEN its DataFusion step completes, its rows reach the notebook, and the
  waterfall draws its duration

##### Scenario: a Flow pipeline on a DataFusion deployment
- GIVEN a DataFusion deployment and a pipeline whose transform names `flow`
- WHEN it is started
- THEN the start is refused with a 422 naming the block, `flow` and `datafusion`,
  and no run is created

#### Requirement: Content names the engine it was written for
A `transform` block and a recipe SHALL carry `engine: flow | datafusion | duckdb`;
absent SHALL read as `flow`, so everything saved before this change keeps its
meaning. The examples SHALL include a variant of each pipeline the seed ships for
every engine its transforms can be written in.

##### Scenario: a pipeline saved before the field
- GIVEN a pipeline revision whose transform has no `engine`
- WHEN it is read and compiled
- THEN it is a Flow transform, and its `plan_id` is the one it compiled to before

##### Scenario: the seed on any deployment
- GIVEN a fresh store
- WHEN the seed imports
- THEN every variant of the benchmark pipeline is saved, and each names its engine

#### Requirement: The panel follows the deployed engine
The Query tab, the recipe editor and the pipeline inspector SHALL write in the
language `/query/healthz` names. Build mode SHALL emit that language, and SHALL
still never refuse a chip. Content written for another engine SHALL be shown, not
run, with the sentence that says which engine it needs.

##### Scenario: Build mode on DataFusion
- GIVEN a DataFusion deployment
- WHEN somebody picks `model` and `count` in Build mode
- THEN the text in the editor is DataFusion Python, and it runs

##### Scenario: a Flow recipe opened on DataFusion
- GIVEN a DataFusion deployment and a recipe whose engine is `flow`
- WHEN it is opened
- THEN it is shown read-only, saying it was written for Flow and this deployment
  runs DataFusion

### MODIFIED Requirements

#### Requirement: The query engine's address
The server, the panel's proxy and the CLI SHALL read the engine's address from
`AIWATCHER_QUERY_URL`. `AIWATCHER_FLOW_URL` SHALL be accepted as an alias for one
release, and both set to different values MUST be refused naming both.
(Previously: `AIWATCHER_FLOW_URL` alone — `config.rs:875`, `vite.config.ts:31`,
`justfile:41`, `_helpers.tpl:402`, `aiwatcher-cli/src/commands/stack.rs:275`.)

##### Scenario: an existing deployment upgrades
- GIVEN only `AIWATCHER_FLOW_URL` set
- WHEN the new release starts
- THEN it runs Flow at that address, exactly as before

##### Scenario: two addresses for one engine
- GIVEN `AIWATCHER_QUERY_URL` and `AIWATCHER_FLOW_URL` set to different addresses
- WHEN the server starts
- THEN it refuses to, naming both variables

#### Requirement: The query limits are the engine's, not Flow's
The step timeout SHALL be `AIWATCHER_QUERY_STEP_TIMEOUT_SECONDS` and the service's
ceiling `AIWATCHER_QUERY_TIMEOUT_SECONDS`, for whichever engine runs; the service's
MUST stay above the step's. (Previously: `AIWATCHER_FLOW_STEP_TIMEOUT_SECONDS` and
`AIWATCHER_FLOW_TIMEOUT_SECONDS`, added on this branch and never released, so they
are renamed without an alias.)

##### Scenario: a long corpus query on any engine
- GIVEN `AIWATCHER_QUERY_STEP_TIMEOUT_SECONDS=7200`
- WHEN a managed step of any engine runs for twenty minutes
- THEN it completes, with one attempt

#### Requirement: The engines live side by side
The engines SHALL live under `services/query/` — `flow/`, moved from
`services/flow`, `datafusion/` and `duckdb/` — with the contract and its
conformance suite in `services/query/contract/`. `just query-serve`,
`just query-check` and the CI job SHALL serve and check the engine named by
`AIWATCHER_QUERY_ENGINE`, and the `flow-*` recipes SHALL keep working as aliases
for one release. (Previously: one engine, `services/flow`, with `flow-*` recipes
and a `flow` CI job.)

##### Scenario: the check an engine change has to pass
- GIVEN a change under `services/query/`
- WHEN CI runs
- THEN every engine's own checks and the conformance suite against each run

#### Requirement: Deployment values name the engine
The Helm chart SHALL take `query.engine: flow | datafusion | duckdb` and
`query.enabled`, with an image per engine; `flow.*` values SHALL map onto `query.*`
for one release. Docker Compose SHALL select the engine the same way. (Previously:
`flow.enabled` and one image, `values.yaml:250`.)

##### Scenario: switching an installed release
- GIVEN a release running `query.engine: flow`
- WHEN it is upgraded with `query.engine: datafusion`
- THEN the DataFusion engine replaces the Flow one, and the server's
  `AIWATCHER_QUERY_ENGINE` says `datafusion`

#### Requirement: A query surface is admitted, or runs where code runs
Flow queries SHALL stay parsed rather than executed (ADR_0008). A DataFusion or
DuckDB query in `strict` mode SHALL be admitted before it runs; in `open` mode it
runs as a notebook does — in a child process, without credentials, on a service
bound to localhost. ADR_0028 SHALL record this and amend ADR_0008, ADR_0014 and
ADR_0024. (Previously: ADR_0008 — the query surface is Flow, and no query text is
executed.)

##### Scenario: the decision is where the next reader looks
- GIVEN this change merged
- WHEN somebody reads ADR_0008
- THEN its amendment points at ADR_0028, which states what `open` costs and what
  would make it wrong

## Log
- 2026-09-10 13:31 — spec drafted on `feat/agent-sdk-merge`
- 2026-09-10 14:56 — DataFusion added as a third engine (`datafusion`, SQL admitted by its own SQLOptions) after engines_5gb.py measured it at Polars' speed in a twentieth of the memory
- 2026-09-10 15:05 — DataFusion measured in a Rust process (rust-datafusion/): 1.8 s over the 5 GB CSV at 93 MiB, 0.148 s over Parquet at 98 MiB — an in-process execution/datafusion.rs in the work role is a real option, to weigh in the job phase
- 2026-09-10 15:14 — decided by the user: DataFusion through its Python DataFrame API, not SQL — no engine takes SQL; admission generalised to AIWATCHER_QUERY_ADMISSION for both Python engines
- 2026-09-10 15:20 — decided by the user: DuckDB added as a fourth engine — three beside Flow (polars, datafusion, duckdb), each through its own Python API; DuckDB's relational API with no SQL string (engines_5gb.py: 2.02 s over the 5 GB CSV at 677 MiB, 0.215 s over Parquet at 227 MiB)
- 2026-09-10 15:26 — decided by the user: Polars dropped as an engine for its peak (5.4–5.5 GiB over the 5 GB CSV, against DataFusion's 280 MiB); three options — flow, datafusion, and duckdb last as the addition; spec retitled
