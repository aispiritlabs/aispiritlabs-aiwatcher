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
   pipeline engine ──► Flyte 2       what could be started; read, and asked
   dataset hubs    ──► Kaggle, HF    what exists; never what is permitted
```

## Commands

```bash
just               # list every recipe
just check         # everything CI runs; green here means green there
just test          # cargo test --workspace --all-targets
just lint          # cargo clippy -Dwarnings
just openapi       # regenerate contracts/openapi.json AND the panel's client
just run           # server on :8080, write-ahead log in ./.data
just run-hubs      # the same, with Kaggle/Hugging Face dataset search on
just dev           # server (in-memory bus) + panel dev server on :5173
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
just stack-up      # docker compose: VictoriaTraces, VictoriaMetrics, Collector, Grafana
just tilt-up       # the same stack on a local Kubernetes, rebuilt on save
```

Installing into a cluster that is not a scratch one:

```bash
just detect NS         # what that cluster already runs — reads only
just install-plan      # render and diff; change nothing
just install-cluster   # apply, after asking on any non-local context
just images            # build aiwatcher and aiwatcher-panel
```

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

With an object store, for the prompt registry:

With an orchestrator, for launching registered pipelines:

```bash
just run-flyte     # the server with the Flyte engine wired to a local control plane
just test-pipeline # the adapter, then the whole stack, against a stand-in Flyte admin
```

```bash
just rustfs-up     # RustFS on :9010
just run-rustfs    # server with the registry in the object store
just test-rustfs   # five integration tests against it — this is what verifies the SigV4 signer
```

The Python SDK is a `uv` project of its own:

```bash
just sdk-install   # uv sync --all-groups
just sdk-check     # ruff format --check, ruff check, mypy --strict, pytest
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
| `aiwatcher-runner` | The workflow rerun dispatcher: one HTTP POST to one configured endpoint, behind `core::ports::WorkflowRunner`. |
| `aiwatcher-pipeline` | Pipeline engines behind `core::engine::WorkflowEngine`: the orchestrator's launchable catalog, the inputs each entry declares, and starting one. Flyte 2 over its `/api/v1/` gateway, plus the literal encoder that binds a form's JSON to Flyte's declared types. With the runner, the second and last thing here that asks another system to do work. |
| `aiwatcher-auth` | Single sign-on: OIDC discovery, a JWKS cache, the authorization-code flow with PKCE, HMAC-signed session cookies, authentik's forward-auth headers, and the group-to-role mapping. Knows nothing about axum. |
| `aiwatcher-projector` | The pipeline, live hub, read model, dimension, span, evaluation and workflow-graph folds, dedup, retry, dead letters |
| `aiwatcher-api` | axum router: REST, SSE, WebSocket, OpenAPI |
| `aiwatcher-server` | Config, wiring, graceful shutdown. The only crate that knows every implementation exists. |

Everything else: `apps/panel` (React), `sdk/python`, `sdk/typescript`,
`contracts/` (the OpenAPI document and the envelope JSON Schema), `deploy/`
(the Dockerfiles, the docker compose stack, the kustomize test stack, and
`helm/aiwatcher` + `helmfile.yaml.gotmpl` + `scripts/` — the install path),
`docs/ADR/`, and `services/flow` — an **optional** PHP service serving the
panel's Query tab. It is outside the Cargo workspace and the Rust binary does
not know it exists; `just check` does not cover it (`just flow-test` does).

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
   a `data_frame()->…` pipeline, which `services/flow` lexes with
   `token_get_all()`, checks against a whitelist, and turns into Flow objects
   through an explicit `match` — no `eval`, and no name from a query ever
   becomes a callable. Syntax errors come from Mago, which reads the query after
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

## Conventions

### Rust

- MSRV 1.98, edition 2024, pinned in `rust-toolchain.toml`. The floor comes from
  the `laser` feature: `laser_sdk` 0.3 requires rustc 1.97.1.
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

`sdk/python` is a `uv` project with its own `pyproject.toml`, and it has **no
runtime dependencies** — it is imported into agent processes that already have
opinions about `httpx` and `pydantic` versions. `uv.lock` is committed because
it pins only the dev toolchain. `just sdk-check` runs `ruff format --check`,
`ruff check`, `mypy --strict` and `pytest`; CI runs the same on Python 3.11,
which is the floor `requires-python` claims. The lint set is the one `planner`
selects, deliberately: the two repositories are worked on together, and a lint
that fires in one and not the other is a lint people learn to ignore.

The telemetry client and the registry client have **opposite** failure
policies, and that is the design: telemetry must never take an agent down, so
`HttpTransport` swallows and counts; reading the prompt a service is about to
run on is the work, so every `PromptRegistry` method raises.
`aiwatcher_sdk/integrations/deepeval.py` never imports deepeval — it reads the
report structurally, so the SDK stays dependency-free and a DeepEval release is
not an SDK release.

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
- Routes are grouped by product area. `observability.*` is a layout route with
  its own sub-navigation; `evaluation`, `prompts`, `datasets` and `experiments`
  are the other four areas. `/` and `/observability` redirect rather than
  render, so old links keep working.
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

## Guardrails

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
  role is hard-coded in `identity_from_ingest_token` and never comes from the
  group mapping.
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
- **Never remove the `data.workflow` fallback in `EventEnvelope::workflow`.**
  The `agentic` integration sent the workflow name in the payload before
  `workflow_id` existed. Dropping the fallback empties the workflow dimension
  for every log written before the field, including on replay.
- **Never let query text reach a callable in `services/flow`.** The name selects
  a `match` branch, is never called by string, and there is no `eval` anywhere in
  the service. `tests/Dsl/ParserRejectionTest.php` is the list of things that
  must keep failing; adding to it is cheap and is the point.
- **Never offer Flow's loose comparisons in the whitelist.** In Flow 0.43
  `equals` matches null against anything and `notEquals` drops nulls. Every
  column in every dataset is nullable, so both silently return the wrong rows.
  See `Whitelist::DECLINED`.
- **Never make the security boundary depend on Mago.** It is a dev dependency
  and may be absent. It reports syntax; `src/Dsl` decides what runs. `just
  flow-check` is the service's own gate (format, lint, tests) — `just check`
  does not cover PHP.
- **Never expose the Flow service without authentication.** It has none. The
  parser bounds what a query can say, not who may ask, and `just flow-serve`
  binds it to localhost.
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
