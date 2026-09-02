# From zero to a served model

This guide covers both supported data-entry paths:

1. an agent conversation is tracked, its turns are recorded into the governed
   archive, reviewed, exported as an immutable corpus and handed to a trainer;
2. a public dataset is found on Hugging Face, mapped into aiwatcher's image
   structure, reviewed, exported, trained and promoted.

The final section runs the repository's small real model and serves the
checkpoint selected by the `production` label. The production-scale work that
is intentionally not hidden behind this demo is listed in [plan.md](plan.md).

## 1. Start the local stack

Requirements are Rust 1.98, Node.js, Python 3, `uv` and `just`. Composer/PHP are
only needed for Flow curation and Hub imports.

Install the JavaScript dependencies once:

```bash
just install
just sdk-install   # Python SDK and its development environment
```

For the full walkthrough, use three terminals. The write-ahead log and authored
registries live under `.data`, so the state survives a restart.

```bash
# terminal 1 — API, durable log, Hugging Face discovery enabled.
# Use `just run-conversations` instead for Path A: it turns the conversation
# archive on and writes a development key into ./.data. The archive is off by
# default, because retaining what people said to your agents is a decision
# rather than something to inherit.
just run-hubs

# terminal 2 — panel
just panel

# terminal 3 — optional Flow query/curation service
just flow-install
just flow-serve
```

Open <http://127.0.0.1:5173>. API health is available at
<http://127.0.0.1:8080/livez>; Flow health is shown in the panel. If Hub
discovery is not needed, `just run` starts the same durable server without
outbound Hub access. `just dev` is faster for UI work but uses an in-memory log.

## 2. Path A: agent conversation to an immutable corpus

### Track the conversation

Every event from one turn or continuation should carry the same
`conversation_id`. Each execution has its own `run_id`; every participating
agent carries `agent_id`; `call_id` separates concurrent model/tool calls.
These identifiers make the panel's session → run → span → event path
deterministic and keep retries from creating duplicate spans.

The telemetry path stores operational fields — model, tokens, latency, outcome —
and **never the words**. That boundary is not a default anyone can opt out of by
adding a field: prompt and completion bodies do not belong on the event log at
all, and the Collector strips them from spans. See
[ADR_0021](docs/ADR/ADR_0021_CONVERSATION_ARCHIVE.md).

```bash
AIWATCHER_CONVERSATION=training-demo just seed conversation-run-001
```

In the panel, open **Observability → Explore**, choose the `session` dimension,
and select `training-demo`. The run page shows the assembled agent, LLM and tool
spans and the underlying event audit trail — and none of what was said.

### Record the turns

The words go somewhere else: an encrypted archive with its own retention clock,
its own erasure semantics, and a review gate in front of every export. Turning
it on is one decision and one key:

```bash
just run-conversations   # in place of `just run`
```

A producer records turns through the SDK, which redacts **in its own process**
before anything leaves it and attaches the consent that permits keeping the
rest:

```python
from aiwatcher_sdk.conversations import (
    Consent, ConversationArchive, PatternRedactor, Retention,
)

archive = ConversationArchive(
    "http://127.0.0.1:8080",
    redactor=PatternRedactor(),
    consent=Consent(
        subject="tenant-17",
        basis="consent",
        reference="https://example.invalid/policies/training#2026-09",
        scope=["train", "evaluate"],
    ),
    retention=Retention(ttl_days=30, policy_id="training-v2"),
)

archive.record(
    conversation_id="training-demo", message_id="m1", role="user",
    text=question, run_id=run_id, model=model,
)
archive.record(
    conversation_id="training-demo", message_id="m2", role="assistant",
    text=answer, parent_message_id="m1", run_id=run_id, model=model,
)
```

There is no default redactor: `PatternRedactor` is a floor and `NullRedactor` is
how a producer says it meant to send content verbatim. In the default
`protected` mode a turn with no consent record and no redaction record is
refused — with every missing field at once, rather than one per round trip.

The server scans again whatever the producer claimed, because a hook that was
misconfigured reports exactly the same record as one that worked. Its rules are
deliberately the same conservative set, so a clean scan is evidence the hook
ran.

To see the whole path, including the case where a producer's hook was never
wired to tool output:

```bash
just seed-conversations
```

### Review, then export

Open **Conversations → Review**. The list decrypts nothing: every badge, count
and finding comes from a turn's plaintext head. Reading the words is one
explicit click and needs the `admin` role on an authenticated instance.

Approve what may be trained on. Rejecting needs a reason, and the reason is the
record. The **Better answer?** control beside it is a separate axis — approving
says the content may be used at all, and choosing between two sibling answers
says which was better; a preference export pairs only turns somebody actually
labelled.

Then open **Conversations → Corpora** and queue an export. Four shapes come out
of the same turns:

| Shape | One row is | Loses |
|---|---|---|
| `chat` | a whole conversation | nothing — the one an unforeseen task can be rebuilt from |
| `prompt_response` | an assistant turn and the question before it | tool use entirely |
| `sft` | an assistant turn with its preceding context | the branches |
| `dpo` | a labelled preference pair | everything nobody compared |

The export is a job, not a request somebody holds open. It pins the conversation
list when it is created, writes sealed shards, and advances its cursor only
after each one is stored — so a process killed mid-export resumes at the last
shard it committed and reaches the same version an uninterrupted run would.

Read the exclusion table before the row count. An export that quietly produced
forty rows from four thousand turns looks exactly like one that worked; the
counts are what turn that into "three thousand nine hundred are still waiting
for review".

### Train on the result

The reference is `name@sha256` and it is what a training run records:

```text
GET /api/v1/conversation-dataset-rows?name=training%2Fagent-turns&version=<sha256>&offset=0&limit=100
```

Use the returned `next_offset` until it is absent, or
`ConversationArchive.iter_rows` from the SDK. Reading rows needs the `admin`
role: they are conversation content, and the gate is the same one that guards a
single turn.

Then track the trainer with the Python SDK:

```python
from aiwatcher_sdk.training import TrainingClient

tracking = TrainingClient("http://127.0.0.1:8080")
dataset = "training/agent-turns@<sha256>"

with tracking.run(
    "assistant-sft-2026-09-02",
    model="your-base-model",
    dataset=dataset,
    framework="transformers",
    device="cuda",
    code="train.py@<git-sha>",
    params={"epochs": 3, "learning_rate": 2e-5},
) as run:
    for index in range(epochs):
        with run.epoch(index) as epoch:
            for batch in loader:
                loss = train_step(batch)
                epoch.step(loss=float(loss))
            epoch.metrics(val_loss=validate())
    run.checkpoint(
        "s3://models/assistant/<digest>",
        metric="val_loss",
        value=best_val_loss,
        best=True,
    )

registered = tracking.register_model(
    "assistant.answerer",
    run_id=run.run_id,
    checkpoint_uri="s3://models/assistant/<digest>",
    validation={"loss": best_val_loss},
    test={"quality": held_out_quality},
)
tracking.promote("assistant.answerer", registered["version"]["version"])
```

The registry refuses promotion when the run used a mutable dataset name or the
version has no held-out test measurement. aiwatcher tracks this training loop;
it does not ship a one-command LLM SFT runner, and that gap and its acceptance
criteria are in [plan.md](plan.md).

### Erasure, and what it reaches

An erasure request names a *person*, not a conversation:

```bash
curl -X POST http://127.0.0.1:8080/api/v1/conversation-erasures \
  -H 'content-type: application/json' -d '{"subject":"tenant-17"}'
```

It removes the words from the archive **and** from every published corpus that
already had them — stopping at the archive would be an erasure in name only.
What remains everywhere is the record: heads, digests, review decisions and
export manifests, so an auditor can still be told what was there. A withdrawn
corpus answers 410 rather than 404, and its reference still resolves.

The retention sweep does the same thing on a clock, without anybody filing a
request.

### Coming from the old convention

The build before this one kept training pairs by putting `input` and `output` on
`llm.completed` events. If you have those, move them:

```bash
just import-conversation training-demo tenant-17 consent ticket-4102
```

Every imported turn arrives **pending**, because nobody has read it. The script
cannot remove the bodies from the log — it is append-only, which is why they
should never have been there — so rotating those segments is a retention
operation of your own.

## 3. Path B: Hugging Face to the internal image structure

### Discover and verify rights

Start with `just run-hubs`, then open **Datasets → Discover** and search Hugging
Face. A result's `claimed_license` is only what the mirror reports. It never
becomes training permission automatically. Before import:

1. open the original dataset card and primary source;
2. verify who owns both images and annotations;
3. add or update the curated source record used by the deployment;
4. choose the batch rights assertion in the import form.

Kaggle joins the same search when both `AIWATCHER_KAGGLE_USERNAME` and
`AIWATCHER_KAGGLE_KEY` are set.

### Inspect, map and dry-run

Open a result and choose its configuration and split. aiwatcher reads the Hub
schema and up to 100 preview rows, then generates an editable Flow pipeline.
Map the source columns into this internal contract:

| Field | Meaning |
|---|---|
| `uri` | source image URL or stored object URI |
| `width`, `height` | original pixel dimensions |
| `group_id` | the real-world subject/family, not the filename |
| `view`, `level` | optional view metadata |
| `metadata` | source-specific fields worth preserving |

`group_id` is the split boundary. Multiple renderings of the same building or
subject must share it, otherwise train/test leakage looks like model quality.

Click **Dry run** first. Review every rejected row and warning, especially a
batch where each image became its own group. Only then run the import into a
named annotation project. The source record keeps the Hub, dataset id, URL,
claimed licence and exact Flow mapping.

### A corpus, rather than a catalogue

`POST /api/v1/annotation-imports` takes every row in one body and is capped at
five thousand. That is the right shape for a catalogue and the wrong one for a
corpus — the request has to be held open, retried whole, and kept in one
process's memory, and a network failure at row 900 000 loses all of it. So a
corpus is *staged* first and read by a job:

```bash
just seed-import   # stages twelve pictures in three pages and imports them
```

The same path by hand, and the four things worth knowing about it:

```text
POST /api/v1/annotation-import-batches   rights, evidence, and the Hub commit
POST /api/v1/annotation-import-rows      one page — repeat, and retry freely
POST /api/v1/annotation-import-jobs      seals the batch and queues the job
GET  /api/v1/annotation-import-rejects   the rows it refused, and why
```

**Pin the revision.** A dataset id is a moving target: `main` is whatever the
uploader pushed last, so a corpus re-read a month later is a different corpus
under the same name. A batch with a dataset id and no revision comes back with
a warning on a response that succeeded.

**Record the evidence.** `evidence.primary_source_url` is where somebody read
the licence — the paper, the project page, the repository it was published
from, and never the Hugging Face or Kaggle card. It is recorded rather than
enforced, because a refusal that can be satisfied by typing something teaches
people to type something. The one hard refusal stays where a human already read
the licence: a commercial claim on a corpus the curated table calls
research-only is rejected before the job exists.

**Number the pages.** A numbered append is idempotent — identical bytes are an
acknowledged retry, different bytes for a page already stored are a refusal
naming the page. That is what makes a million rows over a flaky link something
to re-send rather than reconcile.

**Read the refusals first.** Open **Annotations → Imports**. An import that
registered four hundred thousand of six hundred thousand pictures looks, from a
success response, exactly like one that worked, and the two hundred thousand it
dropped are the story. The counts are grouped by reason and the rows behind one
reason are a click away.

The bytes are fetched through one bounded downloader, and every row goes
through it: `https` only with the host parsed rather than matched, an allowlist
of a hub's own asset hosts, a check that every resolved address is a public
one, no redirects, a byte ceiling applied while the body streams, a pixel
ceiling read out of the image's own header, and a content address verified
against what the row claimed. A row naming `169.254.169.254` is a rejected row
with a sentence, not a request. See
[ADR_0022](docs/ADR/ADR_0022_STAGED_IMPORT_JOBS.md).

### Label, review and freeze

Open **Annotations → Label**. The project schema, not hard-coded vocabulary,
defines classes, geometry, attributes, links, layers and ignore regions.
Model proposals remain marked as proposals until reviewed.

Accept the images that may enter training, then open **Annotations → Exports**:

1. choose split ratios and an optional commercial-use requirement;
2. build the export;
3. inspect split counts and every exclusion reason;
4. copy the immutable `project@export-sha256` reference;
5. fetch COCO per split or use `aiwatcher_sdk.integrations.vision.ExportDataset`
   to derive tensors and masks from the vector annotations.

The export is content-addressed and idempotent. A changed label, review state,
schema or rights decision produces a different reference.

### Train, register and promote

The smallest executable proof needs no NumPy, Pillow or GPU:

```bash
just e2e-train
```

It generates 12 images, validates and reviews annotations, builds a
family-separated export, fetches it through the API, derives an edge grid,
fits an eight-weight logistic classifier, records 300 epochs, writes a real
checkpoint, registers the version and promotes it. It also registers an
unmeasured version and verifies that the promotion endpoint refuses it.

Open **Training → Runs** for the loss/IoU curve and **Training → Models** for
the production label, validation/test scores and blocked version.

For a real vision model, use the export adapter and the same tracker:

```python
from aiwatcher_sdk.annotations import AnnotationRegistry
from aiwatcher_sdk.integrations.vision import ExportDataset
from aiwatcher_sdk.training import TrainingClient

registry = AnnotationRegistry("http://127.0.0.1:8080")
export = registry.build_export("your/project")
train = ExportDataset(registry, export, split="train", image_size=512)
tracking = TrainingClient("http://127.0.0.1:8080")
```

The complete PyTorch/Lightning loop follows the same `training.run`,
`run.epoch`, `run.checkpoint`, `register_model`, and `promote` calls shown in
[EXAMPLES.md](EXAMPLES.md#from-an-export-to-tensors).

## 4. Serve the promoted model

Run the demo trainer once, then start the serving process in a separate
terminal:

```bash
just e2e-train
just serve-mini-model
```

The server asks the registry which version the `production` label names,
refuses a registry that resolves a different one, **hashes every artifact the
version's package declares and refuses a mismatch**, runs one synthetic request
through the loaded model, and only then reports ready. It serves on port 8091;
8090 is avoided because the optional Iggy broker uses it.

That digest check is the field the package exists for. `s3://models/latest.pt`
is different bytes tomorrow, and a version whose weights cannot be checked is a
provenance chain with a hole in it — so a trainer declares them:

```python
tracking.register_model(
    "assistant.answerer",
    run_id=run.run_id,
    checkpoint_uri=path,
    validation={"loss": best_val_loss},
    test={"quality": held_out_quality},
    package={
        "runtime": "onnx",              # declared, never sniffed
        "runtime_version": "1.17",
        "entry_point": "model.onnx",
        "inputs": [{"name": "pixels", "dtype": "float32",
                    "shape": [None, 3, 512, 512]}],
        "outputs": [{"name": "logits", "dtype": "float32",
                     "shape": [None, 4], "classes": [...]}],
        "preprocessing": ["resize:512", "normalize:imagenet"],
        "artifacts": [{"name": "weights", "uri": path,
                       "digest": sha256_of(path)}],
        "resources": {"memory_mb": 4096, "gpus": 1},
    },
)
```

`classes` belongs there rather than in the serving code: a label order that
lives in a deployment silently permutes when somebody retrains, every metric
stays finite, and nothing says so.

Inspect the resolved model:

```bash
curl -s http://127.0.0.1:8091/v1/model | python3 -m json.tool
```

It reports the version, the package digest, the runtime, whether the artifacts
were verified, and what was serving before. A version registered before
packages existed is loaded and reported `verified: false` — an unverified model
that reported itself as verified would be worse than one that reported the
truth.

Request predictions. Each row is the eight-feature vector produced by the demo
rasteriser:

```bash
curl -s -X POST http://127.0.0.1:8091/v1/predict \
  -H 'content-type: application/json' \
  -d '{"instances":[[1,0.9,0.1,0,0,0,0,0.81],[1,0,0,0,0,0,0,0]]}' \
  | python3 -m json.tool
```

Expected classes are `edge` and `background`. `/livez` is the liveness probe and
`/readyz` is 503 until a version is loaded *and* warmed. Use
`just serve-mini-model 8092` to select another port, `--token` (or
`AIWATCHER_SERVE_TOKEN`) to require a bearer token on `/v1/*` — the probes stay
public — and `--max-concurrency` to bound work in flight.

### Moving the label

The server watches `production`. Move it and the rollout is two-phase:
download, verify and warm the candidate **while the current version keeps
serving**, then swap. A version whose weights do not hash to what its package
says never becomes ready, and the reason is on `/v1/model`:

```json
"rollout_error": "b890e8856a6c cannot become ready: … hashes to 6469fa28… and
                  the package says 29bb4649…. These are not the weights that
                  version was measured on"
```

The previous version stays loaded, so going back needs no rebuild and no fetch:

```bash
curl -s -X POST http://127.0.0.1:8091/v1/rollback | python3 -m json.tool
```

The version being left is then pinned out, because a rollback the next poll
undoes is not a rollback.

### What it reports, and what it never reports

Every request emits `run.started → llm.started → llm.completed` carrying the
model, the version, the label, the row count, the latency and the outcome. A
served model is a model, so an inference joins the same traces and the same
model dimension as an agent's model call — open **Observability → Explore** and
the `model` dimension has it.

What those events never carry is `instances` or `predictions`. **Inference
inputs and outputs do not go on the event log**, the same rule and for the same
reason as ADR_0021: a runtime that wants to retain what was said writes turns
to the conversation archive, with consent and a retention clock, exactly as an
agent does. `--no-telemetry` turns the reporting off entirely.

This profile implements one runtime — `weights`, the JSON vector the demo
trainer produces — and refuses every other **by name** rather than attempting
it, because a loader chosen by looking at the file is a loader chosen by
whoever wrote the file. It reads `file://` only. What a second profile adds is
an ONNX or TorchScript loader and a signed reader for an object store; both are
sequenced in [plan.md](plan.md), and both plug into a manifest that already
exists. See [ADR_0023](docs/ADR/ADR_0023_MODEL_PACKAGE.md).

## 5. Reproduce the full populated UI

With the API, panel and optional Flow service running:

```bash
just seed
just seed-evaluation
just seed-prompts
just seed-workflow
just seed-curation
just seed-annotations
just seed-import
just seed-conversations   # needs `just run-conversations` rather than `just run`
just e2e-train
```

The resulting screens are documented in [EXAMPLES.md](EXAMPLES.md). Before a
code change is pushed, run `just check`; the optional PHP service has its own
`just flow-check` gate.
