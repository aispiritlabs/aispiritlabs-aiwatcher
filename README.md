# aiwatcher

Observability for AI agent runs. Python and TypeScript agents publish events to
a durable log; a Rust backend consumes them, assembles OpenTelemetry traces,
exports to VictoriaTraces and VictoriaMetrics, and serves a live view over
SSE/WebSocket to a React panel.

The log is what is true. Spans are derived from it rather than written
alongside it, by pure functions of the event stream — so a redelivered event
lands on the span it already wrote instead of a duplicate, and a backend that
learns something new about an old event type can be pointed at the same log
again. Most of the design below follows from that one commitment.

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

The registry is the one exception to the paragraph above, and it is a
deliberate one: a prompt is written by a person, not observed, and the version
a run used has to be readable after that run has been evicted from the log. See
[ADR_0011](docs/ADR/ADR_0011_PROMPT_REGISTRY.md).

## Powered by

<table>
<tr>
<td width="90" align="center" valign="middle">
<a href="https://github.com/apache/iggy"><img src="https://raw.githubusercontent.com/apache/iggy/master/assets/logo/SVG/iggy-apache-sygnet-color-lightbg.svg" height="44" alt="Apache Iggy"></a>
</td>
<td valign="middle">
<a href="https://github.com/apache/iggy"><b>Apache Iggy</b></a> — the persistent
message streaming behind the Laser backend. It is the durable log the projector
reads from, under the <code>laser</code> cargo feature; a plain build uses the
built-in write-ahead log instead and needs no broker.
</td>
</tr>
<tr>
<td width="90" align="center" valign="middle">
<a href="https://github.com/rustfs/rustfs"><img src="https://avatars.githubusercontent.com/rustfs" height="44" alt="RustFS"></a>
</td>
<td valign="middle">
<a href="https://github.com/rustfs/rustfs"><b>RustFS</b></a> — the S3-compatible
object store behind the prompt registry: one Rust binary, no JVM, no external
metadata service. The adapter speaks S3 rather than RustFS, so MinIO, Ceph or a
real bucket work by changing one environment variable.
</td>
</tr>
<tr>
<td width="90" align="center" valign="middle">
<a href="https://github.com/flow-php/flow"><img src="https://raw.githubusercontent.com/flow-php/flow/1.x/web/landing/assets/images/elephant.svg" height="44" alt="Flow PHP"></a>
</td>
<td valign="middle">
<a href="https://github.com/flow-php/flow"><b>Flow PHP</b></a> — the strongly
typed data processing framework behind <code>services/flow</code>, the optional
service serving the panel's Query tab. A query is lexed, whitelisted and turned
into Flow objects through an explicit <code>match</code> — parsed, never
executed.
</td>
</tr>
</table>

## Quick start

Rust 1.98 (pinned in `rust-toolchain.toml`, installed on demand by rustup) and
Node for the panel. No broker, no cluster, no Docker.

```bash
just install    # panel and TypeScript SDK dependencies
just dev        # server on :8080 with an in-memory bus, panel on :5173
just seed       # publish a demo run into it
```

What that gets you, screen by screen: [EXAMPLES.md](EXAMPLES.md).

`just dev` keeps nothing across a restart, which is what makes it fast to
iterate against. For a server whose data survives one:

```bash
just run        # :8080, durable write-ahead log in ./.data
```

## Sending events

The contract is the envelope in
[`contracts/envelope.schema.json`](contracts/envelope.schema.json), not the
client libraries — anything that can produce that JSON and get it onto the log
is a valid producer. The SDKs exist so the common case is three lines, and so
that the two things easy to get wrong by hand are not: event ids that let the
backend deduplicate a redelivery, and a `call_id` that keeps two concurrent LLM
calls inside one agent from collapsing into a single span.

```python
from aiwatcher_sdk import AiwatcherClient  # AIWATCHER_URL picks the transport

client = AiwatcherClient(service="research-service")
with client.run("run-123", conversation_id="conv-1") as run:
    with run.agent("researcher") as agent:
        with agent.llm(model="claude-opus-5", provider="anthropic") as call:
            call.first_token()
            call.usage(prompt_tokens=812, completion_tokens=193)
```

An evaluation is the other thing a producer reports, and it is deliberately not
a trace — no span, no row in the runs list, its own projection:

```python
client.record_evaluation(
    suite="catalog-floor-plan",
    dataset="house-catalog@3",     # what makes two reports comparable
    params={"model": "gpt-5-mini", "threshold": 0.9},
    metrics={"mean_score": 0.88},
    report={"scorer": "catalog-contract-v2"},
)
client.flush()  # delivery boundary for a short-lived evaluation CLI
```

That is the same four pieces as an MLflow `start_run` block, on the client that
is already imported for tracing. `client.evaluation(...)` is the scope form, for
a suite that publishes each case as it scores it.

`sdk/typescript` mirrors it. Unset `AIWATCHER_URL` and both drop everything, so
importing either never breaks a test. An event type the backend does not
recognise is **not** rejected: it is stored and streamed live, and simply takes
part in no span, which is what lets a producer run ahead of the backend. See
[docs/event-catalog.md](docs/event-catalog.md).

## Prompts, and what an optimiser did to them

A prompt is the thing an evaluation is usually *about*, and it is the one
artifact here that is authored rather than observed. So it lives in an object
store rather than in the read model — the version a run used has to outlive
every trace of that run.

```python
registry = client.prompts
prompt = registry.resolve("planner.floor-plan")   # what `production` points at
system = prompt.render(page=page_json, language="pl")
```

`version_id` is `sha256(text)`, so publishing the same prompt twice is one
version and a deploy job can publish on every start. `render` refuses a partial
substitution: a missing value would ship a prompt with a literal `{{ page }}` in
it, which the model reads as an instruction.

An optimiser records what it did, and **the server decides whether it counts**:

```python
from aiwatcher_sdk.integrations.deepeval import record_optimization

record = record_optimization(
    registry, "planner.floor-plan",
    report=PromptOptimizer(...).optimize(...),
    baseline=baseline.version_id,
    dev=scores(dev_before, dev_after),      # what the search ran against
    test=scores(test_before, test_after),   # cases it never saw
    promote=True,
)
record.outcome      # "admitted" | "rejected"
record.reason       # "no_held_out_improvement" | "variables_lost" | ...
record.overfit_gap  # how far dev outran the held-out split
```

A candidate is admitted only when it improves the **held-out** score and still
interpolates every variable the baseline declared. Both refusals matter: an
optimiser selected its candidate by maximising the dev number it then reports,
and one that has quietly stopped mentioning `{{ page }}` scores well on a
harness that fed it fixed inputs. Neither is visible in the score.

The Prompts tab shows the version history, a diff against whatever a version
was derived from, and every optimisation with its dev gain beside its held-out
gain.

## The panel

Every view below, with screenshots of it against real data and what each one is
for: [EXAMPLES.md](EXAMPLES.md).

The product areas are served from aiwatcher's own read model — except authored
artifacts such as Prompts, Datasets and Annotations, which
read the registry. Every one of them carries the same time window — 15m, 1h,
6h, 24h, 7d or everything — in the URL, so a link carries the period with it,
and a run is in the window when it was last *heard from* rather than when it
started:

- **Runs** — the flat list, filterable. A run whose producer stopped talking
  reads as `stalled 22m` rather than as a spinner that never stops: nothing
  here promotes silence to a failure, but a run last heard from before the
  span assembler gave up on it should not still look busy.
- **Explore** — one page for every level. The tree pivots on **session, agent,
  model or tool**; below the root it is always run → span → messages, so
  switching what the top level *is* costs no relearning. Selecting a span
  narrows the messages without collapsing the levels above it, and every
  selection is in the URL. Messages group by span, agent or event type.
- **Metrics** — tokens, latency percentiles, cache hit rate, and ranked
  breakdowns by model, agent and tool.
- **Workflows** — the level above a run: pick an orchestration, see its
  executions, and watch one as a graph. Stages carry their status, duration,
  agents and artifacts; a stage nothing has started is drawn dim rather than
  omitted, which is the whole reason the topology rides the log. Messages
  between agents are drawn as their own kind of edge, never merged with the
  declared ones — sequence is not communication. Rerun asks a configured
  orchestrator to run it again; unconfigured, it says which variable is unset.
- **Evaluation** — suites, reports, per-case scores, and each report against the
  previous one on the same dataset. A mean that improved while a case regressed
  is the thing this view exists to show.
- **Prompts** — versions, the text, a diff against the parent, and every
  optimisation with its verdict. Editable: publishing a new version and moving
  the `production` label are two separate acts, because storing a prompt and
  deploying it are two decisions.
- **Datasets** — immutable, content-addressed versions of cases curated from
  production runs. A promotion can target one session, one agent or any of
  several agents, and keeps the source run/session/trace identifiers. Each
  collection opens in a Hugging Face-style TanStack Table viewer: rows arrive
  in lazy 50-row slices, search runs across the whole version, and tabs show
  every linked evaluation plus the Flow PHP lineage. Evaluations should pin the
  exact reference as `dataset-name@version-sha256`.
- **Data Curation** — two ways to produce a dataset, on one page. Either write
  the transformation in Flow PHP with four explicit stages — test without
  reading, simulate 25 rows, execute the full bounded result, save that exact
  output as a version — or pick a curation workflow the orchestrator already
  holds and set only what, where, which rows and over what period. The second
  is a form rendered from the launch plan's own declared inputs, with the
  page's time window and dataset name already filled in.
- **Annotations** — the labelling tool, and the two things around it. **Label**
  is a canvas over a plan: polygons for rooms, polylines with a thickness for
  wall centrelines, named keypoints for a door's opening, hinge and leaf, typed
  attributes for what a shape says about itself, and links between instances so
  an opening knows which wall it sits on and which two rooms it connects. A
  drawing the registry refuses comes back with *every* problem at once, drawn
  red on the shapes that caused them. **Sources** is a dated table of the public
  floor-plan corpora — what each labels, and whether its licence permits a
  commercial model — with the filter that matters as one click. **Exports**
  freezes the project into an immutable manifest whose id is the string a
  training run records, lists every image it left out with the reason, and
  serves COCO per split. The split key is the *building*, not the image, so a
  plan's mirror never lands on the opposite side from the plan.
- **Experiments** — the same picker over training, evaluation and inference
  workflows: the other three legs of the feature/training/inference cycle,
  started from here and watched in Workflows. The comparison half of the area
  is still a placeholder that says what it is waiting on rather than rendering
  plausible fake rows.

Launching needs the `admin` role, exactly as a rerun does, and an instance with
no orchestrator configured says which variable is unset rather than showing an
empty catalog.

## The Laser backend

`adapters::laser` runs against the real `laser_sdk` 0.3 over Apache Iggy, behind
the `laser` cargo feature so a plain build needs neither the SDK nor a broker:

```bash
just iggy-up      # Apache Iggy in Docker, with the three flags it needs
just run-laser
just test-laser   # six integration tests, ~2s against the real broker
```

Running Iggy takes three settings, and each one fails in a way that does not
name its cause. `just iggy-up` and the Kubernetes manifests set all three:

| Setting | What happens without it |
|---------|-------------------------|
| `seccomp=unconfined` | `Cannot create runtime: Operation not permitted`. Iggy's runtime is io_uring; the default seccomp profiles block it. |
| `IGGY_SYSTEM_SHARDING_CPU_ALLOCATION` | `MemoryAffinityFailed`. The default `numa:auto` binds shard memory to a NUMA node, which fails in a container VM. |
| `IGGY_ROOT_USERNAME` / `_PASSWORD` | The server accepts the connection and closes it mid-login. The client reports a VSR header error and then reconnects forever — it looks like a protocol mismatch and is not. |

Pin an Iggy **0.9.x** server: a 0.8.x one never answers the `iggy` 0.11 client's
login regardless of the above.

## The crates

In dependency order. A crate may only depend on ones above it.

| Crate | Holds |
|-------|-------|
| `aiwatcher-core` | Domain: ids, envelope, correlation, event catalog, ports. Knows nothing about Laser, HTTP or OTLP. |
| `aiwatcher-bus` | `MessageSource` / `MessageSink` / `Checkpointer` + memory, write-ahead-log, Laser and generic-broker adapters |
| `aiwatcher-trace` | `SpanAssembler` and the OTLP/JSON exporters |
| `aiwatcher-prompts` | The prompt registry over an `ObjectStore` port: content-addressed versions, optimisation verdicts, RustFS/S3 and filesystem adapters, and a hand-written SigV4 signer |
| `aiwatcher-annotations` | Vector image annotations over the same `ObjectStore` port: label schemas, content-addressed revisions, review state, the family-keyed split, immutable training exports and COCO, plus a dated table of public floor-plan corpora and their licences |
| `aiwatcher-pipeline` | Pipeline engines behind a `WorkflowEngine` port: an orchestrator's launchable catalog, the inputs each entry declares, and starting one. Flyte 2 over its `/api/v1/` gateway |
| `aiwatcher-auth` | Single sign-on: OIDC discovery, a JWKS cache, the authorization-code flow with PKCE, signed session cookies, authentik's forward-auth headers, group-to-role mapping |
| `aiwatcher-projector` | The pipeline, live hub, read model, dimension and span folds, dedup, retry, dead letters |
| `aiwatcher-api` | axum router: REST, SSE, WebSocket, OpenAPI |
| `aiwatcher-server` | Config, wiring, graceful shutdown. The only crate that knows every implementation exists. |

Around them: `apps/panel` (React), `sdk/python`, `sdk/typescript`, `contracts/`,
`deploy/`, `docs/ADR/`, and `services/flow` — an optional PHP service serving
the panel's Query tab, outside the Cargo workspace and unknown to the Rust
binary.

## The decisions that explain most of the code

Each has an ADR. Read the relevant one before changing that area; the section
that matters in every one of them is what would make the decision wrong.

- **Ids are derived, not generated**
  ([0001](docs/ADR/ADR_0001_EVENT_ENVELOPE.md)). Delivery is at-least-once, so
  `TraceId::derive` and `SpanId::derive` are pure functions of the run id and a
  stable span key. A redelivery lands on the same span rather than a duplicate.
- **An event is not a span** ([0003](docs/ADR/ADR_0003_SPAN_ASSEMBLY.md)).
  Hundreds of events fold into a handful of spans. `llm.chunk` is counted, never
  stored per chunk — streaming a 2000-token reply would otherwise write 2000
  trace records for one call.
- **A reconnect closes its own gap**
  ([0004](docs/ADR/ADR_0004_LIVE_STREAM_RESUME.md)). Every SSE frame carries its
  checkpoint as the `id:`, so the browser resumes through `Last-Event-ID` with
  no application code on either side.
- **Laser sits behind a port, feature-gated**
  ([0002](docs/ADR/ADR_0002_EVENT_BUS_PORT.md)). Nothing above `aiwatcher-bus`
  names it; the default backend is the built-in write-ahead log.
- **One fold slices runs every way, and every list is a cursor page**
  ([0007](docs/ADR/ADR_0007_EXPLORER_DIMENSIONS.md)). Nothing loads a whole run,
  and search runs on the server.
- **An evaluation report is not a trace**
  ([0010](docs/ADR/ADR_0010_EVALUATION_REPORTS.md)). `eval.*` events ride the
  same log and form no span: a report is parameters, metrics and a document,
  folded into its own bounded projection. It is what a producer needs to stop
  running MLflow for four fields.
- **A Flow PHP query is parsed, never executed**
  ([0008](docs/ADR/ADR_0008_FLOW_QUERY_SURFACE.md)). The Query tab's pipeline is
  lexed, whitelisted and turned into objects through an explicit `match` — no
  `eval`, and no name from a query ever becomes a callable.
- **Flow executes curation; the Rust registry versions it**
  ([0014](docs/ADR/ADR_0014_DATA_CURATION.md)). The optional PHP service remains
  stateless; authenticated Rust endpoints save content-addressed script
  revisions and the exact rows a completed transformation produced.
- **A workflow graph is declared on the log, not read from an orchestrator**
  ([0012](docs/ADR/ADR_0012_WORKFLOW_GRAPH.md)). `workflow.declared` carries the
  shape, `step.*` executes a node of it, and `workflow_run_id` joins the stages
  a per-pod orchestrator scatters across four runs. That is what makes a stage
  nothing has started drawable, and what makes swapping the orchestrator a
  change aiwatcher never notices.
- **An annotation is vector-first, and split by family**
  ([0017](docs/ADR/ADR_0017_IMAGE_ANNOTATION.md)). A mask cannot say which wall
  an opening sits on or which way a door swings, so the shape is the source and
  every raster is derived. The split key is the building rather than the image,
  because a catalogue plan, its mirror and its garage variant are four
  renderings of one house and splitting them apart measures memorisation. Usage
  rights are required, and an export excludes what fails its policy by name —
  the best public corpora are non-commercial, and that failure shows up in a
  legal review rather than in a metric.
- **A training run rides the log; an epoch is a point, a step is a count**
  ([0018](docs/ADR/ADR_0018_TRAINING_RUNS.md)). `train.*` is one span and one
  row in the runs list, and the model version an agent run used is then
  traceable back to the export it was trained on. Two hundred epochs are two
  hundred points on a curve rather than two hundred bars in a waterfall, a step
  never reaches the log at all, and a profiler session arrives as a summary and
  a link rather than as fifty thousand spans.

- **The orchestrator is read for its inventory, never for its history**
  ([0016](docs/ADR/ADR_0016_PIPELINE_ENGINE.md)). Nothing publishes an event
  about a workflow nobody has run, and no event carries an input interface — so
  `/api/v1/engine` asks Flyte what it could start while `/api/v1/workflows`
  still folds what has run. A launch binds its inputs to the types the engine
  declares at that moment, always pins a version, and carries an id the panel
  can stream before anything has published.

The full index, including trace storage and the local-Kubernetes guards, is in
[docs/ADR/README.md](docs/ADR/README.md).

## Signing in

Off by default. `AIWATCHER_AUTH_MODE=oidc` makes aiwatcher an OpenID Connect
relying party — built against authentik, and nothing in the code is specific to
it beyond two defaults — and `proxy` reads the identity from an authenticating
reverse proxy that is already in front.

```bash
just authentik-up   # authentik in Docker: server, worker, PostgreSQL, Redis
just run-sso        # the server as a relying party against it
```

The authorization-code exchange happens in the server, the provider's tokens
are read once and dropped, and the browser keeps an HttpOnly cookie the server
signed — because the panel's two most important routes are an SSE stream and a
WebSocket, and a browser can set headers on neither
([0013](docs/ADR/ADR_0013_SINGLE_SIGN_ON.md)).

Roles come from authentik groups and there are three: `viewer` reads, `editor`
publishes prompt versions and events, `admin` dispatches a rerun — the one
route that asks another system to do work. A producer cannot sign in
interactively, so it carries a token instead (`AIWATCHER_TOKEN`), which grants
editor and never admin.

Setting it up, either way round: [deploy/authentik/README.md](deploy/authentik/README.md).

## Deploying

```bash
just detect NS         # what that cluster already runs — reads only
just install-plan      # render and diff; change nothing
just install-cluster   # apply, after asking on any non-local context
```

Installation reads the cluster instead of trusting a flag
([0009](docs/ADR/ADR_0009_INSTALL_BY_DETECTION.md)). A second VictoriaMetrics
beside an existing one splits one workload's metrics across two stores with no
error anywhere — just a gap in a graph. So every backend is
`install | external | none`, never a boolean, and a Collector that detection
merely *found* is never reused: a foreign one almost certainly lacks the
processor that redacts prompt and completion text.

Full walkthrough, including installing beside an existing stack:
[docs/INSTALL.md](docs/INSTALL.md).

## Commands

```bash
just               # every recipe, with what each one is for
just check         # everything CI runs; green here means green there
just test          # cargo test --workspace --all-targets
just test-one PAT  # one test by name, e.g. `just test-one two_parallel`
just openapi       # regenerate contracts/openapi.json and the panel's client
just seed-annotations  # six synthetic plans, three families, an export, a training run
just stack-up      # docker compose: VictoriaTraces, VictoriaMetrics, Collector, Grafana
just tilt-up       # the same stack on a local Kubernetes, rebuilt on save
just flow-check    # the PHP query service's own gate — `just check` excludes it
just authentik-up  # a local identity provider, for `just run-sso`
```

`contracts/openapi.json` is generated from the axum routes and the panel's
client is generated from it, so `just openapi` after any route change and commit
both — CI fails on a stale contract, because a stale client is a runtime
`undefined` rather than a compile error.

Before opening a PR, and what else is expected of a change:
[CONTRIBUTING.md](CONTRIBUTING.md).

## Licence

Proprietary and confidential. No licence is granted — see [LICENSE](LICENSE).
