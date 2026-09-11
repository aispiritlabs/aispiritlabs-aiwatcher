# Workflow SDK: runtime composition, execution and scaling

Reviewed 2026-09-08. This revises the Python SDK sketch in
`PIPELINE_ARCHITECTURE.md` §36.3. The migration documents establish the problem;
their example signatures are not a public API specification.

The application-facing hierarchy is **Runtime → Workflow → task implementations**.
Runtime owns setup, shared services, routing, execution pools, local capacity and
shutdown. Workflow is the versioned process, with an independent execution history
per instance. A worker is an implementation detail of an execution pool.

This follows the user's composition pattern in
[`AgenticRuntime`](/Users/mkubaszek/Projects/ai_spirit/ai_spirit_agent/packages/agentic_runtime/src/agentic_runtime/runtime.py):
workflow sequences or factories receive runtime services, registration happens
centrally, and runtime owns cleanup. Its
[`WorkflowRuntime`](/Users/mkubaszek/Projects/ai_spirit/ai_spirit_agent/packages/agentic/src/agentic/workflow/runtime.py)
coordinates workflow handling; infrastructure stays outside individual workflows.
The new SDK adopts that responsibility split without introducing the agent
repository's SQLite/Redis execution path beside the Rust workflow store.

## Workflow durability and the Emmett proposal

The linked [Oskar Dudycz proposal](https://www.architecture-weekly.com/p/workflow-engine-design-proposal-tell)
models workflow instances as message streams: incoming messages are persisted,
state is folded with `evolve`, `decide` produces outgoing messages, and output
processors perform effects. Routing chooses the workflow instance; processing
many independent instances provides the horizontal scaling boundary. This is a
design proposal dated July 2025, not evidence of a feature released in aiwatcher.

Our decision is to preserve this separation. The Rust handler/store already own
atomic inbox, stream and outbox persistence. Runtime configures which workflows
and implementations an application hosts; it does not become a second durable
store. Static graph compilation is one authoring form. A message-driven
`decide/evolve/initial_state` workflow is another form requiring the hosted-decider
protocol; neither should be disguised as the other.

Concurrency has three distinct scopes: serialized decisions within one workflow
instance, independent runnable effects, and independent workflow instances.
`Runtime.scale(pool, concurrency=N)` implemented now controls only local effect
slots. Replica count and global workflow limits belong to later infrastructure
and admission-control support; a thread count must never claim either guarantee.

The implemented entry point is documented in the
[runtime guide](../sdk/python/aiwatcher_sdk/runtime/README.md). It accepts workflow
definitions, validates placement, constructs internal workers, shares services,
changes local capacity and drains on shutdown. Workflow registration, pinned
launches and run handles now use the Rust APIs; see the
[Rust runtime guide](RUST_WORKFLOW_RUNTIME.md). Hosted decision processing
remains a separate server protocol.

## What each system actually separates

| System | Authoring and execution | Useful pattern here | Boundary we keep |
| --- | --- | --- | --- |
| Temporal | Workflow code coordinates activities; workers explicitly register their workflows and activities. Workflow execution can replay history and therefore imposes determinism rules. | Explicit worker capabilities, ordinary typed activity functions, context available separately from function arguments. | Our Rust decider already owns a compiled plan. Adding a workflow decorator does not implement replay of arbitrary Python. |
| Prefect | Decorated flows compose tasks. Direct calls, task-runner submission and background task dispatch are distinct operations. Infrastructure workers provision flow runs; background task workers serve selected tasks. | Familiar Python signatures and explicit execution operations. Separate executing task code from provisioning a process or pod. | Do not call a queue poller an infrastructure backend, or add `.submit()` without a real future and dependency contract. |
| ZenML | Static pipelines compile step invocations into a graph; dynamic pipelines execute their composition at runtime. Steps exchange materialized artifacts; a stack supplies execution infrastructure and storage. | Keep step code independent of placement; give data exchange and lineage an explicit place in the design. | Start with the artifact catalog and runtime bindings already in aiwatcher. A second stack registry is unnecessary. |

Sources: [Temporal SDK architecture and examples](https://github.com/temporalio/sdk-python),
[Prefect task runners](https://docs.prefect.io/v3/concepts/task-runners),
[Prefect infrastructure workers](https://docs.prefect.io/v3/concepts/workers),
[Prefect background task workers](https://docs.prefect.io/v3/how-to-guides/workflows/run-background-tasks),
[ZenML core concepts](https://docs.zenml.io/getting-started/core-concepts),
[ZenML execution](https://docs.zenml.io/concepts/steps_and_pipelines/execution).

The source review also included Temporal's
[`activity.py`](https://raw.githubusercontent.com/temporalio/sdk-python/main/temporalio/activity.py),
Prefect's [`task_worker.py`](https://raw.githubusercontent.com/PrefectHQ/prefect/main/src/prefect/task_worker.py)
and ZenML's [`pipeline_definition.py`](https://raw.githubusercontent.com/zenml-io/zenml/main/src/zenml/pipelines/pipeline_definition.py).
These are moving main branches, not pinned dependency versions; none is added
as a runtime dependency or copied into this repository.

## Changes applied to the initial worker implementation

**A task is a normal typed callable.** The original `(inputs: dict, ctx)`
requirement leaked the transport envelope into every application function.
`Task[P, R]` now preserves positional arguments, keyword arguments, defaults and
return typing for local calls. Managed calls bind a parameter mapping to the
same signature. Missing or unexpected arguments are validation failures before
user code starts. Type annotations support static checking; they do not yet
deserialize dataclasses or enforce runtime value types.

**Registration is explicit.** `@task` declares metadata and returns a callable
`Task`; it mutates no global registry. `Worker(tasks=[acquire, normalize])`
defines the process's capabilities. The CLI selects `module:function` exports.
Importing a module cannot accidentally advertise every task it happens to import.
Duplicate references in a worker are refused; pinned versions remain mandatory.
This follows the explicit registration boundary visible in Temporal's worker
example and Prefect's multi-task `serve(...)` API, without copying either SDK.

**Context is scoped to an invocation.** `get_task_context()` exposes cancellation,
artifacts and correlation through a `ContextVar`. The worker activates it only
around user code and resets it on success or failure. A call outside managed
execution fails clearly. New threads created by task code must propagate the
context explicitly. Temporal's activity module uses the same Python primitive;
Prefect also exposes scoped task contexts through its
[context API](https://docs.prefect.io/v3/api-ref/python/prefect-context).

**Lease renewal is not a progress checkpoint.** The current HTTP heartbeat only
renews ownership. Its background thread proves this process can still renew a
lease; it does not prove a function is making progress. Do not expose
`heartbeat(details)` or advertise resumable checkpoints until the server stores
and returns those details. Local timeout checks remain cooperative, so hung
synchronous code requires process isolation for forced termination.

**A worker executes; the server decides.** Retry policy, cache admission,
dependency ordering and final settlement stay in Rust. Python reports a failure
classification. A network timeout after a report is an ambiguous outcome, not
evidence of user-code failure. Assignments now advertise durable result
acknowledgement: the SDK retries the same report, and Rust compares it with the
accepted outcome in history without appending again. Older servers keep the
single-delivery policy. This does not make application side effects exactly once.

## The task-level API beneath Runtime

```python
from aiwatcher_sdk.worker import task

@task("planner.house.inspect", version="image-sha256-abc")
def inspect_house(job_id: str, floor: int = 0) -> dict[str, object]:
    return {"job_id": job_id, "floor": floor}

local_result = inspect_house("house-1", floor=2)

# Production applications register a Workflow containing this task with Runtime.
# Worker remains available as a low-level embedding/testing interface.
```

The local call executes the function directly. It supplies no durable state,
retry policy or managed context. The managed call uses the same function body
and signature while the server owns the attempt. Code needing artifacts or
cancellation calls `get_task_context()` explicitly.

## Next design steps, in dependency order

1. **Runtime-to-server workflow registration — delivered.** Workflow definitions
   and runtime placement bind into an immutable plan. `Runtime.register()` and
   `Runtime.run()` use the same registry and execution APIs as the panel.
2. **Data contract and codecs.** Define input/output schemas and distinguish
   inline control values from typed artifact references. Use a small explicit
   codec interface; add JSON/dataclass support before framework-specific codecs.
   Automatic result materialization, inspired by ZenML, must preserve names,
   digests, lineage and schema versions already owned by the artifact catalog.
   The current API only transports JSON rows; it is not yet a generic blob store.
3. **Workflow authoring and Rust compilation — delivered for static graphs.**
   `WorkflowStep` declares bindings, dependencies, outputs, retry and timeout;
   Rust validates and compiles them without executing application code. Planner's
   four stages still need adoption and parity verification. A function-shaped
   workflow syntax can come later if its build-time restrictions are explicit.
4. **Run handles — delivered for launch and control.** A client starts a pinned
   definition with an idempotency key and returns a handle for status, history,
   wait, retry, pause, resume and cancellation. Artifact access is still explicit;
   there is no automatic return-value materialization. Durable result
   acknowledgement and claim-by-reference are now implemented, with a Runtime
   factory entry point for one attempt. Pod provisioning is still separate.
5. **Execution profiles.** Run a compiled plan locally through the existing API;
   then add a process/container profile using the same task/data contract.
   Queue, image, resources and secrets belong to deployment configuration.
   No Kubernetes objects in a task function, and no new SDK-owned scheduler.
6. **Dynamic coordination.** Hosted deciders, timers, joins and human input need
   their own durability semantics. Choose those semantics before exposing
   arbitrary branching Python as a resumable workflow. Temporal replay and
   ZenML graph compilation solve different problems; mixing their syntax does
   not provide either guarantee.

The real two-stage PostgreSQL recovery run passed on 2026-09-09, including a
worker kill and Rust restart before lease expiry. Planner's byte-for-byte
artifact parity against its direct path remains the next admission gate.
Planner has since removed Flyte (2026-09-09), and aiwatcher removed its
pipeline engine with AW-4 (2026-09-11).
