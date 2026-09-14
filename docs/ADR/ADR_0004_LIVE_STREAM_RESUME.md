# ADR_0004: The live channel is the projector's own fan-out, and a reconnect closes its own gap

- **Status**: accepted; the trace viewer it names is Perses since 2026-09-09 — see [ADR_0005](ADR_0005_TRACE_STORAGE.md#amendment-2026-09-09-the-waterfall-comes-from-perses-not-grafana); amended 2026-09-11 (below) for the Live view's selection and Pause; amended 2026-09-14 (below): the frame goes out after the read model, still before storage
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
with an event, before the read model and before storage. *(Superseded in part
2026-09-14: after the read model, still before storage — see below.)* The panel's job is to
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

## Amendment, 2026-09-11: a selection is filtered by the server, and Pause keeps the subscription

### What prompted it

The Live view watches a *selection* — agents, runtimes, workflows, sessions —
rather than one run, on this same channel. Two of its choices are about a live
view that must not go quietly wrong, which is this ADR's subject.

### The server applies the selection

`/api/v1/events/stream` takes the selection as repeated parameters, and
`Scope::Selection` narrows the stream before a frame is sent. **Subscribing to
everything and filtering in the browser** lost on volume: `llm.chunk` is most
of the log, and every one would cross the wire to be thrown away.

Model, tool and status cannot be selected on. The first two are span-level
facts assembled from several events ([ADR_0003](ADR_0003_SPAN_ASSEMBLY.md)) and
status is a fold over a whole run, so no event carries any of them. A selection
built in the Query view may name them validly, and the Live view says which
parts it is not following. **Passing them through anyway** lost because a
stream that ignores a filter looks identical to one where nothing is happening.

### Pause freezes the rendering, never the subscription

The Live view opens its stream with no cursor — live only, as above — because
resuming from a checkpoint would replay the retained log through the filter on
every change of selection, which is Explore's job. **Closing the connection on
Pause** would therefore resume live only too, and the paused interval would be
exactly the silent gap this ADR exists to prevent. So the connection stays
open, the counters keep counting, and only the feed stops moving. The feed
keeps the last few hundred events and the counters keep totals, so memory stays
flat however long a tab is left open.

**What it costs.** A paused tab still receives every matching frame it does not
draw: Pause saves the browser's rendering and nothing on the wire. If paused
tabs became a measurable share of what the hub fans out, the answer would be a
Pause that resumes from its own checkpoint and accepts the replay, capped at
`MAX_RESYNC_EVENTS` — never one that resumes live and loses the interval.

## Amendment, 2026-09-14: the frame goes out after the read model, still before storage

### What prompted it

The explorer showed a run a step late. A run finished, the lists did not show
it, and it appeared when the next run started — on a quiet deployment, minutes
later or never. The panel refreshes its lists on a frame (the Observability
layout invalidates every read-model query half a second after one arrives) and
not again until the next frame. The Decision above published the frame
**before the read model**, and the read model took a run's spans only at the
flush, up to `flush_interval` (500 ms) later. Measured against a local server:
the `llm.completed` frame at 4.545 s, the span and the `model` row it makes at
4.898 s. A refresh that lands inside that gap reads the lists without the
thing the frame announced, and nothing asks again. The `session` and `trace`
rows had the same race in microseconds, because the frame also went out
before `ReadModel::apply`.

### What changes

For each event the projector now folds it into every in-memory projection —
`ReadModel::apply`, the period fold, the asked index — assembles it, and hands
the spans it finished to the read model, **then** publishes the frame. The
trace store and the metric sink still wait for the flush. Spans that the
sweeper or a shutdown close go into the read model the same way, so no path
leaves a span on its way to storage that the panel cannot list.

The reason the Decision gives is kept whole: a slow trace store must not delay
the frame, and it does not — nothing between the event and its frame touches
disk or network. What is dropped is only "first". A frame now means *the lists
already show this*, which is what a consumer that refreshes on a frame needs.

**Refreshing again a moment later, in the panel,** lost: a second timer
narrows the race without closing it, and every other consumer that reads after
a frame would need its own.

Resume is unaffected. A client that fetched a run after `apply` and opened the
stream at that response's `last_checkpoint` can now receive that same frame
from the broadcast, and the handler already drops a frame at or below its
cursor.

**What it costs.** The frame waits for the in-memory folds and one write lock
on the read model per finished span batch — microseconds, against the half
second the panel waits anyway.
