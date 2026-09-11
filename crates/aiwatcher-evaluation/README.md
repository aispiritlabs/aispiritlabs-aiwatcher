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
between sweeps. Uncommitted crash/conflict artifacts are not discoverable;
orphan garbage collection is not implemented yet.

The first server adapter accepts one operator-approved **synthetic** short-answer
bundle. It compares the manifest pins, verifies the files and rechecks them on
each read. Removing its directory/files revokes dependent evidence and the
60-second sweep erases copies. It never fetches producer URLs. Native datasets,
annotations, conversations, model/prompt references and judges require dedicated
owner adapters; this implementation refuses them. Do not place sensitive or
conversation-derived data in the synthetic bundle.

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
