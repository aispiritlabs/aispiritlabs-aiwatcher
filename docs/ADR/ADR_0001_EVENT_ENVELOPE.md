# ADR_0001: The event envelope carries four correlation ids, and the backend derives what a producer omits

- **Status**: accepted, amended 2026-08-28 (see [Amendment](#amendment-2026-08-28-the-derivation-gains-an-avalanche-finalizer))
- **Date**: 2026-08-27

## Context

Python and TypeScript agents publish events; a Rust backend consumes them and
assembles traces. Producers vary in how much they know: a well-instrumented
service can supply a `trace_id` and `span_id`, a three-line script inside a
notebook cannot.

Delivery is at-least-once. The same event will arrive twice, and a projector
restart will replay a stretch of the log. Anything the backend *generates* while
processing an event therefore has to come out the same on the second pass, or a
redelivery writes a duplicate span and a replay rewrites history.

## Decision

The wire form is flat `snake_case` JSON — see
[`contracts/envelope.schema.json`](../../contracts/envelope.schema.json) — with
`event_type`, `occurred_at`, `run_id` and `source` required and everything else
optional.

It carries four ids, in two pairs, taken from Emmett's
`RecordedMessageMetadata`:

- `trace_id` / `span_id` identify **the operation**.
- `correlation_id` / `causation_id` trace **the message flow**.

Resolution follows Emmett's scope rule, with one change:

```
trace_id       = sent ?? inherited ?? derive(run_id)
span_id        = sent ?? derive(trace_id, span_key)
parent_span_id = sent ?? the innermost open container span of this run
correlation_id = sent ?? inherited ?? message_id
causation_id   = sent ?? inherited ?? correlation_id
```

The change is `derive` where Emmett has `generate`. `TraceId::derive` and
`SpanId::derive` are pure functions of their inputs — FNV-1a, followed since
the amendment below by an avalanche finalizer — so a redelivery and a cold
replay land on the same ids.

The last line is Emmett's, kept verbatim: an event that nothing explicitly
caused roots its causation on the correlation, so "what caused this" always has
an answer.

The backend promotes an envelope into a `RecordedEvent` exactly once, at the log
boundary, adding `stream_position`, `global_position`, `checkpoint`,
`ingested_at` and the resolved ids. Every consumer downstream sees complete
metadata and never has to guess.

## Alternatives considered

**Require producers to supply trace and span ids.** Correct, and it puts the
burden on the code least able to carry it. A notebook that cannot generate a
W3C-compliant span id would simply not be observable.

**Generate ids on the backend with a UUID.** What Emmett does, and wrong here:
every redelivery would produce a new span id and a duplicate span. Emmett's
store writes each message once; a projector does not have that luxury.

**Emmett's nested `{kind, type, data, metadata}` shape on the wire.** Rejected
for the producer-facing form only: a flat envelope is easier to hand-write in
three languages and to eyeball in a log. The *recorded* form does use Emmett's
shape, because by then the metadata block is real.

**One trace per conversation.** Rejected. A conversation can run for hours and
fan out into parallel agent runs; a trace that never closes is unreadable in
every trace UI. Conversations are grouped by `conversation_id` instead.

## Consequences

- Two representations to keep in step: `EventEnvelope` and `RecordedEvent`.
  `EventEnvelope::record` is the only bridge, and it is tested for stability
  under replay.
- Deriving a span id needs a **stable span key**. For LLM and tool calls that
  key includes `data.call_id`; without one, two calls issued in parallel inside
  one agent collapse into a single span. The SDKs generate a `call_id` by
  default and the schema documents it.
- Omitting `event_id` disables deduplication for that event: the backend
  generates one, so a redelivery looks new and double-counts its tokens. Both
  SDKs always send it.
- FNV-1a is not a cryptographic hash and does not need to be. Its inputs are
  internal identifiers, and a collision within one trace would need two
  different span keys to collide in 64 bits inside a handful of spans. This
  held up when it was finally measured — see the amendment — but the same
  hash's *distribution* did not.

**What would make this wrong.** A producer that legitimately needs two spans
per `(run_id, span_key)` pair — for instance a retry loop that reuses one
`call_id` — would silently merge them. If that appears, the span key needs an
attempt counter rather than the derivation being abandoned.

---

## Amendment 2026-08-28: the derivation gains an avalanche finalizer

### What prompted it

Sequentially named runs derived trace ids that were hard to tell apart:

```text
run-1 -> cf5c62fe3cb22757e060139f368527ff
run-2 -> cf5c62fe3db22757e060139f3685293a
run-3 -> cf5c62fe3eb22757e060139f36852a75
```

Nine identical leading hex digits and an identical middle. This is structural,
not bad luck. FNV-1a's only diffusion is the multiply, and a multiply
propagates carries upward only. The 128-bit prime is `2^88 + 0x13b` — two
narrow groups of set bits — so a difference confined to the *last* input byte
lands as `d * 2^88 + d * 0x13b`, with no further rounds to spread it. Measured
across `run-1`..`run-9`, exactly **18 of 128 output bits** can vary, in two
runs: bits 0..=13 and 88..=91.

Sequential run ids are ordinary: batch jobs, `run-<counter>`,
`session-<timestamp>`. Averaged over 1000 ids, consecutive trace ids shared
**8.6 leading hex digits**. Producer-supplied UUID-shaped run ids were already
fine (0.06), which locates the problem precisely.

Span ids have it worse in proportion. The span key is appended to the trace id,
so siblings inside one trace differ only in their last byte — the case FNV-1a
diffuses worst, and the one a waterfall shows side by side:

```text
tool:search:1 -> b8c93944a090c21c
tool:search:2 -> b8c93c44a090c735
```

22 of 64 bits varying, five shared leading digits and a shared middle.

### Collisions: checked, not assumed

The original consequence bullet claimed a collision was implausible. It is
right, and the claim now rests on measurement rather than intuition:

- 5M sequential run ids through the raw hash: **0 collisions**.
- A difference in a single input byte *provably* cannot collide. The delta is
  `d * PRIME`, and `PRIME` is odd and therefore invertible mod `2^128`, so the
  delta is non-zero for every `d != 0`.
- Within one trace — the only scope where a span-id collision would merge two
  spans — 2M span keys collided 0 times. That is 4000x `max_spans_per_run`.
  Even the structurally worst family, 1000 same-length sibling keys, still
  varied 55 of 64 bits, a birthday bound near `2^28`.

**Collisions were never the problem. Distribution was.**

### Why distribution is not only a readability concern

Two costs, and only the first is cosmetic:

1. A reader cannot tell two runs apart at a glance. The panel already works
   around this: `pinchId` renders trace ids from both ends.
2. The **rightmost seven bytes** stay nearly constant — 14 of 56 bits varied
   across sequential runs. Those bytes are exactly what W3C Trace Context's
   random-trace-id flag and OpenTelemetry's consistent probability sampling
   read as the random part of a trace id. A 1% ratio sampler over 1000
   sequential runs kept **0 of them**, where ~10 was expected: all-or-nothing,
   silently. Nothing in this stack samples today, and the Collector
   (`deploy/otel-collector.yaml`) is exactly where a `probabilistic_sampler`
   would be added.

The second is what settles it. A workaround in one panel component does not
reach a sampler, and it does not reach span ids at all — `pinchId` is applied
to trace ids in the explore route only.

## Decision

Both derivations pass their FNV-1a output through an avalanche finalizer before
truncation: MurmurHash3's `fmix64` for the 64-bit span id, and the same
function's x64-128 finalization step for the 128-bit trace id. The signatures,
the purity and the determinism are untouched, so everything ADR_0001 decided
still holds.

The property that makes this cheap to reason about: **the finalizer is a
bijection.** `wrapping_add` and `fmix64` both invert — verified by round-tripping
2M values through an explicit inverse. So it changes how ids are *distributed*
and cannot change which inputs collide. Whatever collision behaviour FNV-1a had,
it still has, exactly.

| | before | after |
|---|---|---|
| bits varying across `run-1`..`run-9` | 18 / 128 | 128 / 128 |
| mean single-bit avalanche | 55.6 (worst 7) | 64.0 — the ideal (worst 46) |
| shared leading hex digits, sequential ids | 8.6 mean, 9 max | 0.07 mean, 2 max |
| rightmost 56 bits varying | 14 | 56 |
| 1% sampler over 1000 sequential runs | 0 kept | 11 kept (~10 expected) |

One subtlety worth keeping: the finalizers fix zero (`fmix64(0) == 0`), so the
all-zero guard in each `derive` still runs *after* mixing.

## Alternatives considered

**Leave it, and treat it purely as a display concern.** The cheapest option and
the tempting one, since the panel already reads well. Rejected because
`pinchId` reaches neither span ids nor a sampler, and because the sampling
failure mode is silent — nothing would report it, the traces would simply not
be there.

**Switch to SipHash or xxHash.** Both fix the distribution. SipHash through
`DefaultHasher` is out for the reason the original comment gives: no
cross-version stability guarantee, and these ids must survive a rebuild. A
keyed SipHash with a pinned key, or xxHash3, would work — at the price of a
dependency in `aiwatcher-core`, which is deliberately dependency-light, for no
measurable gain over twelve lines that already reach ideal avalanche.

**Version the derivation so old data keeps its old ids.** There is nothing to
key the version on: `derive(run_id)` sees only the run id. The switch would
have to be process-wide and permanent — a branch inside the one function whose
whole value is being unconditional. A bounded migration is the better trade.

## Consequences

**Every id re-derives. What that costs is not uniform across backends**, which
is the part worth knowing before deploying:

- **Write-ahead log, memory, generic broker.** These promote `EventEnvelope`
  into `RecordedEvent` at *append* time, so the resolved ids are frozen into
  the durable record. A replay deserializes them and reproduces the **old**
  ids. Existing traces keep matching; nothing to do.
- **Laser.** The topic carries the *envelope*, and the consumer promotes on
  *read* (`adapters::laser::record`) — because the broker assigns the position
  and a producer cannot know it in advance. So every replay re-derives. After
  this change, a replay of pre-change events emits **new** trace ids: the
  matching traces already in VictoriaTraces are orphaned, and the replay writes
  a second copy of that history under the new ids.

Beyond that:

- A run **in flight across the deploy** splits into two traces on every
  backend: events appended before carry old ids, events after get new ones.
  Bounded by how long a run takes.
- The mismatch is self-limiting. VictoriaTraces retention is 30d in
  `docker-compose.yml` and 7d in the k8s base; pre-change traces age out.
- The cost grows monotonically with stored history, so at 0.1.0 this is the
  cheapest it will ever be. That, more than anything, is why it was done now
  rather than filed.
- The derived values are now **pinned in
  `crates/aiwatcher-core/src/ids.rs`** (`derived_ids_are_pinned_to_exact_values`).
  Changing the derivation again means changing that test, which means arriving
  back here on purpose. Four further tests hold the distribution properties, so
  a regression fails as a statement about behaviour rather than as a surprise
  in the panel.
- `pinchId` in the panel is now a display choice rather than a workaround.
  Worth keeping — 32 hex digits in a table column deserve pinching — but it no
  longer carries load.

**What would make this wrong.** If pre-change and post-change traces ever need
to coexist on the Laser backend *beyond* the retention window, a process-wide
flag is not the answer; the derivation version would have to be stamped into
the envelope at publish time, so a record carries the rule that produced it.
And if a hash with a real security property is ever needed — a producer that
must not be able to steer its run into another tenant's trace — no finalizer
over FNV supplies that. That needs a keyed hash, and it is a different ADR.
