# ADR_0030: Evaluation owns pinned variants and durable evidence

- **Status**: accepted; B1 contract and B2 persistence implemented, amended
  2026-09-12 with approvals as a resource, the read/verify split, restore, and
  the admission rule a judge will need, the adapter for a judge this
  deployment asks, what that judge is shown and keeps, a judge over the
  archive told to everyone, and a bundle digest over what a bundle adds;
  amended 2026-09-13 with a framework's metrics behind a scorer service, a
  cohort derived from a dataset version, and a run's settings and stop; a
  variant on the trace and answers held to it; and a second witness, the
  workflow's shape, folds by case, a model addressed by its package, and
  observations kept past the read model; and witnesses a deployment names, a
  gateway for a provider and the prompt, the order a workflow leads, a result
  priced, and periods folded with a state of their own; and an answer bound to
  what a witness relayed, how often a node runs, a price history, and a window
  answered by the period fold alone; and an answer taken out of its reply, a
  request holding only the prompt, a window to the second, and a fold that does
  not start over; and values accounted for, steps to take an answer out,
  integers digit for digit, a log that no longer holds what a fold reads, and
  every width sliced by the second; and a value taken out of another, a tool's
  result, a label the variant pins, an answer made of parts, numbers scored as
  written, a gap refilled, and a count every client keeps
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

**Amended (2026-09-12): the bundle arrives over the API.**
`PUT /api/v1/evaluation-approvals/{id}/bundle/{name}` (**admin**, same reason)
stages one member — a declaration, a scorer, a case manifest, a model artifact —
into the adapter's own prefix, `evaluation-bundles/{approval_id}/`, which every
replica reads and no host holds. `GET` lists what is staged, by name and size
only: the declaration already pins a digest for every member and the approval
pins one for the bundle, so a third copy of that fact could only disagree with
them. `DELETE` clears one, for correcting a bundle before approving it.

The prefix is the **adapter's**, deliberately beside `evaluations/` rather than
inside it. What a bundle *is* is the adapter's question — `aiwatcher-evaluation`
knows only the digest it was told — and a second crate writing the registry's
private layout is the thing that rule exists to prevent. It is reached through
`ApprovalBundles`, a port beside `SourceAuthority` and implemented by the same
adapter, so the route sits with the approval it is for while the bytes land
where their owner decides.

Staging admits nothing by itself. An approval resolves the bundle as a whole and
records its digest, so bytes that arrive afterwards do not widen it: the pair
stops reading until they are what was admitted again. A name is one segment, or
the one folder a bundle has (`model-artifacts/`); anything else is refused in
both adapters rather than resolved as a path. And what is staged takes
precedence over a directory, so an instance keeps whatever it was configured
with and an instance with no directory at all can still admit a pair.

With that, a new variant needs no step on the server's host: an operator stages
and approves over the API, and every repetition afterwards publishes on its own.

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
is not that half — the sweep resolves every live pair once a minute, as it
always did, and a read enforces it immediately. (An earlier draft of this
paragraph said the hour applied to source deletion too. It does not.)

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

## Amendment (2026-09-12): comparing two published results

B3 asked for "comparability for durable evidence", and the shape it wanted was
the folded half's rule applied to published results. That rule compares five
optional strings — dataset, dataset kind, dataset version, suite version,
scorer version, split — pairwise, and most of it is about what a producer did
not send: the three answers exist because a log fold has nothing better than
what arrived on it.

Published evidence has nothing missing to reason about. `context_id` is
`sha256` over the whole `EvaluationContext`: the dataset, the case manifest,
the case count, the split, the suite, the scorer, both schemas, the judge
configuration and every metric definition with its unit, direction and
aggregation. Two results either share that address or they do not, and the
equality *is* the comparability rule. Everything else in
`GET /api/v1/evaluation-results/{id}/comparison` is one of two things:

- **which field moved**, for a reader who has to fix it. Both contexts are in
  hand, so a refusal says "Different split" rather than "different context" —
  and where one side is a tombstone, which keeps no manifest, it says the
  context differs and honestly nothing about how.
- **whether the evidence behind a number can still be read.** An expired,
  withdrawn or damaged side is not *incompatible* — nothing about it differs —
  it is `unverified`, which is the same word the folded half uses for a
  judgement it cannot make.

Three consequences follow, and each was a decision rather than a detail.

**The rules stay two; the vocabulary becomes one.** Collapsing them into one
function would mean projecting a pinned context down to five optional strings,
which loses exactly what makes the second one stronger. What a reader does with
`unverified` is identical on both halves, so `Comparability` moves to
`aiwatcher_core::comparability` — above both crates, taking only the three
words each surface needs — and the panel draws one control. This is
`aiwatcher_core::human_input`'s reason, for a verdict rather than for a
question.

**There is no automatic baseline.** The folded half picks the previous success
because a log fold has no other way to offer a pair. Here the catalogue answers
it: `GET /api/v1/evaluation-results?context_id=…` returns exactly the results
that may be compared with one another, so which of them is the baseline is a
decision somebody makes rather than a default they might not notice. The filter
walks the published index rather than a second one keyed by context — a page
therefore costs the rows it passed over as well as the ones it carries, and a
cursor on a filtered page promises another entry rather than another match. An
index by context is the same derived-head shape as `evaluations/index/` and is
cheap to add the day a catalogue is large enough to need it.

**A comparison reads two headers, never two results.** The metrics it subtracts
are in the metadata each side was published with, so it costs what two
summaries cost however many cases sit behind them. Which cases regressed —
"passed on the baseline, fails now", the view that has to be read before a
release — is deliberately *not* here: both results are sorted by `case_id` at
publication, so a diff can page them in lockstep with a high-water boundary,
but it is still a full read of both sides and it is named as absent rather than
served quietly by a route that walks a hundred shards.

Two smaller rules are recorded here because they are easy to get wrong:

- **One variant measured twice is comparable**, and it is a `same_variant`
  field rather than a reason. The delta is real and it measures repetition —
  how much this measurement moves when nothing changed — which is worth knowing
  and is not the effect of a change. As a "reason" under `comparable` it would
  read as a problem; absent, it would read as an A/B.
- **A delta is withheld rather than absent.** Both numbers stay on screen when
  the two are incompatible: each is a fact somebody measured, and only the
  difference is a claim nobody did.

And one that a metric's declaration finally settles: a delta may be
**coloured** here. Elsewhere in the panel it may not, because a producer's
metric name says nothing about whether a rise is an improvement or a bill;
`MetricDefinition::direction` is declared as part of the pinned context, so the
colour is the declaration's and a metric declaring `none` stays plain.

**What this rule does not yet cover.** The four judge conditions above are the
open edge: a judge's result is not reproducible by re-reading, so two
judge-scored results sharing a context are *not* thereby proven comparable —
the fourth condition's field has to enter this rule when the adapter lands.
Until then `LocalSource` refuses a manifest carrying a judge by name, so no
such evidence exists to compare.

## Amendment (2026-09-12): what an assessment is

The decision above promised B4's shape in one paragraph — one target, a rubric
version, a typed value, a source, an author, a rationale, a time and a
revision. Building it settled four things that paragraph left open.

**A rubric is a resource, not a field.** A number with no scale behind it
cannot be read back: `3` is excellent on one team's form and a failure on
another's. So the form is authored — the question, the words a person and a
judge are both given, the answers it admits (a bounded number, named levels in
the order declared, or a flag) and which end of it is better — and versioned by
its content, exactly as a prompt version is. An assessment names the concrete
version rather than the head, so rewriting the levels is a new version and not
a quiet re-reading of every score already given.

**The standing identity is the mechanism.** "Human and judge assessments
coexist" is not a rule the code has to remember: a standing judgement is
addressed by its target, its rubric *and its source and author*, so the two are
two records and both are returned. A key ending at target and rubric would have
made the second writer an editor of the first. The author comes from the
session for a person, never from the body, and a judge is named explicitly with
the session that ran it kept beside it.

**A repeat is not a revision.** Writing what the current revision already says
lands on that revision. Without it a lost response and a retry wrote a second
revision saying the same thing, and a judge re-scoring nightly wrote one a
night forever. What is given up is the record of having agreed again: the date
that stands is the first.

**Nothing checks the target exists, and the store is its own.** A judgement
outlives the trace it is about, which is why it is written down at all — and
rubrics and assessments live under their own prefixes rather than under
`evaluations/`, because the sweep and the collection pass each list that whole
prefix to filter it by suffix. A hundred thousand judgements there would be a
hundred thousand keys every pass walks past, which is the cost rule that made a
summary stop reading its own shards.

The two absences the decision named hold, and both are absences rather than
checks: an assessment carries no expected answer, because the cohort owns those
and a better answer somebody proposes is a change to a dataset made through
review; and it carries no consent, because a turn's review answers whether the
content may be used at all while an assessment answers whether the answer was
good.

## Amendment (2026-09-12): evidence this deployment measured

Every result so far was measured somewhere else and published here. A scoring
run is the first that aiwatcher measures itself: it reads a recording somebody
staged, scores it against a scorecard somebody declared, and publishes the
result through the same gate. It calls no model of the application under test,
so a new scorecard over an unchanged recording differs from the last result by
the measurement alone. Building it settled five things.

**What is measured is a resource, and which way is better is not its author's
to say.** The suite list the API already served is an aggregate of reports — a
name learnt after the fact, with nothing in it that could run again. A scorecard
is the declaration: named scorers from a vocabulary this deployment implements,
the metric each one writes, and where in an answer and an expectation each one
reads. It holds no code, so publishing one is not a way to run something on the
host. Each scorer's metric definition — unit, direction, aggregation — is
derived from the scorer rather than authored beside it, because a forbidden
phrase declared higher-is-better would invert every comparison drawn from it.

**The suite and the scorer are two references because they are two owners.**
`context.suite` names the scorecard at its content version; `context.scorer`
names `aiwatcher.scoring` at the version of the vocabulary that read it, which
is bumped whenever an existing scorer's answer changes for some input. A
rewritten scorer measures an unchanged declaration differently, and one
reference could not say so.

**Admission reads each pin from its owner, and for this evidence two owners are
not a bundle.** A producer's suite and scorer are files it ran, and the adapter
admits them by re-reading `suite.json` and `scorer.py` from the operator's
bundle. Evidence this deployment measured has neither file: its suite is a
scorecard in this registry and its scorer is the binary, whose version is not a
digest. So the registry admits that kind — recognised by the scorer's name —
against those owners before it asks the adapter anything: the version must be
the one compiled in, the scorecard must exist at the version named, and the
context's metrics must be exactly the ones that card derives. The adapter then
skips the two files that do not exist and checks every other pin as before.
The judge rule above is untouched; a scorecard names no judge.

**A declaration is the run's identity, and it is admitted before it starts.** A
scoring run is declared — variant, cohort, card version and the recording's
digest — and the declaration is addressed by its content, so starting it is an
ordinary managed execution whose plan carries that address and whose id is
derived from it. Repeating a start reaches the run already going; measuring the
same variant again is another repetition, and so another declaration. Starting
is refused while nothing admits the pair, naming the approval that would: a run
started without one could only fail at publication, and that failed run would
be what every later start of the same declaration landed on.

**Absence stays absence.** A selected case nobody answered is unscored, not
zero. A case any scorer could not read carries none of the metrics, because the
contract already requires a scored case to carry all of them, and a case in
three averages out of four would give each metric its own denominator. And a
case the recording answered twice is not scored: one publication is one
repetition, and two answers to one case are two measurements it cannot tell
apart.

## Amendment (2026-09-12): a judge this deployment asks, and the archive as answers

The judge rule above was written before an adapter existed. This is that
adapter, for a judge aiwatcher asks itself inside a scoring run; a producer's
judge still has none, and is still refused by name. Building it, and closing the
rest of the first scoring package's limits, settled six things.

**The four conditions, as they are kept.** A `judge` scorer names a rubric
version, and the metric's scale and direction are that rubric's — a flag is a
rate in `ratio`, a bounded number a mean `score`, levels a mean position from
nought. The run declares the rest: a profile (`openai` or `llamacpp`), the model
and a revision its author pins, and the settings, whose digest is
`context.judge.configuration` (condition 1). The calibration set is the human
judgements B4 recorded about a published result's cases, under exactly those
rubric versions, frozen by content through `POST /evaluation-calibrations` and
named in `context.judge.calibration_dataset` with the new dataset kind
`assessments` (condition 2). Admission reads the settings and the set from this
registry and refuses a set with no person's judgement under any rubric the card
asks. The run puts every calibrated answer to the judge beside the cohort's, and
publishes per metric how many items there were, how many the judge answered on
the scale, the fraction it agreed on — over *every* item, so declining the hard
ones does not agree its way up — and the mean distance (condition 3). The
evidence carries `reproducible: false`, and a comparison of two such results
carries `judged: true` rather than a reason, the way `same_variant` does
(condition 4). Revisions to people's judgements after a set was taken make a
later set; they never re-read this one.

**A reply is decoded against the scale, not read generously.** Every call sends
the scale as a JSON Schema. Live, `gemma-4-e2b` on llama.cpp told "true or
false" answered `"false"` in quotes; the strict reader failed every case it said
no to, so only its yeses were counted. Decoding against the schema fixed the
model rather than the reader. A reason never quotes a reply, and a provider's
refusal carries its status and error message only.

**A judge runs where a socket and a credential are.** A judged run is its own
runtime kind, `judge_evaluation`, claimed in the work role where
`AIWATCHER_JUDGE_URL` and `AIWATCHER_JUDGE_PROVIDER` are, and a start is refused
— 501 without a judge, 422 naming both profiles with another — rather than left
for no process to claim. One outage fails the attempt instead of publishing half
a measurement; the retry asks every question again, which is the cost stated.

**The archive is an answer source, under the approval rather than the session.**
A conversation cohort's case is an assistant turn, and its answer is that turn's
response, so a run may declare `"answers": "archive"`. The executor has no
session to hold content access; it asks the gate first and reads with content
access only for a pair an admin admitted — whose approval resolved that very
content. A recording is refused for such a cohort, since it would keep answers to
archived questions unsealed and outside retention and erasure; a card over the
archive may read no expectation, because the expectation is the response being
measured; and no judge is asked about the archive's words, nor calibrated on
conversation evidence.

**Not yet admitted is one answer.** A publication for a pair with no approval
record, including one the adapter found nothing for, answers 409
`pair_not_admitted` naming the approval, as a scoring run's start does. A
withdrawn pair, a bundle that changed under its approval and a caller who may not
read the source stay 403: none is a step somebody still has to take.

**What aiwatcher measured, only its run publishes.** `POST /evaluation-results`
answers 403 `measured_here` for a context scored by `aiwatcher.scoring`. The first
publication of an ID wins, and only the run knows its numbers, its origin and its
agreement.

A scorer measuring a quantity arrived too: `absolute_error`, a mean distance
whose unit is the author's one word about its metric — the scorer sees two
numbers and never what they count — while its direction stays derived.

## Amendment (2026-09-12, later): what a judge is shown, what it said, and what that proves

Four of the judge's stated limits, taken back up. Each is opt-in for a card or
additive for a report, so no card, context or declaration written before it
changes its address, and `SCORING_VERSION` stays `1`.

**A reply is kept before it is used.** The amendment above said a retried attempt
asks every question again, and called that a cost. It was also a defect: an
attempt whose settlement was lost after it published re-asked, the model
answered differently, the fold wrote different bytes, and the first publication
of that ID refused the attempt as a conflict — a failed run beside the result it
had published. Every reply is now kept under the declaration and the digest of
the exact question, first write winning, and the fold reads the kept one. An
attempt after an outage asks only what nobody answered; an attempt after a
publication lands on it.

**What served a reply is recorded, and compared with nothing.** The declared
model and revision stay the author's word. Each reply keeps the provider's own
`model` and `system_fingerprint`, and the report counts them into `served`. A
provider names a model by alias, file or dated snapshot, so refusing on a
mismatch would refuse the honest ones; two rows are what says the run's answers
came from two backends.

**A judge may be shown what the case asked.** A judge scorer may name
`input_path`, a pointer into the case's input, which is sent before the answer.
The source adapter hands inputs over beside expectations and never into a
shard, and a calibration item's comes from its own result's source. A case
without the input fails naming the path; a calibration item without it counts
against the agreement, under the same over-every-item rule as a declined one.

**A level to reach, and how little a small set proves.** On named levels a card
may name `pass_level`: the metric is the fraction at that level or on the
rubric's better side of it — a rate, which ordered levels support — instead of a
mean of positions, which assumes even spacing. The judge's reply and the
person's judgement go through that one mapping, so agreement is about the
number the result publishes. No floor decides how many calibration items are
enough; the agreement carries its 95% Wilson interval instead, so three of three
reads as 100% reaching down to 44%.

## Amendment (2026-09-12, last): a judge over the archive, told to everyone, and what a bundle admits

Two decisions, both reversing a line above.

**A judge may read the conversation archive, and nobody who could stop it is
left untold.** The earlier amendment refused a judge over a conversation cohort,
and a calibration set taken from conversation evidence, by name. Both are
allowed. The cost is unchanged and stated: the provider keeps what it is sent
outside ADR_0021's encryption, retention and erasure, and nothing here takes it
back. What carries the decision is that the fact is not left to be noticed:

- `context.judge.reads_archive` is derived — from the cohort's dataset kind and
  from the calibration set's `from_archive` — and is part of the context, so an
  admin admitting the pair admits it, and admission refuses a context that says
  otherwise;
- a declaration's view carries `warnings` in words, written once on the server;
- the panel holds admitting and starting until the warning is acknowledged,
  including in the Approvals panel, and a result's judge note repeats it;
- the executor logs it when it runs;
- a calibration set from conversation evidence is taken by an admin, who may
  read the cases it is about.

What stays here holds none of it: evidence from the archive is sealed as
before, and a kept judge reply is canonical — the value it scored, or a
stand-in refused for the same reason — never the reply's words.

**An approval admits what a bundle adds, not which run wrote its manifest.** The
bundle digest an approval recorded covered the staged `manifest.json` bytes,
whose `origin` names a run. The second run of an admitted pair therefore changed
it, hid every result the pair had published, and was refused on re-admission as
a conflict of two results under one ID. The digest now covers what a bundle adds
beyond the manifest's pins, by content — a model package, or nothing — which is
what `ApprovalRecord::bundle_digest` always said it was. An approval recorded
over the whole declaration still admits against that earlier digest while those
bytes stand, and a pair admitted over other bytes answers 409
`admitted_other_bytes`, naming the approval.

## Amendment (2026-09-13): a framework's metrics, a derived cohort, and a run that stops

Three additions to what a scoring run is. None moves a context written before
it: every new field is absent from the bytes when it is unused.

**A scorecard may name a framework's metric, and no framework is named here.**
DeepEval, Opik, Ragas and the rest each ship dozens of metrics, and every one of
them is Python a scorecard must never carry. They run in a scorer service the
deployment operates (`services/scorers`) behind a two-route contract — a
catalog and one metric over cases — and `Scorer::External` names an adapter, a
metric and its parameters. Adding a framework is an adapter there; nothing in
`aiwatcher-evaluation` changes. What keeps the rules this ADR already has:

- **Which way is better is still never the author's.** The work role, which
  holds the service's socket, records its catalog in the registry; publishing a
  card copies the catalog's description of each external metric into the card
  version — the framework release, the model a graded metric asks, its unit,
  direction, aggregation, range and what it reads. A later catalog changes no
  published card, so upgrading a framework is publishing the card again: a new
  suite version, and a result that does not compare with the old one.
- **The service is held to the card.** An `external_evaluation` step, claimed
  where `AIWATCHER_SCORER_URL` is, reads the live catalog before it asks
  anything and fails naming both when the release or the model differs; the
  service refuses the same request with a 409.
- **A reply keeps its number and never its words.** The contract has no field
  for a framework's `reason`, a failed case is reported by the exception's class,
  and replies are kept per declaration and question as the judge's are.
- **A number a model graded says so, uncalibrated.** Its metric definition
  carries `measured_by` with the model, the result reads as not reproducible,
  and the declaration warns — acknowledged before admitting and starting — that
  nothing measured how often that model agrees with people. This is weaker than
  the rubric judge's rule above, which publishes agreement beside its numbers;
  the difference is stated rather than hidden, and a calibration for framework
  metrics is an addition behind the same card.
- **Over a conversation cohort, the service is sent the archive's words** and a
  graded metric sends them on to its model's provider. The declaration and the
  Approvals panel say so, as they do for a judge.

**A cohort may be derived from a dataset version this deployment owns.** The
source adapter already derived a curation version's, an annotation export's and
a conversation corpus's cases from their owners every time it admitted a pair;
the three files a cohort pins were a second copy of that answer. `POST
/api/v1/evaluation-cohorts` derives them as canonical bytes — optionally the
owner's first `limit` cases, never a sample — and records where they came from
under the digest of the cases. Nothing is staged: admission derives them again
when a pinned member is not in the bundle and holds them to the pins, and a
staged member that does not match is refused as before. The owner checks accept
a prefix of the owner's cases as declared, so a smoke run of ten cases is its own
cohort, its own context and a result that compares with nothing measured on all
of them. An external cohort is not derivable: its cases are its producer's.

**A declaration carries how it runs, and a run stops when it is cancelled.**
`settings.timeout_seconds` and `settings.concurrency` are part of the
declaration — a retry reads the deadline the first attempt had — and never of
the manifest. A pace past the deployment's `AIWATCHER_JUDGE_CONCURRENCY` or
`AIWATCHER_SCORER_CONCURRENCY` is refused at start rather than lowered. The
reactor now watches every attempt it performs: a run that is cancelling or has
ended, or a deadline that passed, sets the attempt's stop signal, and the
scoring step drops a judge's or a service's questions in flight and publishes
nothing it had not already begun to. A worker hears the same at its next
heartbeat, as 409 `execution_stopping`.

## Amendment (2026-09-13, later): a framework's model held against people, and answers generated now

Two additions, and again neither moves a context written before it.

**A framework's graded metric may carry agreement with people.** The amendment
above published such a metric warned as uncalibrated and named calibration as
an addition behind the same card; this is that addition, and it keeps the rubric
judge's rule rather than a looser one:

- **The card says what agreeing means.** `Scorer::External.calibration` names a
  rubric version, the metric's `pass_at` — a pass at it or on the side the
  catalog declared better — and, on named levels, the `pass_level` a person's
  judgement passes at, or on a numeric rubric its `pass_score`; a yes-or-no
  rubric passes on its better answer. A verdict on each side, because a
  relevancy of 0.83 and "good" are on different scales and only "passed" is a
  claim both make. Refused at publication: a metric or a rubric with no better
  end, a bar outside the declared range, a level the rubric lacks, a score
  outside its scale, and a bar of the wrong kind for the rubric.
- **The run names the people, and the context pins them.** `external_calibration`
  on the declaration names a calibration set taken under that rubric — possibly
  the one a judge names too — and the context pins it as a `CalibrationPin` with
  `reads_archive` derived from where the set was taken. Admission holds it to the
  card as it holds a judge's: present exactly when the card calibrates, a set this
  registry took, covering every rubric asked, the archive flag as derived. A
  result calibrated on other people is a different context.
- **Agreement is counted as a judge's is.** The step asks the metric about every
  item under the rubric, put the answer the person was shown; an item it could
  not be asked about, or one the service scored nothing for, counts against it.
  The result carries `external`: per metric, the fraction of all items whose two
  verdicts matched, its Wilson interval, and the share of answered items they
  differed on. Publication refuses a calibrated context without it and a report
  against another set.
- **Two readings beyond the bar, applied to nothing.** A verdict agreement says
  nothing about a bar a little either side, so each row also carries
  `rank_agreement` — Goodman and Kruskal's gamma between the metric's numbers
  and the people's judgements, both turned so more is better, with the pairs it
  was counted over — and `fitted_pass_at` with `fitted_agreement`: the metric's
  number on this set whose verdicts would have matched most often, nearest the
  card's own among equals. Gamma rather than a tau, because a person's side is
  coarse by design and a tau stops short of one for a metric that put every yes
  above every no. The fitted bar is found on the very items it is scored on, so
  it flatters itself; nothing adopts it, and publishing a card with it is a new
  context whose agreement a later set has to measure.
- **It is still a model's word.** `reproducible` stays false; the warning now
  says where its agreement is measured instead of that nothing measured it.

**A run's answers may be generated when it runs.** C0 scored answers somebody
already had; C1's template generates them. `Answers::Generated` names a worker
task (`name@version`) and its queue — never code — and the plan is three steps:
`evaluation_cases` in the serve role reads the cohort under the pair's admission
and writes each case's input as rows **and nothing the case expected**, the task
answers each case told the variant manifest it answers as, and the score step
reads the rows it wrote from its own input. What makes the answers honest:

- **A generator never sees an expectation.** It could otherwise answer by
  copying one, and nothing in the numbers would say so.
- **A retry never asks the application again.** The score step reads the answers
  the completed generation attempt produced, pinned in the run; a rerun of the
  whole measurement is a new repetition.
- **A declined case is unscored, not zero**, so coverage says what was missing
  and a comparison with it is withheld as unverified.
- **Not over the conversation archive.** Each case's question would reach a
  worker as a row outside the archive's seal, retention and erasure.
- **A task says what it generated with, and is held to the variant's pins.** Only
  the worker holds the code and generation config the variant pins by digest, and
  one built from another commit would answer under the variant's name with
  something else. So the generation step writes `generated_with` — the digest of
  the code, the generation config, and the response schema and tools when the
  variant pins them — and the score step refuses answers whose `generated_with`
  is missing or disagrees, naming both digests. The SDK's `generation_task`
  asks first and fails before a case is answered. It is the worker's word checked
  for agreement, not a proof: a task that echoed the pins would pass. The model,
  prompt and workflow are references a task resolves through a registry rather
  than bytes it holds, and are not reported.

A baseline and a candidate are two declarations that differ in their variant and
evaluation ID alone, so their contexts match and the comparison above applies.

**A result says what its answers took, and an experiment is a context's results
side by side.** C2 needs quality, sample size, errors, time and tokens per
variant, and a trace outlives none of the evidence. So a case carries `usage` —
latency in milliseconds and input and output tokens, each absent where nobody
measured it — as the producer measured it; `generation_task` times each answer
and takes the tokens the application counted. Publication derives `usage` into
the header: per-case latency by nearest rank (p50, p90, p99, max) and token
totals, each with the number of cases it was counted over, so coverage is a
fact on the row. A percentile is of one result's cases and is never combined
with another's. `GET /experiments` groups the newest thousand results by
context; `GET /experiments/{context_id}` returns every result in it, each
compared with the named baseline through `comparison::compare`, beside the log
fold's summary of each managed run behind them — the whole run's duration, kept
apart from a case's latency, and absent once the log no longer holds the run.
Production observations are not rows: a trace does not yet name the variant
that produced it. Nothing is priced without a price source.

**A line admits every variant of one experiment, and a gate decides whether one
may ship.** C3 measures a new variant on every commit, and a pair is a variant:
each commit was an admin staging a bundle and admitting a pair, which is a person
in every pipeline. The operator's decision a gate relies on is not which variant
— that is the point of measuring — but what it is measured on and how. So an
admin may admit a **line**, `(context_id, experiment_id)`, from any declaration
of it: checked as a pair's context is, and refused for evidence a producer
measured (whose numbers nothing here computed), for a context over the
conversation archive (whose pairs an admin who may read them admits one at a
time) and at start for a variant naming a model or a workflow (whose packages a
pipeline cannot send). A variant the line covers is still approved by name when
its run starts: the registry stages the declaration and each file the variant
pins from bytes an editor kept by digest (`PUT /evaluation-variant-artifacts`),
the adapter resolves them as it would an operator's bundle, and the approval's
`approved_by` names the line and who started it. Withdrawing a line admits
nothing further; what it admitted stays admitted, each withdrawable.

The verdict is the server's: `POST /evaluation-results/{id}/gate` holds the
comparison to a policy — a tolerance per metric in its unit, metrics ignored,
critical cases — and answers `pass`, `regression` (a metric worse than its
tolerance, or a critical case not measured or worse, beside any average),
`incomplete` (a case failed or went unscored; never a pass) or `error` (the pair
does not compare, or the candidate is not readable). `aiwatcher-gate` in the SDK
is the recipe around it and exits 0 to 3; `examples/ci-gate` is a deterministic
job, and `just e2e-gate` proves each verdict against a server of its own.

**Feedback becomes a case through people, into a dataset version.** C4 closes the
loop from production back to tests, and the one thing it may not do is turn a
judgement into ground truth by itself. A proposal (`CaseProposal`) names the
curation dataset it would join, the target it was seen on — a trace, a span, a
session or a case, the assessment target's own vocabulary — the question, what
was answered, why, and whose words they are: `written` by a reviewer, or
`observed` and copied from somebody using the application. Its ID is derived
from the dataset and the target, so a second proposal of the same thing lands on
the review under way. Each action is a create-only revision: an expected answer
written (`ready`; an edit of an approved one is `ready` again), approved — by an
admin when the words are observed — or rejected with a reason. Publishing reads
the dataset's current version, refuses one whose rows are not cases, appends one
row per approved case as `review-<id>` with the question and the expected answer
people wrote, writes the next version with `produced_by:
evaluation-reviews/<dataset>`, and only then marks the proposals published in
it; a published case does not change. The conversation archive is not a
source: its words leave the seal, retention and erasure only through a corpus
export, and a proposal holds what its proposer supplied.

## Amendment (2026-09-13, last): what a trace says about a variant, a bar checked where it was not fitted, a line over a model, and a case's own words

Five limits of the amendment above, each closed without moving a context or a
result written before it.

**A run names the variant that answered in it.** The envelope gains
`variant_id` — the content address `Evaluation::prepare` derives, which a
scoring run's view now returns — recorded on every event of the run and as
`aiwatcher.variant.id` on every span; ADR_0001's envelope stays flat, and a
record without the field reads as a run that named none. The telemetry clients
take it once, on the client, never from the environment: a worker measuring a
variant imports the same application, and an inherited variable would put a
benchmark among what the variant was observed doing. A run made to answer a
measurement's case says so on its start (`data.evaluation_id`), and
`Generation.traced` opens such a run. The read model folds both, the explorer
pivots on the variant, and `GET /experiments/{context_id}` answers `observed`:
for each variant the rows measured, the runs that named it **and no
measurement made** — outcomes, duration by nearest rank, tokens, the measured
runs counted apart — over `?window_seconds=`. It is another sample on another
clock, so nothing folds it into a result's numbers.

**A generated answer is held to the variant's prompt and model through its
run's trace.** `generated_with` stays the worker's word about the bytes it holds;
the model and prompt are references it resolves, so their witness is the
application's own telemetry as this deployment folded it. A generated run is now
four steps: `evaluation_cases`, the worker's `generate`, **`evaluation_traces`**
in the serve role — the one role holding the fold — and the score step. An
answer may name its `run_id`; the traces step waits a bounded while for those
runs to end with a span per call and refuses the answers, publishing nothing,
when a run names another variant or result, a call rendered another version of
the pinned prompt, or the pinned model served a call at another version
(`aiwatcher.model.version`, from the serving profile's `model_version`). What
the traces do not show — an answer naming no run, a run the log never received,
a model named without its version — is counted and not refused, because
telemetry is best effort by design and silence contradicts nothing. The result
carries `traces` (answers, named, seen, on the prompt, on the model), a case
without a trace ID leads to the one its run was seen in, and a gate policy with
`require_traces` holds anything short of all of them `incomplete`. Still not a
proof: an application reporting the pins while calling something else passes;
one reporting something else no longer does.

**A calibration's fitted bar is checked on items it was not fitted on.** Each
calibrated metric's row carries `held_out`: the same fit made on one half of the
set and scored on the other, both ways round, so every item is scored once by a
bar that never saw it, with its Wilson interval and the bar each half found. The
halves are dealt by a digest of the case ID, so a case two people judged stays
on one side and a set is dealt the same way on every run. The card's own bar
was fitted on nothing, so `agreement` is already out of sample; `held_out` is
what the fitting procedure is worth, and `fitted_agreement` above both is its
flattery. `rank_interval` gives gamma a 95% interval from its asymptotic
standard error through Fisher's transform — checked against gamma's spread over
fresh samples — absent where every pair was ordered one way. Nothing applies a
fitted bar.

**A line admits a variant naming a model or a workflow.** The refusal said a
pipeline could not send a model's package. It does not have to: the adapter
names what those references imply (`ApprovalBundles::pinned_members`) — the
model's package as the training registry holds it, derived, which is the
declaration admission compares with the owner's, and each artifact and the
workflow's declaration by the digest their owners pinned. The line stages the
package and copies the rest from the variant artifacts a pipeline sent by
digest, held to digest and size; a member nobody sent is refused naming it and
the route. No URI is fetched, and each variant is still approved by name. A line
over the conversation archive and over producer evidence stays refused.

**A result's case gives its own words to a proposal.** A proposal of a case
target may leave out its question and name where the case sits — the `at` a
comparison row carries, never a search through shards. The registry reads that
case and the cohort's input for it: the question asked and what the variant
answered, content `measured` — this deployment's data already, approved by an
editor. A position that is another case, conversation evidence, and a trace or
session with no question are refused; a question must say whose words it is.
Proposals are indexed by their target after the revision is written, and `GET
/evaluation-reviews/of-target` answers every review of one target whichever
dataset it joins, which a case's judgements show.

## Amendment (2026-09-13, closing): a second witness, the workflow's shape, folds by case, a model addressed by its package, and observations past the read model

Six limits of the amendment above, closed without moving a context or a result
written before it.

**A generated answer may have a witness that is not the application.** The
traces were the application's word from the worker's host. The log now records
who published each event (ADR_0001, amended), and a run that served another
run's call names it (`caller_run_id`). The traces step reads, for each answer's
run, the runs that say they served one of its calls, and counts the answer as
**witnessed** when a call on the pinned model at the pinned version was
published under another credential than the answer's run; the same credential's
serving run is its own word and counts for nothing. A witness saying another
version refuses the answers. `GenerationTrace.witnessed_model` counts them, and a
gate policy's `require_witness` holds anything short of all `incomplete`. A
serving host still has to report — `aiwatcher_sdk.serving` does, from the header
`LlmCall.caller_headers()` gives the application — and a provider outside the
deployment does not; what a provider said served each call
(`gen_ai.response.model`) is counted in `served` and compared with nothing, a
judge's `served` for a generator.

**A variant's workflow is held to its declaration.** A run's own
`workflow.declared` is folded into a digest of its shape — node IDs and edges,
never the producer's version string (`aiwatcher_core::topology`) — and the nodes
it stepped through are kept. The traces step reads the pinned `workflow.json`
from the pair's bundle, held to its digest, and refuses a run that declared the
pinned workflow in another shape or stepped through a node that declaration
lacks; a run that declared nothing is not seen on it, and a declaration naming
no node is said to be one no run can be seen executing. `on_workflow` counts
them into `complete`. `Generation.traced_workflow` opens such a run.

**A small calibration set is held out a case at a time.** Two halves left each
fit half of a set that was small to begin with. While a set holds at most 200
cases, every case is a fold — each bar fitted on all the others and scored on
the one left out — and a larger set is dealt into ten folds of cases by digest;
`held_out` says how many bars were fitted and the lowest and highest.
`fitted_pass_interval` is the fitted bar's own interval: the middle 95% of the
bars 1000 redraws of the set's cases fit, the redraws seeded from a digest of
those cases so a retry folds the same bytes. A fit is one pass over the sorted
numbers. A result measured with two halves still reads, with its two bars.

**A line admits a model this deployment never registered.** A variant's model
version the training registry does not hold is read as the sha256 of the
`model-package.json` a pipeline sends, like the weights; it must hash to the
version and pass ADR_0023's package checks, and the artifacts it lists are named
once it is staged — the adapter is asked again with what the line has staged
until nothing new is named. Admission holds a package that hashes to the
version to its own bytes and any other to the registry, as before; a registry
version is a digest of another tuple, so the two are never mistaken.

**A result's every case has a position, and a reviewed case names its split.**
The case route gives each case its own `at`, the first included, so a case is
proposed from a result's case list as from a comparison row. A proposal and the
expected-answer action carry a split, published in a `split` column; a curation
cohort of a split takes the rows naming it and the rows naming none, in the
owner's order, so a version without the column derives the digests it did. A
row naming none joins every split's cohort, which is kept rather than refused
and counted in the derived cohort's `unsplit`.

**What a variant was observed doing outlives the read model.** The serve role
writes each closed period — an hour unless configured — as a create-only record
per variant and a marker that commits them, only when its fold can vouch it
holds every run that ended in the period. A window reads the written periods
lying wholly inside it and the runs the read model holds that ended in none of
them, so a run is counted once; durations are histograms an eighth of a doubling
wide, so periods add and a percentile over them says it is bucketed. Each model
call is timed with its first token, tokens are kept per model, and a deployment's
price table prices them — one currency, every price with the page it was read
from and the day, refused at start-up without either — with unpriced calls
counted, never free. A result's usage names no model, so it is not priced.

## Amendment (2026-09-13, final): witnesses a deployment names, a gateway, the order a workflow leads, a result priced, and periods with a state of their own

Four limits of the amendment above.

**A witness is a credential the deployment names, and a shared one is said to
be.** `AIWATCHER_WITNESSES` lists the credentials whose runs may witness a
generated answer; with none listed, any credential other than the answer's own
still does. A serving run published under the answer's own credential — one
token on two hosts — is counted `self_witnessed` and named in the gate's reasons,
rather than passed over in silence.

**A provider outside the deployment, and the prompt, get a witness.**
`aiwatcher_sdk.gateway` is an OpenAI-compatible relay that can hold the
provider's key, so the application need not. For each call it publishes, under
its own credential, a run naming the caller's run with the model the provider's
reply said served it — `model_version`, which the traces step holds to the
variant's pin — and whether the request's text holds the template of the prompt
version the caller names (`prompt_verified`): every literal part of the template
in order, with anything where a placeholder stands. A gateway that found the
pinned template witnesses the prompt (`witnessed_prompt`), and one that found
other words refuses the answers. It publishes neither the request nor the reply,
and it posts the witness before the reply ends. `require_witness` now requires
both witnesses wherever the variant pins what they show.

**A workflow is held to the order its declaration leads.** A run's node steps
are kept in log order, and a run that started a node before any node the pinned
declaration leads into it from had completed is refused. One completed
predecessor admits a node, so a branch and the join after it pass.

**A result's cases are priced.** A case's usage may name its calls by model
(`ModelUsage`); for generated answers the traces step reads them from the run's
spans and the score step fills them in. The Experiments route prices each row
from its result's usage at the deployment's table, at read time, with the day
each price was read; the evidence carries tokens, never a price.

**Observed periods are a projector output with a state of their own.** The
snapshot of the read model this replaces could not write a period after a
restart that does not replay (Laser), and lost a run whose end arrived after its
period was written. The fold reads each event once, closes a period when the
log's clock — `min(occurred_at, ingested_at)` — has passed it, and saves its
state with the position it was folded through; the projector resumes from that
position when it is behind its checkpoint, and a period that could not be
written holds the checkpoint back. A late end is counted in the oldest open
period and said to be (`late_runs`); a run whose start the fold never saw is
counted without a duration, in a period marked incomplete.

## Amendment (2026-09-13, after): an answer bound to what a witness relayed, how often a node runs, a price history, and one answer per window

Four limits of the amendment above.

**A witness can say an answer is the reply it relayed, to a request that held
the case's input.** A gateway sees a call's words and must put none on the log,
so it publishes keyed digests instead: of each message, of each value the
application says it rendered the prompt with
(`LlmCall.caller_body`, a body field the gateway removes before the provider
sees it) where it found exactly that rendering in the request, and of each reply
— as text and, where the reply is JSON, as its canonical form. The key is an
HMAC of the gateway's own credential's secret (`aiwatcher_core::witness`), which
this deployment issued: the traces step can ask whether an answer is among the
replies and whether a case's input is among what was asked, while a reader of
the log cannot test a guess against a one-word answer, and the application,
holding neither the credential nor the key, cannot publish one. It counts
`witnessed_answer` and `witnessed_input`; a gate's `require_witnessed_answer`
holds anything short of all incomplete. An application that holds its own
provider key and answers from a call made around the gateway has neither, even
where its model and prompt are witnessed; so has one that reshapes a reply before
answering, which is why this is a policy of its own. A template made mostly of
variables now says more too: the values it was rendered with are checked by
exact rendering, and one of them is the case's input.

**A workflow is held to how often it leads a node.** A completed node sends the
run on along each edge out of it, once; a start uses one of those and a failed
start gives its back, so a retry needs no second completion. The run enters
where nothing outside leads in — a node no edge enters, or a cycle entered from
nowhere else — once. A declared loop goes round as often as its nodes complete,
and a node run twice for one completion, or again when nothing leads back into
it, is refused. A node declared `"repeats": true` runs as often as it likes once
admitted, and is part of the shape's digest only where declared. A run that took
more steps than its fold keeps is counted as unseen on the workflow.

**A price table keeps its history.** A model may have an entry per day its price
was read; a call is priced by the latest read on or before its day, and one older
than every entry by the earliest, counted as priced before its price was read. A
result is priced on the day it was committed, and an observed period on its own
day, so the same figure reads the same whichever day it is looked at.

**A window is the period fold's alone.** It counts every period it reaches into,
whole — written ones from the store, the rest from the fold's memory, with the
runs in flight — so a late run is counted the moment it ends and no run is
counted from both a period and the read model. Periods are five minutes unless
configured, a whole part of an hour, written only where something ended, and
rolled up into hours and days, so a week reads a handful of records; the answer
says where counting began (`counted_from`), whether the window reaches back
before the fold began, and how many runs were late. Without a window the answer
is still the read model's. The fold's state is saved as generations under the
position it reached, and the one furthest along is loaded, so two processes
sharing a processor ID cannot set it back.

## Amendment (2026-09-13, beyond): an answer taken out of its reply, a request holding only the prompt, a window to the second, and a fold that does not start over

Four limits of the amendment above.

**An answer read out of a reply is witnessed as that reply's.** An application
may say, in the body field the gateway removes, how it takes its answer out of
the reply before the reply arrives — `{"json_pointer": "/label"}` or
`{"between": ["Answer:", null]}` — and the gateway takes it the same way and
digests what it took beside the reply. The vocabulary is those two rules and
nothing a caller can extend: a regular expression would be code a caller runs
inside the gateway. Canonical JSON spells every number as JavaScript's
`String(number)` in both languages, so a float digests the same on either side.

**Words beside the prompt witness nothing.** The gateway says whether the
request was nothing but the pinned template rendered and the values it was
rendered with (`prompt_exact`). The traces step counts an exchange where one
witnessed call relayed the answer to such a request holding the case's input,
with the answer itself not among what was asked; `require_witnessed_answer`
requires an exchange for every answer wherever the variant pins a prompt. An
application that tells the witnessed model what to say — in another message, or
as the answer handed over whole — has the reply witnessed and no exchange. What
it can still do is put that answer inside a value the template renders, which
is its own word.

**A window counts to the second, and a late run where it ended.** A run whose
end reaches the log after its period closed is held in the oldest open period
by the period it ended in (`late`), and every period keeps its runs in up to 300
slices by when they ended — a second at five-minute periods. A window counts the
runs that ended from its start on: whole periods after the one its start falls
in, that one by its slices, and each late run by the period it ended in, so a
late run is counted where it ended and never where it arrived. A day and an hour
add up their periods' figures and late runs, not their slices.

**Neither a new width nor a lost state starts observations over.** A width
configured anew takes over at the next hour, which every width divides, so a
period of the old width and one of the new never overlap, runs in flight carry
across, and hours and days keep adding up. The last three saved states are
kept; a marker says where the fold was when its period closed, so a fold with no
state left starts again from the last period written, reads the hour and the day
that period lay in back from the store, and marks the period it resumed in
incomplete — runs that had ended in periods not yet written are the gap. A
price is read for each call on the day it ended, so a run across midnight pays
both days.

## Amendment (2026-09-13, further): values accounted for, steps to take an answer out, integers digit for digit, a log that no longer holds what a fold reads, and every width sliced by the second

Five limits of the amendment above.

**A value the application made witnesses nothing.** A request that is nothing
but the pinned template rendered could still carry an answer obtained elsewhere
inside a value it was rendered with. The gateway now also digests each value as
a reply's digest is made (`aiwatcher.witness.rendered`, ADR_0001 amended), and a
call counts towards an exchange only when every value is accounted for: a part
of the case's input — the input itself or anything inside it — or the reply of
another call of the run that is accounted for in turn. Calls are added once all
their values are, until none is left to add, so a chain feeding a model's reply
into the next call counts from where the input went in, and two calls rendered
only with each other's replies count for nothing. The cost is stated: a value
the application derives from the input — a part of a string, a reformatting —
is its own word and breaks the exchange, so an application passes the input's
fields as they are. What a tool returns is not accounted for either.

**An answer is taken out with steps, not with a pattern.** `answer_from` may be
one step, a list of steps each reading what the one before took, or
alternatives of which the first to find something is taken. The steps are a
JSON pointer, text between two markers (either may be absent), text after a
marker's last occurrence, one non-blank line, the body of a fenced code block,
stripping characters, lower-casing, and reading a number to spell canonically.
Each reads the text once and none runs a pattern a caller wrote; the
application takes its answer with the same function the gateway does
(`aiwatcher_sdk.gateway.extracted`). A transform with knowledge of its own — a
label mapped to a word, an answer combined from several replies — is still the
application's word.

**An integer is compared digit for digit.** Canonical JSON spells an integer
exactly however wide, in Python and in Rust. A parsed row holds an integer wider
than 64 bits only as the double it rounds to, so the worker's output route
stores a table holding one as it was sent, and the traces step compares each
answer from the JSON the generation step wrote. A scorer comparing such an
answer with an expectation still reads both parsed.

**A log that no longer holds what the fold reads says so.** The fold needs the
log from the position its saved state reached, and after every state is lost
from the position the last period written names. On a log that numbers every
event (ADR_0002, amended), a position further on than the one after the last
the fold read is events it was never given — retention passed them, or a record
there could not be read: the fold writes down the
positions and the span of time they may have lain in, from the log's clock
before them to the first event after, under `gaps/`, marks every period that
span reaches incomplete, and every window over the span returns it as `missed`
with the number of events. The runs are not recovered — nothing kept them — and
a log that does not number every event reports none.

**Every width is sliced by the second.** A period keeps its runs by the second
they ended in, whatever its width: an hour-wide period holds as many slices as
twelve five-minute periods, so the same span costs the same. A period written
before keeps its wider slices, and a window starting inside one says it counted
from where that slice began.

**A cycle is bounded by its way back.** An edge may declare `at_most`, bounding
how often a run follows it — the rounds of a cycle through several nodes
(ADR_0012, amended).

## Amendment (2026-09-13, closing): a value taken out of another, a tool's result, a label the variant pins, an answer made of parts, numbers scored as written, a gap refilled, and a count every client keeps

Six limits of the amendment above.

**A value taken out of the input is the input's.** The caller may say, beside a
value, which of its other values it took it out of and how
(`derived={"country": {"from": "question", "take": {"between": ["capital of ",
"?"]}}}`), in the closed steps an answer is taken out with. The gateway takes it
out the same way and, where it gets the same text, publishes the pair of digests
(ADR_0001, amended); the traces step then accounts for the value as the one it
came from, down a chain of them. A way of taking that knows more than the text
it reads — a `map` — derives nothing. Rendering a value any other way is still
the application's word.

**A tool's result is the tool's word when a witness relayed it.** The gateway
relays the tools the deployment names (`--tool atlas=https://…`, a bearer token
per tool from its own variable), never a URL a caller names, and publishes keyed
digests of each part of the arguments and of what came back. It digests a
model's tool-call arguments as that model's reply. One fixpoint accounts for
calls and tools together: a tool call is accounted for once every part of its
arguments is — a part of the case's input, a reply of an accounted call, a
result of another accounted tool — and its result then accounts for a value
rendered with it. Arguments the application made account for nothing.

**A label's word counts where the variant pins how it is taken.** `answer_from`
may map a label to the word it stands for (`{"map": {"A": "Paris"}}`). That
knowledge is not in the reply, so the gateway publishes what a rule holding one
took apart from the replies, beside the rule's keyed digest; the traces step
reads `answer_from` from the variant's pinned generation config, read out of the
pair's bundle under its digest, and counts those as replies only when the digests
agree. A map chosen per call — which could name any answer — counts for nothing.

**An answer made of several replies counts in the shape the variant pins.** An
object or a list answer is an exchange when the variant pins a response schema,
every key the answer uses is one the schema names at that place, and each part
— text as itself, a number or a flag as its canonical JSON — is the reply of an
accounted call whose request did not hold it. Words joined into one text are not
split back into replies.

**A scorer reads numbers as they were written.** Numeric scorers and exact match
compare exact decimals (`aiwatcher_core::exact`): an answer from the JSON its
generation or its recording wrote, and a text that is nothing but a number as
that number, so a curation cohort's expectations — texts — compare with a
number digit for digit. A distance becomes a double only as the published
metric. A table holding a decimal longer than a double keeps is stored as sent,
as a wide integer already was. The scorer vocabulary is version 2, because some
answers now score differently, and a context declared under version 1 is refused
at admission naming the version this binary implements.

**A gap is refilled where a journal kept it, and lost events show on any log.**
The period fold looks for a gap in the journal's pages first (ADR_0002, amended)
and folds what they hold as it would have from the log; only positions no page
covers are written down. Every SDK client numbers its events per run (ADR_0001,
amended), and the fold counts the numbers it never read for each run it tracked:
the run is counted with what arrived, its period says it is incomplete, and a
window returns `lost_events` beside the log's gaps.
