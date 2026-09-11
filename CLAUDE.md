# CLAUDE.md

Guidance for Claude Code when working in this repository.

## What This Is

Observability for AI agent runs. Python and TypeScript agents publish events to
a durable log; a Rust backend consumes them, assembles OpenTelemetry traces,
exports to VictoriaTraces and VictoriaMetrics, and serves a live view over
SSE/WebSocket to a React panel.

```
Python / TypeScript agents
         │  events
         ▼
   durable log  (Laser, or the built-in write-ahead log)
         │
         ▼
   Rust projector ─┬─► live events ──► Axum SSE/WebSocket ──► panel
                   ├─► finished spans ─► VictoriaTraces
                   ├─► aggregates ─────► VictoriaMetrics
                   └─► read model ─────► REST reads

   prompt registry ──► RustFS (S3)   authored, and outside retention entirely
   annotations     ──► RustFS (S3)   drawn, versioned, exported for training
   conversations   ──► RustFS (S3)   encrypted, its own retention, erasable
   training runs   ──► RustFS (S3)   a curve and a model registry; off the log
   dataset hubs    ──► Kaggle, HF    what exists; never what is permitted
   query engine    ──► Flow PHP | DataFusion | DuckDB — one per deployment,
                                     behind the Query tab, recipes and a chain
   curation blocks ──► that engine   one query, up to the first notebook
                   └─► ml_pipeline   a marimo notebook: run as a step, served live
   managed runs    ──► PostgreSQL    the plan, the decisions, the outbox — and
                                     back onto the log as facts about work
                   └─► serve | work  two roles, one binary; `work` is the only
                                     one that opens a socket to a query engine
```

## Commands

```bash
just               # list every recipe
just check         # everything CI runs; green here means green there
just test          # cargo test --workspace --all-targets
just lint          # cargo clippy -Dwarnings
just openapi       # regenerate contracts/openapi.json AND the panel's client
just run           # server on :8080, write-ahead log in ./.data
just run-execution # the same, with managed query execution wired to :8081
just run-postgres  # the same, with the workflow store on PostgreSQL
just run-hubs      # the same, with Kaggle/Hugging Face dataset search on
just dev           # server (in-memory bus) + panel on :5173, seeded with data to click around in
just seed-dev      # the same seed into a running server (`--live` keeps runs arriving)
just pii-demo      # the whole curation chain: API + Flow PHP + notebooks + panel
just seed          # publish a demo run into a running server
just seed-evaluation  # publish two comparable evaluation reports
just seed-prompts  # publish a prompt plus three optimisations, one admitted
just seed-workflow    # publish two executions of one declared graph
just seed-annotations # six synthetic plans, three families, an export, a training run
just seed-import      # stage a corpus in pages, import it with the queued job
just run-conversations # the server with the encrypted conversation archive on
just seed-conversations # one reviewed exchange, an export job, an immutable corpus
just e2e-train        # the whole chain: annotate → export → fit a real tiny model → promote
just serve-model      # verify the promoted package's digests, load it, serve it, watch the label
just onnx-version     # re-express that model as an ONNX graph, check it agrees, move the label
just ml-pipeline-serve # the marimo notebook runtime on :8082, for notebook blocks
just ml-pipeline-check # ruff, mypy --strict and pytest for that service
just query-serve   # the query engine AIWATCHER_QUERY_ENGINE names (flow | datafusion | duckdb) on :8081
just query-check   # that engine's own checks; `query-contract-check` is the Python workspace's
just query-conformance # the same four questions asked of that engine, compared with Flow's rows
just stack-up      # docker compose: VictoriaTraces, VictoriaMetrics, Collector, Perses
just tilt-up       # the same stack on a local Kubernetes, rebuilt on save
just skills        # re-vendor .claude/skills at their pinned commits
```

Installing into a cluster that is not a scratch one:

```bash
just detect NS         # what that cluster already runs — reads only
just install-plan      # render and diff; change nothing
just install-cluster   # apply, after asking on any non-local context
just images            # build aiwatcher and aiwatcher-panel
```

With a broker, for the Laser backend:

With a database, for the workflow store:

```bash
just postgres-up   # PostgreSQL on :5433 — not 5432, so a suite never lands in
                   # a project database somebody already has there
just test-postgres # the storage contract, the same sixteen properties the
                   # memory and file adapters prove, against a real database
just run-serve     # the API, the read model and the object store, no ingress out
just run-work      # the outbox and the reactors, no ingress in
```

The last two are the split of section 27, and they need three shared backends —
`postgres`, `laser` and `s3` — because that is what stops being per-process when
the binary is two processes. The start-up refuses each by name. One process
holding both roles is the default and needs none of them, which is what `just
run` and `just dev` are.

```bash
just iggy-up       # Apache Iggy in Docker, with the three flags it needs
just run-laser     # server on the Laser backend
just test-laser    # six integration tests against the real broker
```

With an identity provider, for single sign-on:

```bash
just authentik-up  # authentik in Docker: server, worker, PostgreSQL, Redis
just run-sso       # the server as an OIDC relying party against it
```

With an object store, for the prompt registry:

```bash
just rustfs-up     # RustFS on :9010
just run-rustfs    # server with the registry in the object store
just test-rustfs   # five integration tests against it — this is what verifies the SigV4 signer
```

The Python SDK is a `uv` project of its own:

```bash
just sdk-install   # uv sync --all-groups
just sdk-check     # ruff format --check, ruff check, mypy --strict, pytest
just agentic-install  # the workflow engine in sdk/agentic, likewise
just agentic-check    # the same four checks, on the engine
```

Run a single Rust test: `just test-one two_parallel`.
Panel: `cd apps/panel && npm run build` (vite build followed by a full `tsc`
project check).

## Architecture

Crates, in dependency order. A crate may only depend on ones above it.

| Crate | Holds |
|-------|-------|
| `aiwatcher-core` | Domain: ids, envelope, correlation, event catalog, ports. Knows nothing about Laser, HTTP or OTLP. |
| `aiwatcher-bus` | `MessageSource` / `MessageSink` / `Checkpointer` + memory, write-ahead-log, Laser and generic-broker adapters |
| `aiwatcher-trace` | `SpanAssembler` and the OTLP/JSON exporters |
| `aiwatcher-jobs` | What a long job over an object store *is*: `JobState`, `ShardRef`, the lease, the retry decision, and `version_of` — the content address a finished job is named by. The rules, not the records; both callers keep their own shape and call these (ADR_0022). |
| `aiwatcher-prompts` | The prompt registry: content-addressed versions, optimisation records, and the RustFS/S3 and filesystem adapters behind `ObjectStore`. Includes a hand-written SigV4 signer. |
| `aiwatcher-annotations` | Vector image annotations for **any** vision domain — it ships no vocabulary, and the project's label schema carries the domain (ADR_0020). Sliced by noun: `images/` (one picture — head, revisions, review, bytes, bulk import), `imports/` (the staged batch and the queued job that reads it, ADR_0022), `project`, `export`, `license` (what may be done with the data), `schema`, `shapes`, `sources` (a catalogue an instance loads), `integrations/` — `hubs` (Kaggle and Hugging Face) and `fetch`, the bounded downloader every outbound byte goes through. `registry` is the facade and the only public door; `store` is the private key layout every slice reads through. |
| `aiwatcher-conversations` | Governed conversation training data: the `turn` contract, consent and retention, the **encrypted** archive, the human review gate, and the resumable export job that freezes a corpus. The one authored store that is off by default, whose content is sealed, and whose deletions delete. Sliced by noun: `turn`, `policy`, `redaction`, `review`, `archive/` (the store and its retention clock, `crypt` beneath it), `export/` (the job, `format` beneath it). `registry` is the facade and the only public door; `store` is the private key layout. |
| `aiwatcher-training` | Training runs and the model versions they produce. The one registry here whose contents never came from the event log: a run is a record that grows in place, and a promotion is refused without a held-out score. `package` is what a serving runtime is handed — the runtime, the entry point, the shapes, and every artifact with its digest (ADR_0023). |
| `aiwatcher-datasets` | Curation recipes, the dataset versions they produce, and the **block pipelines** of ADR_0024 — a chain of source, transform, notebook, approval and view, refused as a whole with every problem at once. Nothing here executes anything; the panel drives the chain because the engines are three different systems. |
| `aiwatcher-execution` | Owned execution (ADR_0025, ADR_0026): the compiled `ExecutionPlan` and its `plan_id`, the states, the attempts, the pure `decide`/`evolve`, the cache key, the compiler from ADR_0024's blocks, the atomic command handler, the claim table, the `ContextSnapshot` that reopens a block, the fact encoder and the outbox publisher. Three ports: `WorkflowStore` (`memory | file | postgres | duckdb`, the last two behind features so `sqlx` and DuckDB's C++ amalgamation are out of every build that does not ask for them — the shape `laser` has in `aiwatcher-bus`), `ActivityExecutor` (what a reactor does with a claimed attempt) and `ArtifactCatalog` (metadata, lineage, the cache index). Executes nothing itself, and holds no second copy of `aiwatcher-jobs`' rules — it calls them. |
| `aiwatcher-runner` | The workflow rerun dispatcher: one HTTP POST to one configured endpoint, behind `core::ports::WorkflowRunner`. |
| `aiwatcher-auth` | Single sign-on: OIDC discovery, a JWKS cache, the authorization-code flow with PKCE, HMAC-signed session cookies, authentik's forward-auth headers, and the group-to-role mapping. Knows nothing about axum. |
| `aiwatcher-projector` | The pipeline, live hub, read model, dimension, span, evaluation and workflow-graph folds, dedup, retry, dead letters |
| `aiwatcher-api` | axum router: REST, SSE, WebSocket, OpenAPI. `worker` is the one module whose caller is not a browser: the reactor's own loop with an HTTP seam where the work happens (Phase 10). |
| `aiwatcher-server` | Config, wiring, graceful shutdown, and the **reactors** — the one place an executor's client lives, because an executor holds a socket and a credential. `execution/` is mostly the work role: `artifacts` (the object store's sixth prefix, and the receipt a lookup reads), `query` (the client every query engine shares) with `flow`, `datafusion` and `duckdb` beside it (one executor per engine, and only the deployed one registered) and `publish` (the dataset version, which runs in `serve` because it executes nothing) — and `editor`, which runs in `serve` because opening a block on a step's rows is a person waiting on a request rather than an attempt somebody claimed. The only crate that knows every implementation exists. |

Everything else: `apps/panel` (React), `sdk/python`, `sdk/agentic`, `sdk/typescript`,
`contracts/` (the OpenAPI document and the envelope JSON Schema), `deploy/`
(the Dockerfiles, the docker compose stack, the kustomize test stack, and
`helm/aiwatcher` + `helmfile.yaml.gotmpl` + `scripts/` — the install path),
`docs/ADR/`, and two **optional** services outside the Cargo workspace that the
Rust binary does not know exist. `services/query` holds the **query engines** a
deployment chooses between with `AIWATCHER_QUERY_ENGINE` (ADR_0028), behind the
panel's Query tab, its recipes and a chain's query step: `flow` is the PHP surface
(`just flow-check`), and `contract`, `datafusion` and `duckdb` are one `uv`
workspace — the contract every Python engine serves, the catalog all three load,
and the two engines on it (`just query-contract-check`; `services/query/README.md`).
`services/ml_pipeline` is the Python 3.14 notebook runtime behind a pipeline's
marimo blocks: it runs one as a step through marimo's own `App.run(defs=…)` — in
a worker thread, so a run does not hold the loop that serves everything else —
and serves the same file as a live app for the block's editor (`just
ml-pipeline-check`). `just check` covers neither — PHP and a Python toolchain
may not be on a machine that only touches the Rust crates — but **CI runs each**,
in their own jobs and once per query engine, because a managed query or `marimo`
step runs through them and a break there is a break in the execution path.

### The words, since "workflow" meant four things

| Term | Meaning |
|------|---------|
| `CurationPipelineDefinition` | ADR_0024's authored source/transform/notebook/approval/view blocks |
| `WorkflowDefinition` | An authored or registered agent, search or ML workflow |
| `DefinitionRevision` | The immutable content-addressed version of either — the whole authored request, canvas positions included |
| `ExecutionPlan` | The compiled, runtime-neutral graph, addressed by `plan_id` over the executable fields only |
| `ExecutionRun` | One attempt to execute one pinned plan |
| `StepRun` | The logical state of one plan step, across every attempt |
| `StepAttempt` | One physical attempt. Immutable once terminal; a retry increments, never rewrites |
| `WorkflowStream` | One execution's ordered inputs and outputs, in the `WorkflowStore` |
| `ArtifactRef` | `aiwatcher-core`'s pointer to bytes stored outside the message |
| `RuntimeBinding` | Where a step runs and what that runtime needs — never a host |
| `ExecutionOwner` | Who decides: `local`, `engine:<name>`, or `worker` |
| `ExecutionMode` | `compiled` — the Rust decider schedules a static plan; `hosted` — a worker decides and this keeps the history |
| `ObservedWorkflow` | A graph folded from telemetry (ADR_0012). Not necessarily launchable, and not an `ExecutionPlan` |

A curation pipeline, a planner workflow and an agent graph may all compile to an
`ExecutionPlan` and remain distinct authoring experiences — and an agent graph
compiles only to its *shape*, because its decisions stay in the worker.

## The decisions that explain most of the code

Each has an ADR under `docs/ADR/`. Read the relevant one before changing that
area.

1. **Ids are derived, not generated** ([ADR_0001](docs/ADR/ADR_0001_EVENT_ENVELOPE.md)).
   Delivery is at-least-once, so `TraceId::derive` and `SpanId::derive` are pure
   functions of `run_id` and a stable span key. A redelivery lands on the same
   span instead of writing a duplicate. Never replace a derivation with a UUID.

2. **Laser is behind a port, and feature-gated** ([ADR_0002](docs/ADR/ADR_0002_EVENT_BUS_PORT.md)).
   Nothing above `aiwatcher-bus` names it, and the `laser` cargo feature is off
   by default — a plain build needs neither the SDK nor a broker. The default
   backend is the built-in write-ahead log.

3. **An event is not a span** ([ADR_0003](docs/ADR/ADR_0003_SPAN_ASSEMBLY.md)).
   Hundreds of events fold into a handful of spans. `llm.chunk` is counted, never
   stored per chunk. A span is written only when its end event arrives.

4. **A reconnect closes its own gap** ([ADR_0004](docs/ADR/ADR_0004_LIVE_STREAM_RESUME.md)).
   Every SSE frame carries its checkpoint as the `id:`, so the browser resumes
   via `Last-Event-ID` with no application code.

5. **Tilt runs the stack on a local Kubernetes, and refuses anything else**
   ([ADR_0006](docs/ADR/ADR_0006_LOCAL_K8S_WITH_TILT.md)). This kubeconfig has
   production EKS contexts in it; both the `Tiltfile` and `just tilt-up` hard-stop
   on a non-local context.

6. **One fold slices runs every way, and every list is a cursor page**
   ([ADR_0007](docs/ADR/ADR_0007_EXPLORER_DIMENSIONS.md)).
   `dimensions::compute` answers `session | agent | runtime | workflow | trace |
   model | tool` with one row shape — the pivots differ only in which key a run
   contributes. Nothing loads a whole run: `read_stream_page` pages the log,
   `/spans` and `/dimensions` page the read model, and search runs on the server.
   The live path stays in Rust.

7. **Installation reads the cluster instead of trusting a flag**
   ([ADR_0009](docs/ADR/ADR_0009_INSTALL_BY_DETECTION.md)).
   `deploy/helmfile.yaml.gotmpl` runs `deploy/scripts/detect-stack.py` while it renders
   and lets the findings pick the release's values, because a second
   VictoriaMetrics beside an existing one splits one workload's metrics across
   two stores with no error anywhere — just a gap in a graph. Every backend is
   `install | external | none`, never a boolean. The findings-to-values mapping
   lives only in `--format helm-values`, so plain `helm -f <(…)` reaches the
   same result. See `docs/INSTALL.md` for the planner case.

8. **An evaluation report is not a trace** ([ADR_0010](docs/ADR/ADR_0010_EVALUATION_REPORTS.md)).
   `eval.started | eval.case | eval.completed | eval.failed` ride the same log,
   carry phases, and form **no span** — `EventType::forms_span` is false and
   `SpanAssembler::ingest` returns immediately. They fold into their own bounded
   projection, not the runs list, and are served from `/api/v1/evaluations`.
   This is what lets a producer drop MLflow's `start_run` / `log_params` /
   `log_metrics` / `log_dict` block: `record_evaluation` in both SDKs is the
   same four pieces on the client that is already there for tracing.

9. **A prompt is authored, not observed** ([ADR_0011](docs/ADR/ADR_0011_PROMPT_REGISTRY.md)).
   Everything else here is a fold over the log, and everything else is
   therefore bounded by retention. A prompt is not: the version a run used has
   to be readable after that run has been evicted. So the registry is an object
   store — RustFS in a deployment, a directory under `just run` — behind
   `core::prompts::ObjectStore`, and `aiwatcher-prompts` owns the key layout.
   Three rules carry it: a version id is `sha256(text)` so publishing is
   idempotent; the version object is written before the head that indexes it;
   and `OptimizationRecord::verdict` decides whether a candidate was an
   improvement, from the held-out split, because the optimiser picked it by
   maximising the number it is reporting.

10. **A Flow PHP query is parsed, never executed**
   ([ADR_0008](docs/ADR/ADR_0008_FLOW_QUERY_SURFACE.md)). The Query tab accepts
   a `data_frame()->…` pipeline, which `services/query/flow` lexes with
   `token_get_all()`, admits through `Dsl\Registry` — Flow's own signatures,
   by return-type namespace and by refusing any parameter that takes a
   callable — and turns into Flow objects through an explicit `match`: no
   `eval`, and no name from a query ever becomes a callable. There is no
   hand-written whitelist; the one that used to sit beside the registry decided
   nothing and read like the boundary, and is deleted (ADR_0008, amended). Syntax errors come from Mago, which reads the query after
   `Enrichment` substitutes the bareword dataset names that are not valid PHP;
   it advises, it never decides what may run. It reads the aiwatcher API: measured at 210 ms for
   `groupBy(agent)` over 1500 runs, against 5 ms for the Rust dimension route
   and 2 s for the same question over 175 000 raw events. Grain decides that,
   not transport, which is why the live path stays in Rust and there is no
   export.

11. **Signing in happens here, and the session is a cookie this server signs**
   ([ADR_0013](docs/ADR/ADR_0013_SINGLE_SIGN_ON.md)). `AIWATCHER_AUTH_MODE` is
   `none | oidc | proxy` and defaults to `none` — a release that started
   refusing requests would be an upgrade that took an installation down. The
   panel's two most important routes are an SSE stream and a WebSocket, and a
   browser can set headers on neither, so the authorization-code exchange runs
   *in this process*, the provider's tokens are read once and dropped, and what
   the browser keeps is an HttpOnly cookie holding a signed `Identity`. There
   is no session store: the cookie is self-contained, which means the session
   TTL *is* the revocation window. Roles are `viewer | editor | admin`, mapped
   from authentik group names, and `admin` guards exactly one route — the
   rerun. `proxy` mode reads the outpost's headers instead, which is one
   variable where planner's ingress already authenticates, and trusts the
   network rather than a signature.

12. **A workflow graph is declared, not discovered**
   ([ADR_0012](docs/ADR/ADR_0012_WORKFLOW_GRAPH.md)). planner runs its house
   import as four Flyte stages *and* as the same four functions in-process,
   depending on `settings.flyte_enabled`. So aiwatcher never asks an
   orchestrator anything: `workflow.declared` carries the topology on the log,
   `step.*` with `data.node` executes a node of it, `artifact.produced` points
   at what a node handed on, and `agent.message` records one agent addressing
   another — the one thing nesting cannot show. `workflow_run_id` joins the
   stages a per-pod orchestrator scatters across four runs; omit it and the run
   *is* the execution. A stage nothing has started is `Pending`, which is the
   whole reason the declaration exists, and rerun is a dispatch to one endpoint
   from *configuration* — `aiwatcher-runner`, 501 when unset.

13. **The orchestrator is read for its inventory, never for its history**
   ([ADR_0016](docs/ADR/ADR_0016_PIPELINE_ENGINE.md)). Nothing publishes an
   event about a workflow nobody has run, and no event carries an input
   interface — so `/api/v1/engine` asks Flyte what it *could* start, while
   `/api/v1/workflows` still folds what *has* run from the log. ADR_0012 is
   unchanged by this: the shape of a graph is still the declaration, because
   that is the source that is right when the orchestrator is bypassed. A
   launch binds inputs to the types the engine declares *at launch time*,
   always pins a version, and carries a `workflow_run_id` aiwatcher mints — as
   a Flyte label and, when the entity declares one, as an input — which is what
   lets the panel stream an execution that has not started. `AIWATCHER_ENGINE`
   defaults to `none` and every route answers 501 naming it.

14. **An annotation is authored, vector-first, and split by family**
   ([ADR_0017](docs/ADR/ADR_0017_IMAGE_ANNOTATION.md),
   [ADR_0020](docs/ADR/ADR_0020_GENERIC_VISION_ANNOTATION.md)). A segmentation
   mask cannot say which of two things an overlay belongs to, which way it
   faces, or what it connects — and those are fields a product's output JSON
   has to carry, so drawing pixels loses them at the moment of drawing. The
   vector shape is the source and every raster is derived. Identity is content,
   as it is for a prompt; review state is a label in the head, as prompt labels
   are; and the split key is `group_id`, the *subject*, so every rendering of
   one thing lands on the same side by construction. Usage rights are a
   required field and an export enforces a policy, because the best public
   corpora are often non-commercial and a licence breach shows up in a legal
   review rather than in a metric.

   **This ships no vocabulary.** The label schema is the domain — its classes,
   their geometry, which are `ignore`, and which `layer` each paints into — and
   every mechanism reads it. A class on a higher layer overlays one below
   without erasing it, which is the generic form of "an opening is a segment of
   a wall": one grid could only draw the overlay by deleting what it sits in.

15. **A hub says what exists; the table says what is permitted**
   ([ADR_0019](docs/ADR/ADR_0019_DATASET_HUB_DISCOVERY.md)). ADR_0017's
   `sources` table is a signpost a human wrote, and its docstring says why it
   is not a client: Hugging Face and Kaggle restate corpus licences wrongly
   often enough that a live answer would be worse than none. Searching them is
   still worth doing, because "what exists" and "what may we train on" are
   different questions with different costs of being wrong. So a hub row
   carries `claimed_license` (the mirror's word, named for what it is) and
   `usage` (`unclear`, unless it matched a curated row, which is then named).
   The first live search proved the point: `Voxel51/FloorPlanCAD` declares
   `cc-by-sa-4.0` for a corpus whose authors say the drawings are not theirs to
   license. Importing is a **Flow PHP** pipeline into
   `POST /api/v1/annotation-imports`, because every hub lays its files out
   differently and that mapping belongs somewhere versioned.

16. **A training run is a record, not a trace**
   ([ADR_0018](docs/ADR/ADR_0018_TRAINING_RUNS.md)). The first design put
   `train.*` on the event log; following it through, an epoch turned out not to
   be a span, a step not to belong on the log at all, and a profiler session
   not to be a trace — which left one span with no children and an exception in
   the read model's status fold to make it work. A design whose last step is an
   exception in somebody else's fold is in the wrong place. So training is its
   own module with its own store and its own three write routes; a run opens,
   accumulates a curve and closes; a retried epoch replaces the one it already
   wrote, and reusing a finished run id is a 409. The other half is the **model
   registry**, which is why this lives here rather than in W&B: a version names
   the run and the export behind it, an agent span names a model, and that join
   is the whole point. A label is refused without a held-out measurement — the
   validation score is what early stopping maximised — and refused on a mutable
   dataset name.

17. **Conversation content is encrypted, separately retained and erasable**
   ([ADR_0021](docs/ADR/ADR_0021_CONVERSATION_ARCHIVE.md)). The build before
   this one kept training pairs by putting `input` and `output` on
   `llm.completed`, which wrote somebody's words into the durable log the
   Collector exists to keep them out of, on a retention clock sized for volume
   rather than for what they were told, in a store where a deletion cannot
   delete. So a turn is not an event. `aiwatcher-conversations` is the fifth
   authored registry and the only one that is **off by default**: content is
   sealed with AES-256-GCM under a per-object derived key, the head beside it
   is plaintext so a review queue and an exclusion report need no decryption,
   retention is this module's own clock, and an erasure request names a
   *subject*. An export is an asynchronous job whose cursor advances only after
   a shard is stored, and whose version is a content address over those shards.

18. **A long job over an object store is one primitive, and a corpus is
   staged before it is imported** ([ADR_0022](docs/ADR/ADR_0022_STAGED_IMPORT_JOBS.md)).
   The conversation export was the first job of this shape; the Hub importer is
   the second, and the plan said to decide before writing it rather than after.
   `aiwatcher-jobs` holds the **rules** — shard before cursor, lease per shard,
   retryable versus rejected, `version_of` — and not the records, because an
   export counts exclusions by policy reason and an import counts rejected rows
   by what was wrong with them. A corpus is now staged as digested JSONL pages
   and sealed into a content address before a job reads it, so a million rows
   are resumable rather than a body somebody holds open; and every outbound byte
   goes through `integrations::fetch`, which is the only place in this system
   that downloads bytes an outside party chose.

19. **A serving runtime is handed a declared package, and a checkpoint URI is
   not one** ([ADR_0023](docs/ADR/ADR_0023_MODEL_PACKAGE.md)). A version's
   `ModelPackage` names the runtime, the entry point, the input and output
   shapes, the dependencies and **every artifact with its `sha256`** — because
   `s3://models/latest.pt` is different bytes tomorrow and the registry's whole
   promise is that a span naming a version can be traced to what it learned
   from. A runtime is declared, never sniffed, and `Runtime::executes_packaged_code`
   is what a host answers *before* it opens anything.
   `aiwatcher_sdk.serving` is the hardened profile against it, behind
   `scripts/serve-model.py`: verify, warm, bound, validate, watch the label,
   roll forward in two phases, keep the previous version for a rollback — and
   report each inference as `run.started → llm.started → llm.completed`
   carrying model, version, runtime, latency and outcome, and no inputs and no
   outputs. Two runtimes load, `weights` and `onnx`, and the split is the
   design: the hardened half is the same for every framework and a runtime is
   four members. Where an artifact describes itself, the package's declaration
   is **cross-checked** against it rather than trusted. `file://` and signed,
   bounded `s3://` meet behind `ArtifactReader`; the persistent cache is keyed
   by immutable version and digest, admits only verified bytes and evicts LRU.
   An optional shadow label loads through the same gates; mirrored work has its
   own no-queue concurrency bound, its answers are discarded, and its health
   window resets per candidate version.

20. **A curation may be a chain of blocks, and each block belongs to the engine
   that can run it** ([ADR_0024](docs/ADR/ADR_0024_CURATION_BLOCKS.md)).
   ADR_0014's answer is right while the whole curation is one query. Detecting
   personal data in a hub corpus is not: something has to read the text, and
   the Flow surface admits no way to — a query composes values and never reaches
   arbitrary code. That line is narrower than this repository first drew it:
   arithmetic, a group's median beside a row and a join were all called
   "the far side of the seam" when they were only missing from a hand-written
   list. They are in the query now (ADR_0008, amended); a notebook is for what
   the language has *no vocabulary* for, which is a model or a scanner. So a pipeline is
   `source → transform → notebook → view`, with an `approval` wherever a gate
   belongs, saved as a content-addressed revision beside the recipes, and
   **the panel drives it** — every source and transform
   compiles to one Flow query, its rows go to a marimo notebook the
   `ml_pipeline` service runs, and the view publishes a dataset version carrying
   `produced_by`. The chain's rules live in `aiwatcher-datasets` and a refusal
   carries every problem at once. A notebook is *one file doing two jobs*:
   `App.run(defs={"rows": …, "params": …})` injects the rows for a step, and the
   same file served as a live app reads what the last run staged, so the widgets
   move against the rows the block will actually run on.

21. **A managed execution is owned by the server, and the browser only asks for
   one** ([ADR_0025](docs/ADR/ADR_0025_MANAGED_EXECUTION.md)). ADR_0024's own
   Consequences named what would make it wrong — a chain that has to run
   unattended, on a schedule, or over a corpus too large for one browser
   session — and all three arrived at once, from planner's four-stage import,
   `ai_spirit_agent`'s graphs and the personal-data corpus. So a definition
   compiles to an immutable `ExecutionPlan` addressed by `plan_id` over the
   *executable* fields only, a run pins one plan, `decide` is pure, and one
   workflow input is six writes in one transaction behind a `WorkflowStore`
   port — `memory | file | postgres`, the pattern already set twice. The
   `file` adapter holds one process and says so by name, so a development
   store never becomes a production one by omission. ADR_0024's blocks, chain
   validation and content-addressed revisions all stand; what is withdrawn is
   "the chain is driven from the browser", for managed runs only.

22. **The execution engine is a producer on its own log**
   ([ADR_0026](docs/ADR/ADR_0026_ENGINE_AS_PRODUCER.md)). The question ADR_0016
   deferred, decided in favour. `execution.*` and `Subject::Execution` join the
   catalog with `forms_span = false`; a started plan publishes
   `workflow.declared`, an attempt publishes `step.*` with `data.published_by`,
   a result publishes `artifact.produced` with a digest. The workflow fold, the
   waterfall, `Pending`, the live SSE and VictoriaTraces then draw a managed run
   with no second read path — and the PostgreSQL projection is for accepting the
   next command, never for a list the fold already serves. Facts, never
   decisions: the *why* stays in the store.

23. **A deployment chooses its query engine, and a typed query is admitted or
   runs where code runs** ([ADR_0028](docs/ADR/ADR_0028_QUERY_ENGINES.md)). Over a
   5 GB corpus Flow took 548.8 s where DataFusion and DuckDB took about two, and a
   managed run waits on its slowest step. So `AIWATCHER_QUERY_ENGINE` is `flow |
   datafusion | duckdb`, one per deployment, each its own `RuntimeKind`; content
   names the engine it was written for, absent read as Flow so no stored digest
   moves; and a plan for another engine is a 422 at start naming the block and both
   engines. The two Python engines run each query in a child of a fork server that
   imported the engine and never ran one, under `open` admission (the default: the
   query is code, run as a notebook's cell is, with ceilings and no credentials) or
   `strict` (parsed and admitted from the engine's own vocabulary, ADR_0008's shape
   in Python). One contract — the six `/query` routes, one `catalog.json` — and a
   conformance suite asking every engine the same four questions.

## Conventions

### Rust

- MSRV 1.98, edition 2024, pinned in `rust-toolchain.toml`. The floor comes from
  the `laser` feature: `laser_sdk` 0.3.1 requires rustc 1.98.0.
- `cargo clippy --workspace --all-targets --all-features -- -Dwarnings` must pass
  clean. `unwrap`, `expect` and `panic` are warned against in production code and
  allowed in tests (`clippy.toml`).
- Domain errors are typed (`thiserror`); only the binaries use `anyhow`.
- Adapters wrap transport failures in `ports::PortError` and set retryability
  correctly: `Unavailable` is retried, `Rejected` is dead-lettered. Getting this
  backwards either spins forever or discards good data.
- Tests are named as sentences that state the behaviour
  (`a_redelivered_event_does_not_double_count_tokens`), not
  `test_dedup`. A failing test name should explain the bug.
- Integration tests under `tests/` need
  `#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]` — the
  `clippy.toml` allowances only reach `#[cfg(test)]` modules.

### Module layout

Two crates are sliced by noun rather than by layer, and the rule that decides
where something goes is the same in both: **a change to what one thing *is*
should touch one directory.**

In `aiwatcher-annotations`, `images/` owns everything about one picture and
`registry` is a facade that resolves a project and delegates. The slice never
looks a project up — every operation takes an already-resolved
`AnnotationProject`, which keeps "does this project exist" answered in one
place and stops an import of six hundred rows resolving it six hundred times.
Slice operations are `pub(crate)`; `Registry` is the public API.

`license` is a module rather than three scattered types because it is one
question. `UsageRights` is what somebody *asserted*, `RightsPolicy` what an
export *demands*, and `SourceUsage` what a human *recorded at the original* —
and only the third outranks a caller. They used to sit in three files with the
rule connecting them in a fourth.

In `aiwatcher-api` every module is a **facade**, and the facade is the whole
contract: `pub fn router()` and `pub fn openapi()`, and nothing else. Handlers
are private, the `#[derive(OpenApi)]` that lists them sits beside the router
that serves them, and neither `routes` nor `openapi` names a single handler —
they compose facades. Two things follow. Adding a route and forgetting the
contract is now a change to one file rather than to two, and
`every_module_facade_reaches_the_document` fails when a module has a perfectly
good router and openapi and is simply missing from `ApiDoc::document()` — the
one failure this layout introduces, where both halves compile and the panel's
client has no method for a route that serves traffic.

The one asymmetry is `components(schemas(...))`, which stays in the root. An
OpenAPI components block is a single global namespace and these types come from
the *domain crates* rather than from the API modules, so splitting them would
mean picking an owner for `RunSummary` between `runs`, `workflows` and `live` —
a choice with no right answer that would be re-made every time a type gained a
second reader. A module owns its operations; the vocabulary they speak is
shared.

`integrations/` is the one grouping in either crate that is not a product area:
it holds what the crate reaches *out* to. Everything else answers from the log,
the read model or the object store; these leave the building, with a timeout, a
credential and a third party whose answers are data rather than truth. Both
`aiwatcher-annotations` and `aiwatcher-api` group hubs there for that reason,
and a reader of `routes::router` can see which routes leave at a glance.

### Contract

`contracts/openapi.json` is generated from the axum routes and the panel's
TypeScript client is generated from it. After changing any route or any type
that appears in one, run `just openapi` and commit both the contract and
`apps/panel/src/api/generated`. CI fails if either is stale.

### Python SDK

`sdk/python` is a `uv` project with its own `pyproject.toml`, and its
dependencies are **split by half**. The telemetry client — `aiwatcher_sdk`
itself, which is what an instrumented agent imports — depends on nothing and
stays on `urllib`: it is imported into processes that already pinned `httpx`
and `pydantic`, and it must never take an agent down. The four **registry**
clients depend on `httpx` and `tenacity`, because they run in training jobs and
deploy steps rather than in a request path, every method raises, and the retry
policy is the thing most worth writing once — `aiwatcher_sdk/api.py` is that
one place. `uv.lock` is committed. `just sdk-check` runs `ruff format --check`,
`ruff check`, `mypy --strict` and `pytest`; CI runs the same on Python 3.13,
which is the floor `requires-python` claims. The lint set is the one `planner`
selects, deliberately: the two repositories are worked on together, and a lint
that fires in one and not the other is a lint people learn to ignore.

`sdk/agentic` is the **third** distribution, `aiwatcher-agentic`: what agents
are built from, moved from `ai_spirit_agent` (AW-2) — the workflow engine
(`aiwatcher_agentic.workflow`: messages, deciders, event stores, sagas), the
agent core beside it (`Agent`, tools, messages, prompt builders), and the
runtime that composes and hosts them (`aiwatcher_agentic.runtime`:
`AgenticRuntime`, its stores, `hosted`, and the transport on which a hop
between two agents is a run of the target's workflow). Nothing in
`aiwatcher-sdk` imports it, so importing telemetry never imports an agent; the
runtime reaches aiwatcher through `aiwatcher-sdk`, imported only when used —
the `[aiwatcher]` extra. The engine is the standard library alone and the core
adds `structlog` and `orjson`; what none of them may bring is a model stack, so
where they need one they declare a port — `model.ModelSource` for a model,
`tracer.LLMTracer` for a tracer, `prompts.PromptSource` for a prompt read by
name, `runtime.config.RuntimeSettings` for settings, `AgentRun` for what the
engine reads off a turn — and the adapters (`providers`, the MLflow tracer and
registry, the `.env` settings) stay with the application. `test_agent_port` and
`test_runtime_port` fail if importing either pulls a stack in. A store handed
no path goes under the working directory, never beside the code — derived from
its own file, it named this checkout. Its records keep the wire names they had
before the move — `serialization.WIRE_PREFIX` — because stored rows carry them,
and `tests/fixtures/` holds them to those bytes. `just agentic-check` runs the
same four checks on its own lock.

The telemetry client and the registry client have **opposite** failure
policies, and that is the design: telemetry must never take an agent down, so
`HttpTransport` swallows and counts; reading the prompt a service is about to
run on is the work, so every `PromptRegistry` method raises.
`aiwatcher_sdk/integrations/deepeval.py` never imports deepeval — it reads the
report structurally, so a DeepEval release is not an SDK release. The same rule
holds for torch, with one deliberate exception:
`ExportDataset.as_torch_dataloader` imports it *inside the method*, because
handing back a `DataLoader` is the one thing that cannot be done structurally.

An annotation export is read the shape PyTorch reads a dataset.
`data_registry.build_dataloader(project)` freezes the project and hands back
the loader — the `Export` with the registry attached, whose `source` is the
`project@sha256` a run records and whose `excluded_samples` say what was left
out and why. `dataloader.get_split("test")` is a `SplitView` — a
`Sequence[Sample]` that also answers `get_groups()` and `get_counts()` — and
`.as_dataset(...)` turns it into the map-style dataset a `DataLoader` takes,
through the registry the loader remembers being read from rather than one the
caller repeats. Everything on that path pickles, because a `DataLoader` with
workers puts it across a process boundary under `spawn`. The split is where the
group rule already lives, so nothing downstream gets a second chance to decide
which subjects a model may see.

**Registry clients are named by one rule, and a new method follows it.** A
method is a verb phrase: a read is `get_<noun>`, `iter_<noun>` when it pages, a
conversion is `as_<noun>`, and `build_<noun>` asks the server to make
something; a field or a property is a noun; a collection is named for what it
holds (`samples`, `excluded_samples`). Writes keep the verb that says what
they do. A bare-noun method — `split()`, `families()`, `counts()`, `job()`,
`policy()` — reads like a field and has to be looked up to find out that it is
a request, which is why none is left. The telemetry client's scopes
(`client.run`, `run.agent`, `agent.llm`) are a different idiom, a `with` block
that names what it opens, and are deliberately not covered.

Two more rules hold everywhere in `sdk/python`. **A class name never starts
with an underscore** — internal means absent from `__all__`, and a leading
underscore is the same statement made more weakly, which then leaks into every
annotation that mentions it (`list[_Context]` in a neighbouring module is a
private name crossing a module boundary in public). And **a `@contextmanager`
is annotated `Generator[T, None, None]`, never `Iterator[T]`**: the decorated
function is a generator, `contextlib` throws exceptions back into it at the
`yield`, and the three-argument spelling is written out: `Generator[T]` needed
PEP 696 defaults while the floor was 3.11, and the full form stayed the house
spelling when it rose to 3.13, which is why ruff's `UP043` is ignored.

`aiwatcher_sdk/annotations` is a **package sliced by noun**, and the slicing
rule is the Rust one above: a change to what one thing *is* touches one file.
`errors` → `split` (the rule that deals a group a side) → `sample` (`Sample`
and `ExcludedSample`, the two halves of a manifest) → `image_source` → `view`
(`SplitView`) → `export` (`Export`) → `registry` (`AnnotationRegistry`, the
only file that knows a network exists), with `__init__` as the door that
re-exports all of it. Every import points up that list. The one thing holding
it straight is `image_source::ImageSource` — the three reads a dataset needs, a
project's schema, one revision's shapes and an image's bytes. A manifest
carries the source it was read from and the registry hands back manifests, so
written against the concrete client those two would import each other; written
against the protocol they meet at the abstraction. It is also what makes a
cache, an offline corpus or a test double substitutable for the client without
inheriting from it, which `test_a_dataset_reads_through_the_port_rather_than_through_the_client`
is there to keep true.

`aiwatcher_sdk/serving` is the one thing here that reads a decision back out and
acts on it rather than recording one: it resolves the `production` label and
serves what it names. Its shape is a split — `serving/server.py` holds what
every framework needs (resolve, verify, warm, bound, validate, watch, roll back,
report) and `serving/runtimes/` holds what one framework needs, which is four
members and a loader. That is why a runtime is cheap to add and why the rollout
exists once. `weights` needs nothing; `onnx` needs the `[onnx]` extra and is
imported lazily, so a host that registers no ONNX loader never sees the wheel.
Its gates are tested against a stub session rather than the real one — they are
pure functions of what a session says about itself, and `just onnx-version` is
what runs a real graph.

### Panel

- `apps/panel/src/api/generated` is generated. Never edit it by hand. It and
  `routeTree.gen.ts` are in `.prettierignore`: their generators emit their own
  formatting, and `just fmt` reformatting them would fight `just openapi`.
- Runtime validation belongs only where codegen cannot reach — the SSE and
  WebSocket frames, in `src/lib/live.ts`. Everything the generated SDK returns is
  already typed.
- Filters live in the URL, not in component state, so a link to a filtered view
  lands the reader on the same view. That includes the search boxes: the input
  holds a draft, a 250 ms debounce commits it to the search params.
- Routes are grouped by product area, and the areas are grouped into **three
  sections** — Feature, Training, Inference — described once in
  `src/lib/navigation.ts` and drawn by the root layout as a header of sections
  and a sidebar of that section's areas and their pages. The split is the
  lifecycle: Feature is what a model learns from and reads an object store,
  Training is fitting it and judging it, Inference is what is running now and
  folds the event log. `/` and `/observability` redirect rather than render, so
  old links keep working. Which section is lit comes from the pathname, never
  from a click, so a run detail reached from a pasted link lights the same tab
  as one reached by pressing it; a path in no section lights nothing rather
  than the first. An area layout route holds only what its views *share* — the
  stream, in `observability.tsx` — because a tab row there would be the
  sidebar's links a second time, free to disagree the day a page is added to
  one of them. What survives a move between an area's views is that area's own
  `NavArea.carries`: the period in Observability, the project in Annotations.
- `observability/live` is the one view that reads the log rather than a fold of
  it. Its filter is applied by the server — `Scope::Selection` and the repeated
  `?agent=&runtime=&workflow=&session=` parameters — because `llm.chunk` is
  most of the log by volume and narrowing in the browser would mean receiving
  all of it to discard it. What it *cannot* filter on is model and tool: those
  are span-level facts assembled from several events (ADR_0003), so the page
  says which parts of a selection it is not following rather than going quiet.
  Its feed is bounded and Pause freezes the rendering, never the subscription.
- The Query view has a **Build** mode that compiles clicked attributes into the
  deployed engine's language — Flow, DataFusion or DuckDB — and the move to
  **Write** is one-way. `query-builder.ts` generates; the engine decides
  (`services/query/flow/src/Dsl` for Flow, admission for a Python engine). Parsing
  text back into chips would be a second grammar in TypeScript for languages whose
  real ones live in the engines, and the day they disagreed opening a hand-written
  query in the builder would silently rewrite it.
- `training` is the one area that reads nothing folded from the log at all. It
  polls while a run is `running` and stops when none is — an epoch is minutes,
  so five seconds costs one request and answers the same question a live
  channel would. It draws no progress bar: nothing knows how many epochs a run
  intends, and a bar that guesses is a bar that lies.
- `prompts` is the one area that reads something other than the log, and the
  one that writes. It answers 501 rather than 404 when no store is configured,
  and `RegistryDisabled` says which variable is unset — an empty list would be
  a different problem with a different fix. `datasets` and `annotations` share
  that store and that component, because one setting decides all three.
- `datasets` is the one area that reads a service aiwatcher does not run. Its
  Discover view searches Kaggle and Hugging Face, and renders the mirror's
  licence claim and aiwatcher's verdict as two separate things — never one
  badge, and the rights selector is never pre-filled from a hub's word. A hub
  nobody configured renders the 501 with its variable, not an empty list.
- `conversations` is the one area that shows content, and the one where a role
  decides whether it is shown at all. Its list decrypts nothing — every badge,
  count and finding comes from the plaintext head — and a turn's words are
  fetched one at a time by an explicit click, which the API answers only for an
  `admin`. A 403 there renders as "reading content needs the admin role" rather
  than as a failure, because it is not one. It draws a progress bar for a
  running export and Training draws none, and the difference is honest: an
  export's denominator is a conversation list that was pinned when the job was
  created, while nothing knows how many epochs a run intends.
- `annotations` gained a fourth view, **Imports**, and it is the one screen
  here where the refusals come before the successes. An import of six hundred
  thousand pictures that registered four hundred thousand looks, from a success
  response, exactly like one that worked; the counts by reason and the rows
  behind them are the whole story. It draws a progress bar and Training does
  not, for the same reason Conversations does: the pages were counted when the
  batch was sealed, so the denominator is a fact.
- `data-curation` is the one area that orchestrates. Its **Pipeline** view runs
  a chain across three systems from the browser — Flow PHP, the notebook
  runtime, the Rust registry — and reports each block as it goes, including the
  three that light up together because Flow executes them as one query. Both
  engines are optional and their absence is a badge, not a failure. Its
  **Recipe** view is ADR_0014's single-script editor, still the right tool when
  the whole curation is one query.
- `data-curation` is also the one area with a *managed* path beside its ad-hoc
  one: **Run on the server** saves the revision, calls `POST /api/v1/executions`
  and the browser may close (ADR_0025) — which is why both the pipeline's name
  and the execution's id live in the search params, and why a reload comes back
  to the same canvas following the same run. Beside it is the **schedule**: when
  that definition runs unattended, with `Save and run now` for the case where it
  should also go once immediately. It renders `next_run` and never computes one. It follows that run with the stream that
  already exists — `openWorkflowStream`, because a managed run's facts carry the
  execution as their `workflow_run_id` — and the stream is only the *signal*:
  every frame re-reads `GET /executions/{id}`, whose projection was written in
  the same transaction as the decision. Nothing here parses a frame's payload,
  and the waterfall is a link to the Workflows view rather than a second
  drawing of one run. The ad-hoc path stays, because a preview and a block at a
  time are what an editor needs; the two empty states say which of the two ran,
  since "nothing run yet" under a card reading `completed` is a contradiction.
- `annotations` is the one area that draws. Its canvas puts an `<img>` and an
  `<svg>` in one transformed container, both sized to the image's *natural*
  pixels, so SVG user units are image coordinates and no shape ever carries a
  zoom level. Stroke widths and vertex radii divide by the zoom, or the plan
  disappears under ink as soon as somebody zooms out. The draft lives in
  component state and is saved explicitly: a revision is content-addressed, so
  autosaving every vertex drag would mint one per mouse move. The canvas
  implements no validation — the registry's 422 carries every problem, and a
  second rule set in TypeScript would drift from the first.
- Any list that can grow with retention is a `useInfiniteQuery` feeding
  `VirtualList` (`src/components/virtual-list.tsx`). A `.map` over a full
  response is only correct for a list with a fixed ceiling.
- Every list that can grow with retention also carries the time window
  (`src/components/time-range.tsx`), in the URL as `window` seconds and served
  by the API as `window_seconds`. One control, one preset list, one default
  across every tab: a period that means the last hour in Explore and something
  else in Metrics is a control people re-read before every click. It carries
  across the observability sub-navigation and nothing else does — having
  narrowed to fifteen minutes, "now the metrics for it" is the next question.
- An area that exists in the navigation before it exists in the backend renders
  `AreaPlaceholder`, which names what is missing. Never mock data to fill a
  screen — a plausible fake reads as working software.
- `src/components/ui/primitives.tsx` holds the shadcn-style primitives in use
  (button, badge, card, stat, id chip). Radix is not a dependency yet — none of
  those need it. It goes in with the first dialog or select, and TanStack Form
  with the first form, which will be the WebSocket control path (cancel a run,
  approve a tool call).

### Agent skills

`.claude/skills/` holds reference material an agent loads on demand, vendored
at pinned commits rather than fetched — `.claude/skills/README.md` says which
nine are there and what each one earns its place with. Four are
OpenTelemetry (instrumentation, semantic conventions, the Collector, OTTL),
one is Rust, two are the panel's TanStack Query and Router, and two are the
Hugging Face hub this repository already searches.

None of them is about *this* repository. This file and the ADRs under
`docs/ADR/` are that, and a skill restating either would be a second copy free
to disagree with the first. `just skills-check` says whether the tree still
matches `vendor.json`; `just skills-update` moves the pins, and the diff is
the review.

## Guardrails

- **Never let a process claim work it cannot perform.** The claim filter is
  built from the `ExecutorRegistry`, so a process with no `AIWATCHER_FLOW_URL`
  registers no Flow executor and never takes a `flow_php` attempt. The
  reactor's "no executor for this runtime" branch is defensive rather than
  reachable, and a runtime whose client would not build is one this process
  claims nothing for rather than one it fails every attempt of.
- **Never let a worker decide anything but its own function.** The reactor's
  loop is claim → load the plan → cache lookup → `step.started` → **perform** →
  re-check the lease → record → report, and only `perform` crosses the HTTP
  seam. `Reactor::take`, `settle` and `resume` are that split, so the worker
  routes call the same code the in-process reactor does. A worker written in
  another language re-implementing any of the rest — which cache entry answers,
  when a retry is due, whether its lease still holds — is the drift the seam
  exists to prevent, and the last of those is one a claimant cannot check about
  itself at all.
- **Never authorise a worker route by the name it claimed under.** Worker names
  are not secret, so holding the lease and the token's own queue scope are
  checked *together* in `worker::held`. Either alone is a hole: without the
  lease a token settles any attempt on its queue, and without the scope a token
  for one queue settles another's by guessing the name that claimed it.
- **Never let a queue widen a token.** `AIWATCHER_AUTH_INGEST_TOKENS` grew
  `name[queue other]=secret`, and the guardrail below it is amended rather than
  dropped: the role is still hard-coded to `Editor`, and a queue only ever
  *narrows* what may be claimed. A producer's token names none and claims
  nothing. Neither does a person's session or an OIDC identity, whatever the
  group mapping says — claiming takes a lease something has to renew, and a
  browser is not that something. The queues sit in the label because the secret
  is split off with `split_once` and may contain an `=`.
- **Never accept an artifact reference a worker described rather than wrote.**
  Rows go through the attempt's own `outputs/{name}` route, which digests what
  it stored — the prompt registry's rule — and the result route checks every
  reported output exists before it settles. A completed step pointing at an
  object that 404s is the one failure nothing downstream catches.
- **Never presign a bucket to a process outside the cluster.** A worker runs on
  somebody's laptop, and a presigned URL is a bearer credential for a store that
  also holds prompts, datasets, annotations, conversations and training, scoped
  by a prefix and a clock. `core::ports::AttemptArtifacts` proxies instead,
  because the route can check the one thing that matters: that the caller holds
  the lease on the attempt whose bytes it is asking for. The cost — every byte
  through the API process — is stated rather than hidden, and a presigned path
  for in-cluster workers is an addition behind the same port when it is measured
  to be the problem.
- **Never derive a message id from less than what it identifies.** A reactor's
  report id *is* the inbox key. Derived from the execution and the event name
  it is unique for a one-step plan and collides for a two-step one — the second
  step's `step_started` reads as a redelivery of the first's, the decider never
  hears about it, and the step sits `pending` behind a lease nothing releases.
  It names the execution, the step, the attempt and which of the two facts it
  is. Section 43.10.
- **Never derive a command's message id without the run's version.** The same
  rule, arriving from the other side. `execution + command name` makes a
  double-click idempotent and makes a Pause *after* a Resume a redelivery of
  the first Pause — accepted, deduplicated, and the run keeps going with a
  success on the wire. `RunProjection::last_message_version` is the
  discriminator. Section 43.20.
- **Never list an action whose command would be refused.**
  `ContextSnapshot::allowed` carries `Retry` only from `Failed` or `Crashed`
  and not while the run is cancelling, and `Answer` only while a step actually
  holds a question — each mirroring `decide`'s own precondition. An action
  offered where the command 409s is a button that does not work, which is worse
  than an absent one. Cancel, pause and resume are not there at all: they are
  done to a run, and that is a block's context.
- **Never store a finished attempt as a row.** A settlement is the claim row
  ceasing to exist — `AttemptWrite::Retire`, a `remove` in two adapters and a
  `delete` in the third. The older shape wrote a terminal row, which meant
  blanking `command_id`, `queue`, `task_ref` and the holder to store a record
  that described nothing, and left the file adapter reading and `fsync`ing the
  whole history on every claim: 458 ms over fifty thousand rows, unbounded
  because retention is opt-in. Nothing reads a finished attempt back — a
  redelivery is recognised by the stream's inbox key, and a takeover reads
  `previous_owner` on a row that is still live. `awaiting_input` keeps its row,
  because it is not an ending. Section 43.34.
- **Never leave a parked attempt holding its lease.** `AttemptWrite::Park` is
  the third shape and the one that changes a row rather than adding or removing
  one: the row stays, because a question is not an ending and the attempt that
  asked has to stay readable, and the lease goes, because a worker that stopped
  to ask does not hold a pod for the answer. Left running, the lease expires
  and the next claimant runs the work again having been told nothing about the
  question — 43.34 reserved `awaiting_input`'s row for exactly this and nothing
  wrote one until now. Only a step the plan *dispatched* gets one: a
  `HumanInput` step is parked by the decider when it schedules it and reaches
  the claim table never. Section 43.40.
- **Never let an answer leave the row it parked.** A park keeps its row because
  a question is not an ending; an answer *is* one, so `InputProvided` retires it
  in the same decision that dispatches attempt *n+1* under its own key. The
  attempt that asked is read back from the stream and the projection, never from
  the claim table — the rule that retires a finished attempt, which the resume
  walked straight past. Left behind, the table gains a row per park for ever,
  which is the claim table growing with the history that rule exists to prevent,
  and the `file` adapter pays for it on every claim and every heartbeat.
  Section 43.40.
- **Never ask the plan what only the question knows.** A gate has two authors —
  the plan for one it declared, a running attempt for one it chose to ask — and
  a `PythonTask` spec has no `on_timeout` to read. So the policy rides on
  `InputRequest`, copied from the spec when a gate is scheduled, with the plan
  kept only as the fallback for streams written before the field. Resolved from
  the plan alone, a lapsed deadline on a parked attempt was refused as
  `NoSuchStep` and the run stayed parked for ever behind a clock that had
  already passed. The cancel side is the same mistake from the other end: it
  is read from the decision's own facts, because `evolve` clears `awaiting` on
  the very events a cancel follows. Section 43.40.
- **Never let an answer be a step's result unless the step was the question.**
  `ProvideInput` completes a `HumanInput` step, because that step *is* the
  question and there is nothing else for it to do. An attempt that stopped in
  the middle of its own work is the other case: the answer is one input to the
  rest of it, so it schedules attempt *n+1* and the attempt that asked stays
  immutable. The resumed attempt re-runs from the beginning — the rule every
  retry already lives under — so the answers are kept on the **step** and
  accumulate: attempt two replays to the first question and must read it rather
  than ask again, and a task that asks twice needs both by attempt three.
  Section 43.40.
- **Never decide what an answer does in two places.** A person answers through
  `ProvideInput` and a lapsed deadline answers through `OnTimeout::Answer`, and
  what an answer *does* is one question — so both go through `answer_lands`.
  Written twice, the two disagreed as soon as the first was corrected: the
  timeout completed the step, leaving a parked worker attempt `Completed` at the
  attempt that never finished and with no outputs, so anything bound to its rows
  read nothing. What follows is part of the same rule: `continue_after` belongs
  to the arm that actually ended the step, because a shared one declares the run
  finished over a step `Answer` has just re-scheduled. Section 43.40.
- **Never cache a step somebody answered.** A human decision is addressed by
  nothing, so a step that has one no longer has all its inputs addressed —
  `cache_key`'s own rule. Two runs of one step answered differently would share
  a key, and the second would be served the first one's rows without ever
  seeing its own answer. `None`, rather than a key that means "probably the
  same". Section 43.40.
- **Never let a measurement cost a run.** The scheduler reports its lateness and
  its backlog *after* it has started the slots, and a sink that is down is a
  warning rather than a failed tick — the cursor still moves, because whether a
  graph was written says nothing about whether the interval was read. Lateness
  is measured to the tick that *found* the slot and never to the moment the run
  started, which would fold the store's latency and the compiler's into a number
  about the clock; and it is reported beside the backlog from the same tick,
  because one slot four minutes behind and forty of them are the same lateness
  and very different news. Section 43.41.
- **Never report a number for a disk this process cannot see.** A notebook's
  staged rows live in `services/ml_pipeline`, keyed by a hash of the context in
  that process's own scratch directory, and nothing here can list them — so the
  staging figure is `GET /ml-pipeline/staging` and not a zero reported beside
  the artifact totals. The artifact half is this binary's, hourly, and a count
  rather than an opinion: whether an object is still reachable is a question
  about streams retention has already been deleting. Section 43.41.
- **Never put a run's timings in the workflow store.** When an execution
  started and ended is the log fold's answer — with `duration_ms` — and an
  attempt's is the span assembler's, from `step.*`. `RunProjection` and
  `AttemptRecord` carried four such fields, written by nothing and read by
  nothing; filling them in would have been the second answer, not the fix. The
  projection is for accepting the next command and for the run's own page.
  Section 43.33.
- **Never build a second live view of one run.** ADR_0026 puts a managed run's
  facts on the log carrying the execution as `workflow_run_id`, and
  `/api/v1/workflow-executions/{id}/stream` scopes by that field — so a managed
  execution has been streamable since the first one ran. Section 20's
  `/executions/{id}/stream` is struck rather than built, and
  `every_fact_a_managed_run_publishes_is_reachable_by_the_execution_id` is what
  keeps the reason true. Section 43.21.
- **Never decide whether a scheduled run may start from the read model.** It is
  an asynchronous fold and it is *empty in the `work` role*, where the tick
  runs — `bin/aiwatcher.rs` ends that path before the projector starts — so
  `overlap = skip` never skipped there, and in the combined role it read a
  projection that lagged the start it was meant to block. `admit_slot` asks the
  workflow store, in the transaction that takes the slot, against the
  projection written by the decision itself. Review R1.
- **Never write a transient failure down as a decision.** A refused compile
  says the same thing on the next tick; an unreachable store does not. The tick
  wrote both as `refused` and then advanced a global cursor past the slot, so
  ten seconds of unavailability at 09:00 cost the day's run and left a note
  claiming it had been refused. `SlotSettlement::TryAgain` drops the lease and
  leaves the slot due, and the caller draws the line from the status the API
  gave it — 4xx is about the definition, 5xx is about reaching something.
  Review R2.
- **Never let the tick write a schedule's configuration.** It read every
  schedule, did its work and wrote the whole object back, so an edit or a
  DELETE landing in between was overwritten by the snapshot — a deleted
  schedule came back enabled. A `get` before the `put` does not close it: an
  object store offers no compare-and-set. Configuration and slot outcomes have
  different writers and now live in different places, and the tick is handed a
  `ScheduleReader` so widening that trait is what a future change has to do
  first. Review R3.
- **Never resolve a local time by the offset of the instant that found it.**
  Twice a year a wall-clock time is ambiguous or does not exist, and an offset
  read from the sampling instant answers whichever the sample happened to land
  on — which made `slots_between` depend on the tick rate. Daily 02:30 in
  Warsaw on 2026-10-25 fired once over one interval and twice over the same
  span in twenty-minute ticks: two instants, two derived ids, two runs of one
  day's intention. Candidates are enumerated by **local calendar date** and
  each is resolved with a policy stated per cadence — daily and weekly take the
  first of an ambiguous pair and the instant the clocks reach for a skipped
  one, hourly lives a repeated hour twice and a skipped one not at all. Review
  R5.
- **Never let a new schedule version reach into the past.** The tick hands
  every schedule the whole interval its checkpoint accumulated, so one written
  while the worker was down for three days would run three days of slots the
  moment it came back. `effective_from` bounds it — and it is **not**
  `updated_at`, which is the review's own warning: an edit that does not change
  *when* it fires would then silently drop a slot that was already due.
  `Schedule::fires_the_same_as` decides, over cadence, timezone and enabled
  only, so switching `overlap` at 08:59 keeps nine o'clock and re-enabling
  starts from now rather than running the days it was off. Review R7.
- **Never write the derived files of one decision without journalling it
  first.** The `file` adapter touches five — stream, projection, outbox,
  attempts, checkpoint — and a filesystem writes one at a time. It used to
  write them in sequence, which failed in a way no successful-path test could
  see: a crash after the stream left the input's message id recorded with none
  of its consequences, and because that id *is* the inbox key, the retry was
  answered `Duplicate` over a run with no outbox row to publish and no attempt
  to claim. `PendingCommit` is written whole and `fsync`ed first, that rename
  is the commit point, everything after it is idempotent, and the record is
  deleted only once it has all been applied. `recover` runs at `open` **and** at
  the top of every `append` — the second is not belt-and-braces, because A1's
  reproduction never restarted anything. Review A1.
- **Never hold a single-process lock by a file's existence.** The lock is the
  operating system's, taken with `File::try_lock` on the open file, because the
  kernel releases it however the process ends. `create_new` plus a `Drop` that
  removes the file is exactly the code a `SIGKILL` does not run: after one, every
  later start refused a store no process was holding and the only way out was to
  delete a file by hand. And the file is never unlinked while held — that lets
  the next process create a second inode and lock *that*, after which two
  processes each hold "the" lock.
- **Never remove in one release what the release before it names.** The schema
  is applied at start-up by whichever replica gets there first, workers roll
  rather than stop, and an image rollback runs the old binary against the new
  schema. So a removal is two releases: one that stops using the column, and a
  later one that drops it. NULL in every row makes the *data* safe to lose and
  says nothing about the query still naming it — 0003 dropped two dead columns
  the previous release names in both of its projection statements, and 0005 put
  them back. And an applied migration is never edited: a version is recorded
  once and skipped forever after, so a rewrite reaches no database that already
  ran it and only makes two installations at one version disagree about what
  that version did. What withdraws a migration is another migration. Review R4.
- **Never split the binary in two without sharing all three backends.** The
  workflow store (`postgres`), the log the outbox publishes to and the projector
  folds (`laser`), and the object store one role writes a step's result into for
  the other to read (`s3`). `Config::validate` refuses each by name, because two
  of the three fail silently and the third fails three attempts later with
  "holds no object". Section 43.11.
- **Never put the projector in the `work` role.** It *is* the read model the API
  answers from, in process, under `AIWATCHER_MAX_SPANS_TOTAL`'s memory contract.
  A `serve` role without it answers every read from an empty fold. Moving the
  folds out of process is Phase 8, behind its own gate.
- **Never let a lookup's two answers become one question.** A runtime says
  whether it is *still executing* a key — nothing else can know that. The object
  store's receipt says what the finished attempt *produced* — the query service
  keeps no rows, and ADR_0014's refusal of an S3 client for it stands. `done`
  with no receipt means the query finished and its rows never landed, and the
  only way to get them is to run it again.
- **Never write a step's receipt before its data.** `aiwatcher_jobs::ORDERING`,
  in the fifth place it applies. A crash the right way round leaves bytes
  nothing points at, which the next attempt overwrites identically; a crash the
  wrong way round leaves a completed step whose artifact 404s.
- **Never compare a runtime's digest with an artifact's.** They are digests of
  two different encodings by two different languages, and making them agree byte
  for byte is a cross-language contract over row data that nobody could keep.
  The comparison that means something is the runtime's answer against the digest
  *recorded in the receipt*: it says whether the stored bytes are this run of
  this key.
- **Never make a managed execution depend on an open browser tab.** ADR_0025.
  The panel authors, commands, links and renders; it does not compile, sequence,
  retry, resume or publish. `lib/pipeline.ts`'s `orderOf` stays a *traversal*
  for drawing and never an explanation of a refusal, and a managed run's Flow
  script is compiled in Rust — the browser may show the same text, and what runs
  is what the server produced.
- **Never let a scheduler decide what is due.** The tick supplies an
  *interval* — where the last one stopped, and now — and `Schedule::slots_between`
  answers what fell in it, purely. That inversion is where catch-up comes from
  (a slot missed during an outage is simply inside the next interval) and why
  the tick rate is an operational choice rather than a correctness one. A loop
  that asked "is it 09:00?" would answer no at 10:05 and lose the day's run with
  nothing to say so. Section 43.31.
- **Never let a schedule fire without writing down what happened.** The tick
  records the slot, the outcome and the reason on the schedule head — what it
  logged before was a warning nobody reads, and a schedule refused every morning
  for a week looked from the panel exactly like one that had been working. What
  it records is the *scheduler's* decision and never the run's outcome: whether
  the run succeeded is the log's answer, one click away by the id beside it, and
  a second copy would be free to disagree with the fold. Written after the run,
  so a stored `started` always has one behind it. Section 43.32.
- **Never work out in the panel when a schedule next fires.** `next_run` comes
  from the server, from `Schedule::next_after` — which is `slots_between` over
  eight days rather than a second walk, so the hour a card shows and the hour
  the tick fires at cannot differ. The same rule as `RunView::allowed`, and with
  a sharper failure: a second implementation would have its own idea of when the
  clocks change, and the first hour it disagreed on would be one somebody
  planned a morning around.
- **Never let a schedule and its tick reach different compilers.** Two kinds are
  schedulable now — a curation pipeline and a registered workflow — and the
  route that agrees to *save* a schedule compiles the definition first, so a
  name nobody saved is a 404 today rather than a failure at nine tomorrow. The
  tick compiles it again when the slot comes due. Both go through
  `executions::compile_head`, because a `match` on each side is two answers to
  "what does this schedule run": the day a third kind arrives, one of them
  starts a run and the other refuses to save the schedule for it. The kind comes
  from the route's own path and never from a body — the store is keyed by kind
  and name, so a pipeline and a workflow may share a name, and a body carrying
  its own kind would write a schedule no page would ever show.
- **Never put a clock tick on the event log.** The reference this is taken from
  publishes `MinuteHasPassed` on a bus; here the only subscriber is the
  scheduler and the log is the durable one every projector folds. What is
  durable instead is the tick's *cursor*, a Unix second in
  `processor_checkpoints` — the same table and the same question the projector's
  answers.
- **Never give a scheduled run an id that is not its slot's.** Derived from the
  definition and the slot, so two workers that both find 09:00 due produce one
  execution and one conflict — no lease, nothing to expire. Never from the
  compiled plan: two workers reading the head a moment apart compile different
  `plan_id`s, and both runs would go through. And the cursor advances *after*
  the starts commit, so a crash between them repeats rather than skips.
- **Never let a schedule mint a definition revision.** It is a mutable head
  under its own `schedules/` prefix, keyed by definition kind and name. Changing
  nine to ten is not a new pipeline, any more than `produced_by` is part of a
  dataset version's identity — and the prefix is its own because
  `aiwatcher-datasets` owns `pipelines/` and two crates writing one prefix is
  what the private-key-layout rule exists to prevent.
- **Never flatten a body that denies unknown fields.** serde's `flatten` and
  `deny_unknown_fields` do not compose: every field the flattened struct owns is
  reported as unknown. The bodies here deny unknown fields so a `cron` somebody
  hoped would work is a refusal naming it rather than a field silently ignored,
  so the nested shape is the one that keeps the guardrail.
- **Never let `decide` read a clock, open a socket or generate a random
  value.** Time arrives in `Now`, ids are derived from what they name. That is
  what makes a replay reach the same schedule and the same command id, so a
  redelivered dispatch lands on the attempt it already created instead of
  beside it — `TraceId::derive`'s rule, one layer up. A jittered retry delay is
  the dispatcher's, applied when it schedules.
- **Never split the six writes of one workflow decision.** Deduplicate the
  input, check the expected version, append the outputs, update the projection,
  write the outbox, advance the checkpoint — one transaction, or the dual-write
  gap. An outbox row with no decision behind it publishes a `step.completed`
  for an attempt the store does not consider complete; a decision with no
  outbox row is a run the panel never sees finish.
- **Never publish an execution fact before the decision that caused it
  commits.** ADR_0026, and `aiwatcher_jobs::ORDERING` in the fourth place it
  applies. The outbox publishes after commit, never from the handler.
- **Never publish a decision to the log.** Facts about work go on it —
  `workflow.declared`, `step.*`, `artifact.produced`, `execution.*` — and
  commands, retries scheduled, leases and heartbeats stay in the store. An
  engine that published per decision would flood the log it observes.
- **Never let two parties publish one attempt's `step.*`.** A `local`
  execution's reactor publishes for what it ran, a worker for what it ran, an
  `engine:` execution's pods for theirs and the engine for none.
  `data.published_by` makes a second publisher visible; the resolution is that
  a managed step's producer code does not open its own `node()` scope.
- **Never run a managed execution that needs two processes on the `file`
  store.** `StoreCapabilities::multi_process` is `false` there and the refusal
  names `AIWATCHER_WORKFLOW_STORE`. A file offers no compare-and-append across
  processes, and a workflow stream has a decider, a reactor and a worker racing
  to append — so a second process is refused at `open` rather than allowed to
  interleave writes that each look fine alone.
- **Never run a notebook a plan did not pin.** A managed step names its
  `code_revision` and the runtime resolves *that* source, from the history it
  keeps under `.revisions/<name>/<sha256>.py`. What it may not do is fall back
  to the head when the revision is missing — running something else under a
  pinned run's name produces rows that look exactly like a successful run. The
  revision is still checked twice: a GET before anything executes, where the
  runtime recomputes the digest from the stored bytes, and a comparison after
  against what the subprocess imported. `UserCode`, so neither is retried. A
  cache *hit* re-checks nothing, and that is correct: the key holds the pinned
  revision. Section 43.24.
- **Never make an edit strand the runs that came before it.** This read the
  *head's* digest once and refused a run whose pin no longer matched, which
  protected provenance by making every earlier execution unrepeatable — and a
  retry is not something anybody can perform on a run that already happened.
  The history is what replaces it, and the two orderings it keeps are
  ADR_0011's: the revision is written before the head that names it, and
  `keep_current` at start-up keeps what a directory holds *now*, because what
  it held yesterday was never written down. `.revisions` is the one durable
  thing that service has; `.data` beside it is staging and is scratch.
- **Never let a staged file be keyed by the notebook alone.** Two pipelines
  using one notebook then overwrite each other's rows, and a block that passed
  its preview reads somebody else's table on the next one. A run stages under
  its context and *then* points `latest` at it — that order, because the live
  app knows only a notebook's name and follows the pointer. The context is
  hashed into a directory name rather than sanitised: it holds separators.
- **Never let a runtime's own knowledge be assumed by the caller.** Whether a
  Flow query named `now()`, whether a notebook samples or asks a model — only
  the thing that ran it knows, and both report it on the answer for
  `ActivityResult::cacheable` to read. A notebook says `deterministic = False`
  beside its `output`; absent means true, because a curation block normally is
  one and the other default would make every chain pay for the exceptions.
- **Never let a plan name its own executor's address.** `AIWATCHER_FLOW_URL`,
  `AIWATCHER_ML_PIPELINE_URL`, `AIWATCHER_FLYTE_ENDPOINT`, the pod's service
  account. A `PlanStep` names a binding and its parameters, never a host —
  ADR_0012's and ADR_0016's reasoning, unchanged, and for the same reason the
  rerun target is configuration.
- **Never retry the same work in two places.** The owner of an execution owns
  its retries: Rust for a `local` run's steps, the engine for what it was handed
  whole, the store for a hosted decider's *attempt* and never the worker's own
  loop as well. A `ContainerJob` sets `backoffLimit: 0` for the same reason.
- **Never make `plan_id` depend on where a block sits.** The authored revision
  digests the whole request, positions included, because that is what somebody
  saved and what `produced_by` names; `plan_id` digests the executable fields
  only. A canvas tidy-up must invalidate no cache and start no different run.
- **Never cache a step whose inputs are not all addressed.** A moving window, an
  unpinned notebook, a `file://` with no digest — `cache_key` returns `None`
  rather than a key that means "probably the same". Caching is opt-in for the
  same reason: claiming a step is a pure function of digest-addressed things is
  wrong often enough to be worth saying out loud. A pinned window counts because
  the query service reads one: `POST /flow/query` takes `window_from`/`window_to`
  and the API's windowed routes take `as_of`, so a plan that pinned 09:00–10:00
  and a retry five minutes later read the same rows. Sections 43.15 and 43.18.
- **Never decide from the request what only the runtime can answer.** Whether a
  cache key is *well defined* is `cache_key`'s question; whether the run that
  produced a result happened under those conditions is the executor's, on
  `ActivityResult::cacheable`. An older query service that never learnt `as_of`
  reads a drifting window and says so by omission — its rows are produced,
  reported and not remembered. Section 43.18.
- **Never let a window mean "the last hour" to one reader and a span to
  another.** `as_of` is absent for every panel query, which is what keeps a
  shared link meaning the hour it is opened in; a managed step pins it, and only
  then is the window a closed span two reads agree on.
- **Never let a policy field have no reader.** `CachePolicy::ByContent` sat on
  every compiled Flow step for a phase while `cache_key` was called only by its
  own tests — a claim the code did not keep, and a latent unsoundness no test
  could catch because no caller existed. Wiring it is what found the bug.
- **Never forget a run the outbox is still speaking for.** A pending row is a
  fact that has not reached the log; deleting the decision behind it leaves the
  publisher a message with no explanation and the log a gap nothing records.
  `aiwatcher_jobs::ORDERING` in a sixth place, and the second half of
  `store::prunable`'s candidate test. Section 43.25.
- **Never let retention decide a run has died.** `prune` takes terminal
  executions only. A run with no end is `Running` and age tells an OOM kill from
  a twenty-minute think in neither direction — the projector's rule for agent
  runs, in a second store. And it deletes the whole execution together: a kept
  projection whose stream is gone is a run the panel lists and cannot open.
- **Never delete an execution's history by default.**
  `AIWATCHER_WORKFLOW_RETENTION_DAYS` is unset unless a deployment says
  otherwise, and `0` means keep rather than delete. The stream is the
  *explanation* of a run and the one thing the event log does not carry, so this
  is the only copy — the conversation archive's default, for the same reason and
  in a different store. The window also has a floor nothing in the crate can
  check: the inbox goes with the stream, so it must outlast the log's own
  retention or a redelivery is decided again instead of recognised.
- **Never let one adapter decide what "finished long enough ago" means.**
  `store::prunable` is the rule; `last_activity` is the only thing an adapter
  supplies, from whatever it already holds — the last recorded message, a file's
  modification time, an indexed column. A property proving a running execution
  survives would otherwise prove it about one adapter. The same reason
  `StateType::TERMINAL` exists rather than a list of states written out in SQL.
- **Never keep an outbox row the log has accepted.** `mark_published` deletes.
  The fact is on the event log, which is the durable copy and the one every fold
  reads; a second copy answers no question and grows with every step of every
  run. The `file` adapter rewrites the whole outbox on each publish, so
  remembering was quadratic. Section 43.16.
- **Never spend the budget for attempts at the work on a runtime that declined
  it.** `Transient` is a refused connection or a 503 — nothing ran, so running
  it again costs one call and gets ten attempts over ten minutes. `Timeout` and
  `Infrastructure` may have done the work, so they keep three. Counted by kind,
  because one budget of three sized for a job shard killed a run over a
  forty-second outage. Section 43.17.
- **Never put rows, notebook source, a prompt, a completion or an agent's
  inter-node text in a workflow message.** A step hands data on as an
  `ArtifactRef` and its answer as a bounded inline value. The last of those is
  conversation content and belongs in the archive with a retention clock —
  ADR_0021's rule, in a second store.
- **Never let a Flow PHP block read past something it cannot read.** A Flow step
  reads its rows by naming a dataset in the query service's catalog, so the one
  query a chain compiles to ends at the first block that is neither a source nor
  a transform — a notebook, or an approval. A chain that put a transform behind
  either would silently run it against the *source* again and produce something
  else. One rule rather than one per kind: it used to be called "no transform
  after a notebook", which is the same rule under a name narrow enough that the
  second case looked like a new one. The registry refuses it by name, and the
  message says which of the two is in the way rather than "invalid".
- **Never author one gate twice.** A curation block and a registered workflow's
  step both put a question in front of a person, compile to one
  `RuntimeBinding::HumanInput` and are answered through one route, so what a
  valid question *is* lives once — `aiwatcher_core::human_input`, above both
  crates, taking only the word each surface calls the thing it is refusing. The
  panel keeps the same rule from the other end: one `AnswerGate`, used by the
  pipeline's run card and by the Workflows view, because "which answers may be
  pressed and by whom" is one question.
- **Never give `decide` a vocabulary for a timer.** A deadline is a consequence
  of a question having been asked, not a decision of its own: `InputRequested`
  carrying one schedules the row and anything ending that step retires it, both
  derived in the handler and written in the transaction that already holds the
  decision. What `decide` does is resolve the *instant*, from the clock that
  arrives in its input — so a replay reaches the same moment, which is
  `TraceId::derive`'s rule for a moment rather than an id. The row's id names
  the step **and the attempt**, because a retry asks the question again and the
  first attempt's row must not fire on the second; and only a step the plan gave
  a clock to is cancelled, because a `Cancel` per ending step is a write per
  step per transaction that the `file` adapter pays for by rewriting its table.
- **Never let the engine's own delivery go through the caller's door.** The
  effect-command guard in `handle` is about *who is asking*: no caller may post
  an `ExecuteStep`, a `RequestInput` or a `TimeoutInput`. The engine firing a
  deadline it scheduled is not a caller, and it uses
  `ExecutionHandler::deliver` — crate-private, one call site, and everything
  after it identical. Widening the guard instead would have made a timeout
  postable over HTTP.
- **Never record a timeout as an answer somebody gave.** `on_timeout: skip`
  completes the step with **no** `InputProvided`, so the history says the
  question was asked and never answered; `answer` records one attributed to
  `aiwatcher/timeout`. And `skip` is not `StepSkipped` — that event marks what
  will not run because a parent failed, and a run whose steps are not all
  `Completed` never completes, so a gate skipped that way would leave the run
  open for ever.
- **Never let an authored gate name a role the answer route does not check.**
  A gate only ever *raises* the floor, and `provide_input` is the one command
  route with a rule of its own: the editor floor is held for every command, and
  then the role the **question** named — from the pinned plan, through the
  step's own `awaiting` — is required on top. That check cannot live on the
  route, because which role answers is a fact about the step. So an authored
  gate admits `editor` and `admin` and refuses anything weaker **by name**: a
  gate promising a viewer may answer would offer buttons to somebody the floor
  is about to refuse. The panel asks the same question from the other end and
  keeps "nobody has answered yet" apart from "no" — `useRoleDecision`, because
  a refusal rendered while the session is still being read is a refusal nobody
  issued.
- **Never let a step's edge and its data binding be one cursor.** An approval is
  in the chain without being in the data: it reads the rows before it, produces
  nothing — answering *is* its completion — and the block after it reads those
  same rows, so that block is bound to a step that is not its parent.
  `resolved_inputs` resolves by step id rather than by adjacency, which is what
  makes that legal. One cursor bound the publisher to an output no step
  declares: a dataset version over no rows, which is the one failure that looks
  like a success.
- **Never issue a token to a service that cannot check one.** §16.3 asked an
  editor session to carry permissions, expiry and a signature; the notebook
  runtime has no authentication at all, so a signed token presented to it would
  be ceremony rather than a boundary. The gate is the route that mints the
  session — `Editor`, because staging replaces what everybody looking at that
  notebook's live app is shown — and aiwatcher reads the rows from its own
  object store rather than telling the runtime where to find them. A boundary
  drawn where nothing enforces it is worse than an honest absence: it reads as
  protection.
- **Never run a notebook to fill its editor.** `EditorHost::open` stages and
  stops. Executing would run somebody's code because they clicked "open", and
  would overwrite the output of the run being looked at. And what opens is the
  notebook's **head** — marimo serves the notebook root and the history is kept
  out of it deliberately — so the session names the revision that ran, and the
  code itself is read beside it by digest. Two facts side by side rather than
  one that quietly conflates them.
- **Never work out in the panel which block became which step.**
  `GET /executions/{id}/blocks` answers it from the pinned plan, and
  `RuntimeBinding::blocks` is the one place that knows which specs carry one —
  so the forward lookup and the reverse map cannot come to disagree. A browser
  deciding it would decide from the draft on screen, and the interesting case
  is exactly the one it would get wrong: the compiler folds a source and every
  transform behind it into a single Flow query, so three boxes light from one
  `step.started`. The mapping is immutable, so it is asked once per run rather
  than re-sent with every frame and every command.
- **Never draw a run's outcome on a canvas that is not what it compiled.**
  `followsTheRun` compares the run's `definition_revision` with the revision the
  draft was last loaded or saved at, and `undefined` — an edited draft, whose
  content address the browser cannot compute — reads as drift. A false drift
  costs a line of prose; the other direction claims an outcome for a block that
  never ran. Its three answers are not two: no managed run at all is
  `undefined`, and an edited draft is the ordinary state of working rather than
  a warning about something being wrong.
- **Never let the panel reconstruct a block's context.** Every part of the
  answer is somewhere the browser is not — the pinned plan, the artifacts a
  parent produced, the attempt a staging key is named after — so a canvas that
  guessed would guess from the draft on screen, which is certainly not what an
  old run read. `ContextSnapshot` is the answer, and it carries the plan's own
  `RuntimeBinding` rather than a second description of it. Section 19, 43.19.
- **Never put a service's address in a context's actions.** `allowed` says
  *which* actions apply, because only the server knows the state; where they
  live is the panel's own routing or the generated client's. And an action whose
  route does not exist is not listed — a retry nobody can perform reads as a
  feature.
- **Never decide in the panel which commands a run would accept.**
  `allowed_run_actions` answers it beside `ContextAction::allowed`, and both
  ride back with the thing they describe — `GET /executions/{id}` and every
  command route return a `RunView`, the projection *and* what may be done to
  it. `state.is_terminal()` in TypeScript is three lines and a second copy of
  `decide`'s preconditions in another language. Section 43.27.
- **Never keep a managed run's id out of the URL.** ADR_0025's claim is that
  the browser may close, so a run held in `useState` is a run a reload loses —
  the panel's own URL-state rule, in the one place where it is load-bearing
  rather than a convenience. And the way back to an old run is
  `GET /api/v1/workflow-executions`, which folds the log: a list over the
  inline projection is the second read path ADR_0026 forbids. Section 43.27.
- **Never let the generated client's default decide whether a call worked.** It
  does not throw: a 403 comes back as `{ data: undefined, error }` and the
  promise *resolves*, so a mutation that returns the SDK call runs react-query's
  `onSuccess` over a refusal — the run re-read, nothing changed, nothing said.
  Every call goes through `lib/result.ts`, whose three readers are named for
  what absence means on that route: `answerOf` where there is always a body,
  `answerOrNone` where "no such thing" is an ordinary answer, and `confirmDone`
  where success carries no body at all. That last one is not a nicety — a
  successful DELETE is a 204, which the client turns into `{}`, so "is there
  data" answers yes for the refusal and yes for the success alike. Review R6.
- **Never draw a failed read as an empty state.** 404 is the server saying there
  is no such thing; a 501, a 503 or an expired session is the server saying
  nothing usable, and rendering the second as the first tells somebody their run
  was forgotten or their schedule never existed. `answerOrNone` is the split:
  `null` for the first, an `ApiFailure` for the second. And what follows an
  empty state must not follow a failure — the run card offers *Forget it* only
  when the run is really gone, and the schedule form disables Save while the
  read has failed, because the alternative is writing this component's own
  defaults over a schedule nobody has seen.
- **Never read a body from aiwatcher without checking the status first.** In
  `services/query/flow` the pipeline is lazy, so by the time `array_get(__body,
  'rows')` runs there is no status left to branch on — a 501 naming an unset
  variable arrives as `Path "rows" does not exists`. `CheckedClient` throws at
  the seam, carrying aiwatcher's own message, and a permanent answer is relayed
  as a 4xx so a managed step reads it as `UserCode` rather than spending ten
  attempts on a flag that is still off. Section 43.28. The Python engines' pager
  (`aiwatcher_query.api`) reads the status before the body for the same reason.
- **Never re-implement a pipeline's rules in the panel.** `aiwatcher-datasets`
  decides whether blocks form a runnable chain and returns every problem as
  `details` on a 422; the canvas renders those lines. `lib/pipeline.ts`'s
  `orderOf` is a *traversal* — it answers "in what order" and `null` when there
  is no one order — and it never explains a refusal. Same split as the
  annotation canvas and the shape validator, for the same reason.
- **Never hold a notebook's source in the block.** That file is what marimo
  serves, what `ml_pipeline.step` imports and what a test reads. The block names
  it and pins the `sha256` it was saved against; the panel says when the two have
  drifted. A copy in the registry would be a second source of truth for a file
  that has to stay runnable on its own — and the *history* is not that copy: a
  revision is named by the digest of its own bytes, so it cannot disagree with
  anything. The head answers "what runs next"; a revision answers "what ran".
- **Never let a notebook's injected cell define anything else.** `App.run(defs=)`
  replaces a whole cell, not one name, so the cell binding `rows` and `params`
  binds nothing downstream needs — imports go in a cell of their own, and what
  only that cell uses is underscored. `ml_pipeline.step` catches marimo's own
  message for this and answers it, because that message names the missing
  definitions without saying what to do about them.
- **Never run a notebook on the event loop.** `run_notebook` is a blocking
  `subprocess.run`, and called from an `async` handler it held every other
  request for the length of the run — measured at 657 ms of a 704 ms run. Two
  things depend on it not doing that: `GET /ml-pipeline/executions/{key}` has to
  be answerable *while* a notebook runs, which is the only case it exists for,
  and marimo's live app is served by the same process for the panel's iframe.
  Section 43.30.
- **Never let a runtime be asked only about what it stored.** A receipt says
  what a finished attempt produced; only the runtime knows whether it is *still
  executing* a key, and after a timeout that is the question. Both runtimes
  answer it now — and the notebook one is asked **best effort**, because the
  receipt is durable and the memory is a fifteen-minute window: failing to reach
  the volatile source must not hide the answer that outlives it.
- **Never expose the notebook runtime.** It runs notebook code with no sandbox,
  in its own process for a live app and in a child process for a step, and it
  has no authentication. It binds to `127.0.0.1` and is a development surface —
  the same posture as the Flow service, and a sharper reason.
- **Never make `produced_by` part of a dataset version's identity.** A block
  dragged across the canvas is a new pipeline revision and the same rows, and a
  dataset version per canvas tidy-up would be a version history about layout. It
  is provenance, like the recipe name and the description — and it matters
  because the Flow script alone does not describe an execution a notebook ran
  after.
- **Never let a route decide for itself whether it needs a caller.** The
  authentication layer is applied once, in front of the whole router in
  `routes::router`, with an exception list in `auth::is_public` — the health
  probes and the sign-in routes, which cannot require a session in order to
  establish one. A route added later is then authenticated by default rather
  than by remembering to say so, and the one somebody forgets is not the one
  that leaks. What the layer does *not* decide is whether a caller may perform
  the operation: that is a `Role` check in the handler, because the answer
  differs per handler and a table of paths in a middleware drifts from the
  routes it guards.
- **Never accept `AIWATCHER_AUTH_MODE=proxy` without a network boundary.** In
  that mode a header is a claim, so any pod that can reach port 8080 can assert
  it is an admin. The chart refuses to render it without `networkPolicy.enabled`
  and says why. `oidc` is the mode where the identity is proved to this process
  rather than asserted to it, and it is what a deployment that needs a real
  boundary uses.
- **Never put conversation content on the event log.** It was there once, as
  `data.input` and `data.output` on `llm.completed`, and it wrote somebody's
  words into the durable log the Collector's redaction exists to keep them out
  of. `conversation.turn` is not in the event catalog and adding it means
  re-reading ADR_0021. The archive names the log — `run_id`, `trace_id`,
  `span_id`, `model`, `prompt` — and the log does not know the archive exists.
- **Never retain conversation content by default.**
  `AIWATCHER_CONVERSATION_ARCHIVE` is off unless a deployment says otherwise,
  and it is the only default in the configuration chosen so that doing nothing
  keeps nothing. A release that started holding content on an upgrade is the
  failure ADR_0021 is about. The routes answer 501 naming the variable, never
  an empty list.
- **Never run the archive without a key.** `AIWATCHER_CONVERSATION_KEYS` is
  required whenever the archive is on, and the server refuses to start without
  it — because an archive with no key is a plaintext archive in a bucket that
  prompts, datasets, annotations and training all already read. Object-store
  encryption is not a substitute: it protects the disk, and every process
  holding the bucket's credentials still reads the content in the clear.
- **Never put content in a turn's head.** The head is plaintext by design so a
  review queue, a finding count and an export's exclusion report need no
  decryption; the body is sealed. A finding therefore carries a part index, a
  byte range and a rule id and *never the text it matched* — a finding that
  quoted the secret it found would put that secret in every list response.
- **Never authenticate a sealed object by its ciphertext alone.** The object's
  key path is HKDF `info` and the AEAD's associated data, so a ciphertext copied
  from one turn to another does not open. Without it, anyone who can write to
  the bucket substitutes one person's words for another's and every digest still
  checks out — the plaintext digest is of a real message, just not that one.
- **Never let a turn's approval survive an edit.** Re-sending the same
  `message_id` with different content resets the review to pending. Carrying it
  across is how reviewed text becomes unreviewed text with a tick beside it.
  A *human's* findings do survive a re-scan, because a scanner replacing a
  reviewer's judgement is the same mistake in the other direction.
- **Never infer a preference pair from a review rejection.** A rejection has
  several reasons and only one of them is "the other answer was better".
  `TurnReview::preference` is a separate, explicit field, and a DPO export pairs
  only siblings a reviewer actually labelled — otherwise a turn rejected for
  holding somebody's address becomes the rejected half of a pair and puts that
  address in the corpus.
- **Never ship an unsafe-output classifier and call it a scan.**
  `conversations::redaction::scan` matches credential and identifier *shapes*
  and nothing else; `FindingKind::Unsafe` exists so a human can record one and
  is never produced by the scanner. A keyword list would produce a green tick
  nobody should trust, and the whole reason the review gate exists is that this
  judgement is not automatable. The same reasoning rules out an entropy
  heuristic: at any threshold that catches real keys it also catches base64
  images, and a reviewer who has learned to dismiss findings dismisses the true
  one.
- **Never advance an export's cursor before its shard is stored.** Same
  ordering as the pipeline's checkpoint and the prompt registry's head, and the
  sharpest consequence of the three: a crash the right way round re-does one
  shard and writes byte-identical bytes, and a crash the wrong way round leaves
  a corpus missing rows that nothing can tell you about.
- **Never erase a turn and leave the corpus that already has it.** An erasure —
  and the retention sweep, which is the same problem arriving more quietly —
  deletes the shards of every published corpus whose pinned conversation list it
  touched. The manifest survives with its counts and digests, so the reference
  still answers; only the rows are gone, and the answer is a 410 rather than a
  404. Stopping at the archive would be an erasure in name only.
- **Never write an export shard without re-checking the lease.** A worker
  claims a job under its own name for five minutes and renews with every shard;
  `interrupted` re-reads the record at each boundary and stops the worker that
  no longer holds it. Two deterministic workers over an unchanged archive
  converge on identical bytes, so this is not about the common case — it is
  about the archive changing under them, where the last job record written
  would name shard digests that do not describe the stored shards, and the
  version would stop being a content address of anything.
- **Never let a conversation export decide it is finished early.** A job that is
  cancelled or fails has no manifest and therefore no version, which is what
  stops an interrupted export appearing as a completed dataset. The shards it
  wrote stay written and are re-read by the resume; they are never indexed.
- **Never let an ingest token be more than an editor.** `AIWATCHER_AUTH_INGEST_TOKENS`
  is a shared secret sitting in an agent's environment. It exists because a
  producer reaches the Service directly, never passes the ingress that
  authenticates a browser, and cannot complete an interactive sign-in — not so
  that a leaked environment file can ask an orchestrator to run something. The
  role is hard-coded in `IngestToken::identity` and never comes from the group
  mapping. Amended, not widened, by the queue scope above: `name[queue]=secret`
  adds what may be *claimed* and takes nothing away.
- **Never take the issuer from the discovery document.** `ProviderMetadata::discover`
  compares what the document declares against what was configured and refuses a
  mismatch. Every token accepted afterwards is validated against that issuer, so
  believing the document would hand the choice to whoever answered the request.
- **Never pick a JWT's algorithm from its header alone.** `oidc::algorithms_for`
  derives the permitted set from the *key* and uses the header only to narrow,
  and refuses a symmetric key in a provider's key set outright. Taking `alg`
  from the header and the key by `kid` is the confusion attack where an RSA
  public key is handed back as an HMAC secret.
- **Never start serving when the identity provider could not be reached.**
  `Authenticator::connect` retries while it comes up — in a cluster the two
  start in whatever order the scheduler picks — and then fails the start-up.
  The only other thing an instance could do is serve unauthenticated, which is
  the failure the whole crate exists to prevent.
- **Never widen the session cookie.** `HttpOnly` keeps it out of JavaScript,
  `SameSite=Lax` is what survives the redirect back from the provider (`Strict`
  drops it and the sign-in loops with no error anywhere), and `Secure` is
  derived from the redirect URL's scheme rather than defaulted, because a
  `Secure` cookie is simply not stored over http and an instance served that
  way would sign people in and then behave as though nobody had.
- **Never follow a `next=` that is not a path on this application.**
  `auth::safe_next` refuses anything that is not one leading slash — an open
  redirect on a sign-in route is how a phishing link gets to start on the real
  host.
- **Never commit the checkpoint before the durable write succeeds.** That single
  ordering in `pipeline.rs::flush` is the at-least-once contract; reversing it
  turns a crash into silent data loss.
- **Never store `llm.chunk` as a trace record.** See ADR_0003.
- **Never let an engine's address come from anywhere but configuration.**
  `AIWATCHER_FLYTE_ENDPOINT`, exactly like the rerun target and for exactly the
  same reason. `LaunchBody` is `deny_unknown_fields`, so a body naming its own
  endpoint is a 400 rather than a field that is ignored and reads as accepted.
  Every part of an `EngineRef` is checked against `[A-Za-z0-9._-]` before it is
  interpolated into the orchestrator's URLs — a launch plan name holding `../`
  would be a path traversal aimed at a system aiwatcher authenticates to.
- **Never let a launch carry an input the entity does not declare.**
  `Interface::bind` refuses it. An orchestrator that ignores unknown fields
  turns a typo in a filter into a run over everything, and the panel's form is
  rendered from an interface that may already be stale — which is why binding
  re-reads the interface from the engine rather than trusting what the caller
  was shown. A blank *optional* input is omitted rather than sent empty, so the
  launch plan's own default survives.
- **Never launch without pinning a version.** A reference with no version
  resolves to the newest registered one and that is what goes on the wire. An
  execution recorded against "whatever was current" is not something anybody
  can repeat, which is the entire point of recording it.
- **Never let `stage_hint` decide anything but what a picker shows first.** It
  is guessed from an entity's name — the name first, the description only as a
  tie-break, because "fine-tune on a **curated** dataset" would otherwise file
  a training job under curation. Presentation may depend on it; nothing else
  may.
- **Never poll the engine to fill in a run's status.** The engine's phase is a
  second opinion shown on a launch acknowledgement, never merged into
  `RunStatus`. When they disagree the disagreement is the finding: an execution
  the engine calls succeeded that published no events is a producer nobody
  instrumented, and a status column that quietly took the engine's word would
  hide exactly that. See also the guardrail below about the projector never
  deciding a run has died.
- **Never let a rerun target come from the log.** `AIWATCHER_WORKFLOW_RUNNER_URL`
  is configuration. A `workflow.declared` naming its own callback URL would be a
  request-forgery primitive posted by anything that can reach ingest — aiwatcher
  runs inside the cluster, so "POST this url" is a request to reach the
  cluster's internal network on the caller's behalf. `RerunBody` is
  `deny_unknown_fields` so an attempt to supply one is a 400 rather than a
  silently ignored field that reads as accepted.
- **Never wire a no-op workflow runner.** `NullExporter` is the right shape for
  telemetry aiwatcher already has and the wrong shape here: a null runner would
  answer `202 Accepted` for work no orchestrator was ever asked to do. Absence
  reaches the caller as a 501 naming the variable.
- **Never let a `workflow.*` event reach span assembly.** Same guard as the
  evaluation one, `EventType::forms_span`, and a different reason: a topology is
  a shape with no duration, and a waterfall showing one would be showing the
  moment a producer got round to describing itself. The node executions drawn
  against that shape are `step.*`, and those do form spans.
- **Never store an artifact's content.** The registry stores prompt text because
  storing it is the point; an artifact is a byte range somebody else already
  persisted, so aiwatcher keeps the pointer. A producer that inlines a
  scanned document into `data` puts it in the durable log and in every
  projector's memory. An artifact with no `uri` is dropped rather than listed
  as a row nobody can open.
- **Never draw an inferred edge as a message.** Declared edges are what the
  orchestrator promised; `agent.message` is what was said. `workflow-graph.tsx`
  keeps them visually distinct and never merges them, because sequence is not
  communication and the whole reason somebody opens that view is to find out
  whether the agents talk.
- **Never let an `eval.*` event reach span assembly.** The guard is
  `EventType::forms_span`, checked first in `SpanAssembler::ingest`. A report
  has a start, an end and a duration and is still not a trace: its payload is a
  document, and a twenty-minute batch job is noise in a waterfall. The phase is
  kept because the evaluation fold reads it.
- **Never compare two evaluation reports across datasets.** `baseline_for`
  matches on suite *and* dataset. Two scores measured on different cases are two
  facts; a delta between them claims they are one.
- **An evaluation report is not redacted.** The Collector strips
  `gen_ai.prompt` and `gen_ai.completion` from spans, and an evaluation forms no
  span, so nothing strips `data.report`. A producer that puts model output there
  is putting it in the durable log and in memory — a retention decision, made
  deliberately or not at all.
- **Never let a client decide whether an optimisation was an improvement.**
  `OptimizationRecord::verdict` computes it in `aiwatcher-prompts`, from the
  held-out scores and from `variables_lost`, and the API returns what it
  decided rather than what was sent. An optimiser selected its candidate by
  maximising the number it then reports; a registry that took its word is a
  filing cabinet.
- **Never admit a candidate on a dev score.** The dev split is what the search
  ran against — a gain there is a hypothesis. An optimisation with no held-out
  measurement is recorded and refused a promotion, which is the outcome the
  split exists to produce. `overfit_gap` is the number worth watching across a
  series.
- **Never promote a candidate that dropped a variable.** An optimiser rewrites
  prompt text freely, and one that has stopped interpolating `{{ page }}` can
  score arbitrarily well on a harness that fed it fixed inputs. The bar is
  checked *before* the scores in `verdict`, so the reason says "it stopped
  reading its input" rather than inviting somebody to raise the iteration
  count.
- **Never ship a label vocabulary.** aiwatcher is a generic vision annotation
  tool and the project's schema is where the domain lives — its classes, their
  geometry, which are `ignore`, and which `layer` each paints into. A shipped
  preset is not a neutral default: it decides what the first hour of labelling
  produces, it is what the panel renders, and it is what every example shows. A
  tool that ships one is a tool for that domain with an escape hatch. See
  ADR_0020, and the `floor_plan_classes()` it removed.
- **Never let a class erase what it overlays.** That is what `LabelClass::layer`
  is for. Classes on one layer share an integer grid and paint in declaration
  order; classes on different layers never contend, and a model reads one head
  per layer. An opening in a wall, a defect on a component, a marking on a road
  — in every case the thing underneath is still there, and one grid could only
  represent the overlay by deleting it. A schema that never sets `layer` gets
  one grid and never thinks about it.
- **Never let the rasteriser know a class name.**
  `aiwatcher_sdk.integrations.vision` is driven by the schema it is handed and
  matches on nothing: geometry decides fill or stroke, the class's own `ignore`
  flag decides exclusion, declaration order decides who wins a contested pixel,
  and `layer` decides which grid. It also *checks* the schema against the
  export's pinned `schema_version` — rasterising against a reordered vocabulary
  permutes every label, every metric stays finite, and nothing says so.
- **Never make a raster the source of an annotation.** The mask, the heatmap
  and the COCO document are all derived from the vector shapes and are
  regenerated on demand. Storing an edited mask beside the vector it came from
  is two sources of truth that will disagree, with nothing able to say which
  one is right. See ADR_0017.
- **Never split an annotation corpus by image.** The key is `group_id`, the
  building — one house published as the plain plan, its mirror, a garage
  variant and a re-drawn revision is four images and one observation. Splitting
  them apart makes the test score a measurement of memorisation, and nothing in
  the numbers says so. `export::split_for` hashes the family and the salt and
  *only* those, so adding an image never re-deals an existing family. There is
  no API that assigns a split per image.
- **Never let an image's usage rights be optional.** `UsageRights` has no
  default and `RightsPolicy` defaults to `commercial`, so the strict answer is
  the free one. Many of the best public corpora in any field are
  non-commercial, and a model trained on one by accident is a problem that
  surfaces in a legal review rather than in a metric. An export
  *excludes by name* rather than refusing, so the manifest records what it left
  out and why, forever.
- **Never let a model's proposal become a training target on its own.** Every
  shape carries `origin: human | model | import | ocr`, an export defaults to
  `require_human_review`, and a revision that is entirely machine output is
  excluded with the reason. Pre-annotation is what makes 300 plans affordable;
  what it may not do is produce labels nobody looked at.
- **Never take a content address from the client.** `put_blob` hashes the bytes
  it received and ignores whatever the caller claimed. A content address
  supplied by the caller would let two different images occupy one key, which
  is a training set whose labels belong to a different picture — the one
  corruption no metric detects. `AnnotationRegistry.fetch_image` verifies it
  again on the way out.
- **Never validate a drawing in two places.** The registry refuses an invalid
  revision and reports *every* problem at once, as `details` on a 422; the
  panel renders exactly those lines and implements no rules of its own. A
  second implementation in TypeScript would drift, and the day it does is the
  day somebody trusts the wrong one.
- **Never rename an annotation class in place.** The label schema is versioned
  by the content of its class list, and a revision names the version it was
  drawn against. Changing the classes excludes every earlier revision from the
  next export *by name*, which is the loud failure and the correct one: a
  rename that silently relabelled history would be undetectable afterwards.
- **Never put a training run on the event log.** It was there once. An epoch is
  not a span, a step does not belong on the log at all, a profiler session is
  not a trace and a checkpoint is not an artifact — which left one span with no
  children and a `Subject::Train` arm in the read model's status fold. Training
  has its own module, its own store and its own routes; `train.*` is not in the
  event catalog and adding it back means re-reading ADR_0018.
- **Never send a training step anywhere.** The SDK counts and averages steps
  locally and emits one epoch record. A 300-image run at batch 4 for 200 epochs
  is 15 000 steps; the same loop on a real corpus is millions, and a rule that
  only holds at the small size is not a rule. The finer series that does exist
  is rate-limited at the client and decimated at the server.
- **Never append a retried epoch.** `progress` replaces an epoch index it
  already holds. A network blip during a six-hour run must not produce a curve
  with two points at the same x, which reads as training that went backwards.
- **Never reuse a training run id.** Re-opening an *open* run returns it, so a
  retried start loses nothing; re-opening a *finished* one is a 409, because
  the second run would inherit the first one's curve.
- **Never store a profiler trace or a checkpoint's weights.** The trace is tens
  of thousands of records per step and the weights are hundreds of megabytes.
  `ProfileRecord` is the top operators and a URI; `CheckpointRecord` is a URI
  and what selected it.
- **Never let a model version claim provenance its run does not have.**
  `register_model` reads the dataset, framework and code *from the run it
  names*, and ignores whatever the request said about them.
- **Never promote a model on a validation score.** It is the number early
  stopping maximised, so promoting on it promotes the selection — ADR_0011's
  verdict rule, for weights. `ModelVersion::check_promotable` also refuses a
  mutable dataset name, and the two refusals are deliberately different
  sentences: one invites a held-out evaluation and the other invites an export,
  while "not promotable" invites neither. A version that fails either is still
  recorded, with the reason returned on the registration.
- **Never fit a model against an empty split.** `app/training/run.py` in
  planner refuses a run whose train, validation *or* test side is empty. The
  middle one is the trap: with no validation images every epoch scores zero,
  epoch 0 wins by default, the checkpoint is selected arbitrarily — and the run
  still reports a validation number the model registry accepts as the held-out
  measurement a promotion needs. A metric computed over nothing is worse than a
  missing one, because it looks like a metric. Three families cannot be dealt
  into three non-empty sides, and the message says so.
- **Never make a raster the input to a rasteriser.**
  `aiwatcher_sdk.integrations.vision` derives every grid from the vector shapes
  on demand and writes none of them back — ADR_0017's rule, expressed as code
  that only runs one way. Its z-order is the one ordering decision in it: rooms
  first, walls last, so a wall keeps the pixels it shares with the room it
  bounds. Reverse it and every wall between two rooms has a hole in it exactly
  where two rooms meet.
- **Never let the training registry decide a trainer died.** A run with no end
  is `Running` and `last_heard_from` is what it reports instead — the same rule
  the projector keeps for agent runs, and for the same reason: an OOM kill and a
  twenty-minute think are indistinguishable from here.
- **Never fetch a dataset licence from a mirror.** `sources` is a dated table
  a human wrote and an instance loaded (`AIWATCHER_DATASET_SOURCES`), and every
  row links its original. This build ships **no rows**: which corpora exist and
  what their licences permit is a question about one field, and an empty table
  is a working state — nothing outranks a mirror's claim, so every hub result
  stays `unclear`. Hugging Face,
  Kaggle and Roboflow Universe all restate licences wrongly often enough that a
  live answer would be worse than none, because it would arrive looking
  authoritative. The table is a signpost; the licence at the link is the
  permission. Searching those hubs is a different thing and is
  allowed — see the next three rules.
- **Never let a hub's licence field become a usage verdict.** `hubs::reconcile`
  starts every row at `SourceUsage::Unclear` and only a match against
  `sources::catalog` moves it. The mirror's own words survive verbatim in
  `claimed_license`, named for what they are, and the two are never merged into
  one badge in the panel. This is not hypothetical: the first live search
  returned `Voxel51/FloorPlanCAD` declaring `cc-by-sa-4.0` for a corpus whose
  authors state the drawings are not theirs to license.
- **Never match a corpus name by substring.** The example that produced this
  rule, from the first search that ever ran: `RPLAN` is a substring of
  `floorplans`, and a plain `contains` handed
  `wall-constrained-floorplans-manual-only` RPLAN's licence verdict — a
  permission claim invented by a coincidence of spelling. The rule is a whole-token match, with cross-separator
  matching allowed only from eight characters up. A miss is safe; a wrong match
  is a licence claim.
- **Never let an import assert rights the curated table contradicts.**
  `import::check_rights` refuses a commercial claim on a batch that matched a
  research-only corpus, and only that. Everything else is the caller's
  assertion, recorded with `UsageRights::Unknown` as the default — which a
  commercial export excludes by name, in a manifest, forever. Refusing an
  unknown-rights import outright was considered and rejected: it would teach
  people to claim a licence in order to get past the dialog.
- **Never fetch a byte outside `integrations::fetch`.** It is the only place in
  this system that downloads content an outside party chose, and it carries
  seven gates: https with the host *parsed* rather than matched, an allowlist,
  a public-address check on every resolved address, no redirects, a byte
  ceiling applied while streaming, a header-only pixel ceiling, and a verified
  content address. Both import routes go through the same `ImageSource` port,
  because a fetcher wired into one and not the other is the one somebody routes
  around. The gate that is easiest to under-rate is the redirect: an
  allowlisted host answering `302 → http://169.254.169.254/` walks past every
  check that ran against the address the caller named.
- **Never advance an import's cursor before its page's shards are stored.**
  ADR_0022, and the same ordering as the export's, the pipeline's checkpoint and
  the prompt registry's head — [`aiwatcher_jobs::ORDERING`] states it once. The
  counts move with the cursor for the same reason: a page that was registered
  and whose shard was never written *will be done again*, and counts already
  folded into the job record would then describe that page twice.
- **Never take an import's version from the batch id.** It is
  `sha256(batch content digest ‖ dry-run flag ‖ every result shard digest)`, so
  two people who staged the same rows on the same terms reach the same
  reference. A version derived from the id would change because somebody
  clicked twice, and would then be a content address of nothing.
- **Never let an interrupted import publish a manifest.** A cancelled or failed
  job has no version and no index entry; the images it registered stay
  registered, because an image id is the content address of its bytes and
  re-running writes the same ones. Same rule as a conversation export, same
  reason: an interrupted job must not appear as a completed dataset.
- **Never register a model package whose artifacts carry no digest.** ADR_0023.
  An address is not an identity: `s3://models/latest.pt` is different bytes
  tomorrow, and a version whose weights cannot be checked is a provenance chain
  with a hole in it. A *half* package — a declared runtime with an undigested
  artifact — is refused rather than accepted, because it reads as provenance
  and is not.
- **Never sniff a model's runtime.** A loader chosen by looking at the file is a
  loader chosen by whoever wrote the file. `Runtime` is declared,
  `Runtime::Unspecified` is refused rather than guessed at, and
  `Runtime::executes_packaged_code` is what a host answers *before* it opens
  anything — a package that runs its own code is never loaded in the API
  process, which holds the object store's credentials and every registry behind
  them.
- **Never trust a declared shape an artifact could be asked about.** An ONNX
  graph carries its own input and output names, element types and shapes, so
  `serving.runtimes.onnx` cross-checks the package's `inputs` and `outputs`
  against it and refuses a disagreement naming both sides. This is the one
  declaration in this system that is checked rather than believed, and the
  reason is that a wrong shape is not a typo: it means the package describes a
  *different model*, so the version's held-out score, its dataset lineage and
  its label order all belong to something else. The same check settles
  `classes` — `n` classes over a width-`n` head, or two over a binary one, and
  anything else is refused because nothing at load can tell a mislabelled head
  from a mistrained one.
- **Never let a serving profile discover at the first request what it could
  refuse at load.** `instances` is one rank-2 tensor with a free batch axis, so
  a graph with two inputs, an image tensor, a string input or a pinned batch
  dimension is refused *by name* when it is loaded, each naming the profile it
  would need. `runtime_version` is compared before the bytes are read for the
  same reason. "It loaded and every request 500s" is the outcome these gates
  exist to turn into a deployment decision, and by then the previous version is
  already gone.
- **Never make `preprocessing` executable.** It is what the trainer did in its
  own words, reported on `/v1/model` and applied by nothing. A package that
  shipped preprocessing *code* would be a package that runs code in whatever
  opens it, which is exactly what `Runtime::executes_packaged_code` exists to
  keep visible. The caller holds the raw input, so the caller is the side that
  must already have done it. `entry_point` is the opposite and *is* acted on,
  because it is read as a name in this package — an artifact's name or the last
  segment of its URI — and a value naming neither is a refusal rather than a
  guess.
- **Never put an inference's inputs or outputs on the event log.** The serving
  profile reports `run.started → llm.started → llm.completed` carrying model,
  version, label, traffic, rows, latency and outcome — a primary or shadow
  invocation is a model call, so it joins the same traces and model dimension
  as everything else. What it never carries is what was said. A runtime that
  wants to retain that writes turns to the conversation archive, with consent
  and a retention clock, exactly as an agent does. See ADR_0021 and ADR_0023.
- **Never let a broken new label remove a ready old version.** The rollout is
  two-phase: download, verify and warm the candidate while the current version
  keeps serving, and swap only if all three succeed. The previous version stays
  loaded, so a rollback needs no rebuild and no fetch — and the version being
  left is pinned out, because a rollback the next poll undoes is not a
  rollback.
- **Never derive an import's `group_id` from the file name.** It is the
  *building*, and a per-file key silently turns the family split back into a
  per-image one — after which the test score measures memorisation and nothing
  in the numbers says so. The import route cannot prevent it, so it reports it:
  a batch whose every row is its own family comes back with a warning on a
  response that succeeded.
- **Never write a prompt's head before the version it indexes.** Same ordering
  as the pipeline's checkpoint, same reason: an index naming an object that was
  never stored is a list whose rows 404, while an unindexed object is waiting
  for `Registry::rebuild`. The head is derived; the versions are the truth —
  except the labels, which exist nowhere else and survive a rebuild. The
  annotation registry keeps the same two orderings for the same reason: the
  revision object before the image head that indexes it, and the export
  manifest before the index entry that lists it.
- **Prompt text is not redacted.** The Collector strips `gen_ai.prompt` and
  `gen_ai.completion` from spans; the registry stores prompt text verbatim,
  because storing it is the point. A producer that puts a secret in a prompt is
  putting it in an object store nothing evicts.
- **Never put a prompt on a span, and never leave a call unable to name one.**
  Both halves are the same rule. The text stays off the log (ADR_0021, and the
  Collector's redaction); what a call carries is a *reference* —
  `prompt_name` and `prompt_version`, read by `PromptRef::from_data` and
  written as `aiwatcher.prompt.*`. Without it a trace could name a model, a
  temperature and a token count while the one thing that decided what the model
  was asked was the thing you had to go and find by hand, which made ADR_0011's
  promise — that the version a run used stays readable after the run is evicted
  — true of the registry and useless from a trace. A malformed version id is
  *absence*, never a rejection: the log takes what a producer sends, and the
  panel links only what resolves.
- **Never let a request setting have nowhere to go.** `gen_ai.request.*` covers
  temperature, top-p, top-k, max tokens, seed and stop sequences, and
  `request_attributes` reads every one off whichever of the start and end
  events carries it — `OpenSpan::close` keeps the first, because the Python SDK
  restates the request on both and twice is an exporter writing one fact into
  two rows. A setting that changes the answer belongs in an attribute rather
  than in a payload field nothing indexes: comparing two runs is the question,
  and a payload cannot be grouped by.
- **Never let a managed run go on looking alive because the engine used its own
  vocabulary.** A managed execution emits no `run.completed` — the engine owns
  the run and ends it with `execution.completed` (ADR_0026). The runs fold read
  `Subject::Run` only, so the Workflows tab said `succeeded` while Explore span
  beside it forever, and after fifteen minutes the same run was drawn as
  stalled. The guardrail above refuses to *infer* an ending from silence; this
  is not silence, it is the producer saying it finished. `execution.failed`
  says why in `reason`, which is a third word for it and one the fold now
  knows.
- **Never remove the `data.workflow` fallback in `EventEnvelope::workflow`.**
  The `agentic` integration sent the workflow name in the payload before
  `workflow_id` existed. Dropping the fallback empties the workflow dimension
  for every log written before the field, including on replay.
- **Never let query text reach a callable in `services/query/flow`.** The name selects
  a `match` branch, is never called by string, and there is no `eval` anywhere in
  the service. `tests/Dsl/ParserRejectionTest.php` is the list of things that
  must keep failing; adding to it is cheap and is the point.
- **Never give a transform a second representation.** `BlockSpec::Transform` is
  Flow DSL text and stays text (decision 15, settled). A structured model would
  be the enumeration of 43.22 one layer up — every transform a user could write
  would have to be a case this repository has, and Flow ships 239 functions.
  Structure *beside* the text is worse than either: two authored representations
  of one thing, free to drift, with no rule saying which is the truth. The cost
  is named rather than hidden — a second query engine reads `FlowSourceRef` and
  re-authors the transforms.
- **Never let a statistic answer zero for a group it has no answer for.**
  `median`, `stddev`, `variance` and `percentile` are `services/query/flow`'s own
  aggregations over hi-folks/statistics — Flow ships nothing that says how a
  column is distributed — and each is undefined below some number of values:
  one for a median, two for the rest. Below it the column is **null**. Flow's
  own `average()` answers 0 for an empty group, which is its choice and the
  wrong one to copy: a dataset version is read months later by somebody who was
  not there when it ran, and `0` there is a claim nobody made. These four are
  admitted by `Statistics\Descriptive::tryFrom` rather than by `Registry`,
  because that class's rule is about Flow's namespace and these are ours — the
  enum *is* the implementation, so the vocabulary cannot grow without a `match`
  arm, and ADR_0008's dispatch rule is unchanged.
- **Never refuse a windowed statistic the way a bare one is refused.** An
  aggregation on its own answers one row per group, so `withEntry('typical_age',
  median(ref('age')))` is refused — and the message names *both* ways out,
  because since ADR_0008's third amendment the second one exists:
  `median(ref('age'))->over(window()->partitionBy(ref('status')))` answers it
  beside every row. `PipelineBuilder::isWindowed` is the check, and it asks the
  value rather than the name: `WindowFunction::window()` throws when there is no
  OVER clause, which is Flow's way of saying "not windowed" and the only way to
  ask.
- **Never enumerate what a signature can decide.** ADR_0008's rule is about
  *dispatch*: a name from a query may select a key, never become a callable. A
  hand-written list of 37 names was an enumeration on top of that rule, and an
  enumeration is a queue — Flow ships 239. `Dsl\Registry` derives the
  vocabulary by return-type namespace, refuses any function with a
  `callable`/`Closure` parameter, and keeps `DECLINED`. Adding a name by hand
  now means the rule did not cover it, which is a reason to look at the rule —
  and the list that stayed beside it was deleted for the same reason, once it
  turned out to hold 23 names the registry already admitted and one that no
  position accepted.
- **Never decide where a name may appear from a second list of names.** What
  may sit inside `aggregate()` is what turned out to be an `AggregatingFunction`
  once the builder constructed it; `withEntry()` takes a `ScalarFunction` and
  refuses an aggregation by saying what it is; `write()` takes a `Dsl\Sink`.
  A list of aggregation names beside the arms that build them is the same
  enumeration one layer down, and it drifts in exactly one direction: a name
  Flow adds is usable everywhere except the position somebody forgot to list it
  in. What stays written is `Parser::STEPS` and `Parser::REFERENCE_METHODS`,
  because a step is a `DataFrame` method this service implements and a chained
  method is the one place a name from a query would have to reach a method —
  which is the dispatch ADR_0008 refuses, and what keeps Flow's window
  functions closed here.
- **Never answer "what may a query reach" with a list.** Three surfaces, one
  rule: `Dsl\Admission` refuses any call with a parameter that accepts a
  callable, a `Loader`, an `Extractor`, a `Path`, a `Filesystem`, a
  `Transformer`, a `SaveMode` or Flow's own evaluation machinery, and
  `Registry` (functions), `Frame` (steps) and `Values` (methods on the value in
  hand) apply it. The lists this replaced were the reason this language was
  said to have no arithmetic, no join and no window functions — none of which
  was ever true of Flow, all of which was true of the list. A name added by
  hand now means the rule did not cover it, which is a reason to look at the
  rule.
- **Never dispatch a name globally, and do not confuse that with a `match`.**
  `$name(...)` on a global appears nowhere and never may. A `ReflectionMethod`
  taken from `Frame` or `Values` and invoked against the frame or value in hand
  is not that: the reachable set is fixed, derived from types, and every member
  of it reshapes rows. The `match` arms that remain are the steps needing
  something a signature cannot know — the catalog, the window, which columns
  exist afterwards.
- **Never let a refusal a signature cannot make go unnamed.** `cache(?string
  $id)` passes every type rule and writes files under a name the query chose;
  it is in `Frame::DECLINED` with the reason, as `equals` is in
  `Registry::DECLINED`. A wider type rule to catch it would refuse innocent
  strings everywhere else.
- **Never make a query write `::`.** Some Flow parameters take a pure enum —
  `Rounding` on `divide()`, without which an inexact division throws from
  inside Brick\Math — so a written string is matched against the enum's case
  names, from the parameter's own declared type. That is the *only* coercion in
  the builder, and it is derived rather than listed.
- **Never let a query name something that opens a source or writes a sink.**
  The catalog decides what may be read and `write()` decides where rows go, so
  `Flow\ETL\Loader`, `Extractor` and `Filesystem` are outside the admitted
  return types. `to_csv('/etc/...')` in a service with no authentication is a
  file write, and its name looks as harmless as any other.
- **Never decide admission by loading a class.** `class_exists()` autoloads,
  and `Flow\ETL\Function\Uuid` throws at load without `ramsey/uuid`. The
  return type is matched as a **string**, or the service fails to start over an
  optional dependency of a function nobody called.
- **Never remember a result the query engine says is not deterministic.**
  `now()`, `uuid_v4()` and `random_string()` are honest work in the Query tab
  and a wrong cache entry in a managed step. The answer carries `deterministic`
  beside `window_applied`, for the same reason: only the engine knows what its
  query resolved to. Section 43.22. Every engine answers it: false for a corpus
  read, and for a call to a volatile function — DuckDB's read from the stability
  `duckdb_functions()` reports, through `FunctionExpression`'s text as well as a
  call's own name; DataFusion's from a declared set, because its binding says
  nothing about volatility.
- **Never offer Flow's loose comparisons.** In Flow 0.43 `equals` matches null
  against anything and `notEquals` drops nulls. Every column in every dataset is
  nullable, so both silently return the wrong rows. Refused *with the reason*
  rather than silently absent, because "unknown function" sends somebody looking
  for a typo. See `Registry::DECLINED`.
- **Never make the security boundary depend on Mago.** It is a dev dependency
  and may be absent. It reports syntax; `src/Dsl` decides what runs. `just
  flow-check` is the service's own gate (format, lint, tests) — `just check`
  does not cover PHP, and CI's `query` job does, in its Flow entry.
- **Never expose a query engine without authentication.** None has any. Flow's
  parser and a Python engine's `strict` admission bound what a query can say, not
  who may ask — and under `open` a query is code. `just query-serve` binds to
  localhost, and the chart's NetworkPolicy admits the panel and the server and
  nothing else. ADR_0028 names what `open` costs and what would make it wrong.
- **Never run a query in a process that has run one.** A Python engine's fork
  server imports the engine and runs nothing, and every query — `strict` ones too
  — runs in a child of it. Measured: a child forked after `import datafusion`
  answers, and one forked after the parent *ran* a DataFusion query panics in
  Tokio's I/O driver. For the same reason a DuckDB connection is made in
  `open()`, in the child, and its function catalog is read lazily, never at
  import.
- **Never let a forked query child ask macOS for anything.** Asking the system
  for its proxies calls CoreFoundation in a process that was forked and never
  exec'd, and it crashed every child of some fork servers and none of others —
  which reads as a flaky engine. The child's HTTP client takes nothing from its
  environment (`trust_env=False`), which also keeps `~/.netrc` out of it; anything
  new in the child that wants proxies, a locale or the keychain brings it back.
- **Never run a plan on an engine it was not written for.** A transform's text
  belongs to one language, so a chain naming another engine than the deployment's
  is refused when it is started — a 422 naming the block, the engine it was
  written for and the one deployed, and no run — rather than sent to an engine
  that would read it as a syntax error somebody takes for their own. The panel
  shows such content and does not run it.
- **Never hand a DuckDB relation text under `strict`.** A string handed to an
  expression is a column's name or a constant; a string handed to a relation
  method is parsed as SQL — `project("x + 1")` adds one, and a SQL string can name
  a file. So `strict` refuses anything that may be text — a literal, an f-string,
  a name bound to one, a property, a method of text — in any relation method's
  arguments, except `set_alias`, a join's kind and a group key of bare column
  names. Under either admission the session is locked beneath that:
  `allowed_directories` is the corpus root and `enable_external_access` is off.
- **Never filter a live stream in the browser.** `Scope::Selection` narrows
  `/api/v1/events/stream` server-side, which is why `LiveEvent` carries
  `agent_id` and `service` at all — the same reason it already carried
  `conversation_id` and `workflow_id`, one dimension further. A subscriber
  watching two agents cannot be resolved to a set of run ids when it connects,
  because the interesting run is usually the one that starts next; and
  subscribing to everything to discard most of it is the list-filtered-after-
  downloading mistake with `llm.chunk` volumes behind it. Or within a
  dimension, and across them — and an event with no value for a dimension
  somebody filtered on is not a match, or narrowing to one agent would put its
  whole run back in the stream.
- **Never let the query builder refuse anything.** It generates text in the
  deployed engine's language and the engine decides whether that text runs,
  exactly as it does for typed text — the split the annotation canvas and the
  pipeline canvas already make. It follows that the builder must *drop* an
  attribute the chosen grain cannot express rather than emit a column that is not
  there: a refusal for a chip the builder itself offered reads as the reader's
  mistake. The shapes it emits are pinned per engine, each against the real
  engine — `services/query/flow/tests/Dsl/BuilderShapesTest.php`, and
  `test_panel_shapes.py` and `test_duckdb_panel_shapes.py` under `strict` — which
  guard the language features it leans on rather than copying its output.
- **Never let the projector decide a run has died.** A run with no end event
  stays `Running`: the producer may have been killed, or may be thinking for
  twenty minutes, and nothing in the log distinguishes them. What the read
  model reports instead is `RunSummary::last_event_at` — when the run was last
  heard from — and the panel draws the line at `STALLED_AFTER_MS`, the same
  fifteen minutes as `AssemblerConfig::orphan_timeout`, because past it the
  span assembler has already closed that run's spans with `closed_by=timeout`
  and a runs list still showing a spinner is contradicting the waterfall beside
  it. A dimension row carries the same fact as `running_last_event_at`, over
  its *running* runs only: the row's own `last_activity_at` includes runs that
  finished, and a row that just completed something looks busy either way.
- **The window matches last activity, except on metrics.** `window_seconds` on
  every list means "active in the period" — a run that began three hours ago
  and emitted an event a minute ago is the thing most worth seeing in the last
  fifteen minutes, and windowing on start is exactly what hides it. Metrics
  keeps windowing by start because there the window is the timeline's x-axis: a
  run with no bucket cannot be counted into one. See
  `aiwatcher-projector/src/window.rs`; the panel's Query tab forwards the same
  number to the Flow service, which sends it only to datasets whose route
  accepts it (`Dataset::$windowed`) — the API rejects unknown query parameters,
  so sending it to the per-run events route would turn a scoped query into a
  400.
- **Never return a whole stream from a read route.** `read_stream_page` is the
  one the API uses; `read_stream` remains for the projector, which needs the
  whole thing. A route that pages is what keeps one long run from being a
  request that neither side can hold.
- **Never partition the log by `conversation_id`.** One conversation can fan out
  into parallel runs; partitioning by it serialises runs that have no reason to
  wait for each other. Partition by `run_id`.
- **Prompt and completion text is redacted by the Collector** before export
  (`deploy/otel-collector.yaml`). Enabling it needs a retention policy, not just
  a config change.
- **The read model's caps are a memory contract.** `AIWATCHER_MAX_SPANS_TOTAL`
  is what keeps the process inside 512 MB; `max_runs × max_spans_per_run` alone
  is not a bound. Re-run `just load-test` after changing any of them, and move
  the container limit with them.
- **Never attach a NetworkPolicy to pods this chart does not own unless they are
  already fenced.** Policies are additive, so an ingress rule added to pods that
  some policy already restricts widens them by one path — which is how the
  Collector reaches planner's VictoriaMetrics without editing planner's chart.
  Added to pods that *no* policy selects, the same rule narrows them from
  "accepts everything" to "accepts aiwatcher only" and cuts off whoever was
  already talking to them. `detect-stack.py` reports `fenced`; only that turns
  the rule on.
- **Never let installation reuse a Collector it merely found.** A foreign
  Collector almost certainly lacks the `attributes/redact` processor, and the
  redaction guardrail above is the whole reason the Collector is in the path.
  Detection reports one; `collector.mode: external` stays a human decision.
- **`aiwatcher-core` gains no dependency on a transport or a store.** If
  something needs one, it belongs in an adapter behind a port. `sha2` and `hex`
  are the exception and are not one: `PromptVersionId::of` has to agree byte for
  byte with `hashlib.sha256` on the producer side, unlike `ids::derive`, whose
  only requirement is that every aiwatcher agrees with every other one.
- **Never add a runtime a managed step reaches without opening the path to it.**
  The Flow service's policy admitted the panel and nothing else, which was right
  while the only client proxied a person's query — a managed `flow_php` step is
  aiwatcher reaching that service directly, and the refusal arrives as a refused
  connection the retry budget reads as a transient outage and spends ten
  attempts on. Whichever pod holds the reactor is the one to admit: `server`
  combined, `worker` split. Section 43.25.
- **Never reuse a database the cluster happens to be running.**
  `detect-stack.py` reports PostgreSQL and derives nothing, which is the object
  store's rule with a sharper reason: what this release would do with a database
  it found is *create tables in it*, and which database, whose credentials and
  whether that role may create a schema are none of them discoverable from a
  matching image.
- **Never attach a NetworkPolicy ingress rule to an object store nothing else
  fences.** The general rule below applies with a sharper edge here: planner's
  RustFS serves `planner-web`, `planner-import-api` and `planner-mlflow`, and a
  rule saying "aiwatcher only" would cut all three off. `detect-stack.py`
  reports whether it is fenced; only that turns
  `allowEgressToExternalPromptStore` on.
- **Never point Tilt at a non-local cluster.** The guard is in two places
  (`Tiltfile` and `just _assert-local-context`); do not weaken either. A typo in
  a context name must not be the only thing between a keystroke and production.
- **Never raise `AIWATCHER_LASER_PARTITIONS` above 1** without replacing the
  scalar `Checkpoint` with a per-partition cursor. A scalar has no total order
  across partitions, so live-stream resume would silently skip events.
- **Never switch the Laser consumer to an automatic `CommitPolicy`.** The
  pipeline commits only after a durable write; an automatic policy would move
  the offset past events that were never stored. The cost is that the broker
  redelivers between read and commit, which the adapter's local read position
  absorbs — see `adapters::laser`.
