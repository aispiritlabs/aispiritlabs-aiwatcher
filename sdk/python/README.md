# aiwatcher-sdk

Publish agent-run telemetry to aiwatcher, and read its prompt registry.

Two halves with opposite failure policies, and opposite dependencies.

**Telemetry** — `aiwatcher_sdk` itself, which is what an instrumented agent
imports — depends on nothing. It is `urllib`, `json` and `dataclasses`, because
it gets imported into processes that already have opinions about `httpx` and
`pydantic` versions, and because it must never take an agent down: the
transport batches on a background thread and drops on a full queue, loudly.

**The registries** — prompts, annotations, training, conversations — every
method raises, because reading the prompt a service is about to run on, or the
export a run is about to consume, *is* the work. They run in training jobs,
deploy steps and CLIs rather than in somebody's request path, and they share
one `httpx` client with a `tenacity` retry policy that knows the difference
between a request nothing applied and one that may have been.

```bash
uv add aiwatcher-sdk
```

## Tracing a run

```python
from aiwatcher_sdk import AiwatcherClient

client = AiwatcherClient(service="research-service", base_url="http://aiwatcher:8080")

with client.run("run-123", conversation_id="conv-1") as run:
    with run.agent("researcher") as agent:
        with agent.llm(model="claude-opus-5", provider="anthropic") as call:
            call.first_token()
            call.usage(prompt_tokens=812, completion_tokens=193)
```

The scopes emit start and end events; the backend assembles them into spans.
Telemetry never raises and never blocks: the transport batches on a background
thread and drops on a full queue, loudly.

## Tracing a workflow

```python
with client.workflow(
    "house-import",
    nodes=["acquire", "normalize", "analyze", "persist"],
    edges=[("acquire", "normalize"), ("normalize", "analyze"), ("analyze", "persist")],
) as flow:
    with flow.node("acquire") as stage:
        stage.artifact("acquisition.json", uri="s3://planner-flyte/acquisition.json")

    with flow.node("analyze", kind="agent") as stage, stage.agent("floor-plan") as agent:
        agent.message("importer", kind="response")
```

**Declare the shape, every time.** The version is a hash of the topology, so
re-declaring is idempotent and costs nothing — and it is the only way the panel
can draw a stage that has *not* started. A projection over observed events can
say what happened; it cannot say what has not.

One stage per pod is the case this exists for. Pass the same `execution_id`
from every process and the runs join into one graph:

```python
with client.workflow("house-import", nodes=NODES, execution_id=job_id) as flow:
    with flow.node("normalize"):
        ...
```

Omit it and the run *is* the execution, which is right whenever the whole
workflow runs in one process. `attempt=` distinguishes retries of one stage —
two attempts sharing a value fold into one.

Artifacts are **references**. The bytes stay where you put them; aiwatcher
keeps the `uri` because a pointer is bounded and a floor-plan PDF is not.

### Under Flyte, the execution id is already in the pod

Flyte puts its execution metadata in every task pod's environment, so the
`execution_id` above does not have to be threaded through anything:

```python
from aiwatcher_sdk.integrations.flyte import workflow_arguments

with client.workflow(**workflow_arguments("house-import", nodes=NODES, edges=EDGES)) as flow:
    with flow.node("acquire") as stage:
        stage.artifact("acquisition.json", uri=uri)
```

Off Flyte it returns `{"workflow_id": ...}` and nothing else, so the same call
site serves planner's in-process path — where the run *is* the execution — with
no branch.

When the execution was launched from **aiwatcher's own engine routes**, prefer
the id aiwatcher minted: it is what the panel started streaming before the
first pod existed. Declare an `aiwatcher_workflow_run_id` input on the entity
and pass it through, or put it in the environment as
`AIWATCHER_WORKFLOW_RUN_ID`:

```python
workflow_arguments("house-import", nodes=NODES, workflow_run_id=aiwatcher_workflow_run_id)
```

`FLYTE_INTERNAL_*` is internal to Flyte and has changed shape before. Everything
here degrades to "no execution id" rather than raising, so a rename costs the
join and never a task.

## Recording an evaluation

```python
client.record_evaluation(
    suite="catalog-floor-plan",
    dataset="house-catalog@3",  # what makes two reports comparable
    variant=prompt.version_id,  # the join to the prompt registry
    params={"model": "gpt-5-mini"},
    metrics={"mean_score": 0.88},
    report={"failures": [...]},
)
client.flush()  # a short-lived CLI needs the boundary
```

The direct replacement for an MLflow `start_run` / `log_params` / `log_metrics`
/ `log_dict` block, on the client that is already there for tracing.

## The prompt registry

A prompt is the one thing aiwatcher keeps forever: the version a run used has
to be readable long after that run has been evicted from the log. So it lives
in an object store, and the client for it is the **opposite** of the telemetry
one — every method raises.

```python
registry = client.prompts  # or PromptRegistry("http://aiwatcher:8080")

prompt = registry.resolve("planner.floor-plan")  # what `production` points at
system = prompt.render(page=page_json, language="pl")
```

`render` refuses a partial substitution. A missing value would ship a prompt
with a literal `{{ page }}` in it, which the model reads as an instruction and
nothing catches.

Publishing is content-addressed — `version_id` is `sha256(text)` — so a deploy
job that publishes on every start writes one version, not one per start:

```python
version = registry.publish(
    "planner.floor-plan",
    text=SYSTEM_PROMPT,
    author="deploy",
    model="qwen/qwen3-vl-235b",
    label="production",  # optional: publishing and deploying are separate acts
)
```

## The annotation registry

Vector annotations for any vision domain — the project's label schema carries
the domain, and this ships no vocabulary. A **data loader** is the frozen
export a training run records, together with the registry it reads through; a
**split** of it is a sequence you can measure; and a dataset is built from that
split, which is PyTorch's own two steps:

```python
from aiwatcher_sdk.annotations import AnnotationRegistry

with AnnotationRegistry("http://aiwatcher:8080") as data_registry:
    dataloader = data_registry.build_dataloader("corpora/plans", rights_policy="commercial")
    print(dataloader.source)  # corpora/plans@9f3c… — what a training run records

    test = dataloader.get_split("test")
    # images, and the subjects behind them
    print(len(test), len(test.get_groups()))

    train = dataloader.get_split("train").as_dataset(image_size=512)
    loader = train.as_torch_dataloader(batch_size=4, shuffle=True)
```

`build_dataloader` freezes the project on the server as an **export** — a
content-addressed manifest — and hands it back with this registry attached, so
nothing downstream asks for one again. `get_dataloader(source)` reads one that
already exists. A manifest reconstructed from a file with `Export.from_json`
has no registry and takes one: `split.as_dataset(registry, ...)`.

`get_split()` returns a `SplitView`: a plain `Sequence[Sample]`, so `len`,
indexing, slicing and iteration all work, plus the two questions worth asking
before training on it.

```python
side = dataloader.get_split("test")

# the distinct subjects — what a score is really over
side.get_groups()
# images, groups, instances
side.get_counts()
# a slice is another view
side[:8].get_groups()
```

**The group, not the image, is the unit.** One building published as the plain
plan, its mirror, a garage variant and a re-drawn revision is four images and
one observation. `group_id` is what keeps them on one side, so a test score
measures generalisation rather than memorisation — and `len(side.get_groups())`
is the number that bounds what that score can mean. `split_for` computes the
same rule locally, byte for byte, so "is this house held out" needs no request.

`.as_dataset(...)` needs `pip install 'aiwatcher-sdk[vision]'` — it rasterises
the vector shapes into the grids a loss function reads, one per declared schema
layer, on demand and never stored. `.as_torch_dataloader(...)` is the only
place in this SDK that imports torch, inside the method, so a process that only
rasterises never pays for it.

Exclusions are data: `dataloader.excluded_samples` is a list of
`ExcludedSample` — `group_id`, `reason`, `detail` — because an export that
quietly loses a third of a corpus reads exactly like one that did not.

```python
for excluded in dataloader.excluded_samples:
    print(excluded.group_id, excluded.reason, excluded.detail)
```

### What is where

`aiwatcher_sdk/annotations/` is a package: one file per noun, ordered so every
import points up this list and none points back down.

| file | holds |
|---|---|
| `errors.py` | `RegistryError`, and the sentence a disabled instance should produce |
| `split.py` | the *rule* — the three sides, and `split_for` |
| `sample.py` | `Sample` and `ExcludedSample`, the two halves of a manifest |
| `image_source.py` | `ImageSource`, the three reads a dataset needs |
| `view.py` | `SplitView`, one side as a `Sequence[Sample]` |
| `export.py` | `Export`, the frozen manifest and the string a run records |
| `registry.py` | `AnnotationRegistry`, the only file that knows a network exists |

`__init__.py` is the door and re-exports all of it, so
`from aiwatcher_sdk.annotations import AnnotationRegistry` is what it always
was.

### Substituting the registry

A dataset does not need a client. It needs three answers — a project's schema,
one revision's shapes, and an image's bytes — and that is `ImageSource`:

```python
class OfflineImages:  # names ImageSource nowhere, inherits nothing
    def get_project(self, name): ...
    def get_revision_annotations(self, project, image_id, *, revision=None): ...
    def fetch_image(self, sample): ...


train = dataloader.get_split("train").as_dataset(OfflineImages(), image_size=512)
```

A cache in front of a slow link, a reader over a corpus already on the GPU box,
or a test double is then a small class rather than a subclass of something with
a connection pool inside it.

It also settles the direction. A manifest carries the source it was read from
and the registry hands back manifests; pointed at the concrete client that is a
circular import, and pointed at the protocol it is the straight line the file
table above describes.

### How things are named

One rule, across every registry client here. A **method is a verb phrase**: a
read is `get_<noun>` — `iter_<noun>` when it pages, as `iter_images` and
`iter_rows` do — a conversion is `as_<noun>`, and `build_<noun>` asks the server
to make something. A **field or property is a noun**: `source`, `samples`,
`counts`. A **collection is named for what it holds**, so the rows an export
kept are `samples` and the rows it left out are `excluded_samples`. Writes
keep the verb that says what they do — `publish`, `save_revision`,
`register_model`, `promote`, `record_turns`. A method that is a bare noun reads
like a field, and has to be looked up to find out that it is a request.

A **class name never starts with an underscore.** A class that is internal is
simply left out of `__all__`; a leading underscore says the same thing a second
time, more weakly, and then leaks into every annotation that mentions it —
`list[_Context]` in a neighbouring module is a private name crossing a module
boundary in public. `Correlation`, `Buffer`, `Scope`, `Tick` and `FlushRequest`
were `_Context`, `_Buffer`, `_Scope`, `_Tick` and `_FlushRequest`.

A **`@contextmanager` is annotated `Generator`**, never `Iterator`. The
decorated function really is a generator — `contextlib` throws exceptions back
into it at the `yield`, which is the half `Iterator` cannot express — and
`Generator[Foo, None, None]` is written out in full because the one-argument
spelling needs PEP 696 defaults and `requires-python` here is 3.11.

## Prompt optimisation, with DeepEval

`aiwatcher_sdk.integrations.deepeval` turns a DeepEval `OptimizationReport`
into a stored candidate version plus a record of what it was measured against.
It does **not** import deepeval — the report is read structurally, so a DeepEval
release is not an SDK release.

```python
from deepeval.optimizer import PromptOptimizer
from aiwatcher_sdk.integrations.deepeval import record_optimization
from aiwatcher_sdk.prompts import scores

report = PromptOptimizer(...).optimize(...)

record = record_optimization(
    registry,
    "planner.floor-plan",
    report=report,
    baseline=baseline.version_id,
    dev=scores(dev_before, dev_after),  # what the optimiser searched against
    test=scores(test_before, test_after),  # cases it never saw
    dataset="house-catalog@3",
    promote=True,
)

if not record.admitted:
    raise SystemExit(f"not promoted: {record.reason}")
```

Two things are worth being explicit about.

**The verdict is the server's.** `record.outcome` is computed by aiwatcher from
the held-out scores and from what the candidate did to the baseline's
variables. An optimiser selected its candidate by maximising the number it is
reporting, which makes it the last thing that should grade it.

**`test=` is the caller's measurement.** DeepEval does not know which of its
cases were held out. An optimisation recorded without a held-out split is still
recorded — and is refused a promotion, which is the outcome the split exists to
produce. `record.overfit_gap` is the number worth watching across a series: a
run that gains 0.2 on dev and 0.0 on the held-out split found something about
the dev cases, not about the task.

## Development

```bash
uv sync --all-groups
uv run ruff format . && uv run ruff check .
uv run mypy .
uv run pytest
```

Or `just sdk-check` from the repository root, which runs all four.
