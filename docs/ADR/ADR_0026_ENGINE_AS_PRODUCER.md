# ADR_0026: The execution engine is a producer on its own log

- **Status**: accepted
- **Date**: 2026-09-04
- **Decides**: the question ADR_0016 deferred

## Context

ADR_0016 declined to record a launch on the event log, and said why:

> it would make aiwatcher a producer on its own log, and the event catalog is
> the SDK's contract with every agent that publishes.

That was right for what it was deciding. A launch is a request to somebody
else's orchestrator; recording it would have added a catalog entry to describe
an intention, and ADR_0012 had already settled that the *shape* of a graph
comes from the producer's declaration rather than from an orchestrator.

ADR_0025 changes the input to that question. aiwatcher now owns executions
rather than only asking about them, and an owned execution produces facts:
a plan was compiled and started, an attempt ran and completed or failed, a
result was stored. Those are exactly the facts the panel already draws — the
workflow fold, the waterfall, `Pending` nodes, the live SSE with
`Last-Event-ID`, VictoriaTraces, and the `model` and `tool` dimensions. All of
them read the log.

So the choice is not "publish or stay quiet". It is "publish onto the log every
existing fold already reads, or build a second read path in PostgreSQL that
shows the same run a second time".

## Decision

**A managed execution is a producer like any other.** Its facts ride the event
log under `source.service = "aiwatcher-execution"`, and the folds that exist
draw them with no new read path.

| The engine does | It publishes |
|---|---|
| compiles a plan and starts a run | `workflow.declared` — the plan's steps as nodes and its edges, `workflow_id` = the definition, `workflow_run_id` = the execution, `version` = `plan_id` |
| dispatches an attempt, which a reactor or a worker runs | `step.started` / `step.completed` / `step.failed`, `data.node` = the step id, `data.call_id` = the attempt, `data.published_by` = `engine \| worker` |
| stores a result | `artifact.produced` with `uri`, `digest`, `size_bytes`, `node` |
| changes execution state | `execution.requested \| started \| paused \| awaiting_input \| resumed \| completed \| failed \| cancelled` — **new** catalog entries |

`Subject::Execution` joins the catalog and `forms_span` is **false** for every
one of them, for the reason `eval.*` and `workflow.*` already are: an execution
is a record with a lifecycle, not something that happened to a request. The
executions of its *nodes* are `step.*`, and those do form spans.

**What the engine does not publish**: commands, decisions, retries scheduled,
leases, heartbeats. The *why* stays in the workflow stream, which is what the
transactional store is for. The log gets facts about work; the store keeps the
reasoning. This is the line that stops the log the system observes from being
flooded by the system observing it.

**The one strict ordering.** The engine publishes **through the outbox, after
commit** — never from the handler, never before. A `step.completed` on the log
for an attempt the store does not consider complete is the split-brain this
design exists to prevent. It is `aiwatcher_jobs::ORDERING` in the fourth place
it applies, after the pipeline's checkpoint, the prompt registry's head and the
annotation export's manifest.

**Exactly one party publishes a step's events per attempt.** A `local`
execution's reactor publishes for the steps it ran. A worker publishes for the
attempts it ran, through its own client, because it is the process that ran them
and its agent spans nest under them. An `engine:<name>` execution's pods publish
their own and the engine publishes none. `data.published_by` records which, and
the workflow fold flags a node that received two.

## Alternatives considered

**A PostgreSQL execution view, and no catalog entries.** It is the second
implementation this repository refuses everywhere else: the panel would show two
pictures of one run, and the one on the execution page would disagree with the
one in the workflow tab the first time a producer and the engine described the
same node. The store's inline projection stays — for *accepting the next
command*, and for the run's own page and allowed actions — and is never the
source of a list the fold already serves.

**A separate `workflow-events` topic.** Justified only if execution traffic
needs its own retention or access control. Neither is true today, and a second
topic needs its own consumer group, its own checkpoint and its own outbox to be
published safely. Message-type subscriptions in processors do the same job for
free.

**Publish per decision rather than per fact.** This is the failure mode, not an
alternative: one `workflow.declared` per execution, one `step.*` pair per
attempt, one `artifact.produced` per artifact, and nothing else. An engine that
published a scheduling decision would emit more messages than the agents it
watches.

**Reuse `eval.*`-style types instead of adding `execution.*`.** An execution has
`awaiting_input`, `paused` and `resumed`, which no existing subject has, and
overloading `workflow.declared` to also mean "and it finished" would make the
one type carry two unrelated facts.

## Consequences

This is an **SDK release**: eight event types, `Subject::Execution`, a
`forms_span` arm, and a row in `event-catalog.md` for both SDKs. That is the
cost ADR_0016 named, paid once and deliberately, and it buys "who started what,
when, and what came of it" for every execution.

`EventEnvelope.kind` gains its first user. `MessageKind::Command` has been on
the wire since ADR_0001 and constructed nowhere; a workflow command is now
`kind: command` — in the store, never on the log. A command that ever does reach
the log is parked by the dead-letter sink rather than folded, which is the
existing behaviour for anything the fold does not recognise.

A managed execution draws itself in the workflow tab before any panel work
exists for it: `workflow.declared` gives the graph with every step `Pending`,
and `step.*` fills them in. That is the Phase 3 exit in
`docs/PIPELINE_ARCHITECTURE.md`, and it is deliberately reached before the
panel changes.

**What would make this wrong.** A producer that also opens its own `node()`
scope for a step the engine runs — then one node has two publishers, and
`published_by` makes it visible rather than resolving it; the resolution is that
a managed step's producer code does not declare the node it was scheduled as.
The other signal is volume: if execution facts ever outnumber the agent facts on
one installation's log, the engine is publishing decisions somewhere and this
ADR's line has been crossed.
