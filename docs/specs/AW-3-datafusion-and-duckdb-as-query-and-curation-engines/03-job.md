---
id: AW-3
step: job
status: doing
branch: feat/agent-sdk-merge
repo: aiwatcher
created: 2026-09-10
updated: 2026-09-10
tags: [spec/AW-3, step/job, branch/feat-agent-sdk-merge, status/doing]
---

`#spec/AW-3` · `#step/job` · `#branch/feat-agent-sdk-merge` · repo `aiwatcher`

# ③ Job — AW-3

## Technical approach

Generalise first, then add. Phase 1 turns the Flow service's contract, its Rust
executor, its configuration, its panel client and its chart values into the
*query engine's*, with Flow as the only implementation and no behaviour change —
the whole existing suite is the proof. Phase 2 builds what the two Python engines
share: one `uv` workspace under `services/query/`, a `contract` package that
serves the six routes, reads the catalog (now one declared file Flow also loads),
runs a query either admitted (`strict`) or in a forked child with ceilings
(`open`), and a conformance suite that asks every engine the same four questions.
Phase 3 is DataFusion end to end — engine package, `RuntimeBinding::DataFusion`,
`execution/datafusion.rs`, the compiler's Python script, the panel's emitter and
read-only foreign content, seed variants, image, measurement. Phase 4 is DuckDB
the same way, last, as the addition. Phase 5 graduates the decision to ADR_0028.
Rows between steps stay a ≤ 1 000-row JSON artifact, as the spec keeps them.

## Decisions

- **Decision:** no engine runs inside the aiwatcher process, DataFusion included —
  *because* the language is Python: a DataFusion query is Python text calling
  DataFusion's Python API, and running it needs an interpreter. DataFusion in a
  Rust process measured 93 MiB over the 5 GB CSV, but it could only run a query
  somebody wrote in Rust or SQL, and neither is this spec's language.
  Alternative rejected: `execution/datafusion.rs` embedding the `datafusion` crate
  in the work role — a second language and a heavy query beside the read model's
  512 MB contract.
- **Decision:** one `RuntimeKind` per engine — `flow_php` (unchanged), `datafusion`,
  `duckdb` — rather than one `query` kind carrying the engine as a parameter —
  *because* a reactor routes on the kind without loading the plan (`plan.rs`), so a
  DataFusion process must be able to not claim DuckDB work by its claim filter
  alone. Alternative rejected: `RuntimeBinding::Query { engine, … }`, which would
  need the plan loaded to decide a claim.
- **Decision:** `RuntimeBinding::FlowPhp` and the string `flow_php` stay exactly as
  they are; the spec struct is renamed `QueryStepSpec` with `FlowStepSpec` kept as a
  type alias — *because* `plan_id` digests the binding's serialised fields and
  every stored stream names `flow_php`; a Flow pipeline must compile to the plan it
  compiled to before (spec scenario *a pipeline saved before the field*). Serde does
  not see the struct's name, so only the OpenAPI schema name moves (`just openapi`).
  Alternative rejected: renaming the variant, which changes every Flow `plan_id` and
  strands every cache entry.
- **Decision:** `engine` is on the **transform** and the **recipe**, never on the
  source — *because* a source is a catalog read every engine serves identically,
  while a transform's text belongs to one language. A chain's engine is its
  transforms'; transforms naming two engines are refused by `order_of` in
  `aiwatcher-datasets` (a chain rule, reported with every other problem); a chain
  whose engine is not the deployment's is refused by the compiler (only it knows the
  deployment). A chain with a source and no transform compiles to the deployment's
  engine. The field is `#[serde(default, skip_serializing_if = "QueryEngine::is_flow")]`,
  so every stored revision keeps its digest. Alternative rejected: an engine on the
  pipeline as a whole, which a canvas mixing a Flow example and a DataFusion edit
  would contradict silently.
- **Decision:** the compiler's Python script is `df = read("<dataset>", <arg>=<json
  string>, …)`, then `df = (<transform>)` per transform, then `df` — *because* the
  spec binds `df` to the rows so far, the parentheses let a transform start a line
  with `.aggregate(…)` as one writes a method chain, and a JSON string literal is a
  valid Python literal, so no second escaper is written. An argument name that is
  not a Python identifier is a compile problem. The browser gets the same generator
  (`compilePython` beside `compileFlow` in `lib/pipeline.ts`), ported line for line,
  for the ad-hoc preview — the rule the Flow one already lives under.
- **Decision:** the Flow executor keeps posting to `/flow/*` for the alias release;
  the Python engines serve `/query/*` only, and Flow serves both — *because* a new
  server against an old Flow image would otherwise POST `/query/query`, get a 404,
  and `send` would pass a 404 through to `decode`, which reads `{"error":…}` as an
  answer with no rows: a completed step over nothing. The shared client also stops
  accepting a 404 anywhere but the lookup, which closes the same hole for a port
  collision today (`flow.ts` already treats 404 as *somebody else answered*).
- **Decision:** `execution/query.rs` holds the client every engine shares — send,
  refusal, `QueryAnswer`, `worth_remembering`, the receipt check, the lookup — and
  `flow.rs`, `datafusion.rs`, `duckdb.rs` are each the kind, the binding and the
  request, side by side as the spec lays out. `query::executors(config, artifacts)`
  registers exactly the one `AIWATCHER_QUERY_ENGINE` names. Alternative rejected:
  one generic executor constructed three ways, which hides the three-file layout
  the spec asked for behind a constructor argument.
- **Decision:** the declarative half of the catalog — names, routes, rows path,
  cursor, grain, description, columns, hints, parameters, `windowed`,
  `requires_run` — moves from `Catalog.php` into `services/query/contract/catalog.json`,
  which Flow loads and the Python engines load — *because* the spec says the Python
  engines' catalog **is** Flow's, and a second copy in Python is the guardrail's
  "second list of names" in a second language. The code that reads a dataset stays
  per engine. The conformance suite asserts `/query/datasets` is identical across
  engines.
- **Decision:** the Python engines are one `uv` workspace at `services/query/` with
  three members — `contract` (package `aiwatcher_query`), `datafusion`
  (`query_datafusion`), `duckdb` (`query_duckdb`) — *because* one lock and one
  toolchain serve both, while each image installs only its own engine's wheel.
  Alternative rejected: one package depending on both engines, which puts DuckDB's
  wheel in the DataFusion image.
- **Decision:** `open` mode runs each query in a child of a **forkserver** that
  imported the engine and nothing else — *because* a fresh interpreter per query
  pays the import every time (the `tmp_file.py` row of the benchmark), and a plain
  `fork` of a process that has run a query forks a live Tokio runtime. The child
  scrubs `AIWATCHER_*` from its environment, `chdir`s into a fresh scratch
  directory, sets `RLIMIT_CPU` from `AIWATCHER_QUERY_TIMEOUT_SECONDS` and, on Linux,
  `RLIMIT_AS`; the parent also kills on wall-clock. On Linux the service marks itself
  non-dumpable (`PR_SET_DUMPABLE=0`) so a child cannot read `/proc/<parent>/environ`,
  which keeps the process's *initial* environment whatever is deleted later.
  Measured before building on it: importing `datafusion` starts two native threads
  and `duckdb` nine, and a child forked after the import alone answers a query in
  both (3 of 3); a child forked after the *parent* ran a DataFusion query panics in
  Tokio's I/O driver. So the fork server imports the engine and never runs one,
  and `strict` queries run in the same child as `open` ones rather than in the
  service's own process. Preloading is worth it: the import is 464 ms for
  DataFusion and 47 ms for DuckDB, paid once rather than per query.
- **Decision:** a ceiling that stops a query is a **422** naming the ceiling —
  *because* the same query hits the same ceiling on every retry, so a managed step
  should fail once with the reason, not read a 5xx as an outage and spend ten
  attempts. (Flow's `set_time_limit` answers 500 today; unchanged here.)
- **Decision:** on a laptop `open` mode can reach the network, as a notebook can; in
  a cluster it cannot — with `networkPolicy.enabled` the query pod's egress is the
  server's Service and DNS, nothing else. **This is a decision against the guardrail
  *never fetch a byte outside `integrations::fetch`*, raised rather than made
  quietly:** `open` is the notebook runtime's posture, which already stands outside
  that guardrail, and ADR_0028 must name it as a cost. `strict` keeps it: no import,
  no URL, no file.
- **Decision:** `strict` admission is derived, not enumerated — ADR_0008's shape.
  The text is parsed with `ast`; admitted are names in the query's namespace (plus
  names it assigned), attributes and calls whose name is a public member of the
  engine's own classes and modules, minus a `DECLINED` table that carries a reason
  per name (anything that reads or writes a file or URL, registers, runs SQL, or
  takes a callable). Refused outright: `Import`, `Lambda`, `def`/`class`, any name
  or attribute starting with `_`, and — in DuckDB — any `str` argument to
  `filter`/`project`/`select`/`order`/`sort`/`aggregate` except a bare identifier as
  `aggregate`'s group key. DuckDB's `FunctionExpression` name is admitted against
  `duckdb_functions()` (scalar and aggregate), minus `DECLINED` (`getenv`,
  `current_setting`, …). Alternative rejected: a hand-written allowlist, which is
  the enumeration the Flow guardrails deleted.
- **Decision:** `deterministic` is false for a corpus or hub read (Flow's rule) and
  for a query naming a volatile function — DuckDB's from `duckdb_functions().stability`;
  DataFusion's from a declared `VOLATILE` set (`now`, `random`, `uuid`,
  `current_date`, `current_time`), the one list here, kept because the Python binding
  exposes no volatility. In `open` mode the scan is the author's claim, as a
  notebook's `deterministic` is.
- **Decision:** a DuckDB integer outside 64 bits is refused with a 422 naming the
  column — *because* the reactor decodes rows with `serde_json`, which reads a
  128-bit number as a float and rounds it. The spec asks for "reported rather than
  rounded"; refusing is the honest form of that.
- **Decision:** `hub_rows`' `row` is an Arrow struct whose fields and types are the
  hub's declared columns, read from the `hub_columns` route — *because* types
  inferred from one page of a mixed corpus (Titanic's `Age` is null and float) are
  a guess, and `col("row")["Age"]` is how both engines address a field.
- **Decision:** the conformance questions are the benchmark's q1–q4 in **bounded
  form** — q3 and q4 end in a total sort and a limit of 100 — *because* q3 answers
  one row per run and q4 one row per span, which is millions, and the spec keeps
  every engine inside the 1 000-row answer. **This narrows the spec's scenario *the
  same question, the same rows*; recorded here so `/spec-tests` finds it.** The
  expected rows are Flow's, generated once on a fixed-seed corpus and checked in;
  means are compared within Flow's two-decimal rounding.
- **Decision:** a dataset version gains `engine`, omitted when `flow` — *because*
  a version's `pipeline` field is the text that produced it, and a DataFusion
  recipe's text read later without its engine is ambiguous. It joins the version's
  identity only when it is not `flow`, so every existing version keeps its id.
  Additive; not in the spec's scenarios, in service of *Content names the engine it
  was written for*.
- **Decision:** the chart renames its resources from `-flow` to `-query` — *because*
  the Deployment's selector carries `app.kubernetes.io/component` and is immutable,
  so a new name is a new Deployment Helm can replace cleanly. Cost: the Query tab
  answers 503 for the length of one rollout on the upgrade that crosses it.
- **Decision:** Docker Compose gains a `query` service — it has none today — whose
  image is picked by `AIWATCHER_QUERY_ENGINE`, and the `aiwatcher` service reads the
  same variable: one value, as the spec asks.
- **Decision:** one `deploy/Dockerfile.query` with a target per engine — `flow`,
  `datafusion`, `duckdb` — replacing `Dockerfile.flow` — *because* Compose can pick
  a target from `AIWATCHER_QUERY_ENGINE` but cannot pick between two Dockerfiles
  from one variable, and the spec asks that the engine be chosen *the same way*
  everywhere. Alternative rejected: a separate `Dockerfile.query-python` with an
  `ENGINE` argument, which needed a second variable to choose the file.
- **Decision:** the panel's nginx rewrites `/query/` and `/flow/` to the prefix the
  engine serves — `/flow/` for Flow, `/query/` for a Python engine — and the Flow
  pod is probed on `/flow/healthz` — *because* during the alias release a new panel
  may meet an old Flow image, which serves only `/flow/*`.
- **Decision:** `execution/duckdb.rs` is a module named like the `duckdb` crate the
  server already depends on behind its feature; any use of the crate in the server
  is written `::duckdb`.

## Changes

- **Files:**
  - `crates/aiwatcher-datasets/src/{lib.rs,pipeline.rs}` (modified) — `QueryEngine`;
    `engine` on `Transform`, on recipes and on dataset versions; the one-engine
    chain rule; messages that say "query block" where they are not Flow's.
  - `crates/aiwatcher-execution/src/{plan.rs,compile.rs,cache.rs,context.rs,facts.rs}`
    (modified) — `QueryStepSpec`, `RuntimeBinding::{DataFusion,DuckDb}`,
    `RuntimeKind::{DataFusion,DuckDb}`, `CompileOptions::{engine,query_timeout_seconds}`,
    `python_script`, the deployment refusal; every `FlowPhp` arm gains its siblings.
  - `crates/aiwatcher-execution/src/store/postgres/mod.rs` (modified) — `runtime_from`
    names the two new kinds.
  - `crates/aiwatcher-server/src/config.rs` (modified) — `AIWATCHER_QUERY_ENGINE`,
    `AIWATCHER_QUERY_URL` with the `AIWATCHER_FLOW_URL` alias and the conflict
    refusal, `AIWATCHER_QUERY_STEP_TIMEOUT_SECONDS`.
  - `crates/aiwatcher-server/src/execution/{query.rs (new),flow.rs,datafusion.rs (new),duckdb.rs (new),mod.rs,publish.rs}`.
  - `crates/aiwatcher-api/src/{state.rs,executions.rs,context.rs}` (modified) —
    `query_engine` and the renamed timeout reach both compiles.
  - `crates/aiwatcher-cli/src/commands/{stack.rs,serve.rs}` (modified) — the moved
    path and the engine.
  - `services/flow` → `services/query/flow` (moved) — `/query/*` beside `/flow/*`,
    `engine`/`language` on healthz and datasets, catalog from `catalog.json`,
    `AIWATCHER_QUERY_TIMEOUT_SECONDS`.
  - `services/query/{pyproject.toml,uv.lock,README.md}` (new), `services/query/contract/`
    (new: `aiwatcher_query`, `catalog.json`, `conformance/`), `services/query/datafusion/`
    (new), `services/query/duckdb/` (new).
  - `apps/panel/src/lib/flow.ts` → `apps/panel/src/lib/query.ts`;
    `lib/query-builder.ts` (an emitter per language); `lib/pipeline.ts`
    (`compilePython`); `routes/data-curation.recipe.tsx`,
    `components/block-inspector.tsx`, `routes/observability.query.tsx` and the other
    five call sites; `vite.config.ts` (`/query`, `/flow` kept).
  - `deploy/helm/aiwatcher/{values.yaml,values.schema.json,templates/query.yaml (was flow.yaml),templates/panel.yaml,templates/networkpolicy.yaml,templates/_helpers.tpl}`.
  - `deploy/Dockerfile.flow` → `deploy/Dockerfile.query` (a target per engine),
    `deploy/docker-compose.yml`, `.github/workflows/{ci.yml,release-images.yml}`.
  - `justfile` — `query-*` recipes, `flow-*` aliases, `bench-curation-serve` by engine.
  - `examples/*/pipeline.{datafusion,duckdb}.json` (new), `examples/seed.json`
    (regenerated), `benchmarks/curation/{in_aiwatcher.py,README.md,corpus.py}`.
  - `docs/ADR/ADR_0028_QUERY_ENGINES.md` (new); amendments to ADR_0008, ADR_0014,
    ADR_0024; `CLAUDE.md`; `docs/INSTALL.md`.
- **Contract:** `BlockSpec::Transform.engine`, `SaveRecipeRequest.engine`,
  `PublishDatasetRequest.engine`, `RuntimeBinding`, `RuntimeKind` and the
  `QueryStepSpec` schema name change the OpenAPI document — `just openapi`, then
  commit `contracts/openapi.json` and `apps/panel/src/api/generated`. The query
  engines' `/query/*` contract stays outside it (ADR_0008's reason) and is proved by
  the conformance suite instead.
- **Data / migrations:** none in PostgreSQL — `runtime` is text, and a row naming
  `datafusion` read by an older binary falls to `external_workflow`, which it never
  claims (the rule `runtime_from` states). Every new field is serde-defaulted and
  omitted when `flow`, so no stored revision, plan or version changes its digest.
  The removals — `AIWATCHER_FLOW_URL`, the `flow-*` recipes, the `/flow` proxy
  paths and the `flow.*` values — are a release later.

## Tasks

### 1 · Generalise, with Flow as the only engine and nothing changing
- [x] 1.1 `QueryEngine` (`flow | datafusion | duckdb`, `FromStr` naming the three) in `aiwatcher-datasets`; `engine` on `Transform` and recipes, skipped when `flow`; a test that a stored pipeline and recipe keep their revision — implements *Requirement: Content names the engine it was written for*
- [x] 1.2 The chain rule: transforms naming two engines are one problem in `order_of`'s list — implements *Requirement: Content names the engine it was written for*
- [x] 1.3 Config: `AIWATCHER_QUERY_ENGINE` (refusal naming the variable and `flow, datafusion, duckdb`), `AIWATCHER_QUERY_URL` with the `AIWATCHER_FLOW_URL` alias and the refusal naming both, `AIWATCHER_QUERY_STEP_TIMEOUT_SECONDS` with no alias; `AppState::query_engine`; `CompileOptions::engine` — implements *Requirement: A deployment chooses one query engine*, *The query engine's address*, *The query limits are the engine's, not Flow's*
- [x] 1.4 `execution/query.rs` out of `flow.rs`; a 404 accepted by the lookup only; `query::executors` registering the named engine and no other — implements *Requirement: A deployment chooses one query engine*
- [x] 1.5 `git mv services/flow services/query/flow`; every path updated (justfile, CI, `Dockerfile.flow`, `stack.rs`, `corpus.py`, docs, `CLAUDE.md`, `AGENTS.md`) until `rg services/flow` finds only the aliases; Flow serves `/query/*` beside `/flow/*` and reports `engine: flow`, `language: flow-dsl`; `AIWATCHER_QUERY_TIMEOUT_SECONDS` — implements *Requirement: The engines live side by side*, *One query contract, proved against every engine*
- [x] 1.6 `services/query/contract/catalog.json`, loaded by `Catalog.php`; Flow's tests unchanged — implements *Requirement: The Python engines' catalog is Flow's catalog*
- [x] 1.7 Panel: `lib/query.ts` over `/query`, `useQueryEngine()` from healthz, the vite proxy on `AIWATCHER_QUERY_URL` with `/flow` kept — implements *Requirement: The panel follows the deployed engine*, *The query engine's address*
- [x] 1.8 Helm `query.enabled`, `query.engine`, an image per engine, `flow.*` mapped for one release, `-query` resources, nginx `/query` and `/flow`, NetworkPolicy ingress and the egress rule, server env; Compose `query` service — implements *Requirement: Deployment values name the engine*
- [x] 1.9 Gate: `just check`, `just query-check` (Flow), `npm run build`, `just openapi` clean, `helm template` over the old `flow.enabled` values unchanged in effect — the success criterion *with no new setting, everything behaves as before*

### 2 · The contract the Python engines share
- [x] 2.1 The `uv` workspace and `aiwatcher_query`: the six routes on Starlette bound to `127.0.0.1`, the answer (1 000 cap with `truncated`, `deterministic`, `window_applied`, `digest`, `took_ms`), execution memory, the API pager into Arrow (status before body, declared parameters only, `window_seconds`, `as_of`), `hub_rows` as a struct, `corpus_spans` from `AIWATCHER_CORPUS_DIR`, and the `Engine` protocol each engine implements — implements *Requirement: One query contract, proved against every engine*, *The Python engines' catalog is Flow's catalog*
- [x] 2.2 `open` mode: the forkserver, the scrubbed child, the ceilings answered as a 422 naming them, the non-dumpable parent; the fork-after-import check — implements *Requirement: Running typed Python has a stated boundary*
- [x] 2.3 `strict` mode's walker and `/query/check` with the parser's line and column — implements *Requirement: Running typed Python has a stated boundary*
- [x] 2.4 Conformance: a fixed-seed corpus from `benchmarks/curation/generate.py` small enough for CI, q1–q4 bounded, one text per engine, Flow's rows checked in, the catalog compared — implements *Requirement: One query contract, proved against every engine*
- [x] 2.5 `just query-serve`, `just query-check` and `just query-conformance` by `AIWATCHER_QUERY_ENGINE`; the CI job as a matrix over the three — implements *Requirement: The engines live side by side*

### 3 · DataFusion
- [x] 3.1 `query_datafusion`: `col`, `lit`, `f`, `read`, `df`; `SessionContext` held by the engine; `limit(max + 1)` then Arrow; a result that is not a `DataFrame` refused naming what it was; healthz `datafusion-python` — implements *Requirement: A DataFusion query is ordinary DataFusion Python*
- [x] 3.2 DataFusion's vocabulary and `DECLINED`; tests for `udf`, `SessionContext().sql`, `register_csv` and the four questions admitted — implements *Requirement: Running typed Python has a stated boundary*
- [x] 3.3 Rust: `RuntimeBinding::DataFusion`, `RuntimeKind::DataFusion`, `runtime_from`, the cache, context, facts and publish arms, `execution/datafusion.rs`, `python_script`, and the 422 naming block, engine and deployment with no run created — implements *Requirement: A managed step on the deployed engine*
- [x] 3.4 Panel: the DataFusion emitter in Build mode (still refusing no chip), `compilePython`, foreign content read-only with its sentence in the recipe editor and the block inspector, recipe examples for the engine — implements *Requirement: The panel follows the deployed engine*
- [x] 3.5 Seed variants for every example whose transforms DataFusion can express, `build_seed.py --check`; `in_aiwatcher.py` picks the variant from healthz; `bench-curation-serve` serves the engine it is given — implements *Requirement: Content names the engine it was written for*
- [x] 3.6 The `datafusion` target in `Dockerfile.query`, the `aiwatcher-query-datafusion` image in release-images, the chart's image — implements *Requirement: Deployment values name the engine*
- [x] 3.7 Measure q2 through the service over the 5 GB corpus, CSV and Parquet, `open` and `strict`, peak RSS of the service and its child; a few seconds each; into the benchmark README — implements *Requirement: A DataFusion query is ordinary DataFusion Python* (the memory scenario)
- [x] 3.8 End to end: `AIWATCHER_QUERY_ENGINE=datafusion just bench-curation-serve 1GB`, the DataFusion variant on the server, its step in the waterfall; the Flow variant refused — implements *Requirement: A managed step on the deployed engine*

### 4 · DuckDB, the addition
- [x] 4.1 `query_duckdb`: the five expression classes, `read`, `df`; the connection held by the engine; a result that is not a relation refused; `HUGEINT` as an integer and a value outside 64 bits refused naming the column — implements *Requirement: A DuckDB query is ordinary DuckDB Python*
- [x] 4.2 DuckDB's vocabulary: `DECLINED`, the SQL-string rule, `FunctionExpression` against `duckdb_functions()`; measure whether `enable_external_access=false` can sit under `read()` and record the answer either way — implements *Requirement: Running typed Python has a stated boundary*
- [x] 4.3 Rust: `RuntimeBinding::DuckDb`, `RuntimeKind::DuckDb`, `execution/duckdb.rs` — implements *Requirement: A managed step on the deployed engine*
- [x] 4.4 The DuckDB emitter, examples, seed variants, image and chart value — implements *Requirement: The panel follows the deployed engine*, *Content names the engine it was written for*
- [x] 4.5 Measure DuckDB as 3.7 did, and the end to end as 3.8 did — implements *Requirement: A DuckDB query is ordinary DuckDB Python*

### 5 · The decision, where the next reader looks
- [x] 5.1 ADR_0028 — `open` against `strict`, what `open` costs (it runs code, and on a laptop it can reach the network), what would make it wrong; amendments to ADR_0008, ADR_0014 and ADR_0024 pointing at it — implements *Requirement: A query surface is admitted, or runs where code runs*
- [x] 5.2 `CLAUDE.md` (diagram, services, commands, the Flow guardrails that now speak for every engine), `services/query/README.md`, `docs/INSTALL.md`; the aliases to drop listed for `/spec-deploy`

## Log
- 2026-09-10 15:40 — job planned on `feat/agent-sdk-merge`: five phases, 29 tasks; two points raised rather than decided quietly — `open` mode stands outside *never fetch a byte outside `integrations::fetch`* on a laptop, and the conformance q3/q4 are bounded to fit the 1 000-row answer
- 2026-09-10 15:57 — 1.1–1.4 landed: `QueryEngine` and `engine` on transforms, recipes and versions (stored digests unchanged, tested byte for byte); the one-engine chain rule; `AIWATCHER_QUERY_ENGINE`/`_URL`/`_STEP_TIMEOUT_SECONDS` with the `AIWATCHER_FLOW_URL` alias; `execution/query.rs` with a 404 to a query no longer read as an empty answer; `FlowStepSpec` renamed `QueryStepSpec` behind an alias. `cargo check --workspace --tests` and clippy `-Dwarnings` clean on the touched crates
- 2026-09-10 16:00 — 1.5 landed: moved with `mv` rather than `git mv` (untracked files move too, the index is left alone); Flow answers `/query/*` beside `/flow/*` from one route table, healthz and datasets carry `engine: flow`, `language: flow-dsl`; `AIWATCHER_QUERY_TIMEOUT_SECONDS`; `query-install|serve|check` over `AIWATCHER_QUERY_ENGINE`. `just flow-check` green (166 tests), smoke test over both prefixes, no `services/flow` left outside ADRs and spec history
- 2026-09-10 16:05 — 1.6 landed: `catalog.json` generated from Flow's own definitions rather than transcribed, then proved identical dataset by dataset with `serialize()` before the PHP definitions were deleted; `CatalogFile` refuses an unknown key by name and skips `$comment`; the image carries it at `/contract`. `just flow-check` green, 169 tests
- 2026-09-10 16:16 — 1.7 landed and verified in the browser on the bench stack: the Query tab asks `/query/healthz`, `/query/datasets` and `/query/check` (all 200) of Flow at its new path. Panel: 14 files, 118 tests, prettier and `tsc` clean. Plan changed for 1.8: one `Dockerfile.query` with a target per engine instead of a second Python Dockerfile, so Compose selects by the one variable
- 2026-09-10 16:19 — 1.8 landed: `query.*` values with `flow.*`/`panel.flowUpstream`/`execution.flowUrl` read as aliases through one `aiwatcher.query` helper; `-query` resources; nginx rewrites both prefixes to the one the engine serves; query ingress plus a new egress policy (server and DNS only); `install.sh` takes `AIWATCHER_QUERY[_ENGINE|_IMAGE]`; Compose `query` service by target. Rendered defaults, legacy `flow.enabled`, DataFusion with policies and the planner environment — all valid under kubeconform; `query.engine: polars` refused by the schema
- 2026-09-10 16:20 — measured the fork-server premise: a child forked after `import datafusion` or `import duckdb` answers (3 of 3 each); forked after the parent ran a DataFusion query, the child panics in Tokio (`failed to wake I/O driver`). Decision kept, sharpened: the fork server imports and never queries, and `strict` runs in the same child as `open`
- 2026-09-10 16:24 — 1.9 gate: every step AW-3 touches passes — cargo fmt, clippy -Dwarnings, cargo test, openapi current, panel build+typecheck and tests, python sdk, k8s, helm chart (default and planner), cargo deny, curation seed; `just flow-check` and `just query-check` 169 tests. Fixed on the way: unformatted Rust, the deliberate misspelling tripping typos (now `window`), `lint-comments.py` crashing on files moved but not yet staged, two benchmark TOMLs. `just check` still fails on four steps AW-3 does not touch, all from other work on this branch: agentic engine (ruff in sdk/agentic), typos (the requeue parser variable in sdk/python outbox from f6ed7646, Polish test wording in sdk/agentic), taplo (sdk/agentic/pyproject.toml), comments (workflow-layout.ts, conversations/payload.rs, server/conversations.rs). Phase 1 complete
- 2026-09-10 18:16 — 2.1 landed: `aiwatcher_query` in a `uv` workspace at `services/query` (one member, `contract`, until the engines join it): the six `/query` routes on Starlette, Flow's answer fields in Flow's order; the API paged into Arrow typed from the catalog — status before body, declared arguments only, `window_seconds` and `as_of`, cursors replayed with a guard against one that repeats; `hub_rows`' `row` a struct from the page's own `columns` envelope, so no second request; an event's `data` as JSON text, because a union of payloads has no one Arrow type; execution memory as a dict, since this is one uvicorn process; the `Engine`/`Session` protocol. Three differences from Flow, deliberate: `columns` lists the schema even for zero rows; a bad `hub_rows` limit is a 422 where Flow answers 502; `deterministic` is false for a corpus read only — Flow's code, which this note's decision misstated as 'a corpus or hub read'. Exceptions carry the `Error` suffix the notebook runtime uses
- 2026-09-10 18:16 — 2.2 landed: the service reads its configuration, removes `AIWATCHER_*` from its own environment, marks itself non-dumpable on Linux and only then starts the fork server, which preloads the engine and never runs a query; each child scrubs again, works in a scratch directory removed afterwards, and runs under `RLIMIT_CPU` (and `RLIMIT_AS` on Linux, default 4096 MiB of address space, `AIWATCHER_QUERY_MEMORY_MB`). SIGXCPU or the wall clock is a 422 naming `AIWATCHER_QUERY_TIMEOUT_SECONDS`; SIGKILL a 422 naming the memory ceiling. Proven here: a spinning query stopped at 2 s while healthz answers inside a second; with a token set before start, the child sees no `AIWATCHER_*`. Not provable on this Mac: `RLIMIT_AS` and `PR_SET_DUMPABLE` are Linux-only, and macOS lets a same-user process read another's initial environment through `sysctl` — one more reason `open` is a localhost posture, for ADR_0028
- 2026-09-10 18:16 — 2.3 landed: `strict` admission — a written grammar (names, attributes, calls, literals, operators, list comprehensions; nothing that imports, defines, loops or raises), names from the namespace or assigned, attributes derived from the engine's own classes and modules, a `DECLINED` table per engine (`format` for all), and a hook for an engine's rule over a call's arguments (DuckDB's SQL strings). `/query/check` reports every refusal with line and column and says `strict`; a strict query runs in the child like any other. Decision changed: no empty-`__builtins__` second wall — it aborted the process inside pyarrow, because a C extension reads builtins from its caller's frame; admission is the one wall. The spec's engine-named strict scenarios (`read_csv`, `udf`, `SessionContext().sql`, a SQL string) are proven when each engine supplies its `DECLINED` and rule, in 3.2 and 4.2. 95 tests against an Arrow-only test engine; ruff and mypy --strict clean
- 2026-09-10 18:24 — 2.4 landed: the conformance suite — `python -m aiwatcher_query.conformance --url …` reads healthz for the language, asks q1–q4 in that language and compares with Flow's rows, checked in under `contract/conformance/<q>/expected.json` (counts and sums exactly, a float within Flow's half-hundredth), and compares `/query/datasets` with Flow's recorded catalog. The corpus is `generate.py` at seed 20260910 and 1 MB — 5 515 spans in 460 runs — generated with only polars and numpy. q4 is narrowed a second time, recorded here: its join partner is the per-run count of LLM spans rather than the benchmark's price table, a file no catalog dataset serves; Flow's own join test has that shape. Flow passes, catalog and all four. The DataFusion and DuckDB texts, run through a harness that emulates `read()` over the same files, give Flow's rows on all four; the proof through the served engines is 3.8 and 4.5. Learned on the way: Flow's join key carries one name on both sides, and DuckDB's self-join needs `set_alias`. A test holds Flow's recorded catalog equal to what `aiwatcher_query` describes, with no engine running
- 2026-09-10 18:24 — 2.5 landed: `query-install`, `query-serve` and `query-check` take `datafusion` and `duckdb`, and say the engine arrives in phase 3 or 4 until its directory exists; `query-contract-check` is ruff, mypy --strict and pytest over the workspace (101 tests); `query-conformance [--record]` generates the corpus, starts the named engine on :8091 (`AIWATCHER_CONFORMANCE_PORT`), asks, and stops it — its first version leaked `php -S` workers, which outlive their master, so it now kills the server's children too. CI's `flow` job is now `query`, a matrix over engines running checks then conformance, beside `query-contract`; the matrix holds `flow` today, and `datafusion` and `duckdb` join it with their engines rather than failing until then. Phase 2 complete
- 2026-09-10 19:22 — 3.1 and 3.2 landed: `query_datafusion` is the workspace's second member — `col`, `lit`, `f` and the contract's `read()`; a `SessionContext` per query, made in the child; CSV read with the catalog's types, Parquet by its parts' directory; `limit(max + 1)` then Arrow; healthz `datafusion-python` 54.0.0, the version the benchmark measured. Its strict vocabulary is `DataFrame`, `Expr`, `SortExpr`, `CaseBuilder`, `ExprFuncBuilder` and `functions`; `DECLINED` holds the ways out, the UDF registrars and SQL, and everything a `SessionContext` offers is declined from the class. Two rules went into the contract rather than into one engine: a module's attributes are its `__all__` (`f.pa` was pyarrow), and a member whose signature takes a callable or a path is declined by that alone — `transform`, seven `array_*`/`list_*` functions and the four writers, none of them named. Conformance against the served engine: datasets and q1–q4 same; all four texts admitted and run under strict. Found on the way, both in the contract: on macOS the child's httpx client asked `SCDynamicStoreCopyProxies` for the system proxies — CoreFoundation in a forked child — which segfaulted every child of some fork servers and none of others (one `test_service` run in three); the child's client now takes nothing from its environment (`trust_env=False`), which also keeps `~/.netrc` out of it. And the sandbox killed a child that had closed its pipe but was not yet reaped, reporting a crash as the 30 s timeout. 122 tests, green on three full runs and ten of `test_service`; ruff's `src` no longer takes the `datafusion/` directory for DataFusion itself
- 2026-09-10 19:47 — 3.3 landed: `RuntimeBinding::DataFusion` and `RuntimeKind::DataFusion`, both `datafusion` on the wire (serde's snake_case would have written `data_fusion`); DataFusion joins Flow in the cache key, the context's actions, the node kind and PostgreSQL's `runtime_from`, and `RuntimeBinding::query()` answers the spec and its engine for readers that treat every engine's query alike — so a dataset version now names the engine its query ran on. The compiler's `query_runtime` refuses a chain written for another engine ('clean is written for flow, and this deployment runs datafusion (AIWATCHER_QUERY_ENGINE)…'), a compile problem the API already answers as a 422 before any run exists; `python_script` writes `df = read(…)`, `df = (<transform>)`, `df` with JSON-string values and refuses an argument name Python cannot pass. `execution/datafusion.rs` claims `datafusion` over `/query`, and `query::executors` registers it for a DataFusion deployment and nothing else. A Flow step, and so its `plan_id`, is built exactly as before. Clippy -Dwarnings clean on the three crates, 136 execution and 59 server tests; `just openapi` moved only the new variant and kind (24 lines of contract, 3 of client). No API test posts a pipeline on a non-Flow deployment, so the HTTP 422 is proved end to end in 3.8
- 2026-09-10 19:47 — 3.5 landed: `pipeline.datafusion.json` beside five examples — flow-vs-polars, pii-detection, titanic-features, titanic-survival, titanic-from-scratch — each named `<name>-datafusion`, its transform marked `engine: datafusion`, its view publishing to a dataset of its own; the seed holds 11 pipelines and `build_seed.py --check` passes. Every variant was run as the compiler writes it, under strict admission, against the contract's fake API shaped like the hub (the corpus fixture for the benchmark), and ends in exactly its Flow original's columns — `median` aliased `age_median` as Flow names it, the regex backreference written `\\1`. Not written, and why: `titanic/pipeline-php.json`, whose transforms are this repository's own Flow steps (`trainTestSplit`, `imputeMissing`, `oneHotEncode`, `labelEncode`, fitted on the training split) — DataFusion has no vocabulary for them, and re-authoring preprocessing as expressions is a notebook's job. The benchmark notebook names its columns by `engine` (default `flow`, so the Flow variant's rows are unchanged; `just ml-pipeline-check` 76 passed); `in_aiwatcher.py` asks `/query/healthz` which variant to start; `bench-curation-serve` starts the engine AIWATCHER_QUERY_ENGINE names
- 2026-09-10 19:47 — 3.6 landed: `Dockerfile.query`'s `datafusion` target — uv at CI's 0.12.7 on the official python:3.14-slim, `uv sync --frozen --no-dev --no-editable --package aiwatcher-query-datafusion`, so the image carries the contract and DataFusion and no DuckDB; 416 MB, built in 21 s. `.dockerignore` gained `**/.data/` and `**/.venv/`: the 29 GB benchmark corpus was inside every build context. Release publishes `aiwatcher-query-datafusion`; the chart named it since 1.8; CI's `query` matrix is `flow, datafusion`. Run under the chart's own constraints (read-only root, a tmpfs /tmp, every capability dropped, uid 10001): q2 over the 1 MB corpus, 200 in 37 ms — and the two things this Mac could not prove, proved on Linux: a child reading the service's `/proc/1/environ` is refused (PermissionError; PR_SET_DUMPABLE holds), a 5 GiB allocation is a 422 naming the memory ceiling (RLIMIT_AS holds); a spinning query stops at its 10 s CPU ceiling
- 2026-09-10 19:51 — 3.7 landed: q2 through the served DataFusion engine over the 5 GB corpus, peak RSS sampled every 20 ms (a lower bound) — CSV (5.0 GiB, one part) 6.64 s open and 6.47 s strict; Parquet (the corpus's 0.4 GiB `formats/spans.parquet`, read as one part) 0.35 s and 0.30 s; the service 74 MiB and the fork server 67 MiB throughout, the query's child 129 MiB over CSV and 228–234 MiB over Parquet. Admission costs nothing measurable; every figure is inside the spec's 512 MiB and the chart's 1 GiB. In the benchmark README, which also still named the two timeouts by their pre-AW-3 names — corrected
- 2026-09-10 19:51 — 3.8 landed, proved on the server rather than in the panel: the server, DataFusion and the notebook runtime on 18080–18082 — beside a half-running `bench-curation-serve` somebody left on 8081/8082, not stopped, not this session's — with a fresh store and the 1 GB corpus. The seed imported all 11 pipelines; `bench-curation-run` asked `/query/healthz`, started `curation/flow-vs-polars-datafusion`, and the workflow fold — what the waterfall draws — gave the DataFusion step 0.85 s (Flow: 95.2 s, same step, same corpus), the Polars notebook 1.52 s, publishing 0.04 s, all eight models agreeing. `curation/flow-vs-polars` on the same deployment: 422 `plan_refused`, 'flow is written for flow, and this deployment runs datafusion (AIWATCHER_QUERY_ENGINE)…', and the fold holds no run of it. The waterfall itself was not opened: the panel is mid-restructure in the working tree, which is also what holds 3.4
- 2026-09-10 20:23 — 3.4 landed on the panel's new layout (`src/features`, `src/shared`), on the user's word that the restructure is done. `compilePython` in `features/data-curation/lib/pipeline.ts` is the Rust `python_script` ported line for line and held to its expected text byte for byte; `runPipeline` compiles to the deployed engine's language; Build mode has a DataFusion emitter beside Flow's in `query-builder.ts`, still refusing no chip; DataFusion's content — both starters, the five recipe examples under Flow's names, the cheat sheet, a new block's text — is `shared/lib/datafusion.ts` behind `engine-content.ts`; the Datasets promotion and the hub import write DataFusion too. Content written for another engine is shown with `writtenElsewhere`'s sentence and not run: read-only in the recipe editor (Test, Simulate, Execute and Save off) and the block inspector, Preview and Run off on the pipeline page. Recipes and published versions now send their engine. Two contract fixes it needed: a hub `Image` column is a struct of `src`, `height`, `width` rather than JSON text, which a Python engine had no way to read a picture's address out of; and a query of only comments is a refusal rather than an IndexError. Proven: vitest 164, typecheck and the architecture check clean; `test_panel_shapes.py` runs every DataFusion shape the panel writes through the engine under strict (136 Python tests); the shipped DataFusion content, taken straight out of the TypeScript by Node, runs under strict, all eight texts. In the browser against a DataFusion backend: Build mode on spans grouped by model wrote DataFusion Python and Run answered `claude-opus-5` in 7 ms; a Flow pipeline said 'Written for Flow PHP, and this deployment runs DataFusion…' with Preview and Run disabled and its transform read-only; the DataFusion variant's Preview ran as one DataFusion query (6 rows, 18 ms); a saved Flow recipe opened read-only with the spec's sentence. `.claude/launch.json` gained `panel-datafusion`. Also, on the user's word: stopped the orphaned `bench-curation-serve` — PHP on 8081 and its workers, the notebook runtime on 8082, its panel preview on 5180. Phase 3 complete
- 2026-09-11 10:09 — 4.1 landed: `query_duckdb` is the workspace's third member — the five expression classes and the contract's `read()`, a connection per query made in the child, CSV read with the catalog's types, Parquet as its list of parts, `limit(max + 1)` then `to_arrow_table`; healthz `duckdb-python` 1.5.5, the benchmark's line. A sum over a BIGINT reaches Arrow as `decimal128(38, 0)`, which the contract's answer already turns into an integer or refuses by its column, so the engine holds nothing for HUGEINT (both tested). Three changes to the contract, each additive: `Engine.open(corpus)`, for the lock below; `Engine.function_by_name`, because DuckDB names a function as text and `deterministic` read only call names, so `FunctionExpression("now")` would have been cached; and the call rule is handed the whole query, so it can follow a name to what the query assigned it. Conformance against the served engine: datasets and q1–q4 same. 173 tests (37 DuckDB), ruff and mypy --strict clean
- 2026-09-11 10:09 — 4.2 landed, with the measurement the checklist asked for: `enable_external_access=false` can sit under `read()`. `allowed_directories` has to be set first — DuckDB refuses it at `connect` and refuses to change it once access is off — and then the corpus's CSV and Parquet, rows from Arrow and a spill to the scratch directory all work, while a file named in a SQL string, a glob, re-enabling access and widening the directories are each refused. So every session is locked, in either admission: a second wall under `strict`, and under `open` only the session's, since a query can `import duckdb` and its module-level connection read /etc/passwd when measured (a cost for ADR_0028). DuckDB resolves a path before comparing it, so a part that symlinks out of the corpus is refused (tested; 4.5 hard-links the Parquet file where 3.7 symlinked it). Plan changed for the SQL-string rule: the note named six methods, and measurement found text parsed as SQL in every relation method that takes it — `sum("…")`, `order`, `unique`, `apply`, a join's condition — while text in an expression is a column name (`ColumnExpression("n") == "1+1"` looks for a column called `1+1`). So strict refuses anything that may be text — a literal, an f-string, a name bound to one, a property, a method of text — in any relation method's arguments, except `set_alias`, a join's kind and a group key of bare column names, and refuses a method called through a name the query gave it. `FunctionExpression` is admitted against `duckdb_functions()` — scalar, aggregate and macro, and `unnest`, which the catalog lists only as a table function and the binder takes in a projection — minus `current_setting`, `getenv` (absent from the Python build) and `write_log`; volatile is every scalar whose stability is not CONSISTENT, 27 of them. `DECLINED` carries a reason per name, and the connection's and the module's members are declined from the class and the module, as DataFusion's session is
- 2026-09-11 10:26 — 4.3 landed: `RuntimeBinding::DuckDb` and `RuntimeKind::DuckDb`, both `duckdb` on the wire and in the claim table (tested, since snake_case would write `duck_db`); DuckDB joins the cache key (a key neither other engine shares, tested), the context's actions, the node kind and PostgreSQL's `runtime_from`, and `RuntimeBinding::query()` names it, so a dataset version records `engine: duckdb`. The compiler's DuckDB arm, which refused, now writes the same `python_script` as DataFusion — one closure, the two differing only in the binding — and a DataFusion chain on a DuckDB deployment is refused naming both. `execution/duckdb.rs` claims `duckdb` over `/query`, and `query::executors` registers it for a DuckDB deployment and nothing else; the arm that logged 'no executor' is gone. The server names the `duckdb` crate nowhere by path (the workflow store is `aiwatcher_execution::store::duckdb`), so no `::duckdb` was needed, and the module says so for whoever adds one. cargo fmt, clippy -Dwarnings on the three crates, 139 execution tests (3 new) and 59 server tests; `just openapi` added the variant and the kind, 48 lines of contract and 6 of client
- 2026-09-11 10:32 — 4.4 landed. Panel: Build mode's DuckDB emitter in `query-builder.ts` — a list column unnested by projecting every column beside `unnest` of it (a relation has no `with_column`), keys and numbers in one list with the keys named again as the group, still refusing no chip; DuckDB's content is `shared/lib/duckdb.ts` behind `engine-content.ts` (both starters, the five examples under Flow's names, the cheat sheet, a new block's text); the Datasets promotion and the hub import dispatch on all three engines, the DataFusion writers renamed for the engine they write. `test_duckdb_panel_shapes.py` runs every shape the panel writes through the engine under strict; the shipped content, taken out of the TypeScript by Node, runs under strict, all seven texts and the fifteen cheat-sheet lines admitted. Two things that found bugs: the variants' run caught that strict refused `df` itself — a relation has a `df()`, declined as a way out, and the walker checked the table before asking whether the query had assigned the name — fixed in the contract (a name the query binds is its own; tested there and by making DuckDB's transform test strict); and the catalog rule refused `FunctionExpression("coalesce")` in the features example, which measurement showed DuckDB refuses too — `coalesce` is SQL's operator, not a catalog function — so the example fills the age with a CASE. Seed: `pipeline.duckdb.json` beside the five DataFusion variants, each `<name>-duckdb`, generated from them; each run as the compiler writes it under strict against the fake hub and corpus, beside its DataFusion twin on DataFusion — same columns, same rows, all five; the seed holds 16 pipelines and `build_seed.py --check` passes. Image: the `duckdb` target, 363 MB (DataFusion's still builds, 416 MB, now that `python-build` copies DuckDB's manifest); release publishes `aiwatcher-query-duckdb`; CI's matrix is `flow, datafusion, duckdb`; the chart named the image since 1.8. Under the chart's constraints (read-only root, tmpfs /tmp, capabilities dropped, uid 10001): q2 200 in open and strict; `/proc/1/environ` refused; 5 GiB a 422 naming the memory ceiling; a spin stopped at its 10 s CPU ceiling with healthz answering; a file in a SQL string refused by the rule under strict and by DuckDB's lock under open; `/query/check` reads DuckDB's function catalog in the read-only container. vitest 176, typecheck and architecture check clean; 186 Python tests
- 2026-09-11 10:50 — 4.5 landed. Measured as 3.7 did: q2 through the served DuckDB engine over the 5 GB corpus, peak resident memory every 20 ms from libproc's task info (what psutil reads on macOS; psutil is not installed here and was not fetched), two runs of each — CSV 2.19–2.84 s with the query child at 259–472 MiB whichever the admission, Parquet 0.31–0.48 s at 123–126 MiB, the service 82 MiB and the fork server 75 MiB. The Parquet file is hard-linked into a corpus root rather than symlinked as in 3.7, because the lock refuses a link out of the root. In the benchmark README beside DataFusion's. End to end as 3.8 did: the server, DuckDB and the notebook runtime on 18080–18082 with their own AIWATCHER_DATA_DIR; the seed imported 16 pipelines, the five `-duckdb` variants among them; `bench-curation-run` asked healthz and started `curation/flow-vs-polars-duckdb` — the DuckDB step 0.89 s (Flow's 95.2 s, DataFusion's 0.85 s), the Polars notebook 1.68 s, publishing 0.03 s, all eight models agreeing. An earlier run failed at the notebook step because of my start line, which handed the notebook runtime's interpreter its own path as a script; the step spent its ten transient attempts over seven minutes and failed as it should, and that run is the other one in the fold. `curation/flow-vs-polars` on the same deployment: 422 `plan_refused` naming the block, `flow` and `duckdb`, and no run of it in the fold. In the panel against that backend (`panel-datafusion`, 5181): the Query tab reads 'A DuckDB query'; Build mode, spans grouped by model, wrote the spec scenario's DuckDB Python, `/query/check` accepted it and Run answered from the server in 201 ms. Found there and fixed: the schema's null sentence told a DuckDB reader to ask `.is_null()`, DataFusion's spelling — `Schemas` now takes the engine rather than whether it is Flow, and says `.isnull()` on DuckDB. The Docker daemon (OrbStack) was not running and was started for the image build. Phase 4 complete
- 2026-09-11 10:58 — 5.1 landed: ADR_0028 — the decision, each alternative with the reason it lost, and what `open` costs: it runs code; on a laptop it can reach the network; on macOS a same-user process reads the service's initial environment through `sysctl`; the empty-`__builtins__` second wall was dropped; the fork-safety crash; DuckDB's lock holds only the session's own connection; `RLIMIT_AS` is Linux-only — and what would make it wrong. Dated amendments to ADR_0008, ADR_0014 and ADR_0024 point at it and their status lines say so; the ADR index and the Execution reading list it (twenty-eight now), with the arc's new step and its reopen trigger
- 2026-09-11 10:58 — 5.2 landed: `CLAUDE.md` — the diagram's query-engine line, the `query-*` commands, `services/query` in the optional-services paragraph and the server's crate row, decision 23, Build mode in the deployed engine's language, the guardrails that now speak for every engine (authentication, `deterministic`, the builder that refuses nothing, status before body, the CI job) and four new ones: no query in a process that has run one; no forked child asking macOS for anything; no plan on an engine it was not written for; no text handed to a DuckDB relation under strict. `services/query/README.md` is new: the engines side by side, a query in each language, admission, configuration, what adding an engine touches. `docs/INSTALL.md`: the component table, `execution.queryUrl`, where managed query steps run, the Query tab section on `query.enabled`, `query.engine`, `query.admission` and the per-engine images; `.env.example` on the `AIWATCHER_QUERY_*` names `install.sh` already reads. The comment lint flagged three of AW-3's own blocks over 25 lines — `execution/query.rs`, Flow's `index.php` and `Catalog.php` — trimmed; it now flags only the two conversations files outside AW-3. typos and taplo clean, `just flow-check` 169. The aliases to drop, for /spec-deploy, all in the release after this one: `AIWATCHER_FLOW_URL` (server config, the Vite proxy, the justfile); `install.sh`'s `AIWATCHER_FLOW` and `AIWATCHER_FLOW_IMAGE`; the `flow-install`, `flow-serve`, `flow-test`, `flow-lint`, `flow-fmt`, `flow-fmt-check`, `flow-check` and `flow-query` recipes; the `/flow/*` routes Flow answers beside `/query/*`, with what reaches them there — the Flow executor's `/flow`, Vite's `/flow/` proxy, nginx's `/flow/` location and rewrite, the chart's `/flow/healthz` probe; the chart's `flow.*` values, `panel.flowUpstream`, `execution.flowUrl` and the schema's `flow` block. Phase 5 complete: 29 of 29
