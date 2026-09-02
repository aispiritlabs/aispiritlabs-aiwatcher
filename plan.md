# Plan for the remaining production path

The repository has a runnable local path for both data sources, a governed
archive for conversation content, and a real registry-to-inference handoff.

**Workstream 1 is delivered** — see
[ADR_0021](docs/ADR/ADR_0021_CONVERSATION_ARCHIVE.md) and
`crates/aiwatcher-conversations`.

**Workstream 2 is delivered** — see
[ADR_0022](docs/ADR/ADR_0022_STAGED_IMPORT_JOBS.md), `crates/aiwatcher-jobs`,
`aiwatcher-annotations::imports` and `aiwatcher-annotations::integrations::fetch`.
`just seed-import` walks it.

**Workstream 3 is delivered as far as a manifest and one hardened profile** —
see [ADR_0023](docs/ADR/ADR_0023_MODEL_PACKAGE.md),
`aiwatcher-training::package` and `scripts/serve-mini-model.py`. What remains is
loaders for frameworks this repository does not ship a model for, and the
signed readers that would fetch their artifacts from somewhere other than a
local file. Both are named below with what they need.

Each section is kept as the record of what was asked for, with what satisfies
each item and which test asserts it.

## Current capability matrix

| Stage | Works now | Deliberate limit |
|---|---|---|
| Track agent work | Conversation, run, agent, LLM/tool spans, raw events, live resume | Prompt/completion bodies never ride the event log; the words are a separate, encrypted artifact |
| Retain conversation content | Off by default; when on, an encrypted archive with consent, its own retention clock, producer-side redaction, a server-side scan and a human review gate | The scanner recognises credential and identifier *shapes*; unsafe output is a human judgement and is never automated |
| Export conversation examples | An asynchronous, leased, resumable job producing `name@sha256`, with every excluded turn counted by reason. An erasure reaches the published corpora too | A lease is renewed per shard, so a single shard slower than five minutes costs duplicated work — never a corrupted corpus |
| Curate data | Flow validate → simulate → execute → immutable dataset | Bounded interactive artifacts, not batch-scale ETL |
| Discover Hub data | Hugging Face search/schema/rows; Kaggle when configured | Preview/import reads are bounded; mirror licence is never trusted |
| Map and store images | Editable Flow mapping, dry-run, source/rights provenance, annotation registry | Flow's own execution is still one pass rather than a paged stream |
| Import a corpus | A staged batch of digested JSONL pages, sealed into a content address, read by a leased resumable job with counts and paged dead letters | JSONL, not Parquet: every hub pipeline already produces it and a writer is a dependency |
| Fetch remote images | Allowlisted hosts, a public-address check on every resolved address, no redirects, a streamed byte ceiling, a header-only pixel ceiling, a verified content address | The window between the address check and the connection is closed by the allowlist rather than by a connection-time hook |
| Label and export | Schema-driven canvas, review, family split, exclusions, COCO, immutable reference | No distributed labelling queue |
| Train and register | Generic Python tracker plus executable mini trainer, immutable dataset gate, held-out promotion gate, and a model package naming every artifact by digest | The model-owning repository still supplies the trainer |
| Serve | Resolves `production`, verifies the package's digests, warms, bounds, validates, watches the label, rolls forward in two phases and back in one, and reports every inference with no content | One runtime (`weights`) and `file://`; ONNX, TorchScript and signed readers are what a second profile adds |

## Workstream 1: governed conversation training data — **delivered**

Landed as `crates/aiwatcher-conversations` (ADR_0021), the
`/api/v1/conversation-*` routes, the panel's Conversations area, and
`aiwatcher_sdk.conversations`. `just run-conversations` turns it on and
`just seed-conversations` walks the whole path.

### Goal

Make training-content capture a first-class, auditable choice without turning
observability into an accidental prompt archive.

### Deliverables

1. **A separate contract, not span attributes.** ✅ `turn.rs`: roles
   (`system|developer|user|assistant|tool`), producer message ids,
   `parent_message_id` and `ordinal`, content parts (text, reasoning, tool call,
   redacted, reference), tool results, and `ContentPolicy` carrying consent and
   retention. It is **not** an event: `conversation.turn` is not in the catalog,
   and the archive names the log rather than riding it.
2. **Producer-side redaction and server-side policy validation.** ✅
   `aiwatcher_sdk.conversations` runs a `Redactor` in the producer's process —
   there is no default, so `NullRedactor` is how somebody says they meant to send
   content verbatim. `ArchivePolicy::check` refuses a protected deployment's
   write with every missing field at once
   (`a_protected_deployment_refuses_a_turn_that_claims_no_basis`,
   `a_turn_with_no_consent_record_is_refused_with_every_problem_at_once`).
3. **An encrypted archive with independent retention, access control and
   tombstones.** ✅ AES-256-GCM under an HKDF-derived per-object key with the
   object path as associated data (`archive/crypt.rs`); a plaintext head beside
   a sealed body; `ttl_days` on its own clock; erasure by *subject*; `admin` to
   read content or erase, `editor` to write
   (`nothing_in_the_bucket_holds_the_words`,
   `a_viewer_sees_everything_about_a_turn_except_what_it_says`,
   `an_ingest_token_can_record_a_turn_and_never_read_one_back`).
   The telemetry log is unaffected because it never held the bodies.
4. **A review gate for PII, secrets, unsafe output, duplicates and approval.** ✅
   A conservative shape-matching scanner (`redaction.rs`) plus a duplicate check
   on the content digest, rendered in **Conversations → Review**. Approval and
   *preference* are separate axes, so a turn rejected for holding an address
   cannot become the rejected half of a preference pair
   (`a_preference_export_pairs_only_what_a_reviewer_labelled`).
   No unsafe-output classifier ships — see the residual below.
5. **An asynchronous export job.** ✅ Manifest, stable ordering, counts by
   exclusion reason, a lease, resumability, and a version that is
   `sha256(request ‖ every shard digest)`
   (`a_resumed_export_neither_duplicates_nor_omits_a_row`,
   `an_export_takes_the_approved_turns_and_names_everything_it_left_out`).
6. **Task-specific shapes and SFT/DPO adapters, no training library.** ✅
   `export/format.rs` emits `chat`, `prompt_response`, `sft` and `dpo` as JSONL
   over plain fields. Nothing imports a tokeniser or a trainer.

### Acceptance criteria

- **default deployments retain no conversation content** — ✅
  `AIWATCHER_CONVERSATION_ARCHIVE` is off and every route answers 501 naming it
  (`every_conversation_route_answers_501_when_this_instance_keeps_no_archive`,
  `the_conversation_archive_is_off_until_somebody_turns_it_on`). The server
  refuses to start with the archive on and no key.
- **an authorised operator can prove why every exported row was eligible** — ✅
  each row carries consent basis, subject, reference, scope, retention, the
  redactor that ran, the reviewer and the moment they decided
  (`every_exported_row_carries_the_reason_it_was_eligible`).
- **deletion propagates according to the declared retention policy** — ✅ the
  sweep and an erasure request both remove content *and* withdraw every
  published corpus that read it
  (`the_sweep_removes_what_its_own_retention_ran_out_on`,
  `an_erasure_takes_the_rows_out_of_the_corpus_it_already_reached`,
  `the_retention_sweep_withdraws_a_corpus_the_same_way_a_request_does`).
  Withdrawal was not in the brief; stopping at the archive would have left the
  words readable under a reference a training run had already recorded.
- **retries and resumed exports do not duplicate or omit rows** — ✅ shard
  before cursor, plus a per-shard lease
  (`a_resumed_export_neither_duplicates_nor_omits_a_row`,
  `a_worker_that_lost_its_lease_stops_instead_of_writing_beside_its_replacement`).
- **the resulting reference is immutable and reconstructible** — ✅
  (`re_running_an_export_over_an_unchanged_archive_reaches_the_same_version`).
  Note the shape of it: the version is a content address of the *corpus*,
  including review metadata, so "the same request" does not pin a corpus —
  `name@sha256` does.

### What is still open here

Small, and none of it blocks Workstream 2.

- **The archive has not been run against a real S3 endpoint.** It writes through
  the same `ObjectStore` port the prompt registry uses, and `just test-rustfs`
  covers that contract including multi-page listing, so nothing specific is
  suspected — but nobody has put these code paths over a network. One run
  against `just rustfs-up` closes it.
- **No unsafe-output classifier, deliberately.** `FindingKind::Unsafe` exists so
  a human can record one. A keyword list would produce a green tick nobody
  should trust, and the review gate exists precisely because this judgement is
  not automatable. If it ever ships, it must be a *finding* a reviewer confirms,
  never an exclusion applied silently.
- **Review is per turn.** That is the right grain at demo size and probably the
  wrong one at corpus size: if people start approving in bulk without reading,
  the fix is per-conversation review with sampling, not a faster button.
- **The lease is renewed per shard, not inside one.** A single shard slower than
  five minutes costs duplicated work, never corruption.
- **`AIWATCHER_PROMPT_STORE` now gates five registries.** ADR_0014 called the
  name historical and ADR_0018 said four was enough to want a rename. This is a
  rename with a deprecation window, not a decision.

## Workstream 2: scalable Hub ingestion and dataset artifacts — **delivered**

Landed as `crates/aiwatcher-jobs` (the shared primitive),
`aiwatcher-annotations::imports` (the staged batch and the queued job),
`aiwatcher-annotations::integrations::fetch` (the bounded downloader), the
`/api/v1/annotation-import-*` routes, the panel's **Annotations → Imports**
view, and the import worker in `aiwatcher-server::imports`. ADR_0022 records
the decision. `just seed-import` walks the whole path.

### Goal

Keep the current inspect/map/dry-run experience while moving large reads and
writes out of synchronous HTTP requests.

### The decision this forced

Answered before the second job was written, which is what the plan asked for:
**extract the rules, not the records.** `aiwatcher-jobs` holds `JobState`,
`ShardRef`, `lease_expired`, `after_failure`, `progress`, `version_of` and
`ORDERING` — the shard-before-cursor sentence, written down so a `flush` doing
it backwards has something to be wrong against. The records stay per caller,
because an export counts exclusions by policy reason and an import counts
rejected rows by what was wrong with them, and a generic `Job<Payload>` would
buy that saving with an unnarrowable client type or a trait full of accessors.

The conversation export was retrofitted onto it rather than left as the
original copy, and its 82 tests pass unchanged — which is how the extraction is
known to be an extraction. It also gained one behaviour from the exercise:
counts and exclusions are now held apart from the job until the shard they
describe is stored, so a resumed export no longer double-counts the
conversations it re-reads.

### Deliverables

1. **A staged dataset artifact over the ObjectStore port.** ✅
   `imports::staging`: pages of JSONL written to the store, each hashed, the
   batch manifest updated *after* the page it names, sealed into
   `sha256(request ‖ every page digest)`. A numbered append is idempotent —
   identical bytes are an acknowledged retry, different bytes for a stored page
   are a refusal naming it (`re_sending_a_page_is_a_retry_and_changing_one_is_a_refusal`).
   JSONL only; Parquet is a writer dependency and a schema decision, and
   nothing in the acceptance criteria asks for one.
2. **A queued import job with cursor, retry budget, cancellation, progress and
   a dead-letter report.** ✅ `imports::run`, `ImportJob`, `RejectReason`,
   `RejectedRow`, and `GET /api/v1/annotation-import-rejects` paged. The counts
   are complete and the rows are a capped sample, and the manifest says which
   is which.
3. **A bounded image fetcher.** ✅ `integrations::fetch`: seven gates, in order,
   each of them a mistake somebody has already made — https with the host
   parsed rather than matched, an allowlist, a public-address check on every
   resolved address, no redirects, a byte ceiling applied while streaming, a
   header-only pixel ceiling that is also the decompression-bomb gate, and a
   verified content address. `Hubs` implements the `ImageSource` port over it
   and **both** import routes use that port
   (`the_cloud_metadata_service_is_not_a_public_address`,
   `an_allowlisted_name_in_the_userinfo_is_not_the_host`,
   `a_picture_claiming_more_pixels_than_it_could_hold_is_refused`,
   `no_row_may_send_this_process_at_an_address_nobody_allowlisted`).
4. **Hub revision, config, split and file digests persisted.** ✅
   `ImportSource::revision | config | split | files`, recorded on the batch and
   written onto every image's metadata — because "which commit was this read
   at" is asked about one picture far more often than about a batch. A batch
   with a dataset id and no revision comes back with a warning on a response
   that succeeded.
5. **Rights as an independent human assertion, with evidence.** ✅
   `RightsEvidence` — the primary source, who read it, when, and a note — named
   for the *original* rather than the mirror, because ADR_0019's whole finding
   is that a hub card is evidence about the mirror. Recorded rather than
   enforced: refusing an import with no evidence teaches people to invent one,
   and the one hard refusal stays where a human already read the licence
   (`check_rights` against the curated table).
6. **Chunked publication with deterministic order and one final digest.** ✅
   for the publication half — the page order is the row order, the shards are
   the version material, and the version is a content address of the batch. The
   *Flow execution* half is not: `services/flow` still runs a pipeline in one
   pass. It is outside the Cargo workspace and outside `just check`, and a
   paged executor there is its own change with its own gate.

### Acceptance criteria

- **a million-row import can resume after process restart** — ✅ pages are the
  unit of resume, the cursor is durable, and the test kills the store between a
  page's registration and its shard
  (`an_import_killed_between_a_page_and_its_cursor_resumes_without_duplicating_a_row`).
  Five thousand rows per page and a thousand pages is five million.
- **progress and rejected-row reasons are observable without reading the whole
  artifact** — ✅ counts and reasons on the job, rows in their own shards, read
  through a paged route
  (`progress_and_rejected_rows_are_readable_without_opening_the_artifact`).
- **re-running the same pinned source and pipeline yields the same version** —
  ✅ (`re_running_the_same_pinned_batch_reaches_the_same_version`). The version
  is built from the batch's *content*, never its id, which is what makes that
  true for two people rather than only for one.
- **no private-network URL or oversized/decompression-bomb image is fetched** —
  ✅ gates 1–6 above, with the redirect gate the one worth naming: an
  allowlisted host answering `302 → http://169.254.169.254/` would otherwise
  walk past every check that ran against the address the caller named.
- **an interrupted job never appears as a completed dataset version** — ✅ no
  manifest, no index entry
  (`an_interrupted_import_never_appears_as_a_completed_one`).

### What is still open here

- **Flow's own execution is one pass.** The publication side is paged; the
  query side is not. It lives in `services/flow`, which `just check` does not
  cover, and paging it is a change to the PHP service rather than to this
  workspace.
- **Parquet.** Deliberate, and reversible: a `ParquetPage` beside the JSONL one
  is a writer dependency away, and the reason to add it is a reader that wants
  columnar access rather than a reason of principle.
- **The family warning is about pages, not the batch.** "Every page of this
  batch gave each of its rows its own family" is exact about what it measured
  and catches the mistake it exists for, because a `group_id` mapped from a
  filename is singleton on every page. What it cannot say is "this batch has N
  families", which would need a set of every group id a million-row import has
  seen, held in a manifest.
- **DNS rebinding.** The address check runs at resolution and the connection
  happens after it. The gate that holds is the allowlist; the moment somebody
  adds a customer's own mirror to it, a connection-time hook has to be built.
- **The lease is renewed per page.** Same limit the export has, same
  consequence: a page slower than five minutes costs duplicated work, never
  corruption.

## Workstream 3: general production model serving — **the manifest and one profile**

Landed as `aiwatcher-training::package` (ADR_0023), the `package` field on a
model version and its registration, and `scripts/serve-mini-model.py` — one
hardened runtime profile, end to end. `just e2e-train && just serve-mini-model`
walks it.

### Goal

Turn the model registry's label decision into a safe, observable deployment for
real checkpoint formats and storage backends.

### Deliverables

1. **A model package manifest.** ✅ `ModelPackage`: runtime and runtime
   version, entry point, `TensorSpec` inputs and outputs (with `classes`, so a
   label order cannot silently permute when somebody retrains), preprocessing,
   dependencies, artifacts each with a `sha256`, and resource requirements.
   Every artifact needs a digest and a package with none is refused
   (`an_artifact_with_no_digest_is_refused_because_an_address_is_not_an_identity`);
   the package's digest joins the version's identity, so two registrations
   naming different weights are two versions
   (`two_versions_that_name_different_weights_are_two_versions`).
2. **Signed readers for the configured object store and approved Hub
   repositories.** ⛔ Not built. What *is* built is the half that would
   otherwise be skipped: the digest is verified before the bytes are loaded,
   and a mismatch is a refusal naming both hashes. The profile reads `file://`
   only and says so. A reader for S3 or a Hub is credentials and a fetch, and
   it plugs in where `read_artifact` is — see the note below on what it needs.
3. **Isolated loaders for selected runtimes.** ⛔ One runtime is implemented —
   `weights`, the JSON vector this repository's own trainer produces — and
   every other is refused *by name* rather than attempted, which is the rule
   rather than a stand-in. `Runtime::executes_packaged_code` is the question a
   host answers before it opens anything, and `python` answers yes: such a
   package is never to be loaded in the API process, which holds the object
   store's credentials.
4. **Readiness, warmup, concurrency/batch limits, request validation,
   authentication and resource ceilings.** ✅ `/readyz` is 503 until a version
   is loaded *and* warmed; a semaphore bounds work in flight and returns a
   `Retry-After` rather than growing a queue; the body, the batch and every row
   are validated against the shape the *loaded* version declares; an optional
   bearer token guards `/v1/*` with the probes left public, the same exception
   list `auth::is_public` keeps.
5. **Label watch and a two-phase rollout, with rollback.** ✅ The watcher
   downloads, verifies and warms the candidate while the current version keeps
   serving, and swaps only if all three succeed; a version that failed is not
   retried under the same digest; the previous version stays loaded, so
   `POST /v1/rollback` needs no rebuild and no fetch, and the version being
   left is pinned out so the next poll does not undo the rollback.
6. **Inference events with the same content-redaction default as agent
   telemetry.** ✅ `run.started → llm.started → llm.completed` carrying model,
   version, label, rows, latency and outcome. A served model is a model, so an
   inference joins the same traces, the same model dimension and the same
   "which version served this" question as everything else — and it carries no
   inputs and no outputs, which is ADR_0021's rule restated for serving.
7. **Canary/shadow routing and automatic rollback gates.** ⛔ Not built, and it
   is the deliverable that most wants a real second profile first: a rollback
   gate needs a health signal that is more than "it loaded", and what that
   signal *is* differs per runtime.

### Acceptance criteria

- **a service loads exactly the digest named by `production` and exposes it in
  health/model metadata** — ✅ `/v1/model` reports the version, the package
  digest, the runtime and whether the artifacts were verified. A version with
  no package is loaded and reported `verified: false` rather than reported as
  verified.
- **a broken new label never removes the ready old version** — ✅ the two-phase
  rollout, verified by hand against a tampered checkpoint: the server kept
  serving and put the reason on `/v1/model`.
- **rollback does not require rebuilding an image** — ✅ the previous version is
  already loaded and warm, which is why it is kept.
- **untrusted model artifacts cannot execute in the aiwatcher API process** —
  ✅ by construction: the API never loads a model at all, and
  `Runtime::executes_packaged_code` is what a host that might would check
  first.
- **every inference run can be joined back to model version, training run and
  immutable dataset/export** — ✅ the event carries `model_version`, the version
  names its run, and the run names the export.

### What is still open here, and what each piece needs

- **A signed reader.** Credentials, a bounded fetch and a cache keyed by the
  immutable version. `aiwatcher-prompts`'s SigV4 signer is the S3 half already
  written; what a serving process needs beyond it is a local cache directory
  and an eviction policy, because re-downloading a checkpoint per pod restart
  is the cost this hides.
- **An ONNX or TorchScript loader.** A dependency and a subprocess boundary. The
  manifest is what lets one be added without renegotiating anything, which was
  the point of doing it first.
- **Canary and shadow routing.** Wants a health signal per runtime and a
  decision about where the split lives — in this process, or in the ingress in
  front of it. Worth deciding after the second profile exists, because one
  profile cannot tell you which of the two is general.
- **The profile is single-process.** A semaphore bounds work in flight in *this*
  pod; nothing bounds a fleet. That is a scheduler's job, and the
  `ResourceRequest` on the package is what a scheduler would read.

### One thing Workstream 1 settled in advance

Deliverable 6 says inference events carry "the same content-redaction default as
agent telemetry". That default is now unambiguous: **inference inputs and
outputs do not go on the event log.** A serving runtime that wants to retain
them writes turns to the conversation archive, with consent and a retention
clock, exactly as an agent does. The alternative — a second content path with
its own rules — is the convention ADR_0021 removed.

## Recommended sequence

1. ~~Land the governed conversation schema/archive first; otherwise downstream
   LLM training would institutionalise an unsafe raw-event convention.~~
   **Delivered** — ADR_0021.
2. ~~Decide whether the export job/shard machinery becomes a shared primitive
   before writing the import job, not after. Then introduce staged artifacts and
   job state for Hub ingestion.~~ **Delivered** — ADR_0022. It became
   `aiwatcher-jobs`, holding the rules and not the records, and the export was
   retrofitted onto it rather than left as the original copy.
3. ~~Do the bounded image fetcher as its own piece, and treat it as the security
   work it is rather than as part of the import job.~~ **Delivered** —
   `integrations::fetch`, seven gates, one `ImageSource` port both import routes
   go through.
4. ~~Define the model package manifest before implementing loaders.~~
   **Delivered** — ADR_0023.
5. **Ship the second runtime profile.** ONNX first: a fixed operator set, no
   packaged code, and the loader that most deployments actually want. It needs
   the signed reader (item 6) to be useful outside a single machine, and it is
   what will show whether `entry_point` and `preprocessing` as free text are
   enough to act on.
6. **Then the signed reader and the version cache.** S3 with the SigV4 signer
   that already exists, plus a local cache keyed by the immutable version and an
   eviction policy — a checkpoint re-downloaded per pod restart is the cost this
   hides.
7. **Then canary and shadow routing**, once two profiles exist to tell you which
   half of the rollout decision is general and which is per runtime.
8. Add experiment comparison after `variant` is a query dimension and its
   traces, evaluation and model version can be joined without heuristics.
