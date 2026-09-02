# ADR_0022: A long job over an object store is one primitive, and a corpus is staged before it is imported

- **Status**: accepted
- **Date**: 2026-09-02

## Context

ADR_0021 built an asynchronous export: a job that pins its selection, writes
sealed shards, advances a cursor only after a shard is stored, renews a lease
per shard, and names its result `sha256(request ‖ every shard digest)`. It
works, and it is a *machine* rather than a feature — the same machine any long
read-and-write over an object store needs.

The Hub importer is the second user of it, and the plan said to decide before
writing it rather than after, because deciding after is how two copies drift.
The drift would not be cosmetic. Every rule in that machine is a silent
corruption when one copy has it wrong:

* a cursor advanced before its shard leaves an artifact missing rows nothing
  can tell you about;
* a lease checked once rather than per shard lets a replaced worker write
  beside its replacement, so the job record names digests that do not describe
  the stored shards;
* a retryable failure treated as a rejection discards good work, and the
  reverse spins forever.

Separately, the import path had a shape problem of its own.
`POST /api/v1/annotation-imports` takes every row in one body, capped at five
thousand. That is right for a catalogue and wrong for a corpus, and raising the
cap only moves the number: the request still has to be held open, retried
whole, and kept in one process's memory, and a network failure at row 900 000
loses all of it.

And the *bytes* were the sharpest problem. The import fetched images, and the
whole of the policy was "the host has to be a Hugging Face one". A redirect, a
fifty-gigabyte body, a login page served with `content-type: image/png`, a PNG
header declaring sixty thousand pixels square — all of them went through. This
is the only place in the system that downloads bytes an outside party chose,
and it had one gate.

## Decision

Three things, in one change because the second and third are what the first was
for.

**`aiwatcher-jobs` holds the rules and not the records.** A small crate with
`JobState`, `ShardRef`, `lease_expired`, `after_failure`, `progress`,
`version_of` and the constants, plus `ORDERING` — the one sentence, written
down, that a `flush` doing it backwards can be wrong against. The conversation
export now calls those functions; the importer calls the same ones.

What is deliberately *not* shared is the job record. An export pins a
conversation list and counts exclusions by policy reason; an import pins a
staged batch and counts rejected rows by what was wrong with them. A generic
`Job<Payload>` would either flatten those into the JSON — producing a
TypeScript intersection the panel cannot narrow, the reason `ModelDetail`
nests — or hide them behind a trait with a dozen accessors, which is more
machinery than the thing it abstracts. What each record owes the crate is that
it *keeps the rules*, and the rules are functions rather than a base class so
that keeping them is a call rather than an inheritance.

**A corpus is staged, then imported.** `stage → append page → append page → …
→ queue` writes the rows into the object store as digested JSONL pages, seals
them into a content address, and hands a job a pinned artifact to read. A
numbered append is idempotent: identical bytes are an acknowledged retry,
different bytes for a page already stored are a refusal naming it. The import
version is `sha256(batch content digest ‖ dry-run flag ‖ every result shard
digest)` — built from the batch's *content*, never its id, so two people who
staged the same rows on the same terms reach the same reference.

Rejected rows are written down rather than returned: counts by reason on the
job, the rows themselves in their own shards, read through a paged route. An
import that refused four hundred thousand rows cannot put them in a response,
and "read the counts, then page the rows" is the difference between a
diagnosable import and a number.

**Every outbound byte goes through one bounded fetcher.**
`integrations::fetch` applies seven gates in order — https only with the host
parsed rather than matched, an allowlist, a public-address check on every
resolved address, no redirects, a byte ceiling enforced while streaming, a
"this has to be a picture, by its own header" check that doubles as the
decompression-bomb gate, and a verified content address. `Hubs` implements
`ImageSource` over it, and *both* import routes use that port, because a
fetcher wired into one and not the other is the one somebody routes around.

Provenance gained the fields that were missing: `ImportSource` now carries the
Hub `revision`, `config` and `split`, and `RightsEvidence` records who read a
licence, where, and when — recorded rather than enforced, because refusing an
import with no evidence teaches people to invent one.

## Alternatives considered

**Two copies, and say so.** The plan offered this explicitly and it is not
unreasonable — the two jobs are ~1 200 lines each and share maybe eighty. It
lost on the failure mode rather than the line count: the shared eighty lines
are the ones whose divergence is undetectable. A copy that formats a shard
differently is obvious; a copy that advances a cursor before writing produces a
corpus that is quietly missing rows, in a system whose whole promise is that a
training run can name what it learned from.

**A generic `Job<T>` with the payload flattened in.** Rejected for the reason
in the decision: it moves a real cost (an unnarrowable client type, a trait
with a dozen accessors) to buy a saving in a place that was not expensive.

**Extract into `aiwatcher-core`.** Rejected: core gains no dependency on a
transport or a store, and while the primitive itself depends on neither, the
thing it is *for* is object-store jobs. A crate of its own says that; a module
in core would invite the next person to put the store adapter beside it.

**Make the synchronous import bigger.** Rejected in the context above. It is
kept, unchanged, as the right answer for a catalogue.

**Fetch through an allowlist alone, as before.** Rejected once the list of what
the allowlist does not stop was written out. The most pointed one: an
allowlisted host answering `302 → http://169.254.169.254/` walks past every
check that ran against the address the caller named.

**Refuse an import whose rights nobody evidenced.** Rejected, and this is
ADR_0019's rule applied one level up: a refusal that can be satisfied by typing
something teaches people to type something. The evidence is recorded, its
absence is a warning on a response that succeeded, and the one hard refusal
stays where a human already read the licence — `check_rights` against the
curated table.

## Consequences

The conversation export changed shape slightly and its behaviour in one way:
counts and exclusions are now held apart from the job until the shard they
describe is stored. Before, a job that failed between reading a conversation
and writing its shard recorded those turns' counts and then read them again on
resume, so a resumed export's manifest could double-count. Its 82 tests pass
unchanged, which is the point of retrofitting the first caller rather than only
writing the second one against the new crate.

The importer's page size is 5 000 rows and a batch holds 1 000 pages — five
million rows, which is a corpus rather than a catalogue, and still a bounded
number of objects to list. Past that, the answer is more than one batch.

The family warning is now a statement about *pages*: "every page of this batch
gave each of its rows its own family". Exact about what it measured, and it
still catches the mistake it exists for, because a `group_id` mapped from a
filename is singleton on every page by construction. What it cannot say is
"this batch has N families", which would need a set of every group id a
million-row import has seen, held in a manifest.

The fetcher does not close the window between the address check and the
connection. A resolver that answers differently the second time — DNS
rebinding — is not defeated by checking the first answer; closing it needs a
connection-time hook where the socket is opened. The gate that holds against it
is the allowlist, because rebinding requires a host somebody listed, and the
hosts listed are hubs.

Parquet is not here. The staged artifact is JSONL, which every hub pipeline
already produces and every reader already parses; a Parquet writer is a
dependency and a schema decision, and the acceptance criteria this was written
against do not ask for one.

**What would make this wrong.** A third caller that needs the record shape
shared rather than just the rules — at that point the trait this rejected pays
for itself. Or a deployment where a single page takes longer than the
five-minute lease, which would make the lease bound a page badly rather than
bounding it well: the fix is a renewal inside the page, not a longer lease. Or
an allowlist that grows past hubs — the moment somebody adds a customer's own
mirror, the DNS-rebinding window stops being theoretical and the connection-time
check has to be built.
