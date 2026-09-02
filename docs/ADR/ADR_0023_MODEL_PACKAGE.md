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

`scripts/serve-mini-model.py` is the first hardened profile against it. It
implements exactly one runtime and refuses every other by name; verifies each
artifact's digest before loading; warms the model before reporting ready;
bounds the body, the batch and the work in flight; validates every request
against the shape the *loaded* version declares; takes an optional bearer token
with the probes left public; watches the label and rolls forward in two phases —
download, verify and warm the candidate while the old version keeps serving,
then swap — keeps the previous version loaded for a rollback that needs no
rebuild, and refuses to re-attempt a version that already failed.

It reports one run per request carrying model, version, label, rows, latency
and outcome, as `run.started → llm.started → llm.completed`. A served model is
a model, so an inference joins the same traces, the same model dimension and
the same "which version served this" question as everything else — instead of
arriving as a second kind of thing with its own rules. **It carries no inputs
and no outputs**, which is ADR_0021's rule restated for serving: a runtime that
wants to retain what was said writes turns to the conversation archive, with
consent and a retention clock, exactly as an agent does.

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

The demo profile still only reads `file://`. Signed readers for an object store
and for approved Hub repositories are a separate deliverable with credentials
of their own; what is *not* deferred is the check, because verifying a digest
is the same three lines whatever fetched the bytes, and a loader that skips it
while the fetcher is simple will skip it when the fetcher is not.

There is still no ONNX or TorchScript loader. The manifest is what lets one be
added without renegotiating anything, which was the point of doing it first.

`Runtime::Weights` exists because this repository ships a linear model. Without
it, the first example anybody reads would have to declare itself as ONNX in
order to be loadable — a lie in the field that decides which loader runs.

The rollout keeps exactly one previous version. Two would need a policy for
which to fall back to, and the answer "the one that was serving" is the only
one anybody wants at three in the morning.

**What would make this wrong.** A runtime whose artifacts are not files — a
model served from a registry API, or one whose weights are sharded across a
hundred objects, where `artifacts` becomes a listing rather than a manifest. Or
the first real loader finding that `entry_point` and `preprocessing` as free
text are too loose to act on, at which point they want types and this wants a
version field.
