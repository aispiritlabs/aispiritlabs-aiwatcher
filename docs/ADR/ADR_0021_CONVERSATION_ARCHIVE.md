# ADR_0021: Conversation content is an encrypted archive with its own retention, not events on the log

- **Status**: accepted
- **Date**: 2026-09-02

## Context

The one thing this stack could not do was hand a model the conversations it had
watched. Everything needed to train on them was already here — the runs, the
spans, the model versions, the immutable dataset references — and the only
missing piece was the words.

The build before this one closed that gap with a convention: a producer that
wanted a turn kept added `input` and `output` to the `llm.completed` event, and
`scripts/export-conversation-dataset.py` paged the log, picked the events that
had both, and published them to the dataset registry. It worked, and following
it through produced a list of things that were wrong with it, each of which
turned out to be the same thing.

* **The bodies went into the durable log.** The Collector strips `gen_ai.prompt`
  and `gen_ai.completion` from spans, which is a guardrail this repository
  states in three places — and then the same content arrived in `data.input`,
  which nothing strips. It reached the write-ahead log, every projector's
  memory, and whatever retention the log happens to have.
* **The log's retention is the wrong clock.** It is sized for volume. Consent is
  not: what somebody was told is a number of days, and it has nothing to do with
  how many events a day the cluster produces. Tying them together means raising
  the log's retention silently extends a consent nobody re-asked for.
* **Deletion was not expressible.** An event log is append-only, which is the
  property that makes it trustworthy and the property that makes an erasure
  request unanswerable.
* **There was nowhere to put the reason.** Two flat strings cannot say who spoke,
  which message answered which, what the tool actually returned, on whose
  consent it was kept, or who reviewed it. Every one of those is a field an
  auditor asks for, and none of them fits in a span attribute.
* **The export could not grow.** It was one synchronous HTTP request capped at
  1 000 rows and 4 MiB. Raising the cap moves the number; it does not make a
  process that holds a whole corpus in memory correct.

This is ADR_0018's lesson one turn further. There, training was on the log until
following it through produced an epoch that was not a span, a step that did not
belong, and a special case in somebody else's fold. Here it produces bodies in a
log that must not hold them and a retention policy that cannot be enforced. **A
design whose last step is an exception in somebody else's retention policy is a
design in the wrong place.**

## Decision

**Conversation content is its own module, its own store, and its own routes.**
`aiwatcher-conversations` sits beside `aiwatcher-prompts`, `-datasets`,
`-annotations` and `-training` as the fifth authored artifact over the same
`ObjectStore` port. `conversation.turn` is **not** in the event catalog, and
nothing about this reaches the span assembler, the live hub or the read model.

Seven decisions carry it.

**It is off by default, and that is the only default in the configuration
chosen so that doing nothing keeps nothing.** `AIWATCHER_CONVERSATION_ARCHIVE`
defaults to off; every route answers 501 naming the variable. The other four
registries are on because there is somewhere to put them. This one holds
somebody's words, and a release that started retaining them on an upgrade would
be the exact failure this ADR is about.

**Content is encrypted at rest and the head is not.** A turn is two objects: a
plaintext head — role, ordering, provenance, policy, findings, review state,
digests — and a sealed body. AES-256-GCM under a key derived per object with
HKDF, the object's path authenticated as associated data so a ciphertext moved
between turns does not open. Three things follow that a single encrypted blob
could not do: a review queue and an exclusion report are answerable without
decrypting anything; erasing content leaves a head that can still *explain* a
row an export took; and approving a turn rewrites four hundred bytes rather than
re-sealing a megabyte.

**Retention is this module's own clock, and a deletion deletes.** Every turn
carries `ttl_days`; a sweep removes the content and replaces the head's state
with a tombstone naming why and when. An erasure request names a *subject* — a
person, not a conversation — and subject markers are what make answering one a
prefix listing rather than a scan of the archive.

**A deletion also reaches the corpora that already have the words.** Erasing the
archive and leaving a published export would be an erasure in name only: the
content is still readable, through a different route, under a reference somebody
has written into a training run. So an erasure — and the retention sweep, which
is the same problem arriving more quietly — deletes the shards of every corpus
whose pinned conversation list it touched, and marks the manifest withdrawn.
The manifest itself survives with its counts, digests and exclusions, so a
training run naming that reference still resolves to something that can say what
happened to it. This is bounded by the number of published corpora rather than
by the number of turns, which is why the manifest carries the conversation list
the job pinned.

**Consent is required by default and never inferred.** `PolicyMode::Protected`
refuses a turn with no basis, subject, reference, scope or redaction record, and
reports every missing one at once. `open` records the gaps instead, and an
export then excludes those rows *by name, in a manifest, forever* — which is
`aiwatcher_annotations`' `UsageRights::Unknown` rule, restated. Refusing an
unknown-consent write outright was considered and rejected for the reason
ADR_0019 gives for imports: it teaches people to claim a basis in order to get
past the dialog.

**Redaction is a producer's claim and the server scans anyway.** A hook that was
misconfigured reports the same record as one that worked, so the claim cannot be
checked — what it buys is the ability to refuse content from a producer that has
no hook at all. The server's own scanner is conservative and deliberately
matches the SDK's, so a clean scan is evidence the hook ran. It ships **no
unsafe-output classifier**: that judgement is why the human review gate exists,
and a keyword list would produce a green tick nobody should trust.

**An export is a job, it is leased, and its version is a content address.** It
pins the conversation list when it is created, writes sealed JSONL shards, and
advances its cursor only after a shard is stored. A worker claims it under its
own name — the pod's — for five minutes, renewed with every shard, and
re-checks that claim at each shard boundary; a worker that lost it stops rather
than writing beside its replacement. Two deterministic workers over an
*unchanged* archive would converge on identical bytes anyway; the lease is for
the case where the archive changes under them, where the last job record
written would name digests that do not describe the stored shards. A crash re-does at most one shard and
writes byte-identical bytes to the same key. The version is
`sha256(request ‖ every shard digest)`, so the same request over an unchanged
archive reaches the same `name@sha256` — the immutable reference ADR_0018
refuses a model promotion without. Every excluded turn is counted by reason, and
the manifest says whether its sample list is the whole story.

**Reading content needs `admin`.** This is the second route in the system that
role guards, after the rerun, and the reason is the same shape: it is the
operation that cannot be undone from the caller's side — content leaves the
encryption boundary, and an erasure does not come back. Writing a turn is
`editor`, which is what keeps an ingest token able to record and unable to read
the archive out.

## Alternatives considered

**Keep `data.input` / `data.output` and redact in the Collector.** The Collector
already strips span attributes, so extending it looks cheap. It cannot work: the
Collector is on the *export* path to VictoriaTraces, and the durable log is
written before it. The bodies would still be in the log, which is the thing that
must not happen. It also has nowhere to put a consent record.

**A `conversation.turn` event type on the log, with the archive as a projection.**
This keeps one write path, which is genuinely attractive. It fails on deletion:
the projection could be rebuilt without an erased turn, and the log would still
hold it, so "deleted" would mean "not currently shown". An append-only log and
an erasure obligation are not compatible, and picking the log means picking
against the obligation.

**Encrypt the object store instead.** RustFS supports server-side encryption,
and it would be one setting rather than a module. It protects the disk and
nothing else: every process holding the bucket's credentials — and prompts,
datasets, annotations and training all hold them — reads conversation content in
the clear. The boundary has to be narrower than the bucket.

**Store the whole turn as one sealed object.** Simpler, and it makes the review
queue impossible: listing what needs reviewing would decrypt every message in
the archive, and a queue that does that has made the encryption ornamental.

**Infer a preference pair from the review decision.** Two sibling answers where
one was approved and one rejected look exactly like a chosen/rejected pair, and
building DPO rows from that costs nothing. It is wrong in the one case that
matters: a turn rejected for holding somebody's address would become the
rejected half of a pair and put that address in the corpus. The preference is a
separate, explicit field on the review, and nothing is inferred.

**Publish the export into the dataset registry (ADR_0014).** It already has
`name@version`, a rows route and a viewer. It brings back the 1 000-row and
4 MiB caps, and it would put conversation content in the one authored store that
is not encrypted and has no role gate on reads.

## Consequences

There is a fifth authored registry, a fifth key prefix, and a background worker
in the server process. The worker is not a scheduler: it drains queued exports
and sweeps expired content, both of which are safe to run in several replicas at
once, and both of which are durable enough that a replica running neither loses
nothing.

**A key is now something a deployment can lose.** Retiring a key makes
everything sealed under it unreadable — which is deliberate, because it is also
how a key is destroyed on purpose — but it means key rotation is an operational
procedure this repository did not previously have. The API reports which key ids
a process holds, so an operator can see a rotation land before retiring the old
one.

**A published corpus is immutable and can still be emptied.** Withdrawal is not
a hole in the immutability rule — the manifest, the version and every count are
unchanged — but a reference that used to resolve to rows now resolves to a 410,
and a training run that named it can no longer be reproduced. That is the cost
of an erasure obligation, and the alternative is not keeping the rows: it is not
having the obligation.

**The export's determinism is over the archive's *content*, including review
metadata.** A turn approved by a different person at a different time produces a
different version. That is right for a content address and it means "the same
request" does not by itself pin a corpus; `name@sha256` does.

**Two places now hold something about one conversation.** The log has the runs
and the spans; the archive has the words. The join is `conversation_id` plus the
provenance ids on each turn, and it is deliberately one-directional: the archive
names the log, the log does not know the archive exists.

**`AIWATCHER_PROMPT_STORE` now gates five registries.** ADR_0014 called the name
historical and ADR_0018 said four was enough to want a rename. Five is past that
argument; it is a rename with a deprecation window rather than a decision.

**What would make this wrong.** If deployments routinely run
`AIWATCHER_CONVERSATION_POLICY=open`, then the protected mode is asking for
records people do not have, and the fix is a narrower required set rather than a
louder default. If the review queue is the bottleneck at any real corpus size —
if people start approving in bulk without reading — then human review per turn is
the wrong grain, and the answer is per-conversation review with sampling, not a
faster button. If a second thing in this workspace needs encrypted-at-rest
authored storage, the seal/open pair in `archive::crypt` should become a shared
adapter behind the `ObjectStore` port instead of being copied. And if a single
*shard* routinely takes longer than the five-minute lease — a corpus whose rows
are far larger than a conversation, or an object store having a bad day — the
symptom is duplicated work rather than corruption, and the answer is a renewal
inside the shard rather than a longer lease.
