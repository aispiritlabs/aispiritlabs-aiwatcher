# ADR_0030: Evaluation owns pinned variants and durable evidence

- **Status**: accepted; B1 contract and initial B2 persistence implemented
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
