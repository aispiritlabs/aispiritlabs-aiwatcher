# ADR_0004: The live channel is the projector's own fan-out, and a reconnect closes its own gap

- **Status**: accepted
- **Date**: 2026-08-27

## Context

A panel watching a run in flight needs events within a second of them happening.

Two properties of the storage layer rule it out as the push path. First, a span
is only written when it ends ([ADR_0003](ADR_0003_SPAN_ASSEMBLY.md)), so the
trace store has nothing to show for a run in progress. Second, Laser's change
feed is a poll/watermark mechanism, not a row-level push, so polling it from the
browser would be both slower and more load than tailing the topic once.

The harder problem is the reconnect. A tab loses its connection for four
seconds. If it resumes and silently misses those four seconds, the live view
becomes untrustworthy in the worst way: nothing looks wrong.

## Decision

The projector fans out to an in-process `LiveHub` as the **first** thing it does
with an event, before the read model and before storage. The panel's job is to
be fast, and a slow trace store must not delay it.

Transport is SSE for the run view, WebSocket for anything the panel needs to
send back. SSE is preferred where the traffic is one-way: it reconnects on its
own and survives proxies that mangle upgrades.

Every SSE frame carries the event's checkpoint as its `id:`. That is what makes
resume automatic — on a drop the browser resends the last id it saw as
`Last-Event-ID` with no application code. `EventSource` cannot set headers, so
the *first* connection passes its cursor as `?from=`; every automatic reconnect
after that uses the header, which the server prefers.

Missed events come from the hub's ring buffer when it still holds them, and from
the durable log when it does not — the hub reports a `ReplayGap` rather than
skipping ahead, and the handler falls back. Either way the client receives a
contiguous stream, and a `resynced` frame tells the panel it happened.

A `caught_up` frame marks the boundary between replay and live. It is Emmett's
`MessageSourceCaughtUp` control message, kept one layer further than Emmett
keeps it: Emmett's consumer strips it before any processor sees it, while here
it is exactly what lets the panel switch from "loading" to "live" at the right
moment instead of guessing from a timeout.

**Opening a stream with no cursor gives live only.** The panel fetches history
with `GET /api/v1/runs/{id}` and opens the stream at the `last_checkpoint` that
response carried, so replaying by default would send everything twice.

## Alternatives considered

**Poll the read model every second.** Simplest, and it scales with viewers
rather than with events, wasting most requests on nothing.

**Push from the trace store.** Not possible for a run in flight — the spans do
not exist yet.

**WebSocket everywhere.** More code for the same result on a one-way stream, and
SSE's automatic `Last-Event-ID` reconnect would have to be reimplemented by
hand. WebSocket is kept for the endpoint that will carry inbound control —
cancel a run, approve a tool call, submit feedback.

**Sequence numbers instead of checkpoints.** A per-run sequence does not order
across runs and cannot resume a whole-system stream. The checkpoint is the log's
own global position, and it does both.

## Consequences

- The panel serves in-flight runs from the read model, and historical ones from
  the trace store via Grafana. Two views of the same data, and the split is
  visible to users — worth stating in the UI rather than hiding.
- The read model is bounded (`max_runs`) and evicts **finished** runs first: a
  running run is never dropped out from under a live viewer.
- A subscriber that falls further behind than the broadcast capacity has its
  stream closed rather than silently skipped. Closing forces a reconnect, and
  the reconnect fills the gap properly.
- A client further behind than `MAX_RESYNC_EVENTS` (10,000) is capped and
  logged. Streaming an enormous backlog through a WebSocket is worse for that
  client than reloading the run.

**What would make this wrong.** The live hub is in-process, so the API and the
projector must share a process or share nothing. If the API needs to scale
independently of the projector, the hub becomes a network hop — Redis pub/sub,
or a second Laser consumer per API replica — and this decision needs revisiting
before that split, not after.
