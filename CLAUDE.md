# CLAUDE.md

Guidance for Claude Code when working in this repository.

## What This Is

Observability for AI agent runs. Python, TypeScript and Go agents publish events
to a durable log; a Rust backend consumes them, assembles OpenTelemetry traces,
exports to VictoriaTraces and VictoriaMetrics, and serves a live view over
SSE/WebSocket to a React panel.

```
Python / TypeScript / Go agents
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

   organizations   ──► PostgreSQL    teams, projects and grants: the control
                                     plane, not yet the whole data plane
   one machine     ──► `aiwatcher up` one binary, one DuckDB file, one token
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
just e2e-pods         # four stages as four pods on a local cluster, against the same four in one worker
just e2e-docker       # the same four as four containers on this host: the image and its limits, no cluster
just e2e-processes    # the same four as four processes on this host: no cluster, no image, no cargo feature
just e2e-pod-death    # a step's pod killed mid-attempt: ended as infrastructure, run again in a new pod, no Job left
just e2e-train        # the whole chain: annotate → export → fit a real tiny model → promote
just e2e-generate     # two variants generate answers on a worker and are held to
                      #   everything that can be checked about them: their traces,
                      #   a gateway's word on model, prompt, question and answer,
                      #   tools, judges, gates, prices, the journal and runs lost
                      #   in transport. The longest gate here; read its recipe.
just e2e-gate         # a line admitted once, then CI jobs exit pass, regression, incomplete and error, a model's variant too — registered or not
just e2e-review       # a trace proposed, an expected answer approved, a new version of the cases in their splits, a result's first case in its own words
just serve-model      # verify the promoted package's digests, load it, serve it, watch the label
just onnx-version     # re-express that model as an ONNX graph, check it agrees, move the label
just ml-pipeline-serve # the marimo notebook runtime on :8082, for notebook blocks
just ml-pipeline-check # ruff, mypy --strict and pytest for that service
just scorers-serve    # the scorer service on :8083: DeepEval's and Opik's metrics for a scorecard
just scorers-check    # ruff, mypy --strict and pytest for it, with both frameworks installed
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

With a database, for the workflow store:

```bash
just postgres-up   # PostgreSQL on :5433 — not 5432, so a suite never lands in
                   # a project database somebody already has there
just test-postgres # the storage contract, the same sixteen properties the
                   # memory and file adapters prove, against a real database
just run-serve     # the API, the read model and the object store, no ingress out
just run-work      # the outbox and the reactors, no ingress in
```

The last two are the split into two roles, and they need three shared backends —
`postgres`, `laser` and `s3` — because that is what stops being per-process when
the binary is two processes. The start-up refuses each by name. One process
holding both roles is the default and needs none of them, which is what `just
run` and `just dev` are. `aiwatcher journal` is neither half: the observation
journal alone, on `laser` and `s3`, a reader of the log that needs nothing the
other roles need.

With a broker, for the Laser backend:

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

The IAM control plane rides on the same two: `AIWATCHER_IAM_POSTGRES_URL` — its
own database, never the workflow store's — plus `AIWATCHER_AUTH_MODE=oidc` and
the `aiwatcher-server/postgres` feature, and the start-up refuses anything else
rather than falling back to memory. Its database suite is opt-in and named:
`AIWATCHER_IAM_TEST_POSTGRES_URL=… cargo test -p aiwatcher-iam --features
postgres --test postgres -- --ignored`, ignored rather than reported as passed
when no database is there.

With an object store, for the prompt registry:

```bash
just rustfs-up     # RustFS on :9010
just run-rustfs    # server with the registry in the object store
just test-rustfs   # five integration tests against it — this is what verifies the SigV4 signer
```

The Python SDK is a `uv` project of its own; the Go SDK is a Go module of its
own:

```bash
just sdk-install   # uv sync --all-groups
just sdk-check     # ruff format --check, ruff check, mypy --strict, pytest
just agentic-install  # the workflow engine in sdk/agentic, likewise
just agentic-check    # the same four checks, on the engine
just sdk-go-check     # gofmt, go vet, go test -race, on sdk/go
```

One machine, no containers at all (ADR_0027):

```bash
just build         # the `aiwatcher` binary out of aiwatcher-cli
aiwatcher up       # the server on the write-ahead log, a token if there is none
aiwatcher runs     # …and every other verb, over the same REST API the panel uses
just test-duckdb   # the local workflow store's own suite — a database, no container
```

Run a single Rust test: `just test-one two_parallel`.
Panel: `cd apps/panel && npm run build` (vite build followed by a full `tsc`
project check).

## Architecture

Crates, in dependency order. A crate may only depend on ones above it. Each area
also keeps its own rules in a `CLAUDE.md` beside its code — the table under
Guardrails says which file holds what.


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
| `aiwatcher-evaluation` | Pinned variant/context contracts and durable evidence (ADR_0030). `Evaluation::prepare` validates declarations; `Registry` owns immutable publication, paging, erasure and **approvals** — the pair an operator admitted, which is what lets one instance hold a baseline and a candidate at once — through a `SourceAuthority` adapter. It also measures: a **scorecard** declares named scorers and derives each metric's direction, and a **scoring run** folds a staged recording, a conversation cohort's own archived responses, or answers a worker's task **generates** for each case's input — held to the variant's prompt and model by the traces of their runs — against one — asking a calibrated **judge** first when the card names a rubric, and the **scorer service** when it names a framework's metric, held against people when the card says so — and publishes the result — admitted against the scorecard and the compiled vocabulary rather than a bundle's files. A run's **cohort** may be derived from a dataset version the deployment owns, first cases only when limited. `external` is the scorer service's contract, and the one module that knows one exists. Legacy reports remain in Projector with an explicit API read bridge. |
| `aiwatcher-labs` | A workshop's labs (ADR_0034): the authored brief somebody reads, and the measurement their work is held to. Almost everything a lab needs was already here — its tests are an `aiwatcher-evaluation` scorecard version and a derived cohort, the work handed in is a recording or answers a worker generated, and a mark is a published result — so this holds the one thing that was not: a versioned document naming the brief, the card, the cohort and where it sits. `LabTests::measurement` folds those pins into the `EvaluationContext` every submission publishes under, which is the whole join from a lab to its marks. The second authored thing is the *material*: a lab names a notebook — a file in the notebook runtime, pinned by `sha256`, or a path in a workshop the participants checked out with the command that opens it — and never its source, and never where marimo runs. |
| `aiwatcher-datasets` | Curation recipes, the dataset versions they produce, and the **block pipelines** of ADR_0024 — a chain of source, transform, notebook, approval and view, refused as a whole with every problem at once. Nothing here executes anything; the panel drives the chain because the engines are three different systems. |
| `aiwatcher-execution` | Owned execution (ADR_0025, ADR_0026): the compiled `ExecutionPlan` and its `plan_id`, the states, the attempts, the pure `decide`/`evolve`, the cache key, the compiler from ADR_0024's blocks, the atomic command handler, the claim table, the `ContextSnapshot` that reopens a block, the fact encoder and the outbox publisher. Three ports: `WorkflowStore` (`memory | file | postgres | duckdb`, the last two behind features so `sqlx` and DuckDB's C++ amalgamation are out of every build that does not ask for them — the shape `laser` has in `aiwatcher-bus`), `ActivityExecutor` (what a reactor does with a claimed attempt) and `ArtifactCatalog` (metadata, lineage, the cache index). Executes nothing itself, and holds no second copy of `aiwatcher-jobs`' rules — it calls them. |
| `aiwatcher-runner` | The workflow rerun dispatcher: one HTTP POST to one configured endpoint, behind `core::ports::WorkflowRunner`. |
| `aiwatcher-iam` | Organizations, teams, projects and grants: the control plane an instance role does not reach into. Its audit trail has a clock of its own — off unless a deployment sets one, never reachable over HTTP, and it keeps each organization's newest entry so a binary that predates it still numbers the next one — and a resumable export of ADR_0022's shape, whose cursor moves only after a shard is stored. A principal is the exact `(provider, subject)` pair; the effective project role is the maximum over every live direct and team grant, on half-open windows. `MemoryIamStore` and `PostgresIamStore` (behind `postgres`) run one policy inside their write lock, over one versioned JSONB aggregate per organization with an audit entry in the same transaction. Knows nothing about axum, and authorizes no resource of its own — `crates/aiwatcher-iam/README.md` is the record, and says which boundaries are scoped and which are not. |
| `aiwatcher-auth` | Single sign-on: OIDC discovery, a JWKS cache, the authorization-code flow with PKCE, HMAC-signed session cookies, authentik's forward-auth headers, and the group-to-role mapping. Knows nothing about axum. |
| `aiwatcher-projector` | The pipeline, live hub, read model, dimension, span, evaluation and workflow-graph folds, what a variant was observed doing and the periods of it written as they close and rolled up into hours and days, which answer every window over it (`period_fold`; `journal`, a consumer of its own keeping what that fold reads past the log's retention and saying how close it is to a gap; and `periods`, the one module here that writes an object store), what witnesses saw asked (`asked`, an index a traces step reads past the read model and a restart) and what clients counted of a measurement's runs (`measured`, kept beside that index for the same reason), dedup, retry, dead letters |
| `aiwatcher-api` | axum router: REST, SSE, WebSocket, OpenAPI. `worker` is the one module whose caller is not a browser: the reactor's own loop with an HTTP seam where the work happens (Phase 10). |
| `aiwatcher-server` | Config, wiring, graceful shutdown, and the **reactors** — the one place an executor's client lives, because an executor holds a socket and a credential. `execution/` is mostly the work role: `artifacts` (the object store's sixth prefix, and the receipt a lookup reads), `query` (the client every query engine shares) with `flow`, `datafusion` and `duckdb` beside it (one executor per engine, and only the deployed one registered) `publish` (the dataset version, which runs in `serve` because it executes nothing) and `scoring` (a scoring run's one step, in `serve` for the same reason) — and `editor`, which runs in `serve` because opening a block on a step's rows is a person waiting on a request rather than an attempt somebody claimed. The only crate that knows every implementation exists. |
| `aiwatcher-cli` | The `aiwatcher` binary (ADR_0027): `up` runs the local stack, the read and write verbs speak the REST API the panel speaks, `token` and `profile` decide what a command shows and where it goes, `api` reaches a route with no verb, and `sql` opens the local store read-only. `key=value` anywhere on the line, and a bare `aiwatcher` starts the server rather than printing help, because that is the image's `ENTRYPOINT`. `tests/contract.rs` holds every path *and query parameter* to `contracts/openapi.json`. |

Everything else: `apps/panel` (React), `sdk/python`, `sdk/agentic`,
`sdk/typescript`, `sdk/go`, `contracts/` (the OpenAPI document and the envelope
JSON Schema), `deploy/` (Dockerfiles, the compose stack, the kustomize test stack,
`helm/aiwatcher` + `helmfile.yaml.gotmpl` + `scripts/` — the install path),
`docs/ADR/`, and three **optional** services outside the Cargo workspace that the
Rust binary does not know exist:

| Service | What it is |
|---------|------------|
| `services/query` | The query engines `AIWATCHER_QUERY_ENGINE` chooses between (ADR_0028), behind the Query tab, its recipes and a chain's query step. `flow` is the PHP surface (`just query-check`); `contract`, `datafusion` and `duckdb` are one `uv` workspace — the contract every Python engine serves, the catalog all three load, and the two engines on it (`just query-contract-check`). |
| `services/ml_pipeline` | The Python 3.14 notebook runtime behind a pipeline's marimo blocks: one run as a step through `App.run(defs=…)`, in a worker thread so a run does not hold the loop that serves everything else, and the same file served live for the block's editor (`just ml-pipeline-check`). |
| `services/scorers` | A scorecard's **framework metrics** — DeepEval's and Opik's, each an adapter behind a two-route contract whose fixtures the Rust half reads too — for an `external_evaluation` step, behind a bearer token, installed by the chart's `scorers` block (`just scorers-check`). |

`just check` covers none of them, because PHP and a Python toolchain may not be on
a machine that only touches the Rust crates — but **CI runs each**, in their own
jobs and once per query engine, because a break there is a break in the execution
path.

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
| `ExecutionOwner` | Who decides: `local` or `worker`. Any other text read back is kept as unknown — shown, never scheduled or decided |
| `ExecutionMode` | `compiled` — the Rust decider schedules a static plan; `hosted` — a worker decides and this keeps the history |
| `ObservedWorkflow` | A graph folded from telemetry (ADR_0012). Not necessarily launchable, and not an `ExecutionPlan` |

A curation pipeline, a planner workflow and an agent graph may all compile to an
`ExecutionPlan` and remain distinct authoring experiences — and an agent graph
compiles only to its *shape*, because its decisions stay in the worker.

## The decisions that explain most of the code

Each has an ADR under `docs/ADR/` — except the last, whose record is
`crates/aiwatcher-iam/README.md` until one is written. Read the relevant one
before changing that area.

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
   `dimensions::compute` answers `session | agent | runtime | workflow |
   variant | trace | model | tool | prompt` with one row shape — the pivots
   differ only in which key a run contributes, and `prompt` is the one that
   needed the span's `aiwatcher.prompt.name` lifted to be asked at all. Nothing loads a whole run: `read_stream_page` pages the log,
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
   same four pieces on the client that is already there for tracing. A report
   a managed step records through `TaskContext.record_evaluation` names that
   run and step, which is why an evaluation is a worker task and not a binding
   of its own (ADR_0010, amended; AW-5).

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
   `none | oidc | proxy | local` (the last is decision 24's) and defaults to
   `none` — a release that started
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
   ([ADR_0012](docs/ADR/ADR_0012_WORKFLOW_GRAPH.md)). planner once ran its
   house import as four Flyte stages *or* as the same four functions
   in-process, and only a declaration was right on both paths. It has since
   removed Flyte, and the rule stands: a declaration is right on every path.
   So aiwatcher never asks an orchestrator anything: `workflow.declared`
   carries the topology on the log,
   `step.*` with `data.node` executes a node of it, `artifact.produced` points
   at what a node handed on, and `agent.message` records one agent addressing
   another — the one thing nesting cannot show. `workflow_run_id` joins the
   stages a per-pod orchestrator scatters across four runs; omit it and the run
   *is* the execution. A stage nothing has started is `Pending`, which is the
   whole reason the declaration exists, and rerun is a dispatch to one endpoint
   from *configuration* — `aiwatcher-runner`, 501 when unset.

13. **Superseded: there is no external pipeline engine**
   ([ADR_0016](docs/ADR/ADR_0016_PIPELINE_ENGINE.md), superseded by AW-4).
   aiwatcher once read Flyte for what it *could* start and launched one entry of
   it; its one user moved onto aiwatcher's own workflow engine, so the Flyte
   engine, its routes and the panel's launcher are removed, and a deployment that
   still sets `AIWATCHER_ENGINE` is refused at start by name. A step that needs a
   pod of its own gets one from the engine itself
   ([ADR_0029](docs/ADR/ADR_0029_POD_PER_STEP.md)): a step names an operator's
   template and an image on that template's list, the work role starts one Job per
   attempt behind the `kube` feature and claims none of them, and the pod is a
   worker that claims its one attempt by key. Correctness needs no watch — a pod
   that vanished is a lease that lapses — and what the watch adds is *sooner* and
   *why*. What a pod *is* belongs to the deployment and never to the plan:
   `AIWATCHER_POD_RUNTIME` is `kubernetes`, `docker` or `process`, and the last two
   run each attempt on the work role's own host with the same launcher, name, claim,
   watch and log; `docker` keeps the step's image *and* the template's limits, and
   `process` keeps neither and needs nothing installed. A pod holds its attempt's
   credential and nothing else
   ([ADR_0031](docs/ADR/ADR_0031_POD_ATTEMPT_CREDENTIAL.md)). Proven on real
   infrastructure rather than argued: `just e2e-pods`, `e2e-docker`,
   `e2e-processes` and `e2e-pod-death`, which is what found the two-rustls-provider
   panic in a build that was green everywhere else.

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
   deferred, decided in favour — and it stands with ADR_0016 superseded, since
   "the engine" here is aiwatcher's own. `execution.*` and `Subject::Execution`
   join the catalog with `forms_span = false`; a started plan publishes
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
   in Python). One contract — the seven `/query` routes, one `catalog.json` — and a
   conformance suite asking every engine the same four questions.

24. **A local install is one binary, one database and one token**
   ([ADR_0027](docs/ADR/ADR_0027_LOCAL_INSTALL.md)). Everything above assumes a
   cluster in the middle distance — a broker, PostgreSQL, RustFS, authentik, one
   `just` recipe each — which is the right shape for a deployment and the wrong
   one for the first hour. So `aiwatcher up` starts the server on the write-ahead
   log, mints the token if there is none, and starts the query service when the
   checkout has one. Three things follow, each in the shape this repository
   already uses. The CLI's verbs are **HTTP calls against routes that already
   exist**, so a command cannot disagree with the panel about what a run is —
   and the remote case is free, through `profile`. `AIWATCHER_AUTH_MODE` gains a
   fourth value, `local`: one token in a file only its owner can read, read by
   both halves, authenticating as **admin** on the stated condition that
   `LocalAuth::check_reachable` finds the listen address on loopback — its own
   `Credential::Local` rather than a quiet exception inside the ingest token,
   whose whole point is that it has none, and unrestricted in `may_claim`
   because the mode it replaces ran anonymous and could claim anything. And
   `duckdb` is a fourth `WorkflowStore` behind a cargo feature: one process as
   `file` is, a database rather than a directory, with the rules still in Rust —
   the adapter loads rows and asks `prunable`, `is_claimable`, `matches` and
   `is_available` — passing the same properties the other three do with nothing
   running. A bare `aiwatcher` starts the server rather than printing help,
   because the image's `ENTRYPOINT` is this binary with no arguments.

25. **Who may see a project is aiwatcher's question, not the provider's**
   ([ADR_0033](docs/ADR/ADR_0033_PROJECT_SCOPED_STORAGE.md) for the storage
   boundary; `crates/aiwatcher-iam/README.md` for the control plane). The
   identity provider authenticates; **aiwatcher owns membership**,
   and IdP groups are never copied into teams — a principal is the exact
   `(provider, subject)` pair, and email, display name and instance role are no
   part of it. Organizations hold teams, projects and grants in one transactional
   aggregate; the effective project role is the maximum over every live direct and
   team grant on half-open windows, so one source expiring never removes another.
   An owner has **no implicit project grant**: creating a project atomically adds
   an explicit, removable admin grant to its creator. The data plane is being
   moved across one resource boundary at a time — datasets, prompts, training,
   annotations, forms, workflow definitions, case reviews, cohorts, recordings,
   bundles, producer approvals, calibrations, declarations, artifact bytes — each
   an additive `/api/v1/orgs/{organization}/projects/{project}` family sharing the
   legacy handlers, keyed under `<prefix>/scopes/<org>/<project>/registry/`, with
   the legacy route still serving legacy data under instance authorization.
   Everything that **runs** followed, as ADR_0033: `for_project(scope)` returns
   the same backing store narrowed to one project, and the **unscoped handle
   enforces the same rule from the other side** — the reactor, the worker, the
   pod launcher, the timer tick, the outbox publisher and the retention sweep
   needed no change, because the store they hold does not see a project's work.
   An execution's `ExecutionOwnership { scope, principal, definition }` is
   written in the transaction that creates it and never again, and never comes
   from the plan, a parameter, `requested_by`, a worker's name or a
   declaration's author. Absence is the global side, so nothing existing is
   migrated; `aiwatcher-migrate` copies four registries into a named project by
   hand, blocks the conversation archive and names eleven prefixes it will not
   touch. None of it is authorization: a grant is IAM's answer, asked fresh,
   before the cache lookup as well as before the work — which is what
   `ProjectDispatcher` is, the one loop that reads the owner off the bound
   store, asks IAM when it takes the work and again before it publishes, treats
   an IAM failure as `Transient`, pairs the byte store with the catalog through
   `ProjectArtifacts::bind`, and builds its executor per attempt so nothing of
   a project's is in a process-wide registry. What is **not** done is written
   down as plainly as what is: **no production wiring constructs one**, there is
   no project `/start`, and logs, streams, query and notebook runtimes, worker
   credentials, retention and scheduled jobs are still instance-wide. Until
   those land, no organization or project selector is activated, and this
   deployment is not described as multi-tenant safe.

26. **A lab is an authored brief bound to a pinned measurement, and three of
   the four things it needs already existed**
   ([ADR_0034](docs/ADR/ADR_0034_WORKSHOP_LABS.md)). Learning is built on one
   sentence — a workshop is a project, a participant is a grant, enrolling is
   redeeming an invitation — and it carried the access half and none of the
   content. The obvious reading was four missing things; holding them against
   what this instance has left one. The tests are a `Scorecard` at a version
   plus a `Cohort` derived from a dataset version, both already project-scoped.
   Handing work in is a staged recording, or answers a worker generates for a
   `VariantManifest`. A mark is a published `EvaluationResult` — and the class's
   view of one needs nothing new either, because a `context_id` is the content
   address of the cohort, the split, the suite, the scorer and the metric
   definitions *together*, so every result measured on one lab's pins shares it
   and `/evaluation-results?context_id=` is already "everybody's marks". What
   was missing is the brief, and the document binding it to a card and a cohort:
   `aiwatcher-labs`, ADR_0011's shape exactly, project-scoped from birth. A lab
   answers its own `context_id` through `GET /labs/{name}/measurement` and
   nothing else computes one, the precedent being
   `POST /evaluation-approvals/address`. Two gaps stay visible rather than
   papered over: `/evaluation-runs/{id}/start` has no scoped twin by ADR_0033's
   own rule, and `/experiments` is legacy-only.

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

Being one namespace, it is also where **two domains can quietly overwrite each
other**: `aiwatcher-conversations` and `aiwatcher-evaluation` both have a
`Withdrawal`, and the contract described an approval's with a corpus's fields
for a whole stage — the document generated, the client generated, and nothing
read the field until something did. A type whose name another domain could
plausibly use carries `#[schema(as = …)]` naming the thing it belongs to.

`integrations/` is the one grouping in either crate that is not a product area:
it holds what the crate reaches *out* to. Everything else answers from the log,
the read model or the object store; these leave the building, with a timeout, a
credential and a third party whose answers are data rather than truth. Both
`aiwatcher-annotations` and `aiwatcher-api` group hubs there for that reason,
and a reader of `routes::router` can see which routes leave at a glance.

### Contract

Evaluation manifest wire shapes are generated independently with
`rtk proxy python3 scripts/check-evaluation-contract.py --write`; its schema and
TypeScript consumer fixture are checked in CI. `Evaluation::prepare` also
validates semantic constraints. Python producer types live in
`aiwatcher_sdk.evaluation`, separate from the telemetry entry point.

`contracts/openapi.json` is generated from the axum routes and the panel's
TypeScript client is generated from it. After changing any route or any type
that appears in one, run `just openapi` and commit both the contract and
`apps/panel/src/api/generated`. CI fails if either is stale.

### Panel and SDKs

They have conventions of their own, beside their code: `apps/panel/CLAUDE.md`
(generated client, URL state, navigation areas, virtual lists, the time window,
primitives, the command panel — then what the browser may not decide) and
`sdk/CLAUDE.md` (the four distributions, the split failure policies, the naming
rules, and what each one may not import).

### Agent skills

`.claude/skills/` holds reference material an agent loads on demand, vendored
at pinned commits. `.claude/skills/README.md` describes the observability,
Rust, TanStack, Hugging Face, frontend, UX and system-design skills.
`docs/design-skills-selection.md` records the design selection and when to use
it. `.agents/skills/` exposes the design set to Codex. The Web Interface
Guidelines audit fetches its rules live; its wrapper alone is pinned.

None of them is about *this* repository. This file, the `CLAUDE.md` beside each
area's code and the ADRs under `docs/ADR/` are that, and a skill restating any of
them would be a second copy free to disagree with the first. `just skills-check` says whether the tree still
matches `vendor.json`; `just skills-update` moves the pins, and the diff is
the review.

## Guardrails

The rule, then what it prevents. Where a rule has an ADR it is named; the rest
of the reasoning is in the commit that added it.

What follows is what reaches past one area. **An area's own rules live beside
its code**, in a `CLAUDE.md` this session loads when it opens a file there —
each rule in exactly one place:

| File | Holds |
|------|-------|
| `crates/aiwatcher-execution/CLAUDE.md` | claiming, attempts, gates, schedules, the workflow store, pods, plans and caching |
| `crates/aiwatcher-evaluation/CLAUDE.md` | scorecards, cohorts, judges, generated answers and what a witness saw, evidence, approvals, retention |
| `crates/aiwatcher-conversations/CLAUDE.md` | the encrypted archive, review, erasure, exports |
| `crates/aiwatcher-annotations/CLAUDE.md` | drawing, the family split, usage rights, hubs, imports, `integrations::fetch` |
| `crates/aiwatcher-training/CLAUDE.md` | training runs, the model registry, promotion, the serving profile |
| `crates/aiwatcher-prompts/CLAUDE.md` | versions, optimisation verdicts, labels |
| `crates/aiwatcher-labs/CLAUDE.md` | the brief, the pins, and the context a lab's marks share |
| `crates/aiwatcher-projector/CLAUDE.md` | the period fold, the journal, the `asked` index |
| `apps/panel/CLAUDE.md` | how the panel is built, and what the browser may not decide |
| `sdk/CLAUDE.md` | the four SDKs: their split failure policies, naming rules and what each may not import |
| `services/query/CLAUDE.md` | Flow, DataFusion and DuckDB admission |
| `services/ml_pipeline/CLAUDE.md` | pinned notebooks, staging, the editor |
| `deploy/CLAUDE.md` | the chart, detection, Tilt |

### One ordering, in eight places

**Never write the pointer before the thing it points at, and never move a cursor
past work whose output is not stored.** `aiwatcher_jobs::ORDERING` states it
once; a crash the right way round repeats idempotent work, a crash the wrong way
round leaves a reference to bytes nobody wrote. It applies to:

- the pipeline's checkpoint, committed only after the durable write — reversing
  that one turns a crash into silent data loss;
- a step's receipt, written after its data, or a completed step's artifact 404s;
- an execution fact, published from the outbox *after* the decision commits,
  never from the handler (ADR_0026);
- a conversation export's cursor, advanced only after its shard is stored — the
  wrong way round leaves a corpus missing rows nothing can tell you about;
- an import's cursor, advanced only after its page's shards are stored, with the
  counts moving with it, or a redone page is counted twice;
- a prompt version, written before the head that indexes it — an unindexed
  object waits for `Registry::rebuild`, an index naming nothing is a list whose
  rows 404;
- an annotation revision before the image head, and an export manifest before
  the index entry that lists it;
- the outbox row itself: never prune an execution a pending row still speaks for
  (`store::prunable`), or the publisher holds a message with no explanation.

### Refusals, and where a decision lives

- **Never make a caller recover a classification from a status code.** 501 (a
  registry never wired), 500 (a definition that will not read back) and 502 (a
  store that refused the read) are permanent by meaning and 5xx by number, so
  the scheduler retried each every minute for ever. `StartRefused` answers it
  itself, as `says_the_same_next_time` does for `StoreError` and `HandleError`.
- **Never flatten a port error into a string.** `PortError` already carries
  whether it is worth coming back for. `DefinitionRegistry` flattened an
  unreachable store, a corrupt record and a definition that does not compile
  into one `StoreError::Backend`, so a corrupt workflow answered 503 — a promise
  to come back for something that never would. `DefinitionError` is those three.
- **Never let the compile-and-start use case live in the HTTP module.** The
  route, `run_now` and the tick all mean one thing by starting:
  `aiwatcher_execution::start`. What stays with the caller is **who is asking**
  (a role check a tick does not have and must not fake) and **which id**
  (`RunIdentity`).
- **Never flatten a body that denies unknown fields.** serde's `flatten` and
  `deny_unknown_fields` do not compose, and these bodies deny unknown fields so
  a hopeful `cron` is a refusal naming it.
- **Never let `decide` read a clock, open a socket or generate a random value.**
  Time arrives in `Now` and ids are derived from what they name, which is what
  makes a replay reach the same schedule and the same command id. A jittered
  retry delay is the dispatcher's.

### Authentication, tokens and identity

- **Never let a route decide for itself whether it needs a caller.** The
  authentication layer is applied once in front of the whole router, with an
  exception list in `auth::is_public` — the health probes and the sign-in
  routes, which cannot require a session in order to establish one. A route
  added later is authenticated by default, so the one somebody forgets is not
  the one that leaks. What the layer does *not* decide is whether the caller may
  perform the operation: that is a `Role` check in the handler, because a table
  of paths in a middleware drifts from the routes it guards.
- **Never accept `AIWATCHER_AUTH_MODE=proxy` without a network boundary.** In
  that mode a header is a claim, so any pod that can reach port 8080 can assert
  it is an admin. The chart refuses to render it without `networkPolicy.enabled`
  and says why.
- **Never let an ingest token be more than an editor.** It is a shared secret
  sitting in an agent's environment, and it exists because a producer reaches
  the Service directly and cannot complete an interactive sign-in — not so that
  a leaked environment file can ask an orchestrator to run something. The role
  is hard-coded in `IngestToken::identity` and never comes from the group
  mapping.
- **Never give a pod a credential for more than its attempt** (ADR_0031). The
  launcher mints each pod's `AIWATCHER_TOKEN` for one attempt, and
  `auth::admits_attempt` — that attempt's worker routes and `POST
  /api/v1/events`, nothing else — is checked in the authentication layer, before
  any handler, because most read routes check no role and a credential let
  through there reads every run. It fails closed; `own_attempt` then holds each
  worker route to the key the credential names, before the lease and the queue.
  Its key is derived from `AIWATCHER_POD_CREDENTIAL_SECRET` under a label of its
  own, so a session never opens as one, and on a cluster the value is a Secret
  the Job owns rather than a pod spec a namespace's viewer reads.
- **Never take the issuer from the discovery document.**
  `ProviderMetadata::discover` compares what the document declares against what
  was configured and refuses a mismatch; believing the document hands the choice
  to whoever answered the request.
- **Never pick a JWT's algorithm from its header alone.** `oidc::algorithms_for`
  derives the permitted set from the *key* and uses the header only to narrow,
  and refuses a symmetric key in a provider's key set outright — taking `alg`
  from the header and the key by `kid` is the confusion attack where an RSA
  public key is handed back as an HMAC secret.
- **Never start serving when the identity provider could not be reached.**
  `Authenticator::connect` retries while it comes up and then fails the
  start-up. The only other thing an instance could do is serve unauthenticated.
- **Never widen the session cookie.** `HttpOnly` keeps it out of JavaScript,
  `SameSite=Lax` is what survives the redirect back from the provider (`Strict`
  drops it and the sign-in loops with no error anywhere), and `Secure` is
  derived from the redirect URL's scheme rather than defaulted — a `Secure`
  cookie is simply not stored over http, and an instance served that way would
  sign people in and then behave as though nobody had.
- **Never follow a `next=` that is not a path on this application.**
  `auth::safe_next` refuses anything that is not one leading slash: an open
  redirect on a sign-in route is how a phishing link gets to start on the real
  host.
- **Never let `local` mode be more than one machine's answer** (ADR_0027). The
  token is an **admin** on a stated condition — `LocalAuth::check_reachable`
  finds the listen address on loopback — held in a file only its owner can read
  and read by both halves, so rotating it is one write. `Credential::Local` is
  its own variant, unrestricted in `may_claim`, because the unauthenticated mode
  it replaces let anonymous workers claim anything and adopting a credential
  must not silently break that machine.
- **Never let a route report a secret's value because its presence is worth
  reporting.** `GET /api/v1/system` answers what this deployment has wired —
  which is a fact worth having in one place rather than one 501 at a time — and
  the line it keeps is that *configured* is not a secret and the *value* often
  is. A credential is never printed; neither is an address, because a database
  URL or an object store's endpoint is reconnaissance for somebody already
  inside. Both are reported as the **variable's name**, which is what a reader
  needs anyway. The issuer is the one exception and a deliberate one: it is
  already public on `/auth/config` before anybody signs in. It is an `admin`
  route, it writes nothing, and `aiwatcher-server/tests/system.rs` puts a
  recognisable value into every sensitive variable and fails if one comes back.

### Organizations, projects and grants

- **Never let an instance role become a project role.** An instance admin may
  create an organization and nothing more; an organization owner has **no
  implicit project grant**, and creating a project atomically adds an explicit,
  removable admin grant to its creator. Recovery is a new explicit grant, not a
  role that outranks one.
- **Never copy the identity provider's groups into teams.** The provider
  authenticates; aiwatcher owns membership. A principal is the exact `(provider,
  subject)` pair, and email, display name and instance role are no part of its
  identity or its project authorization. `/account` renders IdP groups read-only
  and says they are not teams.
- **Never cache a project decision for a session.** `ProjectAccess` is a
  snapshot with `evaluated_at`, not a bearer capability: every operation takes a
  fresh decision, and a write rechecks the grant *after* the body arrives and
  before the registry is called. Long-lived streams and jobs still need their
  own revocation story — that is an open gate, not a solved one.
- **Never let scope enter a content hash.** Keys are
  `<registry-prefix>/scopes/<organization>/<project>/registry/`; identical
  content in two projects has the same version ID and separate objects, and
  knowing that ID grants nothing. `Registry::for_project` refuses rebinding to a
  different scope — and it is a *storage* boundary, never an authorization API,
  so a trusted non-HTTP caller makes its own fresh grant check.
- **Never let project admission read as execution authority.** All three scoring
  execution paths refuse a project-bound registry as `FailureClass::Policy`
  before reading its declaration, so passing a project registry into
  `ScoreExecutor::new` cannot silently become permission to run.
  `ScoreExecutor::for_project` takes an explicit `ProjectAuthority` — an IAM
  store, a principal, a scope, an execution and a declaration, not serializable
  and not a request body — which a future dispatcher must obtain from durable
  execution ownership rather than from the plan, the parameters, the worker name
  or the declaration's writer. It asks IAM again immediately before
  `Committing`, and its output is not cacheable: a prior result cannot
  substitute for current authorization.
- **Never require anything less than the mutation header on a scoped write.**
  `X-AIWatcher-IAM: 1` is not simple, so it cannot come from a cross-origin form
  with a session cookie, and there is no wildcard CORS with OIDC. Scoped
  responses carry `Cache-Control: no-store`, the actor comes from the verified
  session rather than the body, and unknown top-level fields are rejected —
  including an actor somebody hoped to supply.
- **Never describe this deployment as multi-tenant yet.** Logs, streams, query
  and notebook runtimes, workers and their credentials, scheduled jobs, and the
  migration of existing resources are all outside the boundary; legacy routes
  still serve legacy data under instance authorization, and unassigned resources
  must not fall back to global access. No organization or project selector is
  activated until those land, and the migration inventory is a **dry run** that
  rewrites no reference.

### The log, spans and the read model

- **Never store `llm.chunk` as a trace record** (ADR_0003). Counted, never
  stored per chunk.
- **Never let an `eval.*` or `workflow.*` event reach span assembly.**
  `EventType::forms_span`, checked first in `SpanAssembler::ingest`. A topology
  is a shape with no duration, and a waterfall of one would be showing the
  moment a producer got round to describing itself; the node executions drawn
  against that shape are `step.*`, and those do form spans.
- **Never store an artifact's content.** The registry stores prompt text because
  storing it is the point; an artifact is a byte range somebody else already
  persisted, so aiwatcher keeps the pointer. A producer inlining a scanned
  document into `data` puts it in the durable log and in every projector's
  memory, and an artifact with no `uri` is dropped rather than listed as a row
  nobody can open.
- **Never draw an inferred edge as a message.** Declared edges are what the
  orchestrator promised; `agent.message` is what was said. Sequence is not
  communication, and whether the agents talk is why somebody opens that view.
- **Never let a rerun target come from the log.**
  `AIWATCHER_WORKFLOW_RUNNER_URL` is configuration: a `workflow.declared` naming
  its own callback URL is a request-forgery primitive posted by anything that
  can reach ingest, on a service inside the cluster. `RerunBody` is
  `deny_unknown_fields`, so supplying one is a 400 rather than a field silently
  ignored.
- **Never wire a no-op workflow runner.** A null runner answers `202 Accepted`
  for work no orchestrator was asked to do; absence reaches the caller as a 501
  naming the variable.
- **Never read who published an event from the event.** `published_by` is what
  the ingest route authenticated, and the envelope neither reads nor writes it
  on the wire, so a producer that names a publisher is ignored. A span names one
  only where one credential sent both ends. A witness is only a witness under
  **another** credential: a serving run the application's own token published is
  the application's word again, counted as self-witnessed and said to be so —
  and where a deployment names its witnesses (`AIWATCHER_WITNESSES`), only those
  credentials witness at all.
- **Never read which project an event belongs to from the producer, and never
  skip it off the wire.** `project` is the ingest route's word like
  `published_by` — `POST /api/v1/events` overwrites it on every envelope with
  the credential's scope, including with absence, so a body naming a project is
  discarded rather than honoured (ADR_0001 amended, ADR_0033). It parts from
  `published_by` in one way that decides the design: it **is** serialised,
  because its reader is the projector consuming the bus rather than the process
  that wrote it, and skipping it would read every project's events as global
  behind a broker. An ingest token names its project in its label
  (`name[queue]@<organization-uuid>/<project-uuid>=secret`), which **narrows**
  like the queues and never raises: the role stays hard-coded `Editor`. Absence
  is the global side, which is every event this build has written, and a
  producer publishing straight to the broker is its own word for its project —
  named here rather than left to be discovered.
- **Never let a read of the log's folds not name a side, and never let the
  instance routes answer with a project's rows.** `ReadScope` is `Global |
  Project` and every read of the runs fold takes one; it is not an axis in
  `RunSelection`, because a scope decides which rows exist rather than narrowing
  the ones that do — including for the counts a fold takes before it narrows
  anything. `runs`, `metrics` and `live` each serve one router twice: under
  `/api/v1` for the global side and under
  `/api/v1/orgs/{organization}/projects/{project}` for a project's, the second
  on a fresh grant. Global data stays under instance authorization and project
  data is additive (ADR_0033, amended) — and the half that makes it a boundary
  rather than a label is the other direction: an instance read answers **none**
  of a project's rows, or the additive family added nothing. A scope refusal is
  **404**, as `StoreError::OutOfScope` already is.
- **Never let a live stream outlive the access that opened it.** SSE and the
  WebSocket carry the scope in the subscription and the hub filters, because
  `llm.chunk` is most of the log and narrowing in the browser means sending
  every project's events to every browser to throw them away. The identity is
  the session cookie, which is what ADR_0013 exists for. A scoped stream
  re-asks its grant every 30 s and closes with a `revoked` frame — a stream
  that simply stops is indistinguishable from one where nothing is happening,
  which is the failure ADR_0004 prevents. An unreachable IAM is not an answer
  and closes nothing; what bounds that is that no new stream opens either.
  `Last-Event-ID` widens nothing: the resume is a new request, so the grant is
  asked before a frame is replayed.
- **Never list a client's count as a run.** `client.counted` says how many runs
  a client opened and its `run_id` names the client: it folds into the
  measured-runs counts and into lost runs, never into the runs list, a span or a
  trace.
- **Never let the projector decide a run has died.** A run with no end event
  stays `Running` — the producer may have been killed, or may be thinking for
  twenty minutes. What the read model reports instead is
  `RunSummary::last_event_at`, and the panel draws the line at
  `STALLED_AFTER_MS`, the same fifteen minutes as
  `AssemblerConfig::orphan_timeout`, past which the assembler has already closed
  that run's spans `closed_by=timeout`. A dimension row carries the same fact as
  `running_last_event_at`, over its *running* runs only.
- **Never let a managed run go on looking alive because the engine used its own
  vocabulary.** A managed execution emits no `run.completed`: the engine owns
  the run and ends it with `execution.completed` (ADR_0026), and a runs fold
  reading `Subject::Run` only left Workflows saying `succeeded` while Explore
  spun. This is not the silence the rule above refuses to infer from — it is the
  producer saying it finished, and `execution.failed` says why in `reason`.
- **Never remove the `data.workflow` fallback in `EventEnvelope::workflow`.**
  The `agentic` integration sent the workflow name in the payload before
  `workflow_id` existed; dropping it empties the workflow dimension for every
  log written before the field, including on replay.
- **Never return a whole stream from a read route.** `read_stream_page` is the
  API's; `read_stream` remains for the projector, which needs the whole thing.
- **Never partition the log by `conversation_id`.** One conversation can fan out
  into parallel runs. Partition by `run_id`.
- **The read model's caps are a memory contract.** `AIWATCHER_MAX_SPANS_TOTAL`
  is what keeps the process inside 512 MB; `max_runs × max_spans_per_run` alone
  is not a bound. Re-run `just load-test` after changing any of them, and move
  the container limit with them.
- **The window matches last activity, except on metrics.** `window_seconds` on
  every list means "active in the period" — a run that began three hours ago and
  emitted an event a minute ago is the thing most worth seeing in the last
  fifteen minutes. Metrics windows by start, because there the window is the
  timeline's x-axis. The panel's Query tab forwards the same number only to
  datasets whose route accepts it (`Dataset::$windowed`), since the API rejects
  unknown query parameters.

### Prompts on a trace, and what a call cost

- **Prompt text is not redacted, and neither is a report.** The Collector strips
  `gen_ai.prompt` and `gen_ai.completion` from spans before export
  (`deploy/otel-collector.yaml`); the registry stores prompt text verbatim
  because storing it is the point. A producer that puts a secret in a prompt
  puts it in an object store nothing evicts. Enabling span text needs a
  retention policy, not a config change.
- **Never put a prompt on a span, and never leave a call unable to name one.**
  The text stays off the log; what a call carries is a *reference* —
  `prompt_name` and `prompt_version`, read by `PromptRef::from_data`, written as
  `aiwatcher.prompt.*`. Without it a trace names a model, a temperature and a
  token count while the thing that decided what the model was asked has to be
  found by hand. A malformed version id is *absence*, never a rejection: the log
  takes what a producer sends, and the panel links only what resolves.
- **Never let a request setting have nowhere to go.** `gen_ai.request.*` covers
  temperature, top-p, top-k, max tokens, seed and stop sequences, and
  `request_attributes` reads each off whichever of the start and end events
  carries it — `OpenSpan::close` keeps the first, because the Python SDK
  restates the request on both. A setting that changes the answer belongs in an
  attribute rather than a payload field nothing indexes: comparing two runs is
  the question, and a payload cannot be grouped by.
- **Never show an absent cost as zero.** What a provider charged rides with the
  call beside what the price table would estimate, and the metrics summary sums
  `aiwatcher.usage.cost_usd` into its totals, breakdowns and timeline beside a
  count of the calls that reported one. A model whose calls said nothing about
  money has an unknown cost, and a total over a partial bill says it is partial.
- **Never price a call without the page and the day.** A price is an entry a
  deployment loads (`AIWATCHER_MODEL_PRICES`), one currency for the table, and
  an entry without the page it was read from or the day is refused at start-up.
  A call no entry covers is counted unpriced, never priced at nought, and
  nothing here fetches a price. A table is a history: a call is priced by the
  entry in force on its day, and one older than every entry by the earliest and
  counted as such. Cost is computed at read time and never written into
  evidence, because a price is the deployment's and a result is not.

### The core crate and the bus

- **`aiwatcher-core` gains no dependency on a transport or a store.** If
  something needs one it belongs in an adapter behind a port. `sha2` and `hex`
  are the exception and are not one: `PromptVersionId::of` has to agree byte for
  byte with `hashlib.sha256` on the producer side, unlike `ids::derive`, whose
  only requirement is that every aiwatcher agrees with every other one.
- **Never raise `AIWATCHER_LASER_PARTITIONS` above 1** without replacing the
  scalar `Checkpoint` with a per-partition cursor. A scalar has no total order
  across partitions, so live-stream resume would silently skip events — and
  positions would stop being contiguous, which is what lets the period fold tell
  events retention removed from events that never were.
- **Never switch the Laser consumer to an automatic `CommitPolicy`.** The
  pipeline commits only after a durable write; an automatic policy would move
  the offset past events that were never stored. The cost is redelivery between
  read and commit, which the adapter's local read position absorbs.
