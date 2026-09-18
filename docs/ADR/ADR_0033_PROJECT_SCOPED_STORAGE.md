# ADR_0033: A project's data is reached through a bound store, and an execution carries a durable owner

- **Status**: accepted
- **Date**: 2026-09-18

## Context

IAM-01 gave this system organizations, teams, projects and grants
([the IAM crate's README](../../crates/aiwatcher-iam/README.md)), and then
isolated one authored registry at a time: datasets, prompts, training, models,
annotations, authored evaluations, workflow definitions, case reviews, cohorts,
recordings, bundles, approvals, calibrations, declarations. Each of those
boundaries was the same shape — `for_project(scope)` on a registry, a prefix per
project, and refusals that name the scope rather than answering for the wider
one.

What was left was everything that **runs**: an execution's history, its claim
table, its timers, its outbox, its artifacts and the measurements a project
declares over its own judges and scorers. Four streams took those four halves at
once, from one snapshot; what is left of their reports is
[the IAM-01 kickoff](../iam-01-kickoff.md).

Two things forced a decision rather than a fourth repetition of the pattern.

**An execution is not a registry.** A dataset version is written by whoever asks
for it; an execution is picked up by loops nobody asks — a reactor claiming a
row, a launcher starting a pod for one, a timer firing a gate's deadline, the
outbox publishing a fact, a retention sweep forgetting a run. Six of them, all
holding a store they were handed. Isolating an execution by asking every caller
to pass a scope would protect the callers that passed it.

**Two stores describe one artifact.** The bytes are written by
`aiwatcher-server` and the manifest, the lineage pointer and the cache entry by
`aiwatcher-execution`. Two crates spelling out one key layout is how a manifest
ends up under one prefix and its bytes under another — a catalog row nobody can
open, which reads exactly like a working one.

## Decision

**1. The scope binds the store, not the call.**

`WorkflowStore::for_project(scope)`, `ObjectArtifactCatalog::for_project`,
`Artifacts::for_project`, `Registry::for_project_evidence` all return the same
backing store — one lock, one directory, one pool, one bucket — narrowed to one
project. `scope()` says which side a handle is on. Rebinding to a second project
is refused; rebinding to the one it holds is the same store.

The consequence that made this the decision rather than a style: **the unscoped
handle enforces the same rule from the other side.** A per-execution operation
on another scope's execution is refused before the backend is touched, and a
query across executions — timers due, the outbox, the claim table, the stranded
count, the launcher's read, retention — answers for one side only. The six loops
above needed no change at all: the store they hold does not see a project's
work.

**2. An execution has a durable owner, written with it.**

`ExecutionOwnership { scope, principal, definition }` — an `aiwatcher_iam::ProjectScope`,
the exact `Principal { provider, subject }`, and what was started. It lands in
the transaction that creates the execution: not a row written before the start,
which is a claim on an id nothing backs, and not one written after it, which
leaves a window in which the run exists and is nobody's. It is immutable; a
repeated start naming a different principal is a refusal, answered before the
inbox, because answering it `Duplicate` would tell that caller the run was
theirs.

It carries the plan binding as well as the principal because its whole job is to
be readable *before* the stream is: a dispatcher deciding whether it may load an
execution at all cannot first load it.

**3. Absence is the global side.**

A global execution has no ownership record, and a global object has no `scopes/`
segment. Every stream and every artifact this build has ever written is one, so
no backfill is performed and no existing deployment changes. The one ambiguity —
an id nobody has used — is resolved by asking whether the store holds any
history for it: unused is nobody's *yet*, and a project may start a run under
it; an execution that **exists** with no owner is a global run, and adopting one
is refused.

**4. The scope is outside the digest, and inside the key.**

The same bytes in two projects are one content address and two URIs. A reference
carries its scope in its key rather than in a field beside it, so a reader that
resolved the key is a reader that already passed the boundary. `layout` is the
one place that says what a key looks like, read by the catalog and by the byte
writer.

**5. An execution id distinguishes the project; a global id does not move.**

`RunIdentity::of` takes the scope. Global derivations are byte-identical to what
they always were, so every historical stream stays addressable. A project folds
`<organization>/<project>` in, so one idempotency key in two projects is two
independent runs, a project's key never addresses a global history, and two
workers that both find one slot due still derive one id.

**6. None of this is authorization.**

Nothing here asks IAM anything. A bound store says which objects a caller may
touch at all; whether that principal still holds a grant is IAM's answer, asked
fresh, by the caller, *before* it asks the store anything — the cache lookup
included, because a hit is an answer about a project's data whether or not any
work follows.

**7. What has no scoped form is refused by name.**

Processor checkpoints and schedule slots are instance-wide: one cursor per
processor, one slot per definition. A bound store answers
`StoreError::NotInThisScope` rather than answering for the global one. An
unsupported migration family is named with its object count rather than read as
empty. Silence is the fallback this boundary exists to refuse.

## Alternatives considered

**A scope parameter on every port method.** Twenty methods across four workflow
adapters, and each caller choosing correctly. It loses the property that decided
this: the unscoped path would answer whatever it was asked, so the reactor, the
worker, the launcher, the timer, the publisher and the sweep would each be one
forgotten argument away from carrying out a project's work.

**A scoped decorator around the store.** It protects callers that use it and
leaves the unscoped store answering everything — the same hole, one layer up.

**Ownership on the `StartExecution` command, in the stream.** It is already
durable and already immutable, and it needs no second table. But it is only
readable by loading the stream, which is the one thing a dispatcher must not do
before it knows whether it may; and a command is a document, so a second start
could carry a different one.

**A separate ownership object in the object store.** Written before the start it
is a claim on an id nothing backs; written after it, the run exists unowned in
between. Neither is a transaction with the decision, which the workflow store
already has.

**Backfilling existing executions and artifacts into a default project.** It
would rewrite `ArtifactRef.uri` inside manifests, cache entries, receipts,
execution streams, `artifact.produced` events on the log and dataset versions.
The global space stays readable and nothing falls out of it, so leaving it where
it is costs nothing — which is why the migration tool copies rather than moves
and refuses a cutover while eleven families have no adapter.

**One `StoreError::OutOfScope` per owner.** Two streams each added a variant of
that name for their own half. They answer the same question the same way —
`says_the_same_next_time` is `true` for both — so one variant carrying a
sentence is the reconciliation, and the API renders it 404: not 503, which
promises a retry for a boundary that never moves, and not 502, which claims
something upstream failed.

## Consequences

* A project's execution is never pruned by the unscoped retention sweep, and no
  scoped sweep is wired. Keeping is the safe direction and the default window
  already keeps everything, but a deployment that turns retention on reclaims no
  project history until one exists.
* A project's facts never reach the event log, because the global publisher does
  not see its outbox rows. That is the isolation asked for; it is also why a
  project run has no live view, no span and no fold.
* A project's artifacts are measured per project and collected by nothing. There
  is no scoped source of truth about reachability while project execution
  history does not exist, and a pass that deleted what no *global* history names
  would delete a project's bytes for the absence of a global run.
* Metric cardinality grows by one series per project that holds artifacts.
* A schedule remains instance-wide, so a project has no unattended run.
* PostgreSQL migration 0011 is additive and backfills nothing: the release
  before it runs unchanged against a database it has been applied to, and a
  rollback leaves ownership rows with no reader — which is what the old binary
  already believed about every execution.
* **No production caller constructs a bound store.** Every `StartRun` passes
  `project: None`, no executor is registered for a project, and there is no
  project `/start`. This ADR describes a floor, not an open path.

**What would make this wrong.**

Binding the store is right while one process holds one scope at a time. The
first thing that would break it is a **dispatcher serving many projects in one
loop**: it would bind per attempt, and a bind that costs a pool, a directory
handle or a connection per claim is a bind that has to become a parameter after
all. The number to watch is binds per second against the cost of one.

The second is a **project that needs the event log** — a live view, a span, a
metric. Today isolation is achieved by its facts not being published at all. A
scoped publisher, a scoped projector and a scoped read path are three more
boundaries, and if they cannot be built without a per-tenant fold then the
premise that one instance serves many projects is the thing to revisit, not this
decision.

The third is **retention**. A store that grows without a sweep is a store that
eventually fails for a reason that has nothing to do with scope.
