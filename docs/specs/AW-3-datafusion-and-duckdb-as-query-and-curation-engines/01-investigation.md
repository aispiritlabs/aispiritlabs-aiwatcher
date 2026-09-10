---
id: AW-3
step: investigation
status: doing
branch: feat/agent-sdk-merge
repo: aiwatcher
created: 2026-09-10
updated: 2026-09-10
tags: [spec/AW-3, step/investigation, branch/feat-agent-sdk-merge, status/doing]
---

`#spec/AW-3` · `#step/investigation` · `#branch/feat-agent-sdk-merge` · repo `aiwatcher`

# ① Investigation — AW-3

## Problem statement

`benchmarks/curation` ran the same four curation queries in Flow PHP and in
Polars over one synthetic corpus, and checked every answer against the other
engine's before reporting a time ([README](../../../benchmarks/curation/README.md)).
Over 10 GB of CSV Polars was **250–350× faster**, and still **50–70× faster on
one thread**; the answers were identical. The cost is Flow's execution model —
one PHP process evaluating an expression tree over a row of objects at a time —
not memory (a bigger batch was slower) and not the query text (the best rewrite
bought 25–35%). Inside aiwatcher the same comparison ran as a managed pipeline:
the Flow step took 95.2 s over 1 GB, the Polars notebook step 1.17 s.

The ask: Polars as a tool for **query** (the Query tab), **integration** (hub
corpora) and **data curation** (recipes and pipeline blocks), beside Flow.

Why now: curation is already run over hub corpora (PII detection, Titanic), the
managed path of ADR_0025 exists to take work the browser cannot hold, and the
measurement is fresh — every number below was produced this week on this tree.

## Current state

### The query surface is one engine, by construction

- `docs/ADR/ADR_0008_FLOW_QUERY_SURFACE.md:41-59` — the Decision: a separate PHP
  service the panel calls directly, over named API datasets, **parsed rather than
  executed**. Three amendments widened what a query may say; none added an engine.
- `apps/panel/src/lib/flow.ts:114-151` — the one client; `:153-210` its calls to
  `/flow/healthz`, `/datasets`, `/check`, `/query`, `/simulate`. The same client
  serves the recipe, datasets, hub-discovery, block-inspector and pipeline views.
- `services/flow/public/index.php:13-19` — the routes; `services/flow/src/QueryRunner.php:29`
  caps an answer at `MAX_ROWS = 1_000`, `:32` a simulation at 25.
- `apps/panel/src/lib/query-builder.ts:278-330` — Build mode compiles clicked
  attributes to Flow text, one way (`:15-21`); aggregations are
  `count|sum|average|max|median` (`:140`).

### Curation runs Flow, and one step of it is the seam

- `docs/ADR/ADR_0014_DATA_CURATION_RECIPES.md:44-47` — a recipe is executed by the
  **browser**: it runs Flow and publishes the rows (`apps/panel/src/routes/data-curation.recipe.tsx:133-151`).
  The server never runs a recipe.
- `crates/aiwatcher-datasets/src/pipeline.rs:51-123` — `BlockSpec` is `source |
  transform | notebook | approval | view`; `:484-501` refuses a transform after a
  notebook or an approval, because a Flow step reads the catalog, not an artifact.
- `crates/aiwatcher-execution/src/compile.rs:97-146` — the source and its leading
  transforms fold into one `RuntimeBinding::FlowPhp`; `plan.rs:92-112` lists the
  bindings: `FlowPhp`, `Marimo`, `PublishDataset`, `PythonTask`, `HumanInput`,
  `ExternalWorkflow`.
- `crates/aiwatcher-server/src/execution/flow.rs:305-315` — `request_of` is
  documented as the seam: *"what a second engine over the same structured source
  would have a sibling of"*.

### Every boundary is sized for a thousand rows of JSON

This is the finding that shapes the plan. Polars' advantage appears at sizes
nothing here can store or hand on:

| boundary | limit | where |
|---|---|---|
| a Flow answer | 1 000 rows, and a managed step refuses a truncated one | `QueryRunner.php:29`; `execution/flow.rs:185-195` |
| a step's rows | one JSON array, `application/json` | `execution/artifacts.rs:96-113` |
| a worker's upload | `Json<RowsBody>`, axum's default body limit | `aiwatcher-api/src/worker.rs:309-314` |
| a dataset version | one JSON object, `MAX_ITEMS = 1_000`, 4 MiB | `aiwatcher-datasets/src/lib.rs:32-33`, `:161-173` |
| reading a version | the whole object loaded per page of ≤ 100 | `aiwatcher-datasets/src/lib.rs:34`, `:400-479` |
| a hub read | `hub_rows` limit 1–1 000 | `services/flow/src/Dataset/Catalog.php:477-485` |
| an annotation import | 5 000 rows a request | `aiwatcher-annotations/src/images/import.rs:45` |
| a staged corpus | JSONL pages; *"Parquet is not here"* | `docs/ADR/ADR_0022_STAGED_IMPORT_JOBS.md:153-156` |

Nothing in Rust or PHP writes Parquet or Arrow, and no route streams a dataset in
bulk — SSE carries live frames only. A Polars engine dropped behind today's
boundaries would run today's thousand-row workloads, where ADR_0008's own
measurement says the grain decides and not the engine (210 ms for `groupBy(agent)`
over 1 500 runs through the API).

### Polars already in the tree

`services/ml_pipeline/pyproject.toml` depends on `polars>=1.44,<1.45` since the
benchmark; `services/ml_pipeline/notebooks/flow_vs_polars.py` runs it as a
curation block; `services/flow/src/Dataset/Corpus.php` serves the corpus both
read. `integrations::fetch` is the one outbound downloader and accepts pictures
only (`aiwatcher-annotations/src/integrations/fetch.rs:252`, `:272`).

## Constraints

- **A query is admitted, never executed.** Measured, not assumed: Polars 1.44.2's
  `SQLContext`, with nothing registered, reads any file a query names —
  `SELECT count(*) FROM read_csv('…/models.csv')` answered 8, and
  `read_parquet('…/part-00000.parquet')` 455 903. Polars also scans `http(s)://`,
  `s3://` and `hf://` paths. So SQL text cannot reach Polars as written: the
  guardrails *never let a query name something that opens a source or writes a
  sink* and *never fetch a byte outside `integrations::fetch`* both apply, and an
  engine here reads only the configured API, configured corpus roots and the
  object store.
- **One transform, one representation** (guardrail, decision 15). A Flow transform
  stays Flow text; a Polars block is a *different block*, which is ADR_0024's rule
  — each block belongs to the engine that can run it — rather than a second
  representation of the same one.
- **Rows travel by reference.** *Never put rows in a workflow message*; at size that
  needs a columnar artifact, and *never accept an artifact reference a worker
  described rather than wrote* — whatever stores it digests it.
- **Caching stays honest.** A read of files is addressed by nothing (the corpus
  dataset already reports `deterministic: false`); *never cache a step whose inputs
  are not all addressed*.
- **The API process keeps its memory contract** (`AIWATCHER_MAX_SPANS_TOTAL`,
  512 MB). The benchmark's q3 peaked at 3.2 GiB in Polars; heavy queries run
  elsewhere.
- **Unauthenticated services bind to localhost**, as Flow and the notebook runtime
  do.
- **The contract regenerates** (`just openapi`) with any `BlockSpec` or
  `RuntimeBinding` change, and a new service gets its own CI job beside `flow` and
  `notebooks` (`.github/workflows/ci.yml:163-212`).
- **Nothing is removed.** Flow stays the default engine; every saved recipe and
  pipeline keeps running unchanged.

## Options

Three questions, mostly independent: what a person writes for Polars, where
Polars runs, and where rows at size live.

### Language

#### Option A — Polars SQL, admitted before it runs
- Pros: Polars' own language, known to most people who query data, and no Python
  crosses the boundary. Admission is ADR_0008's shape — parse, check against the
  catalog, dispatch — and the builder can emit SQL beside Flow text.
- Cons: a second language in the panel. Polars exposes no SQL AST to Python, so
  admission needs a parser of its own (sqlglot, or equivalent) plus a function
  policy, and a parser that disagrees with Polars' is a boundary with a gap. The
  semantics are Polars', not Flow's — the benchmark had to tolerate Flow's
  two-decimal `average()` against Polars' unrounded mean — so a query ported
  between engines can change its answer at the edges.

#### Option B — Flow's DSL, executed by Polars
- Pros: one language; the existing `Dsl\Registry` stays the admission; the builder,
  saved recipes and pipelines are unchanged and could move engine without a rewrite.
- Cons: a translator from Flow's plan to Polars, covering what Flow can say (239
  functions), with a per-query fallback where it cannot — and the parser is PHP, so
  an intermediate form has to cross a process. Trusting it means reproducing Flow's
  semantics (BigDecimal arithmetic, rounded averages, `same` versus `equals`) or
  diverging from them silently, which is the failure the benchmark's agreement
  check exists to catch. It is the enumeration the guardrail on transforms warns
  of, one layer down.

#### Option C — Python expressions or code — rejected
Query text that is evaluated is ADR_0008's first refusal, and code already has a
home: the notebook block.

#### Option D — serialized `LazyFrame` plans — rejected
A plan can carry pickled Python functions; a plan from the browser is code.

### Placement

#### Option 1 — a service of its own, speaking Flow's contract
`/healthz`, `/datasets`, `/check`, `/query`, `/simulate`, `/executions/{id}`, the
same answer fields (`deterministic`, `window_applied`, `digest`, `took_ms`).
- Pros: the panel treats engines alike (`flow.ts` becomes an engine client), the
  Rust executor is `request_of`'s sibling, and the service fails alone, as Flow does.
- Cons: a fourth development process, a Dockerfile, a Helm entry and a CI job.

#### Option 2 — inside `services/ml_pipeline`
- Pros: the uv project, the Polars dependency, the CI job and the deployment exist.
- Cons: one process holding unsandboxed notebook code *and* a surface that takes
  text from anybody who can reach the panel — two trust postures in one place — and
  a notebook's CPU starving the Query tab.

#### Option 3 — in the Rust binary, behind a cargo feature
The `laser` / `duckdb` shape.
- Pros: no service; reads the object store and writes Parquet natively.
- Cons: the `polars` crate's compile time and binary size in every build that asks
  for it; Rust's Polars SQL needs the same admission; a heavy query inside the
  process threatens the read model's memory contract unless only the `work` role
  runs it; and the notebook half keeps a Python Polars anyway.

### Rows at size (every option needs one)

#### Option S1 — Parquet artifacts and sharded versions; Rust stores, engines decode
A step's rows and a published version become Parquet shards under a manifest —
ADR_0022's primitive (shard before cursor, `version_of`). Rust stores bytes and
their digests and never decodes them; `dataset-rows` asks an engine for a page.
- Pros: no Parquet dependency in Rust; the 1 000-row walls lift where they matter;
  the notebook half already reads Parquet.
- Cons: amends ADR_0022 (*"Parquet is not here"*); a streamed, digested upload path
  beside the JSON one; paging a version needs an engine to be up.

#### Option S2 — keep JSON, raise the caps
- Pros: cheap.
- Cons: a JSON array of millions of rows is the wall itself — held whole in memory
  and re-read per page.

## Recommendation

**A + 1 + S1**, in five phases, each ending at something that runs.

- **A — the engine.** A Polars service with Flow's contract. Admission: `SELECT` and
  `WITH` only, tables from the catalog only, no table functions, no paths or URLs.
  The catalog is Flow's — the API datasets, paged — plus `corpus_spans` and
  published versions. The Query tab gains an engine switch, and the builder a SQL
  emitter under its rule that it never refuses. *Accepted when* the benchmark's
  q1–q4 written as SQL in the Query tab agree with Flow on the 100 MB corpus.
- **B — columnar artifacts.** A Parquet rows artifact, streamed with its digest past
  the JSON body limit, stored opaquely by Rust. *Accepted when* a managed Polars
  step hands on q4's 24 M rows at 10 GB.
- **C — curation.** A `sql` block belonging to Polars, reading a source or the
  previous block's rows as `input` — so, unlike a Flow transform, it may follow a
  notebook — compiled to a new binding with its own executor; recipes carry an
  engine. *Accepted when* `curation/flow-vs-polars` gains a SQL block and runs over
  10 GB on the managed path.
- **D — versions at size.** A version becomes a manifest and Parquet shards; the
  JSON versions under 1 000 rows keep working. *Accepted when* a 10 GB curation
  publishes and pages in the Datasets view.
- **E — integrations.** Hub corpora staged as Parquet; a non-image profile in
  `integrations::fetch` — allowlist, ceiling, digest — for the files a hub serves.
  *Accepted when* a Hugging Face Parquet corpus is imported, queried in SQL and
  curated without Flow in the path.

Why A over B: B's one real advantage is a single language, and its price is an
interpreter that has to reproduce Flow's semantics to be trusted — the rounding,
the nulls and the BigDecimal arithmetic are exactly where it would drift, quietly.
A states its semantics openly, and the builder can emit either language.

The decisions graduate to **ADR_0028** — *Polars is the second query engine,
admitted as SQL* — amending ADR_0008 (one surface becomes two), ADR_0014 (a recipe
names its engine), ADR_0022 (Parquet) and ADR_0024 (the `sql` block).

## Open questions

- [ ] **The language** — Polars SQL (recommended) or Flow's DSL executed by Polars.
      The one question that changes every phase after A.
- [ ] **The service's name** — `services/polars` mirrors `services/flow`, each named
      for the engine it adapts; the crate rule names things for the capability
      (`services/query`).
- [ ] **The admission parser** — sqlglot, or a hand-written statement and function
      policy over Polars' own function list; and which families (string, date,
      window) phase A admits.
- [ ] **Reading the read model** — does Polars page the API as Flow does, or does the
      API grow a bulk NDJSON or Arrow read of runs and spans? ADR_0008's grain
      measurement says paging is enough for datasets that size.
- [ ] **Parquet in Rust** — none (opaque bytes, engines decode) or the arrow and
      parquet crates, for server-side paging without an engine up.

## Log
- 2026-09-10 13:22 — investigation written on `feat/agent-sdk-merge`; the 1 000-row JSON boundaries mapped, Polars SQL measured reading files unadmitted
- 2026-09-10 13:31 — decided by the user: Polars as ordinary Python rather than SQL, one engine chosen per deployment, services generalised under services/query — the SQL recommendation is not taken
- 2026-09-10 15:26 — decided by the user: Polars not taken as an engine for its peak (5.4–5.5 GiB over the 5 GB CSV); engines are flow, datafusion, and duckdb last as the addition
