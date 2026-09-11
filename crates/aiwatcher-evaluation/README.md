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

The server accepts one operator-approved short-answer bundle. It compares the
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

Annotations, Conversations and judges still require
owner adapters and remain refused. The current bundle and Evaluation artifacts
are plaintext: do not use sensitive or conversation-derived content here.
Native Conversations requires its own encryption, erasure and role handling.

| Environment | Default |
| --- | --- |
| `AIWATCHER_EVALUATION_SOURCE_DIR` | Unset: publication refused |
| `AIWATCHER_EVALUATION_MAX_CASES` | 10000 |
| `AIWATCHER_EVALUATION_MAX_BYTES` | 104857600 |
| `AIWATCHER_EVALUATION_PAGE_SIZE` | 200 (maximum 200) |
| `AIWATCHER_EVALUATION_RETENTION_SECONDS` | 2592000 |

The HTTP publication body has an additional fixed 100 MiB ceiling. Storage
uses the existing `AIWATCHER_PROMPT_STORE` adapter under its own `evaluations/`
prefix. Memory is for tests and loses all data on process restart. Discovery
currently walks object keys; measure that cost before increasing scale.

To try the pinned fixture on a dedicated local instance:

```sh
rtk proxy env AIWATCHER_LISTEN=127.0.0.1:19080 AIWATCHER_DATA_DIR=/tmp/fti-demo AIWATCHER_EVALUATION_SOURCE_DIR="$PWD/contracts/fixtures/evaluation-v1" cargo run --bin aiwatcher
rtk proxy uv run --project sdk/python python scripts/seed-evaluation-registry.py --base-url http://127.0.0.1:19080 --evaluation-id fti-durable-1 --repetition-id measurement-1
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
