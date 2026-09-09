# Durable workflow execution from the Python Runtime

The application composes workflows and execution pools in Python. Rust owns the
execution: the graph, state transitions, retry budgets, claims and artifact
lineage. This follows `ai_spirit_agent`'s runtime ownership and event-sourcing
model while keeping one durable scheduler in the platform.

The user-facing pattern also follows Prefect's separation between a registered
workflow and a run: register once, start from code or API, observe it in the UI,
and retry a failed step without redoing completed parents. References:
[Prefect deployments](https://docs.prefect.io/v3/concepts/deployments) and
[task retries](https://docs.prefect.io/v3/how-to-guides/workflows/retries).

## One execution path

1. `Runtime.register()` sends workflow declarations and pool queues to
   `POST /api/v1/workflow-definitions`. The registry saves a content-addressed
   revision before updating the head, using the configured object store.
2. `Runtime.run(workflow, parameters=…, idempotency_key=…)` registers and pins
   that revision, then posts to `/api/v1/executions` with `target.kind=workflow`.
   It returns an `ExecutionHandle`; `wait`, `status`, `history`, `retry`, `pause`,
   `resume` and `cancel` all use the same platform API.
3. The compiler validates the DAG, task versions, queues, artifact references,
   retry budgets and timeouts. Artifact inputs also establish dependency edges.
   The resulting `ExecutionPlan` uses `PythonTask` bindings.
4. `ExecutionHandler` commits the input, decisions, projection, claim rows and
   outbox atomically. Its `decide/evolve` functions remain pure. Every execution
   has one ordered history; retry appends an attempt instead of rewriting one.
5. Workers claim only the queues and task versions they host. The existing Rust
   `Reactor` owns settlement, cache and lease checks. The Python process performs
   the function and reports its result. A worker never imports code named by a
   server request and has no direct credentials to the object store or database.
6. The outbox publishes facts after commit. The existing projector supplies the
   workflow graph, artifacts, live SSE and traces. There is no second live event
   feed or execution list.

The API equivalent of starting from code is:

```http
POST /api/v1/executions
Idempotency-Key: import-house-42
Content-Type: application/json

{"target":{"kind":"workflow","name":"demo.worker-import","revision":"<registered digest>"},"parameters":{"number":7}}
```

Automatic retry uses the authored `RetryPolicy`. `TaskError` with `transient`
uses the availability budget; deterministic `user_code` failures stop and can
be retried explicitly after repair. `handle.retry("persist")` calls
`POST /executions/{id}/steps/persist/commands/retry`. This preserves the pinned
plan and successful upstream artifacts.

## Built-in observability

The server emits `workflow.declared`, execution lifecycle facts and step facts.
Assignments include `workflow_id`, `trace_id` and the parent span of that
specific step attempt. `get_task_context().tracer` is ready for `agentic` hooks;
`ctx.run.agent(...)` is the lower-level SDK API. Both attach children to the
managed execution. They do not emit a second run or step lifecycle.

The Workflows page can start registered workflows with parameters and control
a managed run. Selecting a failed node exposes Retry only when the server's
context says it is allowed. Decision history reads bounded pages from
`GET /executions/{id}/history`; live visualization keeps using
`/workflow-executions/{id}/stream` and its existing resume semantics.

The history endpoint currently pages the response after loading the workflow
stream, just as the handler loads it for replay. Database-side history paging
is a later optimization if long executions make this load expensive.

The workflow fold now respects `execution.*` lifecycle events for managed runs.
A completed child cannot finish the workflow while another step is pending;
a failed attempt awaiting retry does not finish it either. Explicit resume
clears the earlier terminal state, and late child telemetry cannot move the
execution's completion time. External workflows without that lifecycle retain
their existing inference from run and step events.

## Result acknowledgement and one process per attempt

Assignments include required output names and a `report_idempotent` capability.
When supported, the SDK can deliver an identical report up to three times after
transport failures. Rust acknowledges the accepted outputs and control result
(or failure class/message) from the workflow history. It does not re-run the
decider, restore the retired lease or rewrite diagnostics/cache hints. A
different outcome receives `409 worker_report_conflict`; a token outside the
pinned task's queue receives 403. Expired/retained-away history cannot provide
an acknowledgement. This is reliable report delivery, not exactly-once effects
inside application code; those still need domain idempotency.

The claim request may include `attempt: {execution_id, step_id, attempt}`. All
three stores apply that restriction together with queue/task, due time and lease
checks. A missing or busy target never falls back to a neighbouring attempt.

```bash
aiwatcher-runtime --factory worker_workflow:build_runtime \
  --pool local --attempt execution-id/acquire/1
```

This uses the application's existing Runtime setup, executes one attempt and
closes its resources. It does not update the registered definition head. The
lower-level `aiwatcher-worker run-attempt --ref ... --queue ... --task ...` is
also available. A fresh runtime can call `run_attempt(ref, pool=...)` directly.
Process/pod provisioning remains the infrastructure controller's responsibility.

Before accepting success, Rust verifies required output names and kinds,
duplicate names and object existence. The SDK reports omitted outputs as a
validation failure, allowing the run to stop with a useful reason.

`just test-worker-protocol` verifies this against an isolated PostgreSQL database:
commit a result, restart Rust and drop its HTTP reply, acknowledge the redelivery,
then run a failed attempt and its retry in separate Runtime processes. Assertions
cover one accepted completion, CLI exit codes 1/0, the graph and parent spans.

## Running locally and testing recovery

`just dev` starts the server, the panel and one Python demo worker. The memory
workflow store is deliberate in this profile: development runs do not survive
a server restart. `aiwatcher-runtime --factory module:function` registers the
factory's workflows before serving its pools.

For durable execution, use PostgreSQL (`just postgres-up`, `just run-postgres`)
and start the same Python Runtime against it. The file workflow store retains
its single-process guard and refuses plans requiring external workers.

The example is `sdk/python/examples/worker_workflow.py`: two steps exchange row
artifacts, a transient failure triggers retry, and task spans attach to the run.
`just test-worker-runtime` starts an isolated database and Rust process, starts a
workflow through the SDK, kills its first worker, restarts Rust with the same
history, and runs a replacement worker. It waits for the real five-minute lease
instead of editing claims or shortening the production policy, and deletes only
the test database on exit.

Verified on 2026-09-09: the recovery scenario completed with 22 recorded
messages and two graph nodes. A separate browser launch completed with the
supplied parameters; the observation check found three step-attempt spans and
two agent spans, each attached to its own step in one trace. The same assertions
are included in the recovery script. Workspace tests passed (936 Rust tests,
24 ignored; 276 SDK tests; 43 panel tests), along with Clippy, Ruff, mypy,
the panel production build and the OpenAPI freshness check.

## Migration boundary

This delivers the Python definition/compiler/start path of Phase 10 and keeps
event sourcing and observability intrinsic to every run. Hosted agent deciders
(`ExecutionOwner::Worker`, dynamic messages and durable joins) remain Phase 13;
this static compiler does not execute agent decisions in Rust. Container jobs,
cluster autoscaling and Planner's four-stage parity gate remain their separate
plan steps. The Flyte integration is not removed by this change.
