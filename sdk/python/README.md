# aiwatcher-sdk

Publish agent-run telemetry to aiwatcher, and read its prompt registry.

No dependencies. Everything here is `urllib`, `json` and `dataclasses`, because
this gets imported into processes that already have opinions about `httpx` and
`pydantic` versions, and a telemetry library that forces one of those is a
library people vendor around.

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

## Prompt optimisation, with DeepEval

`aiwatcher_sdk.integrations.deepeval` turns a DeepEval `OptimizationReport`
into a stored candidate version plus a record of what it was measured against.
It does **not** import deepeval — the report is read structurally, so this SDK
stays dependency-free.

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
