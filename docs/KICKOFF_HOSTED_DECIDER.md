# Kickoff: the hosted decider (Phase 13)

Written 2026-09-09, after section 6 of [KICKOFF](KICKOFF.md) closed. The design
is [architecture](PIPELINE_ARCHITECTURE.md) §40 and the phase scope is Phase 13;
this document is what a session needs to start, not a second design.

This session is in **aiwatcher** (Rust and the Python SDK) and in
`ai_spirit_agent`. It shares no file with the other open session — Planner's
`docs/flyte-removal-kickoff.md`, which is entirely in that repository — so the
two can run at the same time.

## Why this and not container jobs

Phase 12 needs *a concrete need for one pod per stage*, and Planner does not
have one: its resource declaration was a single value for all four tasks, its
cache was disabled, and its retries and timeouts were never set. Phase 13 is
different — it is the only item left that delivers something Flyte never did.

`agentic_graph`'s join lives in process memory on `CompiledGraphSystem` —
`_expected_completion_counts`, `_completion_buckets` and `_last_inputs`
(`compiler.py:188-194`). A fan-out of three whose worker restarts between the
second and third completion loses it, and there is no store it could live in:
lab 6 gives each agent worker its own private SQLite, so two workers share no
history at all. That is the gap, and it is the only one worth filling —
**nothing in `agentic` needs rewriting to use this** (§40.1).

## What already exists, checked in the tree

**Steps 1 and 2 below have since landed, so read this section as history.** The
append route and the decider lease are built: `hosted.rs` holds `HostedAppend`,
`DeciderLease` and `LeaseOutcome`, `POST /api/v1/executions/{id}/stream` serves
the append, and the lease is proved by the storage contract on all three
adapters rather than by one adapter's test. `ExecutionOwner::Worker` and
`ExecutionMode::Hosted` now have a producer — `executions.rs` maps a worker
target onto them.

What is left is steps 3, 4 and 5, plus one thing this document decided and
nothing enforces: `PayloadPolicy::needs_archive` has no caller, so a definition
choosing `sealed` without `AIWATCHER_CONVERSATION_ARCHIVE` and
`AIWATCHER_CONVERSATION_KEYS` is not refused. [Architecture §28's *What is
left*](PIPELINE_ARCHITECTURE.md#what-is-left) is the current list.

What *is* built and is the foundation: the atomic six-write transaction, the
inbox keyed by message id, expected-version append, the outbox publishing after
commit, `processor_checkpoints`, leases with `LEASE_SECONDS`, the pure
`decide`/`evolve`, and — since section 6 — the worker protocol, the Python
`Runtime` and a static `PythonTask` compiler with a real recovery gate behind
`just test-worker-runtime`.

On the other side, `agentic.workflow` already has the worker half: an
`EventStore` port with `append_to_stream(expected_version)` and
`ConcurrencyConflictError`, `DurableWorkflowExecutor` (input and outbox in one
append, duplicates by causation, a **cached decision across OCC retries**),
`ProcessorLock` at 300 s, and `Saga.schedule_timeout` / `due_timeouts` /
`fire_timeout` with deterministic timeout ids. The cached decision is not a
detail: it is what makes a worker that lost its lease reload without calling the
model a second time to discover it lost.

## Decide this before writing code

**The payload policy** (§40.4). Every hop in an agent graph carries text, and
the messages a `DurableWorkflowExecutor` appends carry prompts, completions and
tool results. That is conversation content, and ADR_0021's rule about the *log*
is absolute and unchanged: the facts carry `from`, `to`, `kind`, digests and
sizes, never words.

Where the content lives is the definition's choice, `external | sealed`, and
the default is the free one — `external`, a reference plus a plaintext digest
and a size, content staying in the worker's own store, aiwatcher retaining
nothing. `sealed` goes through the conversation archive's crypt and needs
`AIWATCHER_CONVERSATION_ARCHIVE` and `AIWATCHER_CONVERSATION_KEYS`; a definition
that chooses it without them is **refused naming both**, never silently
downgraded. There is no `plain`.

Settle this first because it decides the stream row's shape, and the row is the
thing every later step reads.

## The work, in an order that keeps a gate at each step

**1. The append route.** `POST /api/v1/executions/{id}/stream` with
`expected_version`, `messages` and an `Idempotency-Key`; `GET …/stream` paged.
409 is `ConcurrencyConflictError`. This is the *append* side and is not the live
view — section 20's read stream was struck because
`/api/v1/workflow-executions/{id}/stream` already scopes by `workflow_run_id`,
and the guardrail against a second live view of one run still holds.

*Exit:* two concurrent appends at one expected version, one 200 and one 409, and
the loser reloads and succeeds.

**2. One lease on the execution.** `ProcessorLock`'s semantics in
`execution_runs.lease_*` — one decider at a time for a hosted run, on all three
store adapters, proved by the contract suite rather than by one adapter's test.

*Exit:* a second decider is refused while the first holds; the lease expires and
the second takes over; the first's next append is a 409.

**3. `AiwatcherEventStore` in `aiwatcher_sdk.integrations.agentic`,** satisfying
`agentic.workflow.EventStore`. `DurableWorkflowExecutor` then works unchanged
over a shared store.

*Exit:* `agentic`'s own executor tests pass against this implementation with no
change to `agentic`.

**4. Timers.** `schedule_timeout` becomes a row and the store appends
`TimeoutElapsed` when it is due. This is the one *active* thing the engine does
for a hosted run, and it is what `agentic`'s sagas have been missing — the
timers exist and nothing fires them.

*Exit:* a saga timeout scheduled before a restart fires after it, once.

**5. `agentic_graph`'s join buckets as events in the stream** — the three
dictionaries above — which is the change `agentic_graph` needs anyway.

*Exit — and this is Phase 13's own:* a searcher → summarizer graph with a fan-out
of three survives a worker restart between the second and third completion and
fires the summarizer **once**.

## What the engine must not do

Interpret the messages. It knows `TurnStarted` and `TurnCompleted` well enough
to project a status; the metadata is `jsonb` and the payload is a reference.
An engine that read the messages would be a second decider, which is the thing
this design exists to avoid.

And the standing rules do not bend for a hosted run: facts on the log, decisions
in the store; ids derived, never generated; no clock, socket or random value
inside `decide`; the six writes of one decision in one transaction.

## Level 0 first, if the session is short

Two changes worth more than their size, both in `ai_spirit_agent` (§40.2):

- `build_compiled_graph_system` builds its `WorkflowRuntime` **without a
  tracer** (`compiler.py:728`), so the graph preview emits no span at all while
  the personal assistant emits full traces. Passing `create_tracer()` is one
  line and the highest-value change in that repository.
- Commit the tee in `agentic_runtime/trace.py` and declare `aiwatcher_sdk` as an
  optional extra so its lazy import can succeed. Then add `declare_graph(graph)`
  to `aiwatcher_sdk.integrations.agentic` — about thirty lines — and the panel
  draws the graph with `Pending` nodes and the dispatches as *messages* rather
  than edges, which makes a fan-out that is serial today visibly serial.

None of this needs Phase 13, and it is what makes the Phase 13 exit observable
when it lands.

## Commands

```bash
just check                 # everything CI runs
just test-postgres         # the storage contract on a real database
just test-worker-runtime   # the recovery gate the static compiler already passes
just openapi               # after any route change; commit the contract and the panel client
```

## Traps

- **A message id derived from less than what it identifies.** A hosted run's
  appends are deduplicated by that id; derive it from the execution and the
  event name and the second step's fact reads as a redelivery of the first.
  Section 43.10, and it will bite again here with different names.
- **A command's message id without the run's version.** Section 43.20, same
  shape from the other side.
- **Never build a second live view of one run.** The read path exists.
- **A lease is not something the claimant can check about itself.** The worker
  asks; the server answers. That is why `Reactor::take`/`settle`/`resume` split
  the way they did, and a hosted decider does not get an exception.
