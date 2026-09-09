# Plan for the remaining production path

Three workstreams, all delivered far enough that this file is now mostly a
record. What each one *is* has an ADR and a crate; what each one **left open**
is below, kept in full, because that is the part somebody picking this up needs.

| Workstream | Delivered | Where it lives |
|---|---|---|
| 1 — governed conversation training data | An encrypted archive with consent, its own retention clock, a human review gate, and a resumable export that freezes a corpus | [ADR_0021](docs/ADR/ADR_0021_CONVERSATION_ARCHIVE.md), `crates/aiwatcher-conversations`; `just seed-conversations` |
| 2 — scalable Hub ingestion and dataset artifacts | One shared job primitive, a corpus staged as digested pages and sealed into a content address, and the bounded fetcher every outbound byte goes through | [ADR_0022](docs/ADR/ADR_0022_STAGED_IMPORT_JOBS.md), `crates/aiwatcher-jobs`, `aiwatcher-annotations::{imports,integrations::fetch}`; `just seed-import` |
| 3 — general production model serving | A declared package naming every artifact by digest, the hardened serving half, and **two** runtime profiles — which is what made a third cheap | [ADR_0023](docs/ADR/ADR_0023_MODEL_PACKAGE.md), `aiwatcher-training::package`, `aiwatcher_sdk.serving`; `just e2e-train`, `just serve-model`, `just onnx-version` |

Workstream 3 is the one still short of its own goal: a loader for a framework
whose artifact is a *program* rather than a graph, an approved-Hub reader beside
the delivered signed S3 one, and canary routing. Each is named below with what
it needs.

Two rules these workstreams settled are now guardrails in `CLAUDE.md` rather
than plan text: **inference inputs and outputs do not go on the event log** — a
runtime that wants to retain them writes turns to the conversation archive, as
an agent does — and **where an artifact describes itself, the package's
declaration is cross-checked against it rather than trusted**.


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
| Serve | Resolves `production`, reads `file://` or signed `s3://` through a verified version cache, verifies every package artifact, warms, bounds, validates, watches the label, rolls forward in two phases and back in one, optionally mirrors into an independently loaded shadow label, and reports every inference with no content | Two runtimes (`weights`, `onnx`); TorchScript, an isolated `python` subprocess, an approved-Hub reader and canary/rollback gates remain |
| Cross-check a package | Where an artifact describes itself, the declaration is compared against it: an ONNX graph's input and output names, element types, shapes and head width against the package's, and a disagreement is a refusal naming both | A graph that carries no shapes — a TorchScript module traced without them — has nothing to compare against, and its declaration goes back to being trusted |

## Workstream 1 — what is still open

Small, and none of it blocked what came after.

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

## Workstream 2 — what is still open

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

## Workstream 3 — what is still open, and what each piece needs

- **An approved-Hub reader.** S3 is delivered: credentials never enter an
  artifact URI, every GET is SigV4-signed and bounded, redirects are refused,
  and a persistent version/digest cache has atomic writes, hit verification
  and an LRU byte budget. A Hub reader still needs an explicit repository
  allowlist, a pinned revision in the URI, token handling and the same streamed
  ceiling behind `ArtifactReader`; the S3 cache can wrap it unchanged.
- **A TorchScript loader, and a `python` one.** These are not the same kind of
  work any more, which the second profile is what showed. TorchScript is a
  dependency and a session; `python` is a *program*, so it is a subprocess with
  its own credentials — the selection already refuses it, and what has to be
  built is the host that can honestly answer `isolates_packaged_code`.
  TorchScript also has the one property that would weaken the cross-check
  above: a module traced without shapes has nothing to compare against.
- **The ONNX gates are tested against a stub session.** They are pure functions
  of what a session says about itself, and a hundred megabytes of runtime in
  every CI run would test onnxruntime rather than this code. A real graph goes
  through `just onnx-version`, which refuses unless the graph and the vector it
  re-expresses agree row by row. What that leaves untested in CI is the wheel's
  own API surface — the one thing a stub cannot check is that the stub is
  shaped right.
- **Canary routing and automatic rollback.** Shadowing is delivered in this
  process: its label has an independently verified/warmed model, mirrored work
  cannot queue, answers are discarded, telemetry names `traffic=shadow`, and
  `/v1/model` exposes a per-version runtime-error/latency window. The remaining
  gate must choose a minimum sample, error and latency thresholds, a traffic
  percentage and a cooldown, and decide whether fleet-wide routing belongs in
  the ingress. There is still no quality signal: runtime health can say "it
  broke", never "its predictions got worse".
- **The profile is single-process.** A semaphore bounds work in flight in *this*
  pod; nothing bounds a fleet. That is a scheduler's job, and the
  `ResourceRequest` on the package is what a scheduler would read.

## What is left, in order

1–6 are delivered: the archive, the shared job primitive, the bounded fetcher,
the package manifest, the second runtime profile, and the signed reader with its
verified version cache. What remains:

7. **Canary routing and automatic rollback.** Shadowing is delivered — an
   independently verified and warmed model, mirrored work under its own
   no-queue bound, answers discarded, `traffic=shadow` on the telemetry, and a
   per-version error/latency window on `/v1/model`. A canary gate can now be
   designed against a real signal, and still needs explicit thresholds, a
   minimum sample, a traffic percentage and a cooldown before it may roll
   anything back on its own. It has no quality signal and will not get one from
   here: runtime health says "it broke", never "its predictions got worse".
8. **Experiment comparison**, after `variant` is a query dimension and its
   traces, evaluation and model version can be joined without heuristics.
