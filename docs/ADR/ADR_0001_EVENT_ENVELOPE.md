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

## Amendment 2026-09-13: a run may name the variant that answered

`variant_id` joins `conversation_id`, `workflow_id` and `agent_id` as a flat,
optional envelope field copied into `RecordedMetadata`: which declared variant
answered in the run — Evaluation's content address of its pins (ADR_0030,
amended). A conversation groups runs by who is talking and a workflow by what is
executed; a variant groups them by the configuration that answered, which is
what lets what it did in production stand beside what it scored. It changes no
derived ID, is written as `aiwatcher.variant.id` on every span, and a record
without it reads as a run that named none. Nothing checks it against a
declaration: the log takes what a producer says, as it does for every other
correlation field.

## Amendment 2026-09-13, later: who published an event, and which call a run served

Two facts join the record, and only one of them is a producer's to write.

**`published_by` is the ingest route's, never the wire's.** `RecordedMetadata`
gains the credential the HTTP ingest authenticated the batch under — an ingest
token's name, a person's subject. The envelope carries the field in memory and
neither reads nor writes it in JSON, so a producer that sends one is ignored, a
broker never delivers one, and the route that checked the credential is the
only writer. A span names its publisher (`aiwatcher.source.published_by`) only
where one credential sent both of its ends: an end another credential sent is
neither's word. This is what makes one publisher's report distinguishable from
another's — a serving host's own run from the application's run about it — and
it changes no derived ID. An event a broker delivered names no publisher, since
nothing here authenticated who published it.

**`caller_run_id` is data on `run.started`.** A run that served another run's
model call — a model server answering a request that carried the
`Aiwatcher-Caller-Run` header — names that run. It is a correlation a producer
writes, like `evaluation_id`, and the read model keeps it on the run so the run
a call was made in can be joined to the run that served it.

**What would make this wrong.** `published_by` is only as strong as the
credential behind it: two producers sharing one ingest token are one publisher
here. And a deployment that publishes only through a broker has no publisher on
any record until the broker authenticates producers and the adapter carries
what it learnt.

## Amendment 2026-09-13, last: a prompt a gateway checked

`llm.*` data may carry `prompt_verified`, recorded on the span as
`aiwatcher.prompt.verified` beside the prompt reference it qualifies: whether a
host that saw the request's text found that prompt version's template in it. It
is what `aiwatcher_sdk.gateway` reports about a call it relayed, and it means
nothing on an application's own call — the application saying it rendered what
it says it rendered. Like every other field, it is taken as the producer sent it;
what makes it a witness is the credential it was published under.

## Amendment 2026-09-13, after: what a witness saw of a call's words

`llm.*` data may carry `asked_digests` and `replied_digests`, recorded on the
span as `aiwatcher.witness.asked` and `aiwatcher.witness.replied`: keyed digests
of what a request held and what came back, never the words. A digest is the
first 128 bits of an HMAC under a key derived from the publishing credential's
secret (`aiwatcher_core::witness`), so only a deployment that issued that
credential can test a text against it, and the assembler keeps only values
shaped like a digest — a producer's words in the same field stay off the span.
Like `prompt_verified`, they mean nothing on an application's own call; what
makes them a witness is the credential they were published under.

## Amendment 2026-09-13, beyond: a request holding only the prompt

`llm.*` data may carry `prompt_exact`, recorded as `aiwatcher.prompt.exact`
beside `aiwatcher.prompt.verified`: whether the host that saw the request found
its text to be nothing but the named template rendered and the values it was
rendered with. A gateway's word like the other two, and only under its
credential.

## Amendment 2026-09-13, further: the values a template was rendered with

`llm.*` data may carry `rendered_digests`, recorded as
`aiwatcher.witness.rendered`: a witness's keyed digest of each value the named
template was found rendered with, made under the side a reply's digest is made
under. So a value that is what a model already replied reads, to a deployment
holding the key, as that reply, and one that is a case's input as that input;
one the application made reads as neither. The assembler keeps only
digest-shaped values, as it does for the other two lists.

## Amendment 2026-09-13, final: a client's count, and what a witness saw of tools, derivations and a label

`source` may carry `client`, the one client that sent the event, and each SDK
client now numbers the events it sends into a run from nought under it
(`sequence`, which the envelope has always had and no client set). A process can
hold two clients publishing into one run — a tracer beside the application — so
a count is read per client and never across two. The period fold reads a number
it skipped as an event it was never given, on any log, whether or not the log
numbers its own records (ADR_0002, amended). A client forgets a run's count at
the run's end and past 100 000 runs at once; a count that starts again passes
nothing over, since a number at or below one already read is never a gap.

What a gateway publishes about a call grows by four, recorded as they arrive and
kept only where digest-shaped: `derived_digests` (`aiwatcher.witness.derived`) —
a value the caller took out of another of its values in steps the gateway
repeated, as `value:source`; `taken_digests` (`aiwatcher.witness.taken`) — what
a way of taking the answer that knows more than the reply, a label's word, took
out of each reply; and `taking_digest` (`aiwatcher.witness.taking`), the keyed
digest of that way itself, under a side of its own. A gateway that relays a tool
publishes `tool.*` on its own run naming the caller's, with
`arguments_digests` and `returned_digests` (`aiwatcher.witness.arguments`,
`aiwatcher.witness.returned`): each part of the arguments, and what came back,
as a reply's digest is made. None of it is a word said in the call.

## Amendment 2026-09-13, last: a client's count of runs, and what a witness says a reply could not be read

A run's start may carry `run_sequence`: the client's count of the runs it
opened naming the run's variant and answering no measurement, from nought, one
count per variant. A count inside a run shows an event that never arrived only
where another event of that run did; a run whose every event was lost left
nothing to count in. The period fold reads a number passed over as a run whose
start never reached it — a run lost whole among them — on any log (ADR_0030,
amended). A client's first start the fold reads counts nothing before it, since
a fold that began midway cannot tell a count it came to late from one that lost
its beginning. A measurement's run is in no count, because what it answered is
a result's rather than a variant's traffic. Each client numbers an event and
hands it to its transport in one step, so events from two threads reach the
transport in the order they were counted: out of order, the later number read
as the earlier one lost.

A gateway's call may carry `took_nothing` (`aiwatcher.witness.took_nothing`):
the caller said how it takes its answer out, and that way took nothing out of
any reply. A tool's host may publish the same `tool.*` a gateway's relay does
(`aiwatcher_sdk.gateway.ToolWitness`), under the gateway's own credential so its
digests are made under the same key. None of it is a word said in the call.

## Amendment 2026-09-14: when a count of runs began, where a value was placed, and a witness's key held by another host

A run's start carrying `run_sequence` also carries `run_counted_from`: when that
count began, the moment the client opened the first run it counted for the
variant. A reader first hearing of a count past nought could not tell a count it
came to late from one whose beginning was lost; one that was already reading
when the count began, and has forgotten no count since then, can — the runs
before were started where it was reading, and never reached it. The period fold
counts them, allowing a client's clock a minute ahead of the log's (ADR_0030,
amended). Absent from every start written before it, which counts as before.

What a gateway publishes about a call grows by one, recorded as it arrives and
kept only where digest-shaped: `placed_digests` (`aiwatcher.witness.placed`) —
for each value the named template was found rendered with, the keyed digest of
the placeholder's name and of the value, as `name:value`, each made as a reply's
is, so a judging call's reply naming a placeholder reads back as the value it
held. And a request the gateway found to hold a template only by its literal
parts, because the caller said nothing of what it rendered, has what stands
between those parts digested among what it asked.

A witness's digests are made under the key of the credential it publishes with,
unless the deployment names the credential whose key it digests under
(`AIWATCHER_WITNESS_DIGESTS=atlas=gateway`): a tool's host then publishes under
a token of its own and holds the gateway's witness key rather than its token
(`aiwatcher-gateway --witness-key`), so what it returned is comparable with what
the gateway relayed and each is still published, and refused, under its own
name. The server refuses a pair naming a credential it did not issue. A
TypeScript host witnesses a tool the same way (`@aiwatcher/sdk/tool-witness`),
byte for byte, and the Python gateway can answer a tool itself, from a function
it is handed, so a tool an application would compute in its own process runs
where the witness is. None of it is a word said in the call.

## Amendment 2026-09-14: a client's count of its runs, a measurement's runs counted apart, a question normalised, a judge's candidates placed, and a tool's code

**`client.counted`** joins the catalog, with `Subject::Client` and no span. A
client says how many runs it has opened for a variant — for a result and an
attempt at generating it too — in `data.runs`, with `run_counted_from` as on a
start, `variant_id`, and `data.evaluation_id` and `data.generation_attempt`
where the count is a measurement's. Its `run_id` names the client
(`client-<source.client>`), not a run, so no fold lists it as one. It is sent for
each count that moved: when the client closes, with the next event once five
minutes have passed, and as a generation attempt ends. It rides the same
transport as the runs, so a transport that stays down loses it too; the Python
`HttpTransport` may keep what it could not deliver in a `spool_dir`, and a
transport started later on that directory sends it first. A client killed
without closing says nothing of its runs since its last count.

**A measurement's runs are numbered too.** `run_sequence` was only on runs
answering no measurement; a run carrying `data.evaluation_id` now carries it as
well, counted apart for that result and the attempt at generating it
(`data.generation_attempt`, which `Generation.traced` sends), so a retried
attempt starts a count of its own and is no gap. What a variant was observed
doing counts none of them, as before.

What a gateway publishes about a call grows by one:
`asked_normalized_digests` (`aiwatcher.witness.asked_normalized`) — each text it
digested as asked, digested again after `aiwatcher_core::witness::normalized`:
NFKC, lower case, every punctuation character gone and white space run into one
space. The same bytes in Rust, Python (`aiwatcher_sdk.gateway.normalized`) and
TypeScript (`@aiwatcher/sdk/tool-witness`), for every character each language's
Unicode version assigns; a change to it is a new digest. A caller may name the
placeholders a judge's candidates stand in (`caller_body(ordered=…)`): the
gateway places their values in the order of their digests under its key,
re-renders the request so, and says in the reply's `Aiwatcher-Placed` header
which of the caller's names each placeholder now holds. A tool call a gateway
answered with a function of its own carries `code_sha256`
(`aiwatcher.witness.tool_code`), the sha256 of the source file that function is
defined in. None of it is a word said in the call.

## Amendment 2026-09-14 (later): a count held as a run starts, and a client's own clock

A client killed without closing said nothing of its runs since its last
`client.counted`, and a long-lived one said its count only with the next event
past five minutes. A client now says its counts that moved every five minutes by
a clock of its own (one thread for every client in a Python process, an
unreferenced interval in TypeScript), with no event after them. And a transport
that keeps counts on a disk — Python's `HttpTransport(spool_dir=…)`, TypeScript's
`HttpTransport({ spool: fileSpool(dir) })` from `@aiwatcher/sdk/node` — is handed
each count as a run starts, before that start is sent, and forgets it only when a
count saying as much was delivered; a transport started later on that disk sends
what it finds. So a client killed with its transport down leaves a count passing
over the runs it lost, at the cost of a write per run's start, which only a
spool asks for. A client killed with no spool still leaves its last runs
unsaid, since its last count.

A tool call carries `code_sha256` beside a function the gateway answers with
from two more places: the `Aiwatcher-Tool-Code` header of a URL tool's reply, and
the `code` a host hands `ToolWitness`.

## Amendment 2026-09-18: which project an event belongs to

`project` joins the envelope and `RecordedMetadata` as an optional
`ProjectScope` — an organization's uuid and a project's, and nothing else about
either. **Absence is the global side**, which is every event this build has
written, so no stored record moves and no historical read changes (ADR_0033
pt. 3). It changes no derived ID: `TraceId::derive` and `SpanId::derive` stay
pure functions of `run_id` and the span key, and a project folds instead into an
*execution* id, which is a different identifier with a different job (ADR_0033
pt. 5).

**It is the ingest route's word, and it is serialised.** That pair is the whole
design, and the second half is where it parts from `published_by`. A publisher
is read by the same process that wrote it, so it never leaves memory
(`#[serde(skip)]`); a scope is read by the **projector**, which consumes the bus
rather than the route, so skipping it would make every project's events read as
global the moment they passed through a broker. Being on the wire, it is a field
a producer can send — so `POST /api/v1/events` **overwrites it on every envelope
in the batch with the credential's scope, including with absence**. A body
naming a project is discarded rather than honoured, and a token bound to one
project cannot write into another. A field that was merged rather than assigned,
or checked only when the credential had a scope, would be a way of publishing
into somebody else's project.

The credential is an ingest token, which gains the scope in its label —
`name[queue]@<organization-uuid>/<project-uuid>=secret`, a suffix, so every
token string written before this parses to exactly what it always did. It is a
**narrowing** beside the queues and never a role: the token still holds
`Editor` and nothing more (ADR_0013), so naming a project cannot make a secret
in an agent's environment able to ask an orchestrator to run something. What it
does is bound where that secret can write. Every other credential — a session, a
bearer, a proxy header, a pod's attempt credential, the local token, the
anonymous identity — publishes globally, as each always has. Which projects a
*person* may read is a grant, asked of IAM fresh on each operation, and is no
part of an identity.

The SDKs are unchanged and send no such field.

**What would make this wrong.** The rule is only as strong as the route: a
deployment whose producers publish **straight to the broker** carries the
producer's own word for its project, exactly as no record there names a
publisher. A broker becomes a boundary when it authenticates producers and the
adapter carries what it learnt — the same sentence this ADR already writes about
`published_by`, and the same work. Until then, a deployment that bounds projects
by credential is one whose producers reach the log through `POST
/api/v1/events`.
