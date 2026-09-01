# aiwatcher, in screenshots

Every screenshot below is the panel against a running server — no mockups. The
data behind them comes from the seed scripts in `scripts/`, so all of it is
reproducible in about a minute; the commands are at the [bottom](#reproducing-these).

The panel is six areas, not eleven pages: watching runs, watching the pipelines
those runs are stages of, judging them, keeping the prompts they run on,
curating what they are judged against, and changing the thing being run. Two of
those six are not built yet, and say so rather than rendering plausible rows.

- [Observability](#observability) — [Runs](#runs) · [A run's trace](#a-runs-trace) · [The same run, still running](#the-same-run-still-running) · [Explore](#explore) · [Metrics](#metrics) · [Query](#query)
- [Workflows](#workflows)
- [Evaluation](#evaluation)
- [Prompts](#prompts)
- [What is not built yet](#what-is-not-built-yet)

## Observability

### Runs

![The runs list](docs/screenshots/runs.png)

The flat list, filterable by status. One row per run: which agents took part,
the trace the run assembled into, how long it took, how many LLM and tool calls
it made, and how many of its input tokens the provider served from cache.

Nothing here loads a whole run. The list is a cursor page from the read model
and a virtual window in the browser, which is what keeps one very long run from
being a request neither side can hold
([ADR_0007](docs/ADR/ADR_0007_EXPLORER_DIMENSIONS.md)).

### A run's trace

![A run's span waterfall](docs/screenshots/run-trace.png)

The same run, opened: ninety events folded into twelve spans. The
`invoke_agent` spans are the agents, the `chat` spans are the LLM calls, and
the `execute_tool` spans are the tools — each one written only when its end
event arrives.

The twenty-odd `llm.chunk` events behind the streamed call are counted and
thrown away rather than stored one trace record each
([ADR_0003](docs/ADR/ADR_0003_SPAN_ASSEMBLY.md)). The span ids in the chips are
derived from the run id and a stable span key, not generated, so a redelivered
event lands on the span it already wrote
([ADR_0001](docs/ADR/ADR_0001_EVENT_ENVELOPE.md)).

### The same run, still running

![A run streaming live](docs/screenshots/run-live.png)

A run whose events are arriving while the page is open: the header says `live`,
the duration is still blank, and the trace holds only the spans that have
already ended.

The events pane below is the live tail over SSE. Every frame carries its
checkpoint as the `id:`, so a browser that loses the connection resumes through
`Last-Event-ID` with no application code on either side
([ADR_0004](docs/ADR/ADR_0004_LIVE_STREAM_RESUME.md)).

### Explore

![The explorer, pivoted on agent](docs/screenshots/explore.png)

One page for every level. The tree pivots on **session, agent, runtime,
workflow, trace, model, tool or span** — here it is rooted on the agent — and
below the root it is always run → span → messages, so switching what the top
level *is* costs no relearning.

Selecting the `chat claude-opus-5` span narrows the messages on the right to
that span without collapsing the levels above it, and the whole selection lives
in the URL, so this exact view is a link. One fold answers every pivot; the
rows differ only in which key a run contributes to
([ADR_0007](docs/ADR/ADR_0007_EXPLORER_DIMENSIONS.md)).

### Metrics

![The metrics page](docs/screenshots/metrics.png)

Tokens, success rate, cache hit rate and the latency percentiles that matter
for an agent — time to first token separately from time to the whole
completion — over a window, with ranked breakdowns by model, agent and tool.
The `extract-agent` row carries the failed run, which is why it is marked.

### Query

![The Flow PHP query tab](docs/screenshots/query.png)

An optional tab. The pipeline is a Flow PHP `data_frame()` expression over the
same runs the explorer shows, answered by `services/flow` — a service that is
outside the Cargo workspace and that the Rust binary does not know exists.

The query is **parsed, never executed**: `token_get_all()` lexes it, a
whitelist checks it, and an explicit `match` turns it into Flow objects. There
is no `eval` in the service and no name from a query ever becomes a callable
([ADR_0008](docs/ADR/ADR_0008_FLOW_QUERY_SURFACE.md)). The starter query is
deliberately the *corrected* form of the obvious first question — `runs`
carries `agents` as a list, so grouping by agent needs the expansion.

Grain is what decides this, not transport: this question is 210 ms in Flow
against 5 ms for the Rust dimension route, which is why the live path stays in
Rust and there is no export.

## Workflows

![One execution of a declared workflow](docs/screenshots/workflows.png)

The level above a run. Pick an orchestration on the left, pick one of its
executions, and the graph is that execution: stage statuses, durations, the
agents that did the work, and the artifacts each one handed on.

**`Render thumbnails` says "not run", and that is the point.** A projection
over observed events can say what has happened and can never say what has not,
because a stage that never started emits nothing. So the topology rides the log
as `workflow.declared` and the graph is drawn against it
([ADR_0012](docs/ADR/ADR_0012_WORKFLOW_GRAPH.md)). The `if public` branch was
not taken here; the same rendering is what shows you a pipeline halfway
through.

**`Analyze floor plans` carries a `2`.** It failed once and was retried.
Attempts are counted by span key rather than by event, so a redelivery does not
invent a retry that never happened.

**Five runs, one execution.** Flyte gives every stage its own pod and therefore
its own `run_id`; `workflow_run_id` is what joins them, and it is why this is a
view of its own rather than a filter on the runs list. The live stream is
scoped the same way, so the pod that has not started yet still arrives.

The dashed edges across the top are the other half: `agent.message` records one
agent addressing another, which is the one thing nesting cannot show — two
agents exchanging work through a queue nest inside nothing at all. They are
drawn as a different kind of edge from the declared ones and never merged with
them, because sequence is not communication.

**Rerun** asks a configured orchestrator to run the workflow again, optionally
from the selected stage. aiwatcher runs nothing itself: it posts to one
endpoint from its own configuration — never a URL from an event — and answers
`202`, because nothing has happened yet. The evidence that the rerun ran is the
events it publishes. With no runner configured the button is replaced by a card
naming the variable to set.

## Evaluation

![An evaluation report against its baseline](docs/screenshots/evaluation.png)

Scoring runs against a dataset. The suite cards are the top level; picking a
report shows its parameters, its metrics against the previous report on the
same dataset, its per-case scores, and the document the producer attached.

**Changed cases** is the reason this view exists. The mean improved by 0.02 and
a case that passed before now fails; a mean alone hides that in either
direction. Two reports are only compared when they agree about suite *and*
dataset — a delta between scores measured on different cases claims they are
one fact when they are two.

An evaluation is deliberately not a trace. `eval.*` events ride the same log,
form no span, and fold into their own bounded projection
([ADR_0010](docs/ADR/ADR_0010_EVALUATION_REPORTS.md)). That is what lets a
producer drop an MLflow `start_run` / `log_params` / `log_metrics` block for
four fields on the client it already imported for tracing.

## Prompts

![The prompt list](docs/screenshots/prompts.png)

The one area that reads something other than the log, and the one that writes.
A prompt is authored rather than observed, and the version a run used has to be
readable after that run has been evicted — so the registry is an object store
(RustFS in a deployment, a directory under `just run`), outside retention
entirely ([ADR_0011](docs/ADR/ADR_0011_PROMPT_REGISTRY.md)).

Each row carries what an optimiser last tried on that prompt, with its dev gain
beside its held-out gain. The two are shown together on purpose: the ratio is
the story, not either number.

![One prompt's versions and optimisations](docs/screenshots/prompt-versions.png)

Opened: the version history, the text, a diff against whatever a version was
derived from, and every optimisation with the server's verdict.

The verdict is the server's, computed from the held-out split — never the
client's, because an optimiser selected its candidate by maximising the very
number it is reporting. One of the three here was admitted; the other two were
refused, and for the two different reasons a candidate gets refused:

- **the held-out score did not improve** — +0.350 on the split the search ran
  against and ±0.000 on the split it never saw. `overfit_gap` is the number
  worth watching across a series.
- **it stopped interpolating a variable the baseline used** — checked *before*
  the scores, so the reason says the candidate stopped reading its input rather
  than inviting somebody to raise the iteration count. It scored well because
  the harness fed it the same fixed text every time.

Publishing a new version and moving the `production` label are two separate
acts, because storing a prompt and deploying it are two decisions.

## Annotations

The one area that draws. Everything else here is a fold over the log and is
bounded by retention; an annotation is authored, and the label a model was
trained on has to outlive every run that used it — so it lives in the same
object store as prompts, under its own prefix
([ADR_0017](docs/ADR/ADR_0017_IMAGE_ANNOTATION.md)).

**Label** is a canvas over one plan, with the three columns in the order
attention moves: which image, the plan, what the selected shape says.

A room is a polygon, a wall is a *centreline plus a thickness* — the thing an
editor drags and a 3D extrusion needs, with the filled rectangle recoverable
from it — and a door is a named keypoint set: the two ends of its opening, its
hinge, and where its leaf ends when open. That last one is the whole reason
this is not a mask tool. A segmentation mask cannot say which wall an opening
sits on, which way a door swings, or which two rooms it connects, and those are
exactly the fields the output JSON has to carry. Draw pixels and they are lost
at the moment of drawing.

Attributes and links carry the rest: `role: exterior | interior` and
`thickness_px` on a wall, `door_type` and `exterior` on a door, `wall` and
`connects` as references to other instances in the same image. A model's
proposal is drawn dashed and marked `model`, so a page of predictions nobody
has checked looks like one.

A drawing the registry refuses comes back as a 422 carrying *every* problem at
once — the shapes that caused them turn red, and the sentences are listed under
the canvas. Fixing one error per round trip is how somebody stops using a tool.

**Sources** is a dated table of the public floor-plan corpora: CubiCasa5K,
ResPlan, MSD, CVC-FP, FloorPlanCAD, WAFFLE, R2V/R3D, ZInD, RPLAN and LIFULL
HOME'S — what each one labels, how large it is, and whether its licence permits
a commercial model. That last filter is one click, because it is the question
with an expensive wrong answer. The table is shipped rather than fetched:
Hugging Face, Kaggle and Roboflow Universe all restate licences wrongly often
enough that a live answer would be worse than none, since it would arrive
looking authoritative. Every row links its original and says when somebody last
read the licence there.

**Exports** freezes a project into an immutable, content-addressed manifest.
Its id is the string a training run records — `project@export-sha256`, the same
shape as `dataset@version` — and two exports of an unchanged project are one
export, so building it before every run costs nothing.

Two things on that page are worth more than the headline counts:

- **the split is by family.** Every image declares a `group_id`: one building,
  however many renderings. A catalogue house published as the plain plan, its
  mirror and a garage variant is four images and one observation, and splitting
  them across train and test makes the score a measurement of memorisation with
  nothing in the numbers to say so. The panel shows a labeller which side the
  plan in front of them is on, because that changes how carefully it gets drawn.
- **every exclusion is listed with its reason.** An export that quietly loses a
  third of a corpus reads exactly like one that did not. The seeded project has
  two: one image nobody accepted, and one CC BY-NC image that a *commercial*
  export refuses — which is the failure mode that otherwise surfaces in a legal
  review rather than in a metric.

COCO is served per split, generated from the manifest rather than stored: a
second copy of the annotations is a copy that can disagree with the first.
Masks and heatmaps are produced in Python, where the array libraries already
are.

## Training

A training run is a run. `train.started | train.completed | train.failed` are
`Subject::Train` with the ordinary phases, so it appears in Explore, in the
runs list and in the metrics fold with no new machinery — and the model version
an agent run used is then traceable back to the export it was trained on
([ADR_0018](docs/ADR/ADR_0018_TRAINING_RUNS.md)).

```python
from aiwatcher_sdk import AiwatcherClient
from aiwatcher_sdk.annotations import AnnotationRegistry

registry = AnnotationRegistry("http://aiwatcher:8080")
export = registry.build_export("floor-plans/dom-projekt")

client = AiwatcherClient(service="floor-plan-trainer", base_url="http://aiwatcher:8080")
with client.training(
    "floorplan-effnetv2s-2026-09-01",
    model="efficientnetv2-s",
    dataset=export.reference,          # project@sha256, never a bare name
    params={"batch_size": 4, "lr": 3e-4},
) as run:
    for index in range(epochs):
        with run.epoch(index) as epoch:
            for batch in loader:
                epoch.step(loss=loss.item())   # counted; never published
            epoch.metrics(val_miou=score)      # measured once; published
    run.checkpoint(path, metric="val_miou", value=score, best=True)
```

What that publishes is deliberately small. An **epoch** is one point carrying
its own duration and metrics — two hundred equal bars in a waterfall is a
picture of the fact that epochs take about the same time, and the curve is the
view. A **step** never reaches the log at all: the SDK averages locally, which
is the same rule `llm.chunk` follows at a different scale. A **checkpoint** is
a pointer, because the projector holds every event it accepts in memory. A
**profiler session** arrives as its top operators and a link, because
`torch.profiler` on one step emits more records than the read model holds for a
week.

For Lightning there is a callback that needs no change to the loop at all, and
it never imports Lightning — it duck-types the hooks, the same way the DeepEval
bridge reads a report structurally:

```python
from aiwatcher_sdk.integrations.torch import TrainingCallback

trainer = Trainer(callbacks=[TrainingCallback(
    client, run_id="…", model="efficientnetv2-s", dataset=export.reference,
)])
```

Weights & Biases stays available and is not the system of record here: pass a
`wandb` run as `mirror=` and the same points go to both. What W&B cannot answer
is which agent runs used the resulting checkpoint, and that is the reason the
training run is on this log.

## Starting the work, not only watching it

Data Curation and Experiments both carry a picker over the orchestrator's own
inventory: the launch plans Flyte already holds, with the inputs each one
declares rendered as a form. The page fills in what it knows — the dataset name
and the time window become the "what" and the "over what period" — and
launching needs the `admin` role, exactly as a rerun does. An instance with no
orchestrator configured says which variable is unset rather than showing an
empty catalog. See [ADR_0016](docs/ADR/ADR_0016_PIPELINE_ENGINE.md).

Nothing has run when the acknowledgement appears, and the panel says so: it
shows the engine's phase as a second opinion and links to aiwatcher's own live
view of the execution, which fills in once the workflow's producer publishes.

## What is not built yet

![The datasets placeholder](docs/screenshots/datasets.png)

Experiments can start a training, evaluation or inference workflow, and cannot
yet *compare* two of them: the join from a variant to the traces it produced —
where latency and token cost live — needs `variant` to become a dimension
first. That half of the area renders a placeholder naming what is missing,
rather than mocked rows: a plausible fake reads as working software, and this
one does not.

## Reproducing these

```bash
just install    # panel and TypeScript SDK dependencies
just dev        # server on :8080 with an in-memory bus, panel on :5173
```

Then, against the running server:

```bash
just seed             # one run: ~35 events in, 5 spans out
just seed-evaluation  # two comparable reports of one suite on one dataset
just seed-prompts     # a prompt, three optimisations, one of them admitted
just seed-workflow    # two executions of one declared graph: one done, one live
just seed-annotations # six synthetic plans, three families, an export, a training run
```

`seed-annotations` draws its own plans in pure Python — no Pillow, no numpy —
so the labels are correct for the pixels rather than approximately correct,
which is the one thing a seeded dataset can get wrong in a way nobody notices.
It leaves one image as a draft and marks one CC BY-NC, so the export has
something to exclude for each of the two reasons.

The Query tab needs the optional PHP service; without it that tab says so and
nothing else is affected:

```bash
just flow-install
just flow-serve       # :8081, reading the aiwatcher API on :8080
```

The runs behind the fuller screenshots — several workflows, four agents, four
models, one failure — are more than `just seed` publishes on its own; run it a
few times with different ids and environment overrides:

```bash
AIWATCHER_WORKFLOW=floor-plan AIWATCHER_SERVICE=planner-service \
  AIWATCHER_CONVERSATION=conv-beta just seed run-002
```
