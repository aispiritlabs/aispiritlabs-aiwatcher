# ADR_0023: A serving runtime is handed a declared package, and a checkpoint URI is not one

- **Status**: accepted
- **Date**: 2026-09-02

## Context

ADR_0018 built the model registry: a version names the run and the export
behind it, a label points at a version, and `production` is what a deployment
reads. The last field in that chain is `checkpoint_uri`, and it is a string.

A serving process handed only that string has to guess six things — which
framework wrote the file, what to load inside it, what shape it eats, what it
hands back, what has to be installed for that to work, and whether the bytes at
that address are the bytes anybody measured. Every one of those guesses is a
way to load the wrong model and serve it confidently:

* `s3://models/edge/latest.pt` is different bytes tomorrow, and nothing says
  so. The registry's promise — that a span naming a version can be traced to
  the images it learned from — is only as strong as "these are the weights".
* A loader chosen by looking at the file is a loader chosen by whoever wrote
  the file. Sniffing a format is how a control-plane process ends up calling
  `torch.load` on somebody's pickle.
* A class order that lives in the serving code silently permutes when somebody
  retrains. Every metric stays finite and nothing says anything — the same
  failure `ExportDataset` checks `schema_version` against.

The plan sequenced the manifest before any loader for exactly this reason: it
is the contract between trainer, registry and runtime, and writing loaders
first means writing three of them against three different implicit contracts.

## Decision

`ModelPackage` is an optional field on a model version, written by whoever
trained it: `runtime` and `runtime_version`, `entry_point`, `inputs` and
`outputs` as `TensorSpec`s (with `classes` for a classifier), `preprocessing`
as named strings, `dependencies`, `artifacts` — each with a `sha256` — and
`resources`.

Three rules carry it.

**Every artifact has a digest, and a package with none is refused.** The same
rule as `put_blob` hashing what it received: an address is not an identity. The
package's own digest is `sha256` over the artifact digests in order, which is
what a running server reports as "the model I have" and what a health check
compares against the registry's answer — one number, so comparing it is an
equality rather than a review. That digest also joins the version's identity,
so two registrations differing only in which weights they name are two
versions.

**A runtime is declared, never sniffed.** `Runtime` is small and ordered by how
much of somebody else's code runs when the artifact is opened: `weights`
(a JSON array of numbers — the smallest thing that is still a runtime, and what
this repository's own mini trainer produces), `onnx`, `torchscript`, `python`,
`unspecified`. `Runtime::executes_packaged_code` is the one question a host has
to answer *before* opening anything, and `python` answers yes — which is why it
is a named variant rather than a fallback. A package that runs its own code
must be loaded in an isolated process, never in the API, which holds the object
store's credentials and every registry behind them.

**A package is optional and, once given, complete.** Versions registered before
this existed have none, and a runtime that meets one says so rather than
guessing — the same choice ADR_0019 makes about a licence nobody recorded. What
is refused is a *half* package: a declared runtime whose weights carry no
digest reads like provenance and is not.

`scripts/serve-model.py` and `aiwatcher_sdk.serving` are the hardened profile
against it. It refuses any runtime it does not implement by name; verifies each
artifact's digest before loading; warms the model before reporting ready;
bounds the body, the batch and the work in flight; validates every request
against the shape the *loaded* version declares; takes an optional bearer token
with the probes left public; watches the label and rolls forward in two phases —
download, verify and warm the candidate while the old version keeps serving,
then swap — keeps the previous version loaded for a rollback that needs no
rebuild, and refuses to re-attempt a version that already failed.

It reports one run per executed model call carrying model, version, runtime,
traffic, rows, latency and outcome, as
`run.started → llm.started → llm.completed`. A primary or shadow invocation is
a model call, so it joins the same traces, the same model dimension and the
same "which version ran this" question as everything else — instead of
arriving as a second kind of thing with its own rules. **It carries no inputs
and no outputs**, which is ADR_0021's rule restated for serving: a runtime that
wants to retain what was said writes turns to the conversation archive, with
consent and a retention clock, exactly as an agent does.

## The second profile, and what it settled

`weights` and `onnx` now both load, which is what turned two open questions
into answers. The split the second one forced is in the code: the hardened half
— resolve, verify, warm, bound, validate, watch, roll back, report — is
`aiwatcher_sdk.serving.server`, and a runtime is four members
(`features`, `classes`, `predict`, `describe`) plus a loader. The first profile
had both inlined; the alternative to splitting them was a second copy of the
rollout, which would have drifted from the first.

**A declaration whose artifact can be asked is cross-checked, not trusted.**
This is the one place in this system where that is true, and it is worth being
precise about why. A workflow's topology, a licence, a label schema — these are
declarations because nothing else knows the answer. An ONNX graph is different:
it carries its own input and output names, element types and shapes, so the
package's `inputs` and `outputs` are a *second* description of something that
already describes itself. The loader compares them and refuses a disagreement
naming both sides. That refusal is not pedantry about a typo: a package whose
declared shape is wrong is a package describing a **different model**, which
means the version's held-out score, its dataset lineage and its label order all
belong to something else. It is also the only check in this chain the model
itself gets a vote in — every other one compares bytes against a digest a human
wrote down.

The same reasoning settles `classes`. `n` classes over a width-`n` head agree;
two classes over a width-1 head are the binary convention; anything else is a
classifier that is either mislabelled or mistrained, and nothing at load time
can tell which. Refused.

**`entry_point` is enough to act on. `preprocessing` is not, and should not
become so.** The ADR flagged both as possibly too loose. `entry_point` turned
out to be actionable for one reason: it is read as *a name in this package* —
an artifact's name, or the last segment of its URI — and a value naming neither
is a refusal rather than a guess. An ONNX package is a graph and often a label
file, so picking between them by convention is how a server loads the labels as
the model. `preprocessing` is the opposite and deliberately stays that way: it
is what the trainer did, in its own words, reported on `/v1/model` and applied
by nothing. A package that shipped preprocessing *code* would be a package that
runs code in whatever opens it, which is what `executes_packaged_code` exists
to keep visible. The caller holds the raw input, so the caller is the side that
must already have done it.

**What a request surface can feed is decided at load, not at the first
request.** `instances` is a list of rows of numbers, which is one rank-2 tensor
with a free batch axis. A graph with two inputs, a rank-4 image tensor, a
string input or a pinned batch dimension is refused by name at load, each
naming the profile it would need — because "it loaded and every request 500s"
is exactly the outcome a pre-flight check exists to turn into a deployment
decision.

`runtime_version` is compared before the bytes are read: an onnxruntime older
than the one that wrote the graph is a refusal naming both versions, which is
what the field was for — a graph using an opset this build lacks otherwise
fails at load, in a crash loop, with the previous version already gone.

## Signed object-store reads and the version cache

The serving process now reads both `file://` and `s3://`. S3 is path-style
against one configured endpoint and one configured bucket, and each GET is
signed with AWS Signature Version 4. The reader refuses redirects and applies
its byte ceiling while streaming rather than trusting `Content-Length`. The
bucket check happens before a request is signed, so credentials configured for
one store cannot be aimed at another bucket by a package URI.

The long-lived cache is not keyed by URI. It is
`<cache>/<immutable-version>/<artifact-sha256>`, populated by an atomic rename
only after the downloaded bytes match the digest. A hit is hashed again before
it is returned; a corrupt entry is deleted and fetched again. The cache touches
a valid hit and evicts least-recently-used entries until its configured byte
budget holds. `_OnceReader` remains the in-memory, per-load deduplication above
it, so a loader and the package-wide verification do not make two reads.

The distinction is deliberate: SigV4 authorises a GET, TLS protects it on the
wire, and the package digest says these are the bytes the held-out measurement
belongs to. None substitutes for another. `load` therefore verifies every
artifact a package declares, including auxiliary files a particular runtime
does not open. `/v1/model` reports the endpoint, approved bucket, schemes,
bounds and cache directory, but never credentials.

The Python signer has a real wire gate in `just test-rustfs` alongside the Rust
signer. A mock server can assert that a header exists; only an S3 server can
say that canonical path, timestamp, scope and signature agree.

## Shadow first, canary second

A serving process may follow one additional label with `--shadow-label`. The
label is resolved to its exact immutable version — not to whatever
`production` returned in the same head — and that version passes the same read,
digest, loader and warm-up gates. A broken or missing shadow is recorded on
`/v1/model` and never changes primary readiness.

After a request is validated for the primary model, its rows may be copied to
the shadow on a daemon worker. The result is always discarded. Shadow work has
its own non-blocking concurrency semaphore: when full, a mirror is counted as
`dropped`, not queued. That keeps a slow candidate from becoming a queue that
outlives the traffic burst or consuming every primary worker. A candidate with
a different feature width is refused at load because the same request cannot
honestly be sent to both.

The shadow health window resets when its version changes and reports requests,
runtime failures, failure rate, dropped mirrors, mean latency and last error.
An old in-flight call still emits telemetry against the version that ran but
cannot add to a new candidate's window. Shadow telemetry carries
`traffic=shadow` and the candidate label, and still carries no inputs or
outputs.

This deliberately stops before canary routing. Runtime errors and latency can
show that a candidate broke; they cannot show that its predictions got worse.
An automatic gate also needs explicit minimum samples, thresholds, traffic
percentage and cooldown. Building those after the shadow has produced an
actual signal avoids freezing a policy around fields no runtime emitted.

## Alternatives considered

**Infer the runtime from the file extension or magic bytes.** Rejected: it puts
the choice of which loader runs in the hands of whoever wrote the file, and the
`python` case makes that a code-execution decision.

**Make the package required.** Rejected: it would make every version registered
before this unloadable and unlistable, and there is a working answer for them —
load and say `verified: false`. Requiring it would also have meant a migration
of the one thing here that must not be rewritten.

**Put the package on the training run instead of the version.** Rejected: a run
can produce several versions (a quantised one, a pruned one), and the artifacts
differ per version. The run is where the *provenance* lives, and
`register_model` already reads dataset, framework and code from it.

**Store the weights in the registry.** Rejected, as everywhere else here: the
registry stores prompt text because storing it is the point, and an artifact is
a byte range somebody else already persisted. Hundreds of megabytes per version
in the object store the prompt registry shares is a different product.

**Emit a new `inference.*` event family.** Rejected: it would need catalog
entries, a fold, a span-assembly decision and a projection, to describe
something the `llm.*` family already describes — a model call with a start, an
end, a latency and an outcome. Reusing it also means the redaction rule already
applies rather than being restated.

## Consequences

The demo defaults to `file://`; a configured serving host adds signed `s3://`
without changing a package or a loader. An approved Hugging Face repository
reader remains a separate deliverable with its own token and repository
allowlist. It will plug into `SchemeReader` and the same cache; digest
verification remains outside every transport because an authenticated address
is still not an identity.

There is still no TorchScript or `python` loader. The manifest is what let the
ONNX one be added without renegotiating anything, which was the point of doing
it first — `weights` and `onnx` differ by one file each and share every gate.
`python` is the one that needs more than a loader: a subprocess boundary, and
a host that answers `isolates_packaged_code` truthfully.

`Runtime::Weights` exists because this repository ships a linear model. Without
it, the first example anybody reads would have to declare itself as ONNX in
order to be loadable — a lie in the field that decides which loader runs.

The rollout keeps exactly one previous version. Two would need a policy for
which to fall back to, and the answer "the one that was serving" is the only
one anybody wants at three in the morning.

Shadow routing is process-local. Its semaphore and counters bound and describe
one pod; fleet-wide sampling still belongs in an ingress or another shared
routing layer. This process has enough state to exercise the semantics and
produce a gate signal, not to pretend one pod's percentage is a fleet's.

The ONNX loader is tested against a stub session rather than the wheel. Its
gates are pure functions of what a session says about itself, and a hundred
megabytes of runtime in every CI run would be testing onnxruntime rather than
this code; `just onnx-version` is what exercises a real graph, and it refuses
unless the graph and the weight vector it re-expresses agree row by row.

**What would make this wrong.** A runtime whose artifacts are not files — a
model served from a registry API, or one whose weights are sharded across a
hundred objects, where `artifacts` becomes a listing rather than a manifest. Or
a runtime that describes itself and *cannot be asked* — where the cross-check
above has nothing to compare against, and `inputs`/`outputs` go back to being
trusted. TorchScript is close to that line: it carries shapes only when the
author traced with them.
