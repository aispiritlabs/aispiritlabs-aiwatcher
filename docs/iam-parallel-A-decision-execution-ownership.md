# Decision — durable execution ownership and a scope-bound workflow store

Stream A of IAM-01. Date: 2026-09-18. Status: **accepted for the store and the
start path; the dispatcher it enables is not built.**

This is a stream document rather than an ADR. Four agents are working from one
snapshot and an ADR number picked here would collide with one picked elsewhere;
the integrator promotes this to `docs/ADR/` with a number once the streams are
merged. Nothing in it contradicts ADR_0025 or ADR_0026 — it is the answer to a
question both of them left open, which is *whose* execution a stream is.

## Context

An execution used to be one thing addressable by id: a stream, an inline
projection, a claim table row, a timer row and an outbox row, all reachable from
whichever process held the store. That is correct with one tenant and wrong with
two. Six loops this binary already runs would each pick a project's execution up
and carry it out under no authority at all:

| Loop | What it reads | What it would have done |
|---|---|---|
| reactor (`Reactor::poll_once`) | `claim_attempt` | run a project's step |
| worker routes (`api::worker`) | `claim_attempt`, `attempt` | the same, from another process |
| pod launcher (`server::execution::pods`) | `claimable_attempts` | start a Job for it |
| timer tick (`fire_due_timers`) | `due_timers` | fire its gate's deadline |
| outbox publisher (`publish_pending`) | `pending_outbox` | put its facts on the instance's log |
| retention sweep | `prune` | delete its history on the instance's window |

The recording executor already takes an explicit `ProjectAuthority`, and the
crate README says in as many words where that authority has to come from: *"A
future dispatcher must obtain those values from trusted durable execution
ownership, not the plan, parameters, worker name or declaration writer. That
durable ownership and dispatcher are not implemented here."* This is the
ownership.

## Decision

### 1. An execution has an owner, and the owner is a durable record

`ExecutionOwnership { scope, principal, definition }` —
`aiwatcher_iam::ProjectScope`, the exact `Principal { provider, subject }`, and
what was started (`kind`, `name`, `revision`, `plan_id`). No second scope model
and no email anywhere.

It is written **in the transaction that creates the execution**, from
`AppendRequest::ownership`, which only the append that creates a stream may
carry. Not an object beside the run, not a row written before the start — that
is a claim on an id nothing backs — and not one written after it, which leaves a
window in which the run exists and is nobody's.

It carries the plan binding as well as the principal because the record's whole
job is to be readable *before* the stream is: a dispatcher deciding whether it
may load an execution at all cannot first load it.

### 2. A global execution has no record, and absence is the unscoped side

Every stream this build has ever written is a global one. Inventing an owner for
them would be a migration, and this stage performs none: migration 0011 adds a
table and backfills nothing. `ScopeBinding::admits` reads absence as "the
unscoped store's", so nothing about an existing deployment changes.

The one place absence is ambiguous is an id nobody has used yet, and the binding
takes an `exists` flag for exactly that: an unused id is nobody's *yet*, so a
project store may start a run under it; an execution that **exists** with no
owner is a global run, and a project store adopting one is refused.

### 3. The binding goes on the store, not on each call

`WorkflowStore::for_project(scope)` returns the same store — one lock, one
directory, one pool, one database — narrowed to one project.
`WorkflowStore::scope()` says which side it is on. Rebinding to a second project
is refused; rebinding to the one it holds is the same store.

This is `Artifacts::for_project` and `DefinitionRegistry::for_project`'s shape,
and it was chosen over a scope parameter on twenty port methods for one reason
that matters more than the churn: **the unscoped store enforces the same rule
from the other side.** A parameter would protect callers that pass it. A binding
protects the six loops above without any of them changing a line, because the
store they were handed simply does not see a project's work.

Every per-execution operation refuses out of scope before touching the backend.
Every query across executions — timers due, the outbox, the claim table, the
stranded count, the launcher's read, retention — answers for one side only.

### 4. Ownership is immutable, and a repeated start says so

A later command, a replay, a retry and a duplicate start all assert the record
and none may repoint it. A repeated start whose owner matches is the redelivery
it looks like; one naming a different principal is
`StoreError::OwnershipConflict`, **answered before the inbox** — because
answering it `Duplicate` would tell the second caller the run was theirs.

The handler enforces the same rule at its own duplicate short-circuit, which is
the one place the store's check would otherwise be skipped: the first decision
committed long ago, so nothing reaches `append`. That hole was live until
`tests/scope.rs` found it.

### 5. An execution id distinguishes the project, and global ids do not move

`RunIdentity::of` takes the scope. A global id is derived exactly as it always
was, byte for byte — a pinned test asserts it — so every historical stream stays
addressable. A project folds `<organization>/<project>` in, so:

* one idempotency key in two projects is two independent runs;
* a project's key never addresses a global history;
* a key is still idempotent inside its project;
* two workers that both find one slot due still derive one id.

### 6. What has no scoped form is refused by name

Processor checkpoints and schedule slots are instance-wide: one cursor per
processor, one slot per definition. A project-bound store answers
`StoreError::NotInThisScope` rather than answering for the global one, because a
silent global answer is the fallback this boundary exists to refuse.

### 7. These refusals say the same thing next time

`OutOfScope`, `OwnershipConflict` and `NotInThisScope` all answer
`says_the_same_next_time() == true`. A boundary is not a bad moment: a scheduler
that came back for one would retry it every minute for ever, which is the defect
`StartRefused::says_the_same_next_time` was written to prevent. The API renders
them 404 rather than 503 for the same reason, and because a run somebody may not
reach is a run they do not have.

## What this is not

**Not authorization.** Nothing here asks IAM anything. It says which executions
a store may touch at all, which is the floor a dispatcher stands on when it
later asks whether this principal still holds a grant. A grant check belongs to
the caller, fresh every time, exactly as `ProjectAccess` already says.

**Not a start path.** No route, no wiring and no production caller constructs a
project store. `project: None` is the only value any caller passes today.

## Consequences

* The four workflow-store adapters each gained the binding and a place to keep
  the record: a map (`memory`), a file beside the stream journalled with the
  decision (`file`), a table (`postgres`, migration 0011), a table (`duckdb`).
  One shared rule — `ScopeBinding` — so four adapters cannot answer "is this
  execution mine" four ways.
* A project execution is never pruned by the unscoped retention sweep. Until a
  scoped sweep is wired, a project's history is kept. Stated rather than hidden:
  keeping is the safe direction, and `AIWATCHER_WORKFLOW_RETENTION_DAYS` already
  defaults to keeping everything.
* A project execution's outbox rows are never published by the global publisher,
  so its facts never reach the event log and no fold sees them. That is the
  isolation asked for; it is also the reason a project run has no live view yet.
* Migration 0011 is additive and the release before it runs unchanged against a
  database it has been applied to. A rollback leaves ownership rows with no
  reader, and the old binary treats every execution as global — which is what it
  already believed.
