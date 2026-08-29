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
```

## Commands

```bash
just               # list every recipe
just check         # everything CI runs; green here means green there
just test          # cargo test --workspace --all-targets
just lint          # cargo clippy -Dwarnings
just openapi       # regenerate contracts/openapi.json AND the panel's client
just run           # server on :8080, write-ahead log in ./.data
just dev           # server (in-memory bus) + panel dev server on :5173
just seed          # publish a demo run into a running server
just seed-evaluation  # publish two comparable evaluation reports
just seed-prompts  # publish a prompt plus three optimisations, one admitted
just seed-workflow    # publish two executions of one declared graph
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
| `aiwatcher-prompts` | The prompt registry: content-addressed versions, optimisation records, and the RustFS/S3 and filesystem adapters behind `ObjectStore`. Includes a hand-written SigV4 signer. |
| `aiwatcher-runner` | The workflow rerun dispatcher: one HTTP POST to one configured endpoint, behind `core::ports::WorkflowRunner`. The only thing here that asks another system to do work. |
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

11. **A workflow graph is declared, not discovered**
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
- `prompts` is the one area that reads something other than the log, and the
  one that writes. It answers 501 rather than 404 when no store is configured,
  and `RegistryDisabled` says which variable is unset — an empty list would be
  a different problem with a different fix.
- Any list that can grow with retention is a `useInfiniteQuery` feeding
  `VirtualList` (`src/components/virtual-list.tsx`). A `.map` over a full
  response is only correct for a list with a fixed ceiling.
- An area that exists in the navigation before it exists in the backend renders
  `AreaPlaceholder`, which names what is missing. Never mock data to fill a
  screen — a plausible fake reads as working software.
- `src/components/ui/primitives.tsx` holds the shadcn-style primitives in use
  (button, badge, card, stat, id chip). Radix is not a dependency yet — none of
  those need it. It goes in with the first dialog or select, and TanStack Form
  with the first form, which will be the WebSocket control path (cancel a run,
  approve a tool call).

## Guardrails

- **Never commit the checkpoint before the durable write succeeds.** That single
  ordering in `pipeline.rs::flush` is the at-least-once contract; reversing it
  turns a crash into silent data loss.
- **Never store `llm.chunk` as a trace record.** See ADR_0003.
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
  floor-plan PDF into `data` puts it in the durable log and in every
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
- **Never write a prompt's head before the version it indexes.** Same ordering
  as the pipeline's checkpoint, same reason: an index naming an object that was
  never stored is a list whose rows 404, while an unindexed object is waiting
  for `Registry::rebuild`. The head is derived; the versions are the truth —
  except the labels, which exist nowhere else and survive a rebuild.
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
