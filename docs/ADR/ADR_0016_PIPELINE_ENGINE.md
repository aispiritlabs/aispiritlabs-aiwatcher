# ADR_0016: The orchestrator is read for its inventory and asked to start one entry of it; the graph still comes from the log

- **Status**: accepted
- **Date**: 2026-08-31

## Context

planner's feature/training/inference cycle is four kinds of work — curating a
dataset, training or fine-tuning on it, evaluating the result, serving or batch
scoring with it — and every one of them is registered in Flyte 2 as a launch
plan somebody already wrote. aiwatcher could watch all four happen and could
start none of them. Its Data Curation tab could only run Flow PHP, which is the
right tool for shaping rows the API already serves and the wrong one for a
twenty-minute job that reads an object store and writes a model.

What people actually wanted from that tab was smaller than "write a pipeline":
find the workflow that already does this, and set what, where, which rows and
over what period. That is not a new capability to build — it is a launch plan's
own declared inputs, rendered as a form.

Two things stood in the way, and they are the reason this needed a decision
rather than a route.

**ADR_0012 rejected reading Flyte's API.** It did, and for a reason that has
not changed: the *shape of a graph* must come from the declaration on the log,
because planner runs the same four stages with no Flyte at all whenever
`settings.flyte_enabled` is off, and a graph read from an orchestrator is wrong
in exactly the case somebody is debugging. That argument is about history.
This is about inventory, and the log cannot hold inventory: nothing publishes
an event about a workflow nobody has ever run, and no event carries a
workflow's input interface.

**Starting work is not observing it.** aiwatcher has exactly one route that
reaches outside itself — the rerun — and it is admin-only, configured rather
than addressed, and 501 when unwired. Whatever went in here had to be the same
shape or it would be the weak point of everything ADR_0012 and ADR_0013
established.

## Decision

A second outbound port, `core::engine::WorkflowEngine`, with four methods:
`describe`, `catalog`, `workflow`, `launch`, `execution`. `aiwatcher-pipeline`
implements it over Flyte's `/api/v1/` gateway — the grpc-gateway mapping that
Flyte 2 kept — and the API exposes it under `/api/v1/engine`.

Seven rules carry it.

1. **Inventory, never history.** The catalog and the input interface come from
   the engine. The graph, the executions, the nodes and the messages still come
   from the log's folds. `/api/v1/engine/workflows` and `/api/v1/workflows` are
   deliberately different routes over different sources, because they answer
   different questions: what could I start, and what has run. Merging them
   would produce one list that offers things nothing can run and hides things
   nobody has run yet — and could not say which was which.

2. **The endpoint is configuration.** `AIWATCHER_FLYTE_ENDPOINT`, never an
   event and never a request body. `LaunchBody` is `deny_unknown_fields`, so an
   attempt to supply one is a 400 rather than a field that is ignored and reads
   as accepted. The reasoning is ADR_0012's, unchanged: aiwatcher runs inside
   the cluster, so a caller-supplied URL is a request to reach that cluster's
   network on the caller's behalf.

3. **A launch needs `admin`.** The second route in this API that does, and the
   last one that should. An ingest token is capped at editor precisely so that
   a leaked agent environment cannot start a training run.

4. **Inputs are bound to the engine's declared types, read at launch time.**
   Not to the shape the caller was rendering: a panel open since before a
   redeploy is showing an interface that no longer exists. An input the entity
   does not declare is **refused**, not dropped — an orchestrator that ignores
   an unknown field turns a typo in a filter into a run over everything. A
   blank optional input is omitted so the launch plan's own default survives.

5. **A launch always pins a version.** A reference with no version resolves to
   the newest registered one and sends that. An execution recorded against
   "whatever was current" is not something anybody can repeat.

6. **The join is a `workflow_run_id` aiwatcher mints.** It goes back to the
   caller in the acknowledgement, onto the Flyte execution as the
   `aiwatcher-workflow-run-id` label, and into a declared input by that name if
   the entity asked for one. The panel subscribes to
   `/api/v1/workflow-executions/{id}/stream` with it immediately — before
   anything has published, which is the interesting part of a launch's first
   thirty seconds. A producer that never publishes under it simply leaves that
   view empty, which is the honest picture.

7. **One adapter serves both outbound ports.** `AIWATCHER_WORKFLOW_RUNNER=engine`
   makes the Workflows tab's rerun a launch of the launch plan with that
   workflow's name. A deployment whose orchestrator is Flyte configures one
   endpoint rather than two.

Unconfigured, every engine route answers `501 engine_disabled` naming
`AIWATCHER_ENGINE` — the prompt registry's pattern, for the prompt registry's
reason.

## Alternatives considered

**gRPC against `flyteidl` directly.** The native transport, and typed. It puts
`prost`, `tonic` and a code generator in a build that uses six messages of a
very large IDL, and the gateway it would replace is a documented, stable HTTP
contract that Flyte 2 kept. The cost of the JSON path is that field names live
in the adapter as strings, which is what the round-trip tests and the
`snake_case`/`camelCase` reader exist for.

**Serving the launchable catalog from `/api/v1/workflows`.** One list, one
picker, no new concept. It also silently changes what that route means, and
the two sets genuinely differ — a deleted launch plan is still in the log's
catalog, and a launch plan registered this morning is in neither. A list that
cannot say which of the two a row is would be worse than two lists.

**Letting `workflow.declared` carry a launch endpoint.** Ergonomic: the
producer knows where its orchestrator is. Also a request-forgery primitive
posted by anything that can reach ingest — `169.254.169.254` and every
unauthenticated admin port in the namespace included. Refused for the same
reason ADR_0012 refused it for reruns.

**Editor rather than admin for launching.** Data Curation is an editor's page
everywhere else on it, so the split reads oddly. It stays admin because an
ingest token is an editor, and an ingest token sits in an agent's environment.
The panel hides the button and says which role is needed rather than letting
the server refuse it after a round trip.

**Storing launch templates beside curation recipes.** A saved "this workflow
with these inputs" would be a natural sibling of a saved Flow script. Deferred
rather than rejected: today a launch is fully described by the URL of the page
that made it — the selection and the window are both in the search params — so
a link is the template. It becomes worth building when somebody wants a launch
that is scheduled or shared rather than sent.

**Recording each launch on the event log.** Tempting, because it would answer
"who started what" after the fact, and the log is where facts live here. It
would also make aiwatcher a producer on its own log, and the event catalog is
the SDK's contract with every agent that publishes. The record today is the
tracing log line, which names the workflow, the version, the execution and the
caller. See what would make this wrong.

**Polling the engine to fill in a run's status.** The engine knows an execution
failed; the read model only knows the events stopped. Rejected because the
projector must never decide a run has died (see the guardrail), and because the
two answers are worth *disagreeing*: an execution the engine calls `succeeded`
that published no events is a producer nobody instrumented, and a status
column that quietly took the engine's word would hide exactly that. The engine
phase is shown on the launch acknowledgement, where it is clearly a second
opinion.

## How the rules above are checked

Three suites, and the split matters because the first two both pass while the
feature is broken.

`aiwatcher-pipeline/tests/admin.rs` drives the adapter against a stand-in
flyteadmin on a loopback socket, with no aiwatcher around it: the version
pinning, the literal binding, the refusals, the token that expires between two
calls. `aiwatcher-api/tests/http.rs` drives the routes against a stub engine,
with no HTTP to Flyte: the 501, the role check, the minted id, the body that
may not name an endpoint.

Everything that actually goes wrong lives between them — a config field read
into something nothing wires, a rerun reaching a 501 the engine would have
served, a correlation id minted by the API and dropped by the adapter — so
`aiwatcher-server/tests/engine_end_to_end.rs` builds an instance through
`wiring::build`, serves it on a socket, points it at a stand-in control plane on
another, and asserts only on what went over the wire in each direction. Rule 6
in particular has no smaller check: it launches, publishes stage events under
the id that came back, and reads the joined execution out of the workflow
route.

## Consequences

aiwatcher gains an outbound dependency on somebody else's control plane. It is
allowed to be absent, to be down, and to be slow: nothing on start-up contacts
it, every failure classifies as retryable or not, and the whole area degrades
to a 501 the panel explains.

The catalog costs one request per row — the deduplicated name listing, then the
newest version of each name — so the page is capped at 100 and defaults to 20.
Listing every launch plan in one call would return every *version* of every
launch plan, which is a picker showing one workflow fifty-seven times.

The stage hint (`curation | training | evaluation | inference`) is a guess from
the entity's name, and it is named `stage_hint` everywhere it is rendered.
Nothing but presentation may depend on it; the cost of being wrong is a filter
somebody switches off.

The panel's form can be stale, and deliberately so — binding re-reads the
interface, so a stale form fails with the engine's own message rather than
sending a literal typed against an interface that no longer exists.

**What would make this wrong.** Three observations, each with a different fix.

If Flyte stops making the launch plan the launchable unit — Flyte 2's
tasks-calling-tasks model has no `@workflow`, and a deployment may register
tasks with no launch plan at all — the catalog goes empty against a cluster
that is plainly full. `EntityKind` already has a `Task` variant and the
reference format carries the kind; adding that path is a change inside
`aiwatcher-pipeline`.

If people start asking who launched what last week, the tracing line is the
wrong place for the answer and this needs a record. That record belongs on the
log with everything else, which means an event type and an SDK release — the
decision deferred above, reopened by a question nothing can currently answer.

If a launch's inputs routinely need to be saved and re-sent rather than typed,
the URL has stopped being a good enough template and the object store is where
that goes, beside the curation recipes it would resemble.
