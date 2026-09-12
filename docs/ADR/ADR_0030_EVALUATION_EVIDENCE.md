# ADR_0030: Evaluation owns pinned variants and durable evidence

- **Status**: accepted; B1 contract and B2 persistence implemented, amended
  2026-09-12 with approvals as a resource, the read/verify split, restore, and
  the admission rule a judge will need
- **Date**: 2026-09-11

## Context

ADR_0010's bounded fold is useful for observing a suite while it runs. Its
retention cannot preserve the evidence behind a later decision. The FTI A
comparison now exposes missing context and shed detail, but saving an ID still
cannot freeze a result. Adding suites, rubrics and durable results to that fold
would make telemetry retention own quality decisions.

## Decision

### One owner, with a small implemented entry point

`aiwatcher-evaluation` owns variant manifests, evaluation context, and the future
suite, result, assessment and comparison rules. It depends only on Core.
`Evaluation::prepare` validates an `EvaluationManifest` and returns a frozen
`PreparedEvaluation`. Its fields have read-only accessors; it has no deserializer
that could accept a caller-supplied fingerprint. This is a local contract
operation, not a registry write, artifact verification or quality verdict.

Core keeps neutral `ArtifactRef` and `storage::{ObjectStore, ObjectEntry}`.
Both the root and `core::prompts` re-export the same storage types for existing
callers. No new evaluation rule enters Core or Execution. API/Server composition
edges will be admitted explicitly when their adapters are implemented.

### Identities and the wire contract

`EvaluationManifest` and its nested `VariantManifest` require `schema_version=1`.
Unknown fields and unsupported versions are rejected. JSON Schema is generated
from the Rust types into `contracts/evaluation-manifest.schema.json`; the facade
also applies semantic checks the wire schema does not express. Python exposes
TypedDicts through `aiwatcher_sdk.evaluation`; TypeScript exposes types through
`@aiwatcher/sdk/evaluation`. Neither is a new telemetry transport or a promise
of runtime validation. The root Python import stays independent of this module.

| Identity | Meaning |
| --- | --- |
| `experiment_id` | Producer-assigned grouping; never inferred from a model name |
| `variant_id` | SHA-256 of the complete normalized variant manifest, including its experiment and dataset |
| `context_id` | SHA-256 of `[schema_version, context]`; fingerprints metadata, not a comparability verdict |
| `evaluation_id` | Existing logical report ID, retained across technical retries |
| `repetition_id` | Explicit independent measurement; a new repetition needs a new evaluation ID |
| `execution_id` | Existing execution ID; the envelope spells it `workflow_run_id`, not a second identity |
| `step_id` | Existing step within that execution; rejected without its execution |

Fingerprints use compact UTF-8 JSON from the normalized Rust structure, with
every object key sorted and array order retained. Omitted optional fields and
explicit null normalize alike. A fixture pins the resulting IDs. SDKs consume
the facade's IDs rather than duplicating serialization rules. A future change
to normalization must preserve version 1 reads or introduce a new version.
Technical worker attempt counters do not enter variant/context identity.

The variant pins the dataset kind/name/version, optional model and prompt
versions, code, full generation configuration, response schema, tool definitions
and workflow revision. At least one of model, prompt or workflow is present.
Configuration and code use existing `ArtifactRef` with SHA-256 and byte length.
They contain actual resolved settings, never secrets or live aliases. A change
to any included reference, including its location, changes the variant ID.

The context pins its dataset, selected-case manifest and count, split, suite and
scorer versions, input and expectations schemas, and metric definitions. Each
metric declares a unit, direction (`higher`, `lower`, `none`) and aggregation
(`mean`, `sum`, `min`, `max`, `rate`, `none`). Names must be unique. A judge also
pins its provider/model revision, complete configuration and calibration dataset.
A new scorer changes context while preserving the variant. The context's dataset
must exactly match the variant's, including its owning registry kind.

Reference validation checks structure and rejects common mutable labels such as
`production` and `latest`. It cannot recognize every owner's alias or prove
existence, digest correctness, immutable revision semantics or access rights.
Before execution/publication, the adapter must resolve each reference through its
owner's public facade, verify artifact bytes and enforce access. Until that
adapter exists, a prepared manifest is only a validated declaration. A digest
supplied by a producer is not a verified receipt. Native dataset, annotation and
conversation exports remain different resources even with identical names.

### Persistence protocol

Evaluation owns the metadata key layout and immutable result artifacts. Existing
object-store adapters provide bytes; private Evaluation storage operations own
commit, read and tombstone semantics. This must not become evaluation CRUD in
`WorkflowStore`, nor writes into another registry's private keys.

1. Validate the entire payload and byte budget before writing. Record an immutable
   pending intent (ID and collection deadline only), then store and verify bounded
   result shards, including digests, lengths,
   schema versions, case IDs and repetition IDs. Expected responses and actual
   responses are separate references. A case may link actual-response bytes to
   existing trace/span IDs; unavailable telemetry does not invent a new span.
2. Publish immutable result metadata with aggregate metrics, selected/scored/
   failed counts, terminal status, manifest and shard references. Its version
   addresses these bytes. No caller supplies a trusted result digest.
3. Atomically claim the logical `evaluation_id` with that version. Same ID and
   same content returns the existing receipt; different content conflicts.
   A read-then-overwrite sequence is insufficient. B2 adds conditional
   creation to the neutral storage port/adapters (filesystem staging + atomic
   hard-link publication + fsync, S3 conditional object write). Concurrent
   writers are covered by the shared adapter acceptance contract. Current `ObjectStore::put` remains overwriting and is not that gate.
4. Only committed metadata becomes discoverable. A crash after writing a shard
   can leave an orphan for collection, never an advertised missing shard.
   Events after commit carry a summary/reference, not result content. A missed
   event cannot make the committed result disappear from the registry API.

The initial durable API will page cases by immutable result version and cursor,
with a default page size of 200. Its read must distinguish complete, partial,
missing artifact, expired, deleted source and forbidden access. Aggregate-only
or partial results never become complete just because a write succeeded.
Partial and final terminal outcomes are immutable; a replacement measurement
uses a new evaluation ID instead of silently overwriting the old evidence.

This protocol is implemented by `Registry` and its private `store`. Acceptance
covers restart, projection loss, large reports, missing/corrupt shards, concurrent
publication, lost responses and source revocation. The backend source capability
is deliberately narrower than the manifest vocabulary; see the scope below.

### Collection and concurrent publication

An incomplete publication becomes eligible for collection one hour after its
first intent, or earlier when source/result retention expires. Retrying does not
extend this window. The collector creates an **abandoned claim at the same key**
that publication uses for its receipt. Atomic create admits only one of them:

- A committed receipt wins: the collector verifies its immutable metadata and
  keeps the metadata plus every actual/expected shard it references. Other
  versions under that ID can never become winners and can be removed. Missing or
  corrupt metadata prevents pruning, since the live reference set is unknown.
- Abandonment wins: no current or future writer can commit that ID. The collector
  erases its artifact prefix. An in-flight object write may finish after erasure;
  the durable abandoned claim remains, so later sweeps erase late bytes as well.

Claims are never replaced or deleted. Concurrent collectors have the same live
reference set. An ambiguous conditional-write response is retried/read back; it
does not authorize deletion based on an assumed outcome. Abandoned claims retain
only the logical ID and deadline, never a fabricated result receipt. They are
excluded from result discovery and block legacy detail/list/suite/baseline reuse;
HTTP reads/retries return 410. A subsequent publication needs a fresh logical ID.

Committed receipt JSON and content-addressed versions retain their previous shape.
Pre-intent, unclaimed artifact prefixes are deliberately not reclaimed from age
alone: a one-way hashed path cannot recover their logical identity or prove an
older writer stopped. Retrying the original publication enrolls that ID in the
new protocol. Deployment-specific reconciliation of remaining old prefixes and
adapter-local staging files is separate from this object-level collector.

### Retention, source rights and working defaults

The user has no target dataset, scorer or deployment scale yet and authorized
selecting defaults. Use the committed synthetic short-answer fixture and an
exact Unicode string equality scorer, with no normalization, generation service
or paid judge. Its three cases include an empty expected answer. This regression
fixture verifies the contract; it is not independent held-out quality evidence.

For B2 start with configurable limits of 10,000 selected cases, 100 MiB total
result bytes, 200 cases per page, and 30 days of result content retention.
The manifest itself is capped now at 256 KiB and 128 metric definitions.
The registry enforces configurable deployment limits; HTTP additionally caps a
publication body at 100 MiB. Actual scale must be measured before raising them.

Keep minimal tombstones containing IDs, digest and disappearance reason after
content expires; do not retain prompts, responses or arbitrary parameters in
tombstones. Do not promise indefinite preservation of content. Source retention
and deletion can shorten the 30-day window. Conversation-derived material must
continue through its governed archive/authorization path, not a public object
URI. B2 must enforce source deletion/revocation for dependent artifacts and
recheck access on reads; fail closed when permission cannot be established.
A saved view or pinned result does not grant consent or bypass source policy.

The initial instance is shared by one group, as in FTI A. A future project filter
is not tenant isolation. Multiple isolated teams require a separate authorization
design before sharing the new resources.

### Compatibility and subsequent rules

Legacy `eval.*` producers and the current HTTP read path stay unchanged in B1.
B2 will make the durable registry authoritative for IDs it has committed. The
read adapter may fall back to the old projection only for an unknown ID, never
after a durable tombstone, denial or corrupt/missing artifact. Legacy events
cannot overwrite durable records. A historical report keeps its ID and explicit
unknown-context/partial-detail state; no versions are fabricated on migration.
The old best-effort SDK call stays best effort. Durable publication will be a
separate registry client operation that raises on failure.

Comparison remains a server decision. B3 must inspect status, versioned cohort,
split, suite/scorer/judge configuration, schemas, metric meaning and completeness;
a matching context hash alone is not proof of valid bytes or held-out independence.
Known differences block quality deltas; missing evidence stays unverified.
Promotion recommendations require complete independent held-out evidence and a
compatible baseline under an explicit policy. First-model initialization is a
separate outcome. Existing Training and Prompts promotion rules remain owned by
their domains and require an explicit migration before changing behavior.

B4 assessments will target exactly one trace, span, session snapshot or case
measurement, with rubric version, typed value, source, author, rationale, time
and revision. Human and judge assessments coexist. Expected answers remain
separate from assessments. Quality feedback cannot authorize conversation reuse.
Scoring is an ordinary existing worker task; it adds no RuntimeKind or retry
state to Execution.

## Consequences

The product gains a testable owner, SDK vocabulary and durable storage protocol
before adding a launcher. There is no new HTTP endpoint or panel screen in
B1. Its JSON Schema, example, semantic tests and architecture gate can evolve
together; the generated schema is checked in CI independently of OpenAPI.

Durable storage will cost a conditional-write capability, access checks and
garbage collection. Keeping metadata in the fold would avoid that work but could
not satisfy retention and concurrent publication. Reusing the workflow store
would give durability while assigning quality records to the wrong owner.
Copying full source content into result blobs would simplify reads while violating
the source's deletion and permission rules. These alternatives are rejected.

This decision amends ADR_0010's log-as-record statement only for future committed
durable results. Legacy event reports keep their existing semantics until the
explicit B2 read bridge is implemented.


## Implemented B2 scope and remaining work

The server exposes the durable registry through `/api/v1/evaluation-results`.
Only claims are discoverable. Metadata version is computed by the server;
expected-response shards come from `SourceAuthority`, actual-response shards
from the producer. Aggregates are computed from the declared metric semantics,
not trusted from a separate aggregate payload. Retries preserve the first claim's
clock. Partial measurements cannot claim success, and a new terminal result
requires a new logical ID.

The `SourceAuthority` adapter uses an operator-approved local short-answer
bundle, configured by `AIWATCHER_EVALUATION_SOURCE_DIR`. Every read rechecks
approved context/variant and file digests, without outbound URL fetches.
External synthetic fixtures remain supported. Curation additionally calls the
owner's public `Registry::verified_version(name, version)`: only that owner
knows its private storage and historical content identity. It verifies exact
catalogue membership, digest, name, count and publication bounds. No new domain
dependency is introduced; Server composes the existing registries.

Curation rows must match the approved case manifest in order, IDs, inputs and
expectations. Its native version is not the case-manifest digest. Shared-instance
API authentication/Viewer reads/Editor publication and operator approval define
the current access policy; no per-dataset ACL or independent Curation retention
is inferred. Removal of native content or catalogue withdraws dependent evidence.
A changed head leaves exact older versions readable. This short-answer slice is
bounded by Curation's existing 1,000 rows/4 MiB limit.

Prompt pins now resolve through `Prompts::Registry::verified_version`, composed
by Server with the same registry used by the API. The owner verifies the exact
name/ID/text digest and derived variables; neither mutable labels nor the
bounded head index determine whether that immutable version exists. A missing
head is recoverable index loss, not a source deletion. A missing version
withdraws dependent evidence; corruption hides it without a false deletion.
The current prompt read policy is shared-instance access, further restricted
for Evaluation by the operator-approved bundle. Prompts do not expire by
themselves; Evaluation retention still applies. Prompt text is not copied into
result shards. Model hints and other prompt metadata are not content-addressed
and cannot prove a model version or promotion quality. Existing prompt read and
promotion operations are unchanged.

Model pins now call Training's `Registry::verified_version`. The owner shares
its historical identity algorithm with registration and verifies exact name/ID,
provenance, metrics and package artifact digests. Its immutable version is the
source of truth; the mutable head or live run is not required. No promotion
policy or existing read API changes. The owner refuses malformed packages and
records over 1 MiB; missing versions withdraw dependent evidence.

Training's historical ID does not cover the full package metadata. Server thus
requires an operator-approved `model-package.json` equal to the current package,
plus all actual files under `model-artifacts/<name>`. It checks every digest and
optional length within a shared 100 MiB budget. No producer URI is fetched and
no model loader executes. The package approval has a 1 MiB limit and follows
the same operator trust policy as the manifest. It is current approval, not a
new immutable package fingerprint; historical runtime/shape attestation would
need an explicit contract extension. These checks establish accessible bytes,
not that the producer executed them or deserves promotion. Shared Viewer/Editor
roles and Evaluation retention apply; artifact loss retires evidence, corruption
hides it, and approval mismatch returns forbidden. Weights are not copied into
Evaluation shards.

Annotations now uses `Registry::verified_coco` through Server. The owner shares
export/revision identity algorithms with their writers and verifies the selected
split's schema, pinned revisions, shapes and native image bytes. It reads current
rights/review decisions and refuses revoked access; `commercial` and `research`
follow existing owner policy, `any` and external image URLs are refused. These
are checks of recorded rights. Server compares every approved case to the full
COCO image and its annotations/categories, with exact order and split coverage.
Missing revisions are unavailable rather than empty expected answers. Image bytes
stay in Annotations; vector expectations enter Evaluation's normal evidence store.

Verified reads have limits of 1,000 export samples and 100 MiB total source bytes,
with the existing per-image/revision bounds. A moved accepted revision or missing
export index does not change the pin; source deletion erases evidence, while
review/rights withdrawal hides it without resetting retention. Annotations has
no historical schema store, so a legitimate schema change refuses the old export
until that schema is available again. Ordinary export/COCO APIs retain behavior.
No domain dependency or public HTTP shape was added. Shared roles and operator
approval still apply; no per-project ACL or independent retention is invented.

Conversations now admits reviewed `prompt_response` exports with explicit
`evaluate` scope through the owner's `verified_evaluation_rows` facade. It
verifies the request/version and each decrypted shard, then both source turns'
content, present consent/review and earliest expiry. Server compares the ordered
full cohort to approved case IDs and input/expected digests. No conversation
text is stored in the operator bundle. Other conversation formats and judges
remain refused; a `test` label does not prove an independent held-out dataset.

Evaluation owns an optional `EvidenceCipher` port; Server implements it with the
existing Conversations Keyring and authenticates the complete Evaluation object
path. Conversation metadata and actual/expected shards must be sealed. Logical
content digests, receipt shape and CAS/GC semantics remain unchanged; retries
verify plaintext identity rather than requiring identical random ciphertext.
Old non-conversation plaintext objects remain readable. A plaintext downgrade
of governed metadata or shards is corrupt evidence, never an accepted fallback.

The registry defaults to no governed-content capability. Each API call receives
a clone with `with_content_access` set from the authenticated Admin role;
publishing also requires Admin. The retention worker explicitly holds this
capability. Subject strings never grant permission. Viewer/Editor can discover
minimal receipts and forbidden states but receive no manifest, metrics or cases.
All durable and legacy detail routes pass through this check. Source deletion
and shorter retention erase encrypted copies via the existing tombstone-first
protocol. Unknown keys hide content without declaring it deleted; receipt expiry
still permits erasure when the key cannot decrypt source linkage. See Evaluation
README and FTI plan section 16 for limits and acceptance.

A 60-second server task enforces expiry and global source deletion, independently
of API reads. The marker is stored before erasing content. Caller-specific
forbidden access suppresses content without globally deleting it. Removing the
approved source files yields `deleted_source`; removing source configuration
fails closed as `forbidden`. File/S3 retain data across restart; the memory
adapter deliberately does not. The same sweep collects abandoned intent-backed
uploads and losing versions using the atomic protocol above. Legacy unclaimed
prefixes without an intent remain a migration limitation.

The old detail route resolves a known durable ID first and never falls back on
an unavailable result. It exposes the legacy shape's bounded first page; the
new API supplies full pages and explicit evidence states. Legacy discovery,
suite summaries and automatic baselines exclude committed registry IDs.
Durable discovery uses the new endpoint and does not depend on a notification.
No additional event notification is emitted yet. Durable comparison and its
panel integration remain B3 work; this change does not infer promotion quality.

Python's `evaluation_registry` client uses the existing raising HTTP transport;
TypeScript exports `evaluation-registry` separately. Manifest-only imports remain
lightweight. Both preserve the best-effort telemetry API unchanged.


## Amendment (2026-09-12): an approval is a resource

The operator's approval was one directory on the server's disk, holding one
`manifest.json`. Everything whose variant or context did not match it was
`forbidden`. That made an instance hold exactly one admitted pair: publishing a
second variant meant swapping the directory, and after the swap **the results
already published under the first one stopped being readable**. A comparison of
two variants needs both readable at once, so this was a blocker rather than a
limitation, and it was equally one for anything publishing unattended — with a
single directory, alternating between two variants meant a human on the host per
publication.

An approval is now a versioned resource Evaluation owns, addressed by the pair
it admits: `approval_id = sha256([1, "evaluation.approval", variant_id,
context_id])`. It records who admitted it and when, plus a `bundle_digest` — what
the deployment adapter verified *beyond* the manifest's own pinned digests,
which is how a model package gets pinned at all, since Training's historical ID
binds artifacts rather than the whole declaration. It is created only after the
adapter resolves the declaration, so an approval is never a promise about bytes
nobody read, and a bundle that changed underneath an admitted pair conflicts
rather than silently moving what every earlier result was measured against.

`POST /api/v1/evaluation-approvals` admits one and `DELETE
/api/v1/evaluation-approvals/{id}` withdraws it. Both are **`admin`**:
`AIWATCHER_AUTH_INGEST_TOKENS` makes a producer an editor by construction, and a
producer that can admit its own evidence has not been approved by anybody.
Publication requires an admitted, unwithdrawn pair — checked *after* the adapter,
so a source that is gone says so rather than arriving as "nobody approved this"
and sending somebody to admit a pair whose bytes are not there.

A **read** requires only that the pair has not been withdrawn. Absence is not
withdrawal: evidence published before an instance kept approvals stays readable,
and the adapter still admits it. Withdrawal hides and is final for that approval
ID; it moves no retention deadline in either direction, because retention is a
promise about how long content is kept and withdrawal is a statement about what
may be read.

The deployment adapter follows: `AIWATCHER_EVALUATION_SOURCE_DIR` is now a
directory **of** approvals, one subdirectory per approval ID, and a single
bundle directly under the root stays readable for instances that have one. A
root holding neither is `forbidden` rather than `deleted_source` — nothing was
deleted, this pair was never admitted.

What this does not yet do is remove the host from a *new* pair: the producer's
artifacts have to reach the adapter somehow, and today that is a mounted
directory. Uploading a bundle through the API is the remaining step, and it is
an addition behind the same resource rather than a change to it.

### Deleting one result

`DELETE /api/v1/evaluation-results/{id}` writes the same durable marker the
retention sweep does, and is `admin`. Evidence therefore disappears exactly
three ways — a withdrawn approval, a deleted or revoked source, or retention —
and this is the narrow case of the first: one measurement rather than every
measurement of its pair. The marker's reason is `deleted_source`, and the
vocabulary deliberately stays at seven states: a reader has to act on what is
missing, and an eighth word for "an operator removed this one" would be a
distinction with no different next step.

## Amendment (2026-09-12): a summary is read, a shard is verified

Reading a header used to read the whole result. `Registry::get` walked every
shard, verified each digest and threw the rows away to return counts and metrics
that were already in the metadata object; `cases` did that and then read its
page; `list` did it per row **including the source resolution**, which reads a
model's artifacts inside a 100 MiB budget or a conversation corpus shard by
shard; and the 60-second sweep did it for every committed claim. Measured at the
instance's starting limits, in object-store requests:

| | before | after |
| --- | --- | --- |
| summary of a 10 000-case result | 105 gets, 1 661 263 B | **5 gets, 11 163 B** |
| first page of 200 cases | 108 gets, 1 704 843 B | **7 gets, 44 165 B** |
| catalogue page of 50 rows | 400 gets, 1 list, 185 550 B | **152 gets, 1 list, 131 535 B** |
| one sweep over 50 rows | 550 gets, 53 lists, 319 050 B | **103 gets, 1 list, 17 806 B** |

Three rules replace it, and none of them relaxes a guarantee.

**A summary answers from its metadata object**, which is itself content-addressed
and verified, and which is where the counts, the metrics and the manifest have
always lived. A shard is verified when the page it is on is read, and a damaged
shard is *that page's* state rather than an error and never a short page — a
page silently missing rows would read as a result with fewer cases in it. The
cost is stated rather than hidden: a result whose shards are gone reads as
complete in the catalogue until somebody opens it, where before it was reported
by any read at all.

**Amended: the pass that already knows says so.** Collection lists what each
result holds in order to delete the rest, beside the header that says what it
should hold, so the gap costs it nothing to notice — and noticing is the only
thing that was missing. `CollectionReport` carries the IDs it found short,
bounded, onto `RetentionReport` as `damaged` and `damaged_count`, which the
catalogue already returns; the row is marked and the detail says when the pass
ran. It is not an eighth `EvidenceState`: the header verifies, the state is
`complete`, and the two saying different things is the fact. A retired result
has no header by design and is not damage — the tombstone is read only where
the header is already missing, so the common result still costs nothing.

**A source is resolved once per admitted pair**, for the length of one list or
one sweep, because a catalogue is mostly repetitions of a handful of pairs and
the answer cannot differ between two rows of one pair. Only a verdict about the
source is remembered; a store that was briefly unreachable is not one, and
caching it would condemn every other row of that pair.

**The sweep asks the receipt first.** `expires_at` is already the minimum of the
instance's clock and the source's, recorded at commit, so expiry needs neither
the metadata nor the owner. Only what is still live is resolved. It counts what
*that pass* retired, never a running total — a count including yesterday's work
cannot tell a working sweep from one that has been failing for a week.

Collection is split off at its own hourly cadence. It lists a prefix per
published result, which is the expensive half and the one that is about a writer
that stopped: an hour late is the same answer as a minute late. Source deletion
is therefore enforced within the hour by the worker, and immediately by any read.

Every pass is written to `evaluations/retention.json` and returned as
`retention` on `GET /api/v1/evaluation-results`: when it ran, what it retired and
collected, and how many consecutive failures precede now. Durable rather than a
log line, so it survives a restart and every replica reads the same one.

### The catalogue's order, and when an index becomes required

The key is `evaluations/{sha256(id)}/`, so the catalogue's order is the order of
a hash and "newest first" would need a full scan. This stays as it is, and the
panel's evidence list therefore carries **no time control** rather than one that
would not narrow anything. A row now costs three requests — a claim, a tombstone
marker and one header — so a 200-row page is roughly 600 requests: fine at the
hundreds, and the point at which an index is required is **about a thousand
published results**, or the first request for an order other than the hash.
That index is one object per commit under a time-ordered key, written after the
claim wins and backfilled by the collection pass; it is deliberately not built
yet, because it is also what would give the catalogue a time order and both
should be decided by the same change.

**Amended (2026-09-12): it is built, and it is what the two halves share.**
`evaluations/index/{i64::MAX - committed_at}-{sha256(id)}` holds the receipt, so
an ascending listing is a descending clock and a period is a bound on the key
rather than a filter over a scan. It is derived — the claim is the truth, and
every detail read still goes through it — which is the shape a prompt's head
already has: losing the whole prefix loses no evidence and the collection pass
rebuilds it, which is also how evidence published before this existed arrives
in the catalogue.

Two things follow. A retirement **marks the row before it writes the tombstone**,
so every window between the three writes shows less than the truth rather than
more, and a retired result keeps a row saying why — vanishing from the catalogue
would say it had never been published. And a page no longer reads the claim or
the tombstone at all: a 50-row page is 103 requests where it was 152, with one
listing of one key per result rather than of every object under `evaluations/`,
which is what the thousand-result threshold was really about. The panel's
evidence list may therefore carry a period control, and this paragraph's "no
time control" is withdrawn.

## Amendment (2026-09-12): what a restore of `evaluations/` means

This prefix, the conversation archive and the execution stream are the three
stores whose contents exist nowhere else, and this is the only one whose
correctness rests on an object *not* coming back. Publication and collection
race at one immutable key; whichever creates `claim.json` first decides whether
a logical ID may ever publish. A restore can therefore resurrect a claim a
collector abandoned, or drop a tombstone that was the record of an erasure.

Restore it whole, to one moment, and never merge two points in time. `content/`
without its `claim.json` is bytes nothing points at; a claim without its
tombstone republishes evidence somebody deleted. Run one collection pass
afterwards and read `retention` to see it complete.

## Amendment (2026-09-12): the admission rule a judge needs

The five source adapters admit a declaration by **reading the owner's bytes
again** — a curated dataset's rows, a prompt's exact version, a model package's
artifact digests, an annotation export's shapes, a conversation corpus's
decrypted shards. A judge's output is not bytes anybody can read back. It is a
model call, and whether it would answer the same way tomorrow depends on a
provider, a revision and a configuration rather than on a digest. Carrying the
"verify the bytes" rule onto it would either refuse every judge or, worse, be
relaxed for all five adapters that do satisfy it.

So a judge gets its own rule, and this ADR states it before the adapter exists:

1. **Its configuration is pinned by content**, exactly as a variant is — the
   provider, the model revision and the complete judge configuration, already
   in `EvaluationContext::judge`. A changed configuration is a changed context,
   never the same evidence measured again.
2. **Its calibration set is part of the evidence.** A judge is admitted against
   a recorded set of cases a human also scored, pinned the way a case manifest
   is. An admission with no calibration set is refused, not defaulted.
3. **Its disagreement with those human scores is stored beside the result**, and
   it is the number worth watching across a series — the judge's analogue of
   `overfit_gap`. A judge selected by maximising agreement on the set it is then
   reported against is the same failure `OptimizationRecord::verdict` exists to
   refuse.
4. **The result is marked as not reproducible by re-reading.** This is a field
   on the evidence, not a convention: a reader comparing two results has to be
   able to tell "these bytes were verified" from "a model said so at the time",
   and a comparison that treats them alike is the reason B3 must inspect more
   than a matching context hash.

Until an adapter implements those four, `LocalSource` refuses a manifest
carrying a judge **by name**, which is the behaviour today and is deliberate:
an absent judge is a working state, and a judge admitted under the bytes rule
would be evidence nobody could interpret.
