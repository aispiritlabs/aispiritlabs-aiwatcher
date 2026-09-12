# Evaluation contracts

`Evaluation::prepare` validates a version 1 manifest and freezes its variant and
measurement context. It returns their fingerprints and the normalized manifest.
It performs no network requests, artifact reads or registry writes.

From the repository root:

```sh
rtk cargo run -p aiwatcher-evaluation --example prepare -- contracts/fixtures/evaluation-v1/manifest.json
rtk cargo test -p aiwatcher-evaluation
rtk proxy python3 scripts/check-evaluation-contract.py --write
```

The first command prints a validated snapshot, including `variant_id` and
`context_id`. Repeating it gives the same IDs. Change a generation-config digest
to change the variant; change a scorer version to change only the context.
Unknown fields, missing artifact digests/lengths, differing dataset references,
duplicate metrics and steps without executions are refused.

The generated JSON Schema describes the wire shape. Rust applies additional
semantic checks; Python TypedDicts and TypeScript interfaces are producer types,
not runtime validators. Artifact existence, byte integrity and permission must be
verified by `SourceAuthority` before accepting durable evidence and on reads.

The crate is sliced by what it owns: `manifest`, `context`, `reference`, `result`,
`registry`, and private `store`. Callers
use its root facade and exported types. It imports only Core from the workspace.
`Registry` now publishes terminal results through atomic object creation. [ADR_0030](../../docs/ADR/ADR_0030_EVALUATION_EVIDENCE.md)
records the publication protocol, retention and legacy-read requirements.


## Durable results

`Registry::publish` validates the cohort/repetition and metric definitions,
computes aggregates, verifies immutable shards and atomically claims the ID.
Retry the same body after a lost response. Reusing that ID for changed content
conflicts; a new measurement needs a new ID and repetition. Empty expected
answers are preserved. Missing cases and scorer failures remain explicit.

`get`, `cases` and `list` return evidence states, including partial, missing or
corrupt artifact, expired, deleted source and forbidden. Case cursors bind to
the immutable result version. `None` alone permits legacy fallback. A sweep
writes a minimal marker before deleting content; readers enforce expiry even
between sweeps. Uncommitted crash/conflict artifacts are not discoverable.

`Registry::collect_orphans` runs through the same 60-second sweep. Before writing
any artifact, publication records a minimal immutable intent containing its ID
and a collection deadline: one hour, shortened by source/result expiry. Retries
do not renew that deadline. After it passes, collection competes with publication
at the same atomic claim key. If collection wins, it erases content and permanently
reserves the abandoned ID; reads/retries return `expired` (HTTP 410), with no
legacy fallback. Use a new ID for a new publication after abandonment. A delayed
write cannot commit and is collected by a subsequent sweep.

If publication wins, collection keeps its metadata and every referenced shard,
including bytes shared with losing versions, and removes only other artifacts.
An unavailable/corrupt metadata object prevents that pruning; normal retention
still applies. Existing committed receipts remain readable without migration.
Older **unclaimed** prefixes without an intent are preserved: their hashes alone
cannot identify a logical ID or establish whether an older writer is still active.
Retrying the original publication creates its intent; do not remove those prefixes
using an age-only script. Adapter staging files (`.tmp`) are outside the object
collection protocol.

The server accepts one operator-approved short-answer, annotation or governed-conversation bundle. It compares the
manifest pins, verifies all local files and rechecks them on each read. It never
fetches producer URLs. `external` uses the original synthetic fixture;
`curation` additionally resolves the exact dataset version through
`aiwatcher_datasets::Registry::verified_version`. The owner recomputes its content
identity, checks catalogue membership and rejects missing or damaged versions.
Moving the dataset head does not retarget the evaluation.

Curation rows must be exactly `{case_id, input: {question}, expected: {answer}}`,
in the same order as the approved case manifest. Every ID, question and expected
answer must match, including empty answers. The native dataset version and the
case manifest digest are distinct pins. The current Curation owner limits a
version to 1,000 rows and 4 MiB; the larger Evaluation limit does not raise that.
Verified snapshots allow an additional 256 KiB for catalogue metadata.
Curation has no independent expiry policy or per-dataset ACL today. API reads
use the shared instance's Viewer role and publication requires Editor; the
operator's bundle approval is an additional restriction on retained evidence.
Removing a native version/catalogue or approved file yields `deleted_source`;
the next read or 60-second sweep erases dependent evidence permanently. Revoking
approval/configuration hides content as `forbidden` without extending retention.
Restoring damaged bytes can restore a `corrupt_artifact` result before expiry;
restoring a deleted source cannot undo a tombstone.

To use Curation, publish those rows through `/api/v1/datasets`, copy the fixture
bundle to an operator-owned directory, and set **both** dataset references in
its `manifest.json` to `kind: "curation"`, the published name and exact returned
`latest.version`. Keep `case_manifest` pinned to the matching local `cases.json`.
Configure `AIWATCHER_EVALUATION_SOURCE_DIR` to that directory and publish the
manifest and case measurements with the registry SDK. A producer's manifest
alone cannot approve a new native version. The existing seed below still targets
the external fixture; it does not publish or approve Curation data.

Optional `variant.prompt` references now use
`aiwatcher_prompts::Registry::verified_version`. Set the exact prompt name and
64-character lowercase text digest in the operator's approved manifest as well
as the producer's manifest. Both external and Curation datasets support this
pin; a prompt may be the variant's sole prompt/model/workflow reference. Code,
generation configuration and all evaluation context artifacts still need their
own approved pins. The adapter verifies existence and integrity, not whether a
producer actually executed that prompt or deserves model promotion.

Prompt versions are authoritative; their head is a derived, bounded index.
Moving `production`, evicting an old version from the index or losing the head
leaves an exact version readable. Removing the version itself withdraws evidence.
The owner verifies name, version ID, actual text digest and derived variables;
malformed or tampered snapshots produce `corrupt_artifact`. Prompt metadata
(including the `model` hint) is outside the text digest and cannot establish a
model pin. The verified read limits text to the registry's configured maximum
(default 256 KiB), with room for JSON escaping and 256 KiB catalogue metadata.
Existing unverified prompt read and promotion APIs retain their behavior.

Prompts have shared-instance read access and no independent expiry policy.
Evaluation still checks operator approval on every access, and its own retention
limits apply. Prompt text is verified through the owner without copying it into
Evaluation shards. Missing ownership/configuration fails closed.

Optional `variant.model` pins now use Training's `Registry::verified_version`.
Put the exact registered model name/version in both manifests. The owner verifies
the historical version identity (run, checkpoint URI, training dataset, scores
and ordered package artifact digests) without consulting the mutable head or
live run. A model can be evaluated before it has held-out promotion evidence;
this read neither promotes it nor changes existing promotion requirements.

The model must have a valid package. In the operator bundle, save its full
`ModelPackage` as `model-package.json` and place **every** artifact's actual bytes
at `model-artifacts/<artifact.name>`. The adapter compares the complete package
to that approved declaration, then hashes every file and checks declared sizes.
It never fetches artifact/checkpoint URIs or loads model code. Filenames are
confined to that directory. Model records and the approved package are limited
to 1 MiB each, and artifact reads share a 100 MiB budget per verification.
Both external and Curation cases work, with or without an additional prompt.
The existing seed does not register models or create these approval files.

Historical Training IDs do **not** hash runtime, entry point, artifact locations,
shapes or other package metadata. This adapter preserves those IDs and requires
operator approval of the current full package; it does not turn that sidecar
into immutable historical execution evidence. Keep approved bundles controlled
by the operator. A future full-package identity needs an explicit contract
extension. Checking bytes does not prove which model a producer actually ran.

Moving a model label or losing its derived head leaves a pin valid; removing
the version or any approved artifact retires evidence permanently. Byte/identity
corruption hides content as `corrupt_artifact`; an unapproved package change is
`forbidden`. Models share the instance read policy and have no independent
retention; Evaluation's retention and operator approval still apply. Model bytes
are verified locally and are not copied into Evaluation result shards.

For `dataset.kind: "annotations"`, both dataset references name an Annotations
project and exact export digest. `context.split` must be `train`, `validation`
or `test`. The approved case manifest contains **every image in that split**,
in the COCO export order, with this mapping:

```python
cases = [
    {
        "case_id": image["file_name"],
        "input": image,
        "expected": {
            "categories": coco["categories"],
            "annotations": [a for a in coco["annotations"] if a["image_id"] == image["id"]],
        },
    }
    for image in coco["images"]
]
case_manifest = {"schema_version": 1, "cases": cases}
```

Get COCO for the exact export and split through Annotations, then pin the JSON
case file's actual digest/length and count. Copy the schemas, suite and scorer
from `contracts/fixtures/evaluation-annotations-v1`, updating their artifact
pins in the operator and producer manifests. Keep the usual code, generation
configuration and prompt/model/workflow pins. That fixture uses exact equality
of full COCO targets, not a detection mAP scorer. A complete native setup and
publication example is exercised in `aiwatcher-server/tests/evaluation/annotations.rs`.
The existing seed remains specific to external short answers.

Server calls `Annotations::Registry::verified_coco`, which checks the export
identity, schema digest, selected revision digests, shape validity and actual
stored image bytes. It rechecks selected images' current rights and review;
`commercial` and `research` use the owner's existing rights rules, while `any`
is refused. These checks use recorded rights, not a new legal determination.
Only native `aiwatcher-blob:` images are supported; external URLs are not fetched.
The entire input and target must equal the verified COCO result, including
categories, image IDs, geometry, attributes and order. Missing revisions cannot
silently become empty ground truth.

Changing the accepted revision or losing the export index does not retarget a
pin. Changes to current image dimensions or grouping also require re-approval
through a new matching export. Missing project, selected image head, revision, image bytes or export retires
evidence. Rights/review revocation hides content as `forbidden`; damaged content
is `corrupt_artifact`. Annotations does not retain historical schemas separately,
so a legitimate project schema change makes old evidence `forbidden` until the
matching schema is available again. The ordinary COCO API is unchanged.
The verified facade limits an export to 1,000 samples, each image to 16 MiB,
and all read source objects to 100 MiB. Export/project/head JSON has a 4 MiB
limit; revisions retain their 4 MiB identity limit with 256 KiB metadata allowance.
Derived manifest counts are outside its historical digest and do not select cases.
Shared Viewer/Editor roles, operator approval and Evaluation retention apply;
there is no independent Annotations expiry or per-project ACL. Images are checked
in their owner's store, not copied into Evaluation shards; vector expectations
are retained as evidence.

Judges still require an owner adapter and remain refused. The original synthetic and annotation Evaluation artifacts
are plaintext: do not use sensitive or conversation-derived content here.
Governed Conversations uses the separate path below.

| Environment | Default |
| --- | --- |
| `AIWATCHER_EVALUATION_SOURCE_DIR` | Unset: publication refused |
| `AIWATCHER_EVALUATION_MAX_CASES` | 10000 |
| `AIWATCHER_EVALUATION_MAX_BYTES` | 104857600 |
| `AIWATCHER_EVALUATION_PAGE_SIZE` | 200 (maximum 200) |
| `AIWATCHER_EVALUATION_RETENTION_SECONDS` | 2592000 |

The HTTP publication body has an additional fixed 100 MiB ceiling. Storage
uses the existing `AIWATCHER_PROMPT_STORE` adapter under its own `evaluations/`
prefix. Memory is for tests and loses all data on process restart.

## Approvals

`AIWATCHER_EVALUATION_SOURCE_DIR` is a directory **of** approvals: one
subdirectory per admitted pair, named by its `approval_id`, plus — for an
instance that has one — a single bundle directly under the root, which stays
readable. Many pairs therefore coexist, which is what a baseline against a
candidate needs and what one bundle could not give: swapping it used to hide
every result already published under the previous one.

```sh
cargo run -q -p aiwatcher-evaluation --example prepare -- manifest.json
# prints variant_id, context_id and the approval_id its directory is named after
scripts/stage-evaluation-approval.py ./approvals ./my-bundle
```

Admitting one is `POST /api/v1/evaluation-approvals` with the manifest, and it
is **admin**: an ingest token is an editor by construction, so a producer that
could admit its own evidence would not have been approved by anybody. The
record is written only after the adapter resolves the declaration, and carries
who admitted it, when, and a `bundle_digest` covering what the adapter verified
beyond the manifest's own pinned digests — a model package, whose historical ID
binds artifacts rather than the whole declaration. A bundle that changed
underneath an admitted pair conflicts rather than moving what earlier results
were measured against.

`DELETE /api/v1/evaluation-approvals/{id}` withdraws one. Every result measured
under that pair stops being readable, no new one can be published, and nothing
already published has its retention moved in either direction. It is final for
that approval ID. `GET` lists them, withdrawn ones included: an approval that
vanished from the list would read as one nobody made.

Publication needs an admitted pair; a **read** needs only that no withdrawal
exists, so evidence published before an instance kept approvals stays readable.
`DELETE /api/v1/evaluation-results/{id}` (admin) forgets one measurement rather
than every measurement of its pair.

A *new* pair needs its bytes somewhere the adapter reads, and that is no longer
a host's disk: `PUT /api/v1/evaluation-approvals/{id}/bundle/{name}` (admin)
stages one member into the adapter's own prefix, `GET` lists what is staged and
`DELETE` clears it. Staging admits nothing — the approval that follows resolves
the bundle and pins its digest, so bytes arriving afterwards stop the pair
reading rather than widening it. Staged bytes take precedence over a configured
directory, and an instance with no directory can still admit a pair.

## What a read costs

A summary answers from its metadata object; a shard is verified when the page it
is on is read, and a damaged shard is that page's `EvidenceState` rather than an
error. A source is resolved once per admitted pair for the length of one `list`
or `sweep`. Retention asks the receipt's own deadline first, every minute;
collection lists a prefix per result and runs hourly. Measured at the starting
limits, in object-store requests: a 10 000-case summary is 5 gets (was 105), a
200-case page 7 (was 108), a 50-row catalogue page 103 (was 400, then 152), and
one sweep 103 with one list (was 550 with 53). A catalogue row costs the index
entry and the header behind it: `evaluations/index/` is one key per published
result in published order, so a page reads neither the claim nor the tombstone
and a period is a bound on the key. See
`crates/aiwatcher-server/tests/evaluation/cost.rs` and `catalogue.rs`.

Every pass writes `evaluations/retention.json`, returned as `retention` on
`GET /api/v1/evaluation-results`: when it ran, what it retired and collected,
and how many consecutive passes have failed.

It also carries `damaged` — the results the last collection pass found short of
the objects their header names. That is the one thing a summary cannot see, and
the pass already compares the two lists in order to delete what is in neither,
so it costs no extra request. The IDs are bounded and `damaged_count` is all of
them; the minute-by-minute passes carry the last collection's finding rather
than blanking it, and `collected_at` says how old it is.

To try the pinned fixture on a dedicated local instance:

```sh
just run-evaluation          # stages the fixture as its own approval, then serves
just approve-evaluation      # admits it; every later publication needs no host step
rtk proxy uv run --project sdk/python python scripts/seed-evaluation-registry.py --base-url http://127.0.0.1:8080 --evaluation-id fti-durable-1 --repetition-id measurement-1
```

The seed requires an explicit destination and ID, calls the actual fixture
response function and exact scorer, and prints a receipt. The example is a
regression contract test, not held-out promotion evidence.

HTTP clients use `/api/v1/evaluation-results` (POST/list), `/{id}` (detail),
and `/{id}/cases?version=…&cursor=…`. Python:
`aiwatcher_sdk.evaluation_registry.EvaluationRegistry`; TypeScript:
`@aiwatcher/sdk/evaluation-registry`. Both raise on transport/refusal errors;
read-state responses remain explicit data. Python uses the existing registry
transport and its bounded retries; TypeScript leaves retry to the caller.
Manifest-only imports and the old `record_evaluation` are unchanged.

Legacy detail URLs prefer registry results and show a bounded first page.
Legacy list/suite/automatic-baseline queries exclude registry-owned IDs, while
the new durable list remains available after projection loss. This avoids using
old telemetry to reintroduce revoked evidence. Durable comparison and the panel's
full result browser follow in B3; explicit durable baselines are refused for now.


## Governed Conversations

Enable the existing archive (`AIWATCHER_CONVERSATION_ARCHIVE` and persistent
`AIWATCHER_CONVERSATION_KEYS`) and configure the approved Evaluation source
directory. Both API and adapter use the same archive. A default-disabled archive
cannot publish governed evidence. Publishing and reading Conversation evidence
requires **Admin**, the archive's content-reader role; ingest/Editor credentials
cannot read expectations indirectly by publishing an evaluation. Other sources
keep their existing Viewer/Editor policy. Auth mode `none` retains its established
meaning: the instance deliberately disables role enforcement.

Create a native export with `format: "prompt_response"`,
`required_scope: "evaluate"`, and `require_human_review: true`. Every included
user and assistant turn must have explicit evaluation consent and current
approval. The owner revalidates the request/version, ordered shard digests, all
rows, both turns' actual content and present policy. It does not rely on the
export index or fetch producer URLs. `train` consent never implies `evaluate`.
The current slice covers the **entire export**, at most 1,000 rows, with the
operator-declared split `test`. Chat/SFT/DPO and arbitrary subcohorts are refused.
A test label does not prove independence from training.

Both dataset references must use `kind: "conversations"` and the exact native
export name/version. The approved case manifest contains **digests, not words**:

```json
{
  "schema_version": 1,
  "cases": [{
    "case_id": "<assistant turn_id from row.eligibility[1]>",
    "input_digest": "<sha256 of compact UTF-8 JSON {question: row.prompt}>",
    "expected_digest": "<sha256 of compact UTF-8 JSON {answer: row.response}>"
  }]
}
```

Keep the native row order. Compute the one-key object hashes using compact JSON,
unescaped UTF-8 (Python `json.dumps(value, ensure_ascii=False, separators=(",", ":"))`)
and SHA-256. Pin the actual bytes of this case file in `context.case_manifest`.
Use the ordinary `evaluation-v1` question/answer schemas and exact string scorer;
pin every other code/config/suite/scorer/workflow artifact as for the other
adapters. The report's `case_id` is the assistant turn ID, and `actual` is an
`{"answer": "..."}` object. Expectations are resolved from the archive.
No plaintext export file is needed in the approved directory. The reproducible
synthetic setup is documented in `contracts/fixtures/evaluation-conversations-v1`.
Do not route these words through telemetry, a Curation copy or the existing seed.

`EvidenceCipher` is an Evaluation port, implemented in Server by the existing
Conversations Keyring (AES-256-GCM/HKDF and authenticated object path). All
Conversation metadata, actual responses and expected responses are sealed before
storage. Only minimal receipts/intents/tombstones stay plaintext; use opaque IDs
and metadata, never conversation text in identifiers. The random seal does not
change logical hashes, versions or retry identity. A downgrade of metadata or
shards to plaintext is corrupt evidence. Existing non-conversation results keep
their format. Evaluation's 100 MiB result budget counts logical plaintext; stored
envelopes additionally incur base64 overhead.

`with_content_access` is a trusted in-process capability on a per-request clone,
not a user-supplied subject convention. It defaults to false; API supplies the
Admin check, and the retention worker explicitly has access. Viewer/Editor
receive minimal `forbidden` states without manifests, metrics or cases through
list/detail/pages; the legacy detail returns 403 without telemetry fallback.
An erased/expired result returns the established unavailable states (legacy 410).

The first receipt caps retention. Every read/publication also checks the minimum
of source deadlines and the archive's **current** TTL cap. Revocation hides data
without renewing its lifetime; source deletion or expiry creates a tombstone
before removing encrypted copies. The 60-second worker enforces this without an
Admin reading the result. A missing key returns `forbidden`; bad ciphertext,
wrong path or mismatched source bytes return `corrupt_artifact`, never false
source deletion. When no key can open metadata, source linkage cannot be checked;
the receipt deadline still allows erasure. Keep old keys during rotation until
retained results expire; restored keys do not undo a tombstone.

Owner verification permits 100 MiB summed source reads (ciphertext and plaintext),
4 MiB manifests and 2 MiB turn heads/content envelopes. The object-store port
returns whole objects before the bound is checked. Export consent and review
are recorded assertions checked by the existing owner, not independently verified
legal permission or proof that the producer executed its declared model.
