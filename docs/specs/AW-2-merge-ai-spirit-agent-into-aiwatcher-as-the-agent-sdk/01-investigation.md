`#spec/AW-2` · `#step/investigation` · `#branch/feat-agent-sdk-merge` · repo `aiwatcher`

# AW-2 — Investigation

## Problem

Two repositories describe one system and coordinate by kickoff document.
`ai_spirit_agent` holds an agent SDK whose durable-messaging half is a
re-implementation of what this repository's execution crate already owns, and
whose distributed mode runs on a second broker. aiwatcher holds the shared
history that half is missing, plus a Python SDK whose own
[runtime guide](../../../sdk/python/aiwatcher_sdk/runtime/README.md) names
`AgenticRuntime` as the pattern it follows.

The cost is visible in the tree: `docs/KICKOFF_JOIN_HARDENING.md` opens with
"Two repositories, and the work is mostly in the second", and
`docs/KICKOFF_MID_ATTEMPT_INPUT.md` warns that a `git commit -a` has *already
once* swept one session's work into the other's. §40 is the design for the
join; nothing in it covers where the code lives.

The ask: the agent system becomes the SDK, and agent-to-agent communication
runs on aiwatcher.

## Current state, measured

### What each side already has

| | `ai_spirit_agent` | aiwatcher |
|---|---|---|
| Durable history | `agentic.workflow` on SQLite, one store per process | the execution stream: compare-and-append with `expected_version`, per-execution |
| Decider | `DurableWorkflowExecutor` — inbox by causation, cached decision across OCC retries, `ProcessorLock` | `ExecutionMode::hosted`, `GET/POST …/decider-lease` |
| Transport | `LaserTransport` over Apache Iggy (879 lines), plus two in-process buses | `POST /api/v1/worker/claims`, lease, heartbeat, result, outputs |
| Timers | `Saga.schedule_timeout` / `due_timeouts` / `fire_timeout` | `GET /api/v1/executions/{id}/timers` |
| Content | in the message | `POST …/payloads`, reference + digest + size (§40.4) |
| Prompts | `registry` — text constants plus MLflow registration | ADR_0011: content-addressed versions, labels, `OptimizationRecord::verdict` |
| Tracing | MLflow | the tracer, teed from `agentic_runtime/trace.py` when `AIWATCHER_URL` is set |
| Graph UI | Gradio + PixiJS canvas: Editor, Runner, Events tabs | React Flow: workflow graph, curation pipeline, annotation canvas |

### The bridge that already exists and is not wired

`sdk/python/aiwatcher_sdk/integrations/agentic/` is 1 625 lines:
`AiwatcherEventStore` implements `agentic.workflow.EventStore` over one hosted
execution, `tracer` tees, `graph.declare_graph` publishes a topology,
`payloads` holds the `external` policy. `agentic_graph/join.py` holds
`StreamJoinLedger`, written against that store.

**Nothing constructs either.** A grep for `AiwatcherEventStore` across
`ai_spirit_agent`'s source finds five docstrings and no call site; the compiler
takes `join: JoinLedger | None` and defaults to `MemoryJoinLedger`. The seam is
built on both sides and open in the middle.

### The split inside `agentic.workflow`

Classified by import, 26 files:

- **15 stdlib-only** — `bus`, `conversation`, `decider`, `durable`, `errors`,
  `event_store`, `message_bus`, `message_stream`, `output_handler`,
  `processor`, `routing`, `runner`, `saga`, `sqlite_event_store`, `types`.
  ~2 100 lines, no third-party import at all.
- **11 LLM-coupled** — and mostly through two lightweight types,
  `observability.TraceSnapshot` and `metadata.Description`. Only `reactor`,
  `execution` and `streaming` reach for `CoreAgentic`, `ToolMessage` or
  `ToolRunResult`.

The engine is separable, and its dependency profile is already the one
`CLAUDE.md` demands of anything an instrumented agent imports.

### Sizes

| Package | src | tests | Disposition |
|---|---|---|---|
| `agentic` | 9 973 | 6 033 | **moves** (SDK) |
| `providers` | 3 912 | 2 274 | **moves** (SDK) |
| `agentic_runtime` | 4 867 | 4 419 | **moves**, minus `distributed/` |
| `registry` | 316 | 194 | **retires** into ADR_0011 |
| `evaluation` | 1 523 | 2 337 | **splits** — optimiser moves, scorers stay |
| `core` | 47 | 144 | folds into SDK settings |
| `agentic_graph` | 5 195 | 1 554 | stays; its *capabilities* inform the panel |
| `personal_assistant` | 4 280 | 3 142 | stays — the application |
| `chat`, `cli`, `knowledge_base`, `workshops` | 2 245 | 864 | stay |

## Constraints

1. **Three failure policies, not two.** `CLAUDE.md` names two: telemetry
   swallows and counts (`_dropped`, 50 000-deep queue), registry clients raise.
   Agent messaging is neither — a dropped handoff loses work silently, and a
   raise takes the agent down. It must **persist locally and drain**. This is
   the outbox, and it is the sharpest new component in the merge.

2. **Python floor 3.13+**, decided. `agentic` is `==3.14.*`; `aiwatcher-sdk` is
   `>=3.11` with CI running the floor because planner does. 47 PEP-695
   constructs are the only syntax at stake and nothing 3.14-only was found, so
   the floor is a policy choice rather than a rewrite. It moves planner's five
   SDK pins and the SDK's CI matrix; that cost is real and is stated here
   rather than discovered later.

3. **The content rule is absolute.** §40.4 and ADR_0021: a hop's text never
   reaches the event log. Facts carry `from`, `to`, `kind`, a digest and a
   size. `external` is the default policy and `sealed` the private one; there
   is no `plain`.

4. **`aiwatcher-core` gains no transport.** The merge adds Python, not a Rust
   dependency. Everything new on the Rust side is a route or a store rule that
   already has a home.

5. **Staged removal.** `CLAUDE.md`: never remove in one release what the
   release before it names. Every moved module keeps a re-export in
   `ai_spirit_agent` for one release.

## Options considered

### A. Copy the packages in, keep both running
Cheapest to start, and it makes the duplication permanent: two prompt
registries, two evaluation paths, two tracers, two durable stores, each free to
drift. Rejected — the merge's whole value is that four duplications get
*resolved*.

### B. One distribution — fold `agentic` into `aiwatcher_sdk`
Simplest import story (`aiwatcher_sdk.agentic`), and it breaks the rule the SDK
is built on: the telemetry half depends on nothing and stays on `urllib`,
precisely so it can be imported into a process that already pinned `httpx`.
Folding in `numpy`, `structlog`, `mlx-audio` and the provider layer makes
`import aiwatcher_sdk` expensive for every agent that only wants a span.
Rejected.

### C. A second distribution in this repository — **recommended**
`sdk/agentic/` beside `sdk/python/`, publishing `aiwatcher-agentic`, depending
on `aiwatcher-sdk` and nothing of `aiwatcher-sdk` depending on it. Three
distributions, three failure policies, one repository:

| Distribution | Failure policy | Depends on |
|---|---|---|
| `aiwatcher-sdk` (telemetry) | swallows and counts | stdlib |
| `aiwatcher-sdk` (registries) | raises | `httpx`, `tenacity` |
| `aiwatcher-agentic` | **persists and drains** | `aiwatcher-sdk`, providers |

This is the Rust rule — *adapters are features, not crates* — expressed for
Python as *one distribution per failure policy*.

### D. Move only the stdlib-only engine
The 2 100-line core, nothing else. Genuinely tempting, and it leaves `agentic`
depending on aiwatcher for its store while its runtime, its providers and its
prompt path stay behind — which is the two-repository coordination problem with
a smaller surface, not without one. Rejected as an end state; it is *phase B*
of the recommendation.

## Recommendation

Option C, delivered in six phases. Each ends at something that runs.

### Phase A — wire the seam that is already built
No code moves. In `ai_spirit_agent`: construct `AiwatcherEventStore` +
`StreamJoinLedger` in the compiler when `AIWATCHER_URL` is set; pass
`create_tracer()` into `build_compiled_graph_system` (§40.2 calls this "one
line and the highest-value change in the repository"); call `declare_graph` on
start. **Acceptance:** a fan-out of three joins across a worker restart, and
the panel draws the graph with `Pending` nodes and the dispatches as messages.

### Phase B — prompt optimisation onto aiwatcher
Asked for first, and well-bounded. `registry/prompts.py`'s MLflow registration
retires into `aiwatcher_sdk.prompts`; `evaluation`'s MIPROv2 optimiser reports
through `record_optimization` instead. The optimiser currently reports the
score it selected on, which ADR_0011 refuses to admit — so this phase adds the
held-out split, and the first honest `verdict` is its acceptance.
**Acceptance:** a candidate admitted on a held-out score and one refused for
`variables_lost`, both visible in the panel's prompts area.

### Phase C — the engine moves
`sdk/agentic/`, floor 3.13, the 15 stdlib-only files first, then the
LLM-coupled half once `TraceSnapshot` and `Description` are inverted onto
protocols. `ai_spirit_agent` re-exports for one release.
**Acceptance:** `just sdk-check` green; `make test-agentic` green unchanged.

### Phase D — the outbox
The new component. `memory` and `duckdb` tiers, mirroring the Rust store's
`memory | file | postgres | duckdb`. Append locally, commit, drain, delete —
`aiwatcher_jobs::ORDERING` in a second language. A human can list and drain it
by hand, because a queue nobody can inspect is a queue nobody trusts.
**Acceptance:** aiwatcher stopped mid-conversation, the agent keeps answering,
and every hop arrives once when it comes back.

### Phase E — one composition root, and the standalone agent
`AgenticRuntime` and `aiwatcher_sdk.runtime.Runtime` become one. The wrapper
the merge needs lands here: an agent registrable as a `WorkflowDefinition` of
kind `agent` on its own, so it is launchable from `POST /api/v1/executions`,
schedulable, and visible in Workflows without a graph around it.
**Acceptance:** one agent started from the panel, on a schedule, with no
`agentic_graph` involved.

### Phase F — `AiwatcherTransport`, and Iggy comes out
§40.6: publish → append; consume → a claim with the queue equal to the target
agent; ack → complete; `XAUTOCLAIM` → lease expiry; dead letter → the sink.
Lab 6's three workers share one history and `laser-sdk` leaves
`agentic_runtime`'s dependencies. **Acceptance:** lab 6 green with no broker
running.

### The GUI, reviewed rather than moved
`agentic_graph` is Gradio plus a 475-line PixiJS canvas. Three capabilities are
worth having and one implementation is not:

- **Author an agent graph** — the panel has no authoring canvas for one, and
  ADR_0024's pipeline canvas is the precedent for how it should behave: the
  panel authors and links, the server decides and refuses.
- **Run overlay** — the Runner tab lights nodes as they execute. The panel
  already does this for a managed run, from `step.*` on the stream, and §40.2
  notes the payoff: *a fan-out that is serial today is visible as serial*.
- **Event inspector** — the Events tab, which is Observability's live view
  scoped to one execution.

The implementation stays behind: the panel's React Flow canvases already carry
the shared visual language, and a second canvas technology would be a second
place every graph decision has to be made.

*Corrected by the review ([gui-review.md](gui-review.md), 2026-09-11):* the
Runner tab does not light nodes as they execute — it lights all of them before
a run and all of them after it. The panel's Workflows view is the only live
per-node view there is.

### Workshops
`workshops` stays in the application and gains aiwatcher as the thing it
observes. A lab that runs three agents and shows the traces, the join and the
outbox draining is a better lab than one that shows a broker.

## Open questions

1. **Does `providers` move whole, or does the SDK take a provider port and
   leave MLX behind?** `agentic` imports it directly, and `mlx-audio` is a
   darwin-only dependency that has no business in a Linux worker image. Likely
   answer: the port moves, the MLX adapter stays — but that is Phase C's call,
   made against the real import graph.
2. **What `kind` does a standalone agent register as?** `agent` beside
   `agent_graph`, or one kind with a single-node graph. The second is fewer
   concepts and may read as a workaround; §40.5 already lists what a graph
   declaration needs and the answer should fall out of that list.
3. **Does the outbox belong to `aiwatcher-agentic` or beneath it?** The Rust
   side has one outbox rule and two callers. If a non-agent worker wants the
   same durability, this is `aiwatcher_sdk.outbox` rather than the agent
   distribution's private component.
4. **Which `evaluation` scorers move.** The DeepEval bridge is already
   structural in `aiwatcher_sdk`; the MLflow scorers read completions, and this
   plan moves no content.

## Log
- 2026-09-10 — investigation written; both trees read, sizes and the
  `agentic.workflow` split measured rather than estimated
