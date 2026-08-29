# ADR_0012: A workflow graph is declared on the log and folded like everything else

- **Status**: accepted
- **Date**: 2026-08-29

## Context

`planner-mlplatform/app/flyte_pipelines.py` runs the house import as four
stages — `acquire_house_assets_task`, `normalize_house_assets_task`,
`analyze_house_floor_plans_task`, `persist_house_review_task` — chained by an
entrypoint task, `house_import_flow`. Flyte gives each of them its own pod, and
the shared PVC is the hand-off boundary between them.

The same four stages also run with no Flyte at all. `run_house_import_flow`
branches on `settings.flyte_enabled` and, when it is off, calls the identical
four functions in-process, tagging the result `orchestrator="direct"`. A third
path, `orchestrator="cache"`, skips three of them entirely.

That branch is the whole problem. Flyte's console can show the task graph, and
it shows nothing when the branch goes the other way — and nothing about the
agents inside a stage in either case. aiwatcher already sees the agents: every
stage that matters calls `trace_aiwatcher_agent`, which opens a run and an
agent scope. What it could not do was assemble them into a picture.

Four things were missing, and the fourth is the one that made this an ADR
rather than a feature:

1. **Nothing joins the stages.** `trace_aiwatcher_agent` mints
   `client.run(str(uuid4()))` per process, so one import is four unrelated runs
   that happen to share a `workflow_id`. The runs list shows four rows.

2. **Nothing declares the shape.** A projection over observed events can answer
   "what has this done" and can never answer "what has it *not* done yet",
   which is the question somebody watching a pipeline is actually asking. A
   stage that has not started emits nothing, and a thing that emits nothing
   cannot be drawn.

3. **Nothing records agents talking to each other.** The log records nesting —
   an agent calls an LLM which calls a tool — and two agents exchanging work
   through a queue, a PVC or a task graph nest inside nothing at all. "Do these
   agents actually communicate, and how" had no answer in the data.

4. **Nothing links a stage to what it produced.** Each stage hands the next a
   JSON document; `write_analysis_artifact` writes one to storage. None of it
   was referenced anywhere in aiwatcher.

The obvious answer to (2) and (4) is to read Flyte's API: it knows the task
graph and the artifact locations, and it knows them without anybody publishing
anything. That is wrong for one reason that settles it: **the branch above is
in production, and the orchestrator is a thing this codebase expects to
replace.** An observability tool that goes blind when its subject changes
orchestrators is observing the orchestrator, not the work.

## Decision

**The graph is declared by whoever runs it, on the same log as everything else,
and folded into a projection like every other view here.** aiwatcher never
calls an orchestrator to draw anything.

Concretely:

1. **A traversal is identified by `workflow_run_id`**, a new envelope field
   resolved by `EventEnvelope::workflow_run`. `workflow_id` names the graph;
   this names one walk of it. It falls back to `data.workflow_run_id` and then
   to `run_id`, so a workflow that runs start to finish in one process is its
   own execution and its producer sets nothing. A stage-per-pod orchestrator
   passes the same value from every pod, and four runs become one graph.

   Deliberately **not** `correlation_id`, which would have needed no new field.
   It is generated per run when nothing seeds it, so every stage would be an
   execution of its own — and it is not stable across a redelivery that carried
   no `event_id`.

2. **The topology rides the log as `workflow.declared`**, carrying nodes and
   edges. It forms no span: a shape has no duration, and a waterfall showing
   one would be showing the moment a producer got round to describing itself.
   `artifact.produced` is withheld from span assembly for the same reason — an
   artifact is a pointer, not something that happened to a request. The guard
   is `EventType::forms_span`, which `Subject::Eval` already used.

   The version is `sha256` of the canonical topology, so re-declaring is free.
   That is what lets the SDK declare unconditionally on every execution, which
   in turn is what keeps the catalog alive across retention eviction.

3. **A node's execution is a `step.*`**, with `data.node` naming which node of
   the declared graph it is. No new span machinery: a stage has a start, an end
   and a duration, it belongs in the waterfall beside the LLM calls it makes,
   and `Subject::Step` already does all of that. Attempts are counted by
   distinct span key, so a redelivery does not invent a retry.

4. **`agent.message` is a point event on `Subject::Agent`**, carrying `from`,
   `to`, `kind` and `channel`. It becomes a span event on the sending agent's
   span and an edge on the graph. This is the only thing here that could not be
   derived from something already recorded.

5. **A rerun is a dispatch to one configured endpoint**, behind
   `core::ports::WorkflowRunner` with an HTTP adapter in `aiwatcher-runner`.
   The route answers `202`, never a result: what comes back is an
   acknowledgement, and the evidence that the rerun happened is the events it
   publishes onto the same log. Unconfigured, it answers `501 runner_disabled`
   naming the variable — the prompt registry's pattern, for the same reason.

### Why the target is configuration and never an event

A `workflow.declared` naming its own callback URL would be the ergonomic
choice: the producer knows where its orchestrator is, and aiwatcher would not
need a second thing to configure. It is also a request-forgery primitive posted
by anything that can reach ingest. aiwatcher runs inside the cluster, so "POST
this url" is a request to reach the cluster's internal network on the caller's
behalf — `169.254.169.254` and every unauthenticated admin port in the
namespace included. The declaration names a *workflow*; the operator names a
*runner*. `RerunBody` is `deny_unknown_fields` so an attempt to supply one is a
400 rather than a field that is silently ignored and reads as accepted.

### Why the graph is not merged with the messages

Declared edges and observed messages are drawn differently and never combined.
An edge says the orchestrator promised `acquire` runs before `normalize`; a
message says one agent addressed another. Rendering both as the same line would
let a picture claim a handoff that nothing recorded — and the whole reason
somebody opens this view is to find out whether the agents talk.

## Alternatives considered

**Read Flyte's API for the task graph.** It is already there, it is accurate,
and it needs no producer change. It is also wrong the moment the
`settings.flyte_enabled` branch goes the other way, which it does in
development, in the cache path, and in whatever replaces Flyte. It would also
make `aiwatcher-core` grow a dependency on an orchestrator, which the layering
rule at the top of `Cargo.toml` forbids for exactly this class of reason.

**Infer the graph from span parentage and stage ordering.** No new event types,
and it works on data planner already publishes. It can only ever show nesting
and sequence, though — never a peer-to-peer message, and never a stage that has
not started. It answers a weaker question than the one being asked, and a graph
that silently omits the unstarted stages is worse than none: it looks complete.

**A `Subject::Node` with its own start and end events.** Symmetrical, and a
duplicate of `Subject::Step` with a different name. The step's span kind, its
`step_type` escape hatch and its nesting are all exactly what a stage needs.

**Keeping the topology in the prompt registry's object store.** Tempting for
one real reason — a declaration on the log is bounded by retention, so a
workflow whose runs have all been evicted loses its shape. Rejected because the
mitigation is cheaper than the store: the version is a content hash, so
re-declaring on every execution costs nothing and any live workflow keeps
itself in the catalog. A prompt needed a store because a *specific version* has
to be readable after the run that used it is gone; a topology only has to be
readable while the workflow is still being run.

**A WebSocket control path for rerun.** The panel's WebSocket exists and will
carry cancel and approve-tool-call eventually. A rerun is a request with a
response, not a stream, and putting it on the socket would mean inventing a
correlation scheme to match an acknowledgement to the request that caused it.

**A null runner that logs instead of dispatching**, so the route always
succeeds. This is the `NullExporter` pattern from `wiring.rs`, and it is right
there and wrong here: a null exporter drops telemetry aiwatcher already has,
while a null runner would answer `202 Accepted` for work nobody was asked to
do. Absence has to reach the caller.

## Consequences

**Producers have to declare.** A workflow that sets `workflow_id` and nothing
else still appears in the catalog and still gets a graph — of whatever ran —
but its unstarted stages are invisible, which is the feature's main claim. The
declaration is four lines in either SDK and the version makes it idempotent, so
the cost is low; it is not zero.

**The catalog is bounded by retention.** `WorkflowConfig::max_definitions` caps
it at 200 and a definition with a live execution under it is never evicted, but
a workflow nothing has run for long enough disappears from the picker. Its
shape comes back the next time it runs. A deployment that wants a permanent
catalogue of workflows it has stopped running needs the object store this ADR
declined to build.

**A skipped branch is indistinguishable from one that has not started yet.**
Both are `Pending`. The execution's own status resolves — `Succeeded` when
nothing failed, nothing is open and something finished — so a conditional
workflow does not hang in `Running` forever, and the pending count is reported
beside it so "4 of 5 ran" is visible rather than implied. What is not
expressible is *why* the fifth did not: a producer that wants to say "skipped
deliberately" has to emit a `step.completed` with a payload saying so.

**The execution status is a heuristic over three signals**, not something a
producer states. An orchestrator that never sends `run.*` for its stages leaves
the execution `Running` until its nodes finish; one that crashes mid-stage
leaves it `Running` for good, exactly as `RunSummary` does. Adding an explicit
`workflow.completed` would fix it and would also make every producer that
forgot to send one look permanently stuck, which is a worse failure.

**A rerun's effect is invisible until it publishes.** The panel shows the
orchestrator's acknowledgement and then nothing changes on the page: the new
execution appears when its first event arrives, under a new `workflow_run_id`.
That is honest and it does read as a delay.

**Artifact content is never stored, and prompt text is.** Two guardrails that
look inconsistent and are not: storing a prompt *is* the registry's point,
while an artifact is a byte range somebody else already persisted. A producer
that inlines a floor-plan PDF into `data` puts it in the durable log and in
every projector's memory, and nothing in this design stops it.

**What would make this wrong.**

- A declaration that is not idempotent in practice — a producer generating node
  ids per execution, say — would make `max_definitions` churn and the catalog
  useless. The number to watch is distinct versions per `workflow_id` over a
  week; more than a handful means the version is hashing something it should
  not.
- A workflow wide enough that `max_nodes_per_execution` (200) truncates it. A
  graph that large is past what a canvas can show, and the answer would be
  collapsing sub-graphs rather than raising the cap.
- Every deployment configuring `AIWATCHER_WORKFLOW_RUNNER` to the same
  orchestrator would mean the port is a fixed cost buying nothing, and a direct
  integration would be simpler. The evidence would be a second orchestrator
  never appearing after a year.
