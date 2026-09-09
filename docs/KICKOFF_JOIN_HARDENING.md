# Kickoff: hardening the graph join

Written 2026-09-09, after Phase 13 closed. The join moved out of process
memory and into an event stream, its exit passes, and what is left is three
things that reduce what happens when it goes wrong. This document is what a
session needs to start, not a second design.

Two repositories, and the work is mostly in the second: **aiwatcher** (the
timer, already built) and **`ai_spirit_agent`** (the join itself).

## What already exists, checked in the tree

All of it uncommitted, in both repositories.

`agentic_graph/join.py` holds `JoinLedger` and two implementations.
`MemoryJoinLedger` is the four dictionaries the system used to carry and stays
the default — a preview that runs once in one process should not pay for
durability. `StreamJoinLedger` folds the same state out of an
`agentic.workflow.EventStore` and appends to it, which is the protocol
`aiwatcher_sdk`'s `AiwatcherEventStore` satisfies: the same ledger works over an
in-memory store in a test and over a shared history in a worker.

`CompiledGraphSystem.__init__` takes `join=`, threaded through
`register_compiled_graph` and `build_compiled_graph_system`. Thirteen tests in
`packages/agentic_graph/tests/test_join_ledger.py`, both implementations
parametrised, and the phase's own exit on a real compiled graph in
`test_builder.py::test_a_fan_in_of_three_survives_a_worker_restart_and_summarizes_once`.

On the aiwatcher side the timer is finished and verified against a live server:
a row, a ten-second tick in the `work` role, delivery and retirement in one
transaction, and a message id derived from the execution and the timer so two
ticks converge. `POST /api/v1/executions/{id}/stream` carries `timers`, and
`aiwatcher_sdk.integrations.agentic.SagaTimers` turns a saga's own
`saga.timeout_scheduled` into one with no change to `agentic`.

## What is already known to be safe, so nobody re-derives it

`expected` cannot undercount. `_record_expected_completions` is the **first
statement** of every workflow handler, so a node's reservation is written before
that node can complete. It can *over*count — a node that started and died — and
that is a stall rather than a wrong answer, which is the safe direction. Do not
"fix" it by taking the arity from the compiled plan: a `route_one` dispatch
starts one of three, and a static arity would wait for two nodes that were never
asked to run.

`claim_summary` returning `True` means **this call appended the claim**. It used
to mean "no claim was found", which read a conflict caused by somebody else's
*completion* as permission to fire — two workers, one summarizer. That is fixed
and `test_a_conflict_caused_by_a_completion_is_not_read_as_permission_to_fire`
is what keeps it fixed; it fails on the old branch.

## Decide this before writing code

**What a join does when its deadline arrives.** Fire with what has arrived, and
record that the answer was partial — or fail the turn and say which node never
came back. It is a product decision rather than a technical one: a partial
answer is useful for a search fan-out and unacceptable where somebody is
counting on a complete set, and the graph cannot tell which it is without being
told. Whichever is chosen, the *other* must be reachable per graph, because one
instance runs both kinds.

Settle it first: it decides what the timer's message says, and the message is
what the ledger folds.

## The work, in an order that keeps a gate at each step

**1. Scope the stream to a turn.** `graph:<graph_id>:<turn_id>` rather than
`graph:<graph_id>`. One line where the ledger is constructed. Today the stream
lives as long as the graph and every completion folds all of it, so the cost is
quadratic inside a turn and unbounded across a graph's life. A turn is a
traversal, so this is what the key always should have been.

*Exit:* two turns of one graph write two streams; a fold after a hundred turns
reads only the last turn's events.

**2. A claim that can be taken over.** The claim is taken *before* the
summarizer runs and nothing releases it, so a worker that dies holding one
leaves a join that never fires and says nothing. Put `holder` and an instant on
the claim; let a claim older than a lease be taken over; and append
`graph.summary_completed` when the summarizer returns, so a finished claim is
final and a live one is not mistaken for a dead one.

This is `AttemptRow`'s shape — the lease, `previous_owner`, and a settlement
that is the row ceasing to matter — and copying its reasoning is cheaper than
inventing a second one.

*Exit:* a claim whose holder stopped is taken over after the lease and the
summarizer fires once; a claim that completed is never taken over, however old.

**3. A deadline on the join.** The remaining silence: a node that never
completes leaves the fan-in waiting for ever. Schedule an aiwatcher timer when
the fan-in is reserved, and on `TimeoutElapsed` do whatever the decision above
settled. Turning a silent stall into a decision is the whole of the reduction.

*Exit:* a fan-in of three where one node never completes reaches a recorded
outcome — partial or failed — without anybody watching it.

## What this must not do

Fire twice. Every change here is about a join that did not finish; none of them
is worth a graph that answers twice, because a late answer is visible and a
double answer is two results nobody can tell apart afterwards. When a change
makes the two trade off, take the stall.

And the standing rules do not bend: the claim is a compare-and-append and never
a read followed by a decision; ids are derived, never generated; a completion's
text goes through the store's own content policy — with `AiwatcherEventStore`
the `data` reaches the payload store and never aiwatcher's history.

## Also open, and none of it blocking

- `discovery.from_settings()` builds `LaserTransport` on its default connection
  string, because `settings` still knows only `redis_url`,
  `redis_stream_prefix` and `redis_stream_maxlen`. Naming the Laser setting,
  defaulting it, and deciding whether `redis_url` goes with it is one decision
  and belongs to the transport migration.
- `workshops/lab6/messages.py` has its own copy of `_metadata_with_updates`
  with the bug `agentic_graph`'s copy just lost: trace keys splatted into a
  constructor where they are properties. It breaks the moment lab6 gets a
  tracer. Fix both or lift the helper into one place.
- `packages/agentic_runtime/tests/e2e_resilience/test_distributed_resilience.py`
  still names `RedisStreamsTransport` and needs a live broker.
- `deploy/Dockerfile` in aiwatcher does not copy `crates/aiwatcher-cli`, so
  `just check`'s Dockerfile stage fails.
- Erasure by subject does not reach a `sealed` hosted payload: a turn names
  whose words it holds and a payload does not, because nothing on the wire says
  so. Retention by age and deletion with the execution do reach them.

## Commands

```bash
# ai_spirit_agent — `uv run pytest` finds no binary; the module form works.
uv run python -m pytest packages/agentic_graph packages/agentic packages/agentic_runtime \
  -q --ignore=packages/agentic_runtime/tests/e2e_resilience
uv run ruff check packages/agentic_graph

# aiwatcher
just sdk-check
just check
```

## Traps

- **A conflict is usually not a competing claim.** On a fan-in's stream it is
  another node's completion. Interpreting it is how the double-fire got in.
- **`uv run pytest` fails and `uv run python -m pytest` works** in
  `ai_spirit_agent`. Twenty minutes went into that once.
- **A pinned SDK revision has to be pushed.** A local commit resolves for
  whoever made it and for nobody else; `uv` says `upload-pack: not our ref`.
  Both repositories are on `770eb970`.
- **Turning a tracer on finds bugs in paths that never had one.** That is the
  point of it, and the next one will look like a keyword argument that does not
  exist.
