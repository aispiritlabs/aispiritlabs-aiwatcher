# ADR_0025: A managed execution is owned by the server, and the browser only asks for one

- **Status**: accepted. The `engine:<name>` owner and the Flyte port it
  delegated through were removed by
  [AW-4](../specs/AW-4-retire-flyte-and-run-steps-in-pods-of-our-own/_index.md)
  (2026-09-11), with ADR_0016 superseded: the owners are `local` and `worker`,
  and any other owner read back is kept as unknown, never scheduled or decided.
  The decision stands.
- **Date**: 2026-09-04
- **Supersedes**: the execution half of ADR_0024, and ADR_0014's
  browser-mediated persistence for managed runs

## Context

ADR_0024 decided that the panel drives a curation chain: it compiles the source
and transform blocks into one Flow query, sends the rows to the notebook
runtime, and publishes what comes back. That was the right call for the thing it
was deciding — three engines, two of them optional services the aiwatcher binary
does not know exist, and a screen whose whole value is watching a chain run
block by block.

Its own Consequences section names what would make it wrong:

> Somebody needing a chain to run unattended, on a schedule, or over a corpus
> too large for one browser session.

All three arrived at once, and not from curation. planner runs a four-stage
house import that takes minutes per stage, one pod per stage, and today needs
Flyte in the cluster to do it. `ai_spirit_agent` runs agent graphs whose joins
and human steps need to survive a restart. And the personal-data curation the
block model was built for is a corpus large enough that "keep the tab open" is a
requirement nobody can meet.

Three further facts were already true and constrain the answer:

* **The durable-job rules exist.** `aiwatcher-jobs` (ADR_0022) already owns the
  lease, the retry decision, the shard-before-cursor ordering and the content
  address. A second copy of any of them is the drift that ADR exists to prevent.
* **An external engine already has a port.** `WorkflowEngine` and its Flyte
  adapter (ADR_0016) read an orchestrator's inventory and start one entry. That
  is a different job from owning an execution, and it is not made redundant by
  this.
* **The browser cannot be a workflow worker.** A tab has no lease, no retry
  budget, no checkpoint and no identity that survives a refresh. Everything
  ADR_0024 asks it to do between two blocks is lost when it closes.

## Decision

**aiwatcher owns durable execution, in Rust.** A managed execution is requested
through the API, compiled to an immutable `ExecutionPlan`, and run by the server
against a transactional store. The panel authors definitions, edits code,
sends commands, follows links and renders state; it does not compile, sequence,
retry, resume or publish a managed run.

Four records, and they are different things (section 5.3 of
`docs/PIPELINE_ARCHITECTURE.md`):

| Record | Mutability |
|---|---|
| `CurationPipelineDefinition` / `WorkflowDefinition` | editable |
| `DefinitionRevision` | immutable, content-addressed — unchanged from ADR_0024 |
| `ExecutionPlan` | immutable, derived, addressed by `plan_id` |
| `ExecutionRun` | one attempt to execute one pinned plan |

`plan_id` is a **second** digest, over the executable fields only — steps,
bindings, parameters, pinned code. The authored revision still digests the whole
request, positions included, because that is what `produced_by` names and what a
person saved. Two pipelines differing only in layout compile to one `plan_id`,
so a canvas tidy-up invalidates no cache.

**Decisions are pure; effects are reactors.** `decide` performs no I/O, reads no
wall clock and generates no random values: time, ids and resolved policies
arrive in the input. Flow calls, notebook calls, object-store writes, engine
launches and dataset publication are effects that report facts back.

**The workflow history is transactional and is not the log.** One workflow input
is handled in one transaction: deduplicate the input, check the expected stream
version, append the outputs, update the inline run/step projection, write the
outbox, advance the processor checkpoint. Behind a `WorkflowStore` port with
three adapters in the pattern this repository already uses twice —
`memory | file | postgres`. A run needing more than one process refuses to start
on `file` and names `AIWATCHER_WORKFLOW_STORE`, so a development store never
becomes a production one by omission.

**The owner of an execution owns its retries.** `ExecutionOwner` is `local`
(the Rust decider), `engine:<name>` (delegated whole to Flyte through the
existing port) or `worker` (a hosted decider). Exactly one party retries a given
piece of work; a `ContainerJob` therefore sets `backoffLimit: 0`, and an
engine's phase is shown beside the status folded from the log and never merged
into it.

**ADR_0024's block vocabulary, its chain validation and its content-addressed
revisions stay.** What is withdrawn is the sentence "the chain is driven from
the browser", for managed runs. The ad-hoc path stays: Observability Query, Flow
validation and an explicit editor test still call the services directly, and are
labelled as not durable.

## Alternatives considered

**Keep the browser as the driver and add a queue behind it.** The tab would
still hold the sequencing state between two blocks, so a refresh would still
lose a run — the queue would move where work is buffered, not who is
responsible for it.

**Delegate everything to Flyte.** ADR_0016 read the orchestrator for its
inventory precisely because aiwatcher must work when there is none, and planner
uses a fraction of Flyte that does not justify the component in its chart. Flyte
stays for what a worker cannot do; it is not the floor.

**Put the workflow stream in the object store.** Compare-and-append over S3
needs a conditional put, and RustFS's `If-None-Match` / `If-Match` handling is an
open issue on both the read and the write side (rustfs/rustfs#791, #1458).
A compare-and-swap that works on MinIO and not on the store this system ships is
a compare-and-swap that works in CI. `aiwatcher-jobs` gets away without one
because a job has a single writer under a lease and a cursor that only moves
forward; a workflow stream has a decider, a reactor and a worker racing to
append.

**Put the workflow stream on Iggy.** It offers ordered offsets, consumer groups
and at-least-once delivery — and no append-with-expected-version and no
cross-topic transaction. Treating an offset as an aggregate version leaves a
dual-write gap between the broker and the state. Revisitable behind the port if
that changes.

**Move the definitions into PostgreSQL too.** Revision 1 of the plan did. It
would make a definition readable exactly when a dataset version naming it is
not, and the registry already versions by content and writes the version before
the head. PostgreSQL gains only what is *derived*: the compiled plan.

**One generic `Job` in `aiwatcher-jobs` instead of a second machine.** An export
counts exclusions by policy reason and an import counts rejected rows by what
was wrong with them; ADR_0022 already refused to flatten those. An execution is
a third shape again — a graph with per-step attempts. What it owes that crate is
that it *keeps the rules*, and it calls them rather than copying them.

## Consequences

PostgreSQL enters the system, for execution only. ADR_0009's
`install | external | none` applies to it as to every other backend, and one
already runs in the `planner` namespace this system installs into. Moving the
observability read models into it is a separate decision with its own gate; the
in-memory fold, its caps and `rebuild_on_start` are unchanged by this.

The binary grows a second process role. `aiwatcher serve` holds the API, the
store and the object store and opens no socket to Flow, marimo, an engine or the
cluster; `aiwatcher work` holds the consumers, the workflow processor and the
reactors and is the only role that reaches those. This is how ADR_0008's "the
binary does not know the optional services exist" survives — as a statement
about the *API*, which is where it was load-bearing.

The panel loses code it currently owns: block ordering, Flow script compilation,
the run loop and publication of a managed result. `lib/pipeline.ts`'s `orderOf`
remains a *traversal* for drawing, and never an explanation of a refusal — the
registry's 422 carries every problem, exactly as the annotation canvas does not
re-implement the shape validator.

A curation authored as a chain still compiles to a graph. The plan uses nodes
and edges from the start so an agent or ML workflow needs no second execution
record; the compiler refuses a shape a runtime cannot run rather than asking the
panel to guess.

**What would make this wrong.** A measurement showing the transactional store is
not needed: if the decider, the reactors and the workers turn out never to race
— one writer per execution, always — then the shard-and-cursor discipline
`aiwatcher-jobs` already has would cover this, and PostgreSQL would be a
component installed for a race that does not happen. The other signal is the
opposite one: workflow traffic large enough that one PostgreSQL is the
bottleneck for both the claim queue and the stream, at which point the claim
queue is what moves to a topic (the plan's section 11.1 says what would make
that right).
