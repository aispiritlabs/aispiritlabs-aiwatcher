"""Two durable steps, artifacts, retries and child spans without an external service."""

import os
import time
from pathlib import Path

from aiwatcher_sdk.runtime import ExecutionPool, Runtime
from aiwatcher_sdk.task import task
from aiwatcher_sdk.worker import TaskError, get_task_context
from aiwatcher_sdk.workflow import RetryPolicy, Workflow, WorkflowInput, WorkflowStep


@task("demo.acquire", version="1")
def acquire(number: int = 3) -> dict[str, int]:
    ctx = get_task_context()
    if marker := os.environ.get("AIWATCHER_DEMO_BLOCK_FILE"):
        Path(marker).write_text(ctx.context_id)
        while True:
            ctx.raise_if_cancelled()
            time.sleep(0.1)
    with ctx.tracer.agent(name="acquire"):
        ctx.write_artifact("source", [{"number": number}])
    return {"rows": 1}


@task("demo.persist", version="1")
def persist(number: int = 3) -> dict[str, int]:
    ctx = get_task_context()
    if ctx.assignment.attempt == 1:
        raise TaskError("demonstrate a recoverable failure", classification="transient")
    with ctx.tracer.agent(name="persist"):
        rows = ctx.read_artifact("source")
        ctx.write_artifact("saved", rows)
    return {"number": number, "rows": len(rows)}


def build_runtime() -> Runtime:
    workflow = Workflow(
        "demo.worker-import",
        "1",
        (
            WorkflowStep("acquire", acquire, outputs=("source",), timeout_seconds=600),
            WorkflowStep(
                "persist",
                persist,
                inputs=(WorkflowInput("acquire", "source"),),
                outputs=("saved",),
                retry=RetryPolicy(delays_seconds=(0,), delays_seconds_unavailable=(0,)),
            ),
        ),
    )
    return Runtime(
        name="demo",
        url=os.environ.get("AIWATCHER_URL", "http://127.0.0.1:8080"),
        workflows=[workflow],
        pools=[ExecutionPool("local", "demo", concurrency=1)],
        placement={workflow.ref: "local"},
        poll_interval=0.1,
    )
