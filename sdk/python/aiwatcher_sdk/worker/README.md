# Worker execution internals

Application setup belongs to [Runtime](../runtime/README.md), which hosts
workflows and owns pools and lifecycle. This guide covers the lower-level
worker API used by Runtime.

The [SDK design comparison](../../../../docs/PYTHON_SDK_DESIGN.md) explains
which patterns were adopted from Temporal, Prefect and ZenML.

This is the Python half of Phase 10: execute registered functions over the
existing managed-execution HTTP API, without importing Flyte or Kubernetes.
The server owns dispatch, retries, leases, dependency ordering and settlement.

Create a local module, for example `planner_tasks.py`:

```python
from aiwatcher_sdk.task import task
from aiwatcher_sdk.worker import get_task_context


@task("planner.house.normalize", version="image-sha256-abc")
def normalize(job_id: str) -> dict[str, int]:
    ctx = get_task_context()
    rows = ctx.read_artifact("acquired")
    normalized = []
    for row in rows:
        ctx.raise_if_cancelled()
        normalized.append({**row, "normalized": True})
    ctx.write_artifact("normalized", normalized)
    return {"row_count": len(normalized)}
```

The task preserves the Python signature and return type. Declaration requires a
version and changes no global registry. Workers select tasks explicitly, reject
duplicate references and never import a module named by a server assignment.

```bash
AIWATCHER_URL=http://localhost:8080 \
AIWATCHER_TOKEN="$PLANNER_WORKER_TOKEN" \
aiwatcher-worker --queue planner-import --task planner_tasks:normalize
```

`python -m aiwatcher_sdk.worker` is the same entry point. Both `--task` and
`--queue` can be repeated. The server token is configured as
`planner[planner-import]=<secret>` in `AIWATCHER_AUTH_INGEST_TOKENS`.

To embed it in a process:

```python
from aiwatcher_sdk.worker import Worker
from planner_tasks import normalize

with Worker("http://localhost:8080", queues=["planner-import"], tasks=[normalize]) as worker:
    worker.run()
```

`run_once()` performs at most one assignment and returns `False` on an idle
204. `stop()` prevents the next claim; it drains the running function. The CLI
uses this behavior for SIGINT/SIGTERM. Every worker receives its own explicit
`tasks=[...]` list. Calling a task directly executes its function without
managed context; pure tasks therefore need no running server in local tests.

## Inputs, outputs and failure

- The worker merges run `parameters` with step `params` and binds them as
  keyword arguments to the function signature; step values win and Python defaults
  apply. Missing and unexpected arguments fail validation. Type annotations are
  preserved for static checking; this version does not coerce or validate value
  types at runtime. Original dictionaries remain on `ctx.assignment`.
- `get_task_context()` returns the current invocation context. It is reset after
  success or failure and raises outside managed execution. Task-created threads
  must propagate it explicitly with `contextvars.copy_context()`.
- Parent results are explicit artifacts, read by name through the API. The
  current server supports JSON rows, so `read_artifact` returns a list and
  `write_artifact` accepts a sequence of JSON objects. Returned artifact references
  are included automatically in the completion report. Blob/path upload is not
  part of this protocol yet.
- A function's return value is JSON control metadata, limited to 64 KiB of
  UTF-8 JSON. It is not automatically passed to downstream functions. This first
  worker reports `cacheable=False`.
- A background thread heartbeats at half the remaining lease advertised by
  the assignment. The server's 204 renewal keeps that cadence; a 409 marks the
  context cancelled. Lease loss discards the result and permits the next claim.
- Timeouts and cancellation are cooperative. `ctx.raise_if_cancelled()` and
  artifact operations check them. Code that ignores these checks can continue
  running, but a result after its local timeout is not reported as success.
- Raise `TaskError(message, classification="transient")` to classify a retryable
  failure. Classifications match the server: `validation`, `user_code`,
  `transient`, `timeout`, `infrastructure`, `policy`. Ordinary exceptions become
  `user_code`; process interrupts are propagated. The server decides the retry.
- `ctx.context_id` is the server's `execution/step/attempt` idempotency key.
  Retaking the same attempt preserves it; a new attempt increments the last
  component. Tasks must deduplicate their own side effects, including across
  attempts where the domain operation requires that.
- `ctx.client` is the telemetry client, and `ctx.run_id` / `ctx.workflow_run_id`
  carry the execution ID. `ctx.tracer` is ready for agentic hooks and `ctx.run`
  opens child agent/LLM scopes under the server-supplied step span. The worker
  emits no duplicate run or step lifecycle events.

Worker API calls raise `WorkerError`. Claims use one HTTP attempt. When an
assignment advertises `report_idempotent`, a result report can be delivered up
to three times: Rust acknowledges the matching outcome from durable history,
even after retiring its lease. Different outputs/result or failure class/message
receive `409 worker_report_conflict`; queue authorization still applies.
Diagnostics and cache hints are not changed by redelivery. Older servers do not
advertise this capability, so their reports retain the single-delivery policy.
Exhausted or malformed replies never become a contradictory task-failure report.
The server recovers an unreported attempt after its lease expires.

## Protocol and model boundaries

`aiwatcher_sdk.task` and `aiwatcher_sdk.workflow` can be imported without loading
HTTP clients or worker execution. The earlier `aiwatcher_sdk.worker.task`
imports remain supported. `TaskError` also lives in the transport-independent
`aiwatcher_sdk.task_errors` module and is re-exported by `worker`.

Assignments are validated before user code runs. `ArtifactRef` requires `name`,
`uri` and `digest`; optional fields and JSON object rows are checked at the HTTP
boundary. Completion and failure reports have separate typed models. Invalid
server data raises `WorkerError`, including a malformed settlement reply; the
worker does not retry a malformed settlement or reclassify it as a task failure.
Assignments also list required output names. Omitting one becomes a validation
failure; Rust independently verifies declarations, output kinds and unique names
before accepting success.

`TaskContext` depends on the small `AttemptAPI` protocol. `HttpAttemptAPI` owns
URL construction and wire validation. The context accepts monotonic and wall
clock sources so deadline tests do not depend on sleeping or CI scheduling.

## Migration boundary

The server registers and compiles Python workflow definitions, and Runtime can
start them through its public API. `just test-worker-runtime` exercises worker
and server restart against PostgreSQL. Planner's four-stage parity gate remains
the next integration step before switching orchestration and removing Flyte.

`aiwatcher-worker run-attempt --ref execution/step/attempt --queue imports
--task module:function` executes exactly one attempt. An unavailable target
fails without taking neighbouring work. Queue scope, pinned code and lease
checks remain mandatory. The application-level equivalent uses its Runtime
factory: `aiwatcher-runtime --factory module:build_runtime --pool imports
--attempt execution/step/attempt`. Exit 0 means success; a reported task failure
exits 1. Container jobs, a Kubernetes operator and hosted deciders remain later
phases; `just dev` already starts a worker.

Validation: `just sdk-check` covers the complete SDK. `tests/test_worker.py`
validates requests and responses against the checked-in OpenAPI schemas while
using `httpx.MockTransport` to exercise the current wire protocol, including
artifact handoff, heartbeat, lease loss and ambiguous delivery. It is not a
replacement for the real-server gates: `just test-worker-runtime` checks worker
death, and `just test-worker-protocol` drops a committed reply across a Rust
restart and runs retries in separate Runtime processes on PostgreSQL.
