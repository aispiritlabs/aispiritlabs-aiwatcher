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
- [ ] 2.1 The `uv` workspace and `aiwatcher_query`: the six routes on Starlette bound to `127.0.0.1`, the answer (1 000 cap with `truncated`, `deterministic`, `window_applied`, `digest`, `took_ms`), execution memory, the API pager into Arrow (status before body, declared parameters only, `window_seconds`, `as_of`), `hub_rows` as a struct, `corpus_spans` from `AIWATCHER_CORPUS_DIR`, and the `Engine` protocol each engine implements — implements *Requirement: One query contract, proved against every engine*, *The Python engines' catalog is Flow's catalog*
- [ ] 2.2 `open` mode: the forkserver, the scrubbed child, the ceilings answered as a 422 naming them, the non-dumpable parent; the fork-after-import check — implements *Requirement: Running typed Python has a stated boundary*
- [ ] 2.3 `strict` mode's walker and `/query/check` with the parser's line and column — implements *Requirement: Running typed Python has a stated boundary*
- [ ] 2.4 Conformance: a fixed-seed corpus from `benchmarks/curation/generate.py` small enough for CI, q1–q4 bounded, one text per engine, Flow's rows checked in, the catalog compared — implements *Requirement: One query contract, proved against every engine*
- [ ] 2.5 `just query-serve`, `just query-check` and `just query-conformance` by `AIWATCHER_QUERY_ENGINE`; the CI job as a matrix over the three — implements *Requirement: The engines live side by side*

### 3 · DataFusion
- [ ] 3.1 `query_datafusion`: `col`, `lit`, `f`, `read`, `df`; `SessionContext` held by the engine; `limit(max + 1)` then Arrow; a result that is not a `DataFrame` refused naming what it was; healthz `datafusion-python` — implements *Requirement: A DataFusion query is ordinary DataFusion Python*
- [ ] 3.2 DataFusion's vocabulary and `DECLINED`; tests for `udf`, `SessionContext().sql`, `register_csv` and the four questions admitted — implements *Requirement: Running typed Python has a stated boundary*
- [ ] 3.3 Rust: `RuntimeBinding::DataFusion`, `RuntimeKind::DataFusion`, `runtime_from`, the cache, context, facts and publish arms, `execution/datafusion.rs`, `python_script`, and the 422 naming block, engine and deployment with no run created — implements *Requirement: A managed step on the deployed engine*
- [ ] 3.4 Panel: the DataFusion emitter in Build mode (still refusing no chip), `compilePython`, foreign content read-only with its sentence in the recipe editor and the block inspector, recipe examples for the engine — implements *Requirement: The panel follows the deployed engine*
- [ ] 3.5 Seed variants for every example whose transforms DataFusion can express, `build_seed.py --check`; `in_aiwatcher.py` picks the variant from healthz; `bench-curation-serve` serves the engine it is given — implements *Requirement: Content names the engine it was written for*
- [ ] 3.6 The `datafusion` target in `Dockerfile.query`, the `aiwatcher-query-datafusion` image in release-images, the chart's image — implements *Requirement: Deployment values name the engine*
- [ ] 3.7 Measure q2 through the service over the 5 GB corpus, CSV and Parquet, `open` and `strict`, peak RSS of the service and its child; a few seconds each; into the benchmark README — implements *Requirement: A DataFusion query is ordinary DataFusion Python* (the memory scenario)
- [ ] 3.8 End to end: `AIWATCHER_QUERY_ENGINE=datafusion just bench-curation-serve 1GB`, the DataFusion variant on the server, its step in the waterfall; the Flow variant refused — implements *Requirement: A managed step on the deployed engine*

### 4 · DuckDB, the addition
- [ ] 4.1 `query_duckdb`: the five expression classes, `read`, `df`; the connection held by the engine; a result that is not a relation refused; `HUGEINT` as an integer and a value outside 64 bits refused naming the column — implements *Requirement: A DuckDB query is ordinary DuckDB Python*
- [ ] 4.2 DuckDB's vocabulary: `DECLINED`, the SQL-string rule, `FunctionExpression` against `duckdb_functions()`; measure whether `enable_external_access=false` can sit under `read()` and record the answer either way — implements *Requirement: Running typed Python has a stated boundary*
- [ ] 4.3 Rust: `RuntimeBinding::DuckDb`, `RuntimeKind::DuckDb`, `execution/duckdb.rs` — implements *Requirement: A managed step on the deployed engine*
- [ ] 4.4 The DuckDB emitter, examples, seed variants, image and chart value — implements *Requirement: The panel follows the deployed engine*, *Content names the engine it was written for*
- [ ] 4.5 Measure DuckDB as 3.7 did, and the end to end as 3.8 did — implements *Requirement: A DuckDB query is ordinary DuckDB Python*

### 5 · The decision, where the next reader looks
- [ ] 5.1 ADR_0028 — `open` against `strict`, what `open` costs (it runs code, and on a laptop it can reach the network), what would make it wrong; amendments to ADR_0008, ADR_0014 and ADR_0024 pointing at it — implements *Requirement: A query surface is admitted, or runs where code runs*
- [ ] 5.2 `CLAUDE.md` (diagram, services, commands, the Flow guardrails that now speak for every engine), `services/query/README.md`, `docs/INSTALL.md`; the aliases to drop listed for `/spec-deploy`

## Log
- 2026-09-10 15:40 — job planned on `feat/agent-sdk-merge`: five phases, 29 tasks; two points raised rather than decided quietly — `open` mode stands outside *never fetch a byte outside `integrations::fetch`* on a laptop, and the conformance q3/q4 are bounded to fit the 1 000-row answer
- 2026-09-10 15:57 — 1.1–1.4 landed: `QueryEngine` and `engine` on transforms, recipes and versions (stored digests unchanged, tested byte for byte); the one-engine chain rule; `AIWATCHER_QUERY_ENGINE`/`_URL`/`_STEP_TIMEOUT_SECONDS` with the `AIWATCHER_FLOW_URL` alias; `execution/query.rs` with a 404 to a query no longer read as an empty answer; `FlowStepSpec` renamed `QueryStepSpec` behind an alias. `cargo check --workspace --tests` and clippy `-Dwarnings` clean on the touched crates
- 2026-09-10 16:00 — 1.5 landed: moved with `mv` rather than `git mv` (untracked files move too, the index is left alone); Flow answers `/query/*` beside `/flow/*` from one route table, healthz and datasets carry `engine: flow`, `language: flow-dsl`; `AIWATCHER_QUERY_TIMEOUT_SECONDS`; `query-install|serve|check` over `AIWATCHER_QUERY_ENGINE`. `just flow-check` green (166 tests), smoke test over both prefixes, no `services/flow` left outside ADRs and spec history
- 2026-09-10 16:05 — 1.6 landed: `catalog.json` generated from Flow's own definitions rather than transcribed, then proved identical dataset by dataset with `serialize()` before the PHP definitions were deleted; `CatalogFile` refuses an unknown key by name and skips `$comment`; the image carries it at `/contract`. `just flow-check` green, 169 tests
- 2026-09-10 16:16 — 1.7 landed and verified in the browser on the bench stack: the Query tab asks `/query/healthz`, `/query/datasets` and `/query/check` (all 200) of Flow at its new path. Panel: 14 files, 118 tests, prettier and `tsc` clean. Plan changed for 1.8: one `Dockerfile.query` with a target per engine instead of a second Python Dockerfile, so Compose selects by the one variable
- 2026-09-10 16:19 — 1.8 landed: `query.*` values with `flow.*`/`panel.flowUpstream`/`execution.flowUrl` read as aliases through one `aiwatcher.query` helper; `-query` resources; nginx rewrites both prefixes to the one the engine serves; query ingress plus a new egress policy (server and DNS only); `install.sh` takes `AIWATCHER_QUERY[_ENGINE|_IMAGE]`; Compose `query` service by target. Rendered defaults, legacy `flow.enabled`, DataFusion with policies and the planner environment — all valid under kubeconform; `query.engine: polars` refused by the schema
- 2026-09-10 16:20 — measured the fork-server premise: a child forked after `import datafusion` or `import duckdb` answers (3 of 3 each); forked after the parent ran a DataFusion query, the child panics in Tokio (`failed to wake I/O driver`). Decision kept, sharpened: the fork server imports and never queries, and `strict` runs in the same child as `open`
- 2026-09-10 16:24 — 1.9 gate: every step AW-3 touches passes — cargo fmt, clippy -Dwarnings, cargo test, openapi current, panel build+typecheck and tests, python sdk, k8s, helm chart (default and planner), cargo deny, curation seed; `just flow-check` and `just query-check` 169 tests. Fixed on the way: unformatted Rust, the deliberate `windowd` tripping typos (now `window`), `lint-comments.py` crashing on files moved but not yet staged, two benchmark TOMLs. `just check` still fails on four steps AW-3 does not touch, all from other work on this branch: agentic engine (ruff in sdk/agentic), typos (`requeueing` in sdk/python outbox from f6ed7646, `usera` in sdk/agentic), taplo (sdk/agentic/pyproject.toml), comments (workflow-layout.ts, conversations/payload.rs, server/conversations.rs). Phase 1 complete
