# Runtime is the composition root; workflow is the unit of work

The application chooses workflows and configures their execution in one
runtime, following the composition pattern in `ai_spirit_agent`. Workflows
describe processes; tasks implement their effects. Workers are runtime-owned
execution slots, not the application's public orchestration model.

```python
import os

from aiwatcher_sdk.runtime import ExecutionPool, Runtime
from aiwatcher_sdk.workflow import Workflow, WorkflowStep
from planner_tasks import acquire, normalize, analyze, persist


def build_runtime() -> Runtime:
    house_import = Workflow(
        name="planner.house-import",
        version="1",
        steps=(
            WorkflowStep("acquire", acquire),
            WorkflowStep("normalize", normalize, after=("acquire",)),
            WorkflowStep("analyze", analyze, after=("normalize",)),
            WorkflowStep("persist", persist, after=("analyze",)),
        ),
    )
    return Runtime(
        name="planner",
        url=os.environ["AIWATCHER_URL"],
        token=os.environ["AIWATCHER_TOKEN"],
        workflows=[house_import],
        pools=[ExecutionPool("imports", queue="planner-import", concurrency=2)],
        placement={house_import.ref: "imports"},
    )
```

Start the application factory:

```bash
aiwatcher-runtime --factory planner_runtime:build_runtime
```

For a process assigned one specific attempt, use the same factory and services:

```bash
aiwatcher-runtime --factory planner_runtime:build_runtime \
  --pool imports --attempt execution-id/acquire/1
```

`runtime.run_attempt(ref, pool="imports")` is the equivalent Python operation.
It requires a fresh runtime, starts one slot regardless of the pool's configured
concurrency, joins it and closes the runtime. It does not publish definitions or
poll other work. A task failure is reported durably and returns `False` (CLI exit
1); transport, claim and lifecycle failures raise. The pinned task version must
be hosted by the selected pool. This supplies the execution entry point for a
future pod-per-attempt controller; it does not provision that pod.

For embedding and live local capacity changes:

```python
with build_runtime() as runtime:
    runtime.start()
    runtime.scale("imports", concurrency=4)
    print(runtime.get_status())
    # The owning application does its work here.
    runtime.scale("imports", concurrency=0)  # drain; stop claiming new work
```

`start()` is nonblocking and idempotent. `serve()` starts and waits until stopped
or a worker fails. `stop()` prevents new claims; `close()` waits for active
attempts, closes resources and raises collected worker failures. `serve()` and
the CLI perform that shutdown automatically. A stopped runtime cannot restart.

Unexpected worker termination (including `SystemExit` and `KeyboardInterrupt`)
stops all pools. `close()` reports the original causes in an `ExceptionGroup`
or, for process-control exceptions, a `BaseExceptionGroup`. Cleanup failures
are collected too. Failure to construct or start a slot during `start()` or
`scale()` stops the runtime, drains already started slots and closes owned
resources before raising. Scaling does not silently recover a failed runtime;
its supervisor must construct a new instance.

The `workflows=` argument also accepts a factory taking `RuntimeServices`.
The factory receives the runtime's shared telemetry client, constructs the
workflow implementations once, and returns the definitions. This is the seam
for application dependency injection; no global provider setup is performed.

## Scaling contract

An `ExecutionPool` currently means synchronous task slots in local threads.
`concurrency=4` permits four active attempts in this runtime process. Scaling
down drains running calls, which may temporarily exceed the new desired count;
`PoolStatus` exposes desired, running and draining counts. Pools have distinct
queues. Multiple workflows may share one pool and its capacity.

This is not a count of simultaneously running workflow instances, a
cross-process quota, Kubernetes replicas or autoscaling. Multiple application
replicas add capacity by polling the same server queue. Cluster replica control,
resource templates and global quotas need their own server/controller support.
The runtime is the place to configure that support when implemented; task and
workflow business code do not acquire deployment settings.

## Current execution boundary

`Workflow` is currently a static definition. It validates references and cycles
without executing tasks, and gives the runtime the implementations to host.
`after` records dependencies; the Python runtime does not schedule them. The
Rust server must already have a matching plan, queue and pinned task versions.

`runtime.register()` publishes these definitions and their queues. The CLI
registers them before serving. `runtime.run(workflow, parameters={...})` pins the
registered revision and returns an execution handle:

```python
with build_runtime() as runtime:
    runtime.start()
    run = runtime.run("planner.house-import@1", parameters={"job_id": "house-42"})
    view = run.wait(timeout=600)
    print(run.execution_id, view)
    # On a failed step, after fixing its external cause:
    # run.retry("persist")
```

The runtime process hosts capacity; Rust starts and schedules the execution.
A separate process or the panel can start the same registered workflow through
`POST /api/v1/executions` with `target.kind = "workflow"`. See
[the Rust execution guide](../../../../docs/RUST_WORKFLOW_RUNTIME.md).

Message-driven workflows with `decide/evolve/initial_state`, as in the linked
Emmett proposal, fit the same runtime ownership model but need the hosted
decider protocol. They are not replaced by a Python loop over these static
steps. See the [design decisions](../../../../docs/PYTHON_SDK_DESIGN.md).
