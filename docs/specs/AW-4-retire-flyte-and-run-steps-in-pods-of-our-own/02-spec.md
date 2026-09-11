---
id: AW-4
step: spec
status: doing
branch: main
repo: aiwatcher
created: 2026-09-11
updated: 2026-09-11
tags: [spec/AW-4, step/spec, branch/main, status/doing]
---

`#spec/AW-4` · `#step/spec` · `#branch/main` · repo `aiwatcher`

# ② Spec — AW-4

## Proposal

**Intent.** Flyte leaves aiwatcher, and a step that asks for one runs in a pod
aiwatcher starts for it. What this buys is planner's per-stage isolation back —
an out-of-memory stage fails that stage, not the import — without an
orchestrator beside the engine, and the first step towards what Flyte offered
that aiwatcher lacks: resources, an image and secrets per step, a cancel that
reaches running work, and the step's log.

**In scope.** *Part 1, removal:* the `aiwatcher-pipeline` crate,
`core::engine`, the `/api/v1/engine` routes, the panel's launcher,
`ExecutionOwner::Engine` and `RuntimeBinding::ExternalWorkflow`, the settings,
the chart's `engine` values, the SDK's Flyte integration, and the documents
that describe any of it — ADR_0016 superseded, not deleted. *Part 2, pods:*
a `ContainerJob` binding a step opts into by naming a template; the work role
starting one Kubernetes Job per attempt; templates and their image allowlists as
chart values; resources from the template; the pod's log kept; a cancel that
deletes the pod.

**Out of scope.** GPU requests and Kueue (the owner: later, once training
exists); map tasks, dynamic graphs and conditionals; closing the cooperative
cancel and the missing logs for a long-lived worker (pod-only here); planner's
own change to name a template on its four stages, which is planner's commit.

**Approach.** Part 1 first, in an order where every commit compiles and
`just check` stays where it was: what no other session holds now (the SDK
integration, the server's wiring and settings, the crate, the chart), then the
API routes, the core port, the contract and the panel once the panel rebuild
has dropped the launcher, then the execution variants once AW-3 has committed
`plan.rs` and its neighbours. Part 2 behind a `kube` feature of
`aiwatcher-server`, with an ADR of its own before the first line of it.

**Success criteria:**
- [ ] No build carries Flyte: no `aiwatcher-pipeline` in `cargo metadata`, no
      `/api/v1/engine` in the contract, no launcher in the panel
- [ ] A deployment that still names the engine is refused at start, by name
- [ ] planner's four stages run as four pods on a local cluster, and the review
      they produce is byte-identical to the one-pod path
- [ ] An out-of-memory stage fails its own attempt and nothing else, and the
      retry budget decides what happens next
- [ ] A cancel stops a pod that is running
- [ ] An image outside a template's allowlist is refused by name before anything
      runs

## Spec (delta)

### REMOVED Requirements

#### Requirement: the pipeline engine (ADR_0016)
(Reason: its one user moved off Flyte. planner runs its house import through
aiwatcher's own worker boundary on every profile but `local`, and refuses
`flyte` as an orchestrator. Nothing registered a launch plan, so the engine
catalogue was empty against planner's cluster; the routes, the adapter and the
literal encoder describe a system nothing here uses.)

#### Requirement: engine-owned executions (Phase 9)
(Reason: withdrawn with the engine. `ExecutionOwner::Engine` was never
constructed, and `RuntimeBinding::ExternalWorkflow` was declared and never
produced.)

### ADDED Requirements

#### Requirement: a removed engine is refused by name
The server SHALL refuse to start when `AIWATCHER_ENGINE` names anything but
`none`, or when `AIWATCHER_WORKFLOW_RUNNER` is `engine` or `flyte`, and the
refusal SHALL say the engine was removed. The chart SHALL fail to render a
release whose values still set `engine`. A variable that is merely present and
empty SHALL NOT be refused.

##### Scenario: an operator who configured Flyte finds out at start
- GIVEN `AIWATCHER_ENGINE=flyte`
- WHEN the server starts
- THEN it exits with an error naming `AIWATCHER_ENGINE` and saying the Flyte
  engine was removed — never a server that starts without the tab it had

##### Scenario: a rerun runner of the removed kind
- GIVEN `AIWATCHER_WORKFLOW_RUNNER=flyte`
- WHEN the server starts
- THEN it is refused naming the variable, and `http` is still accepted

##### Scenario: the chart says the same
- GIVEN a values file with `engine.kind: flyte`
- WHEN the chart is rendered
- THEN rendering fails naming `engine`

#### Requirement: no build carries Flyte
No crate, route, schema, panel component, SDK module or chart template SHALL
name the Flyte engine once Part 1 is complete. ADR_0016 SHALL remain, marked
superseded, with a pointer to this spec.

##### Scenario: the dependency graph
- GIVEN the workspace after Part 1
- WHEN `cargo metadata` lists its packages
- THEN `aiwatcher-pipeline` is not among them, and `Cargo.lock` holds nothing
  only it needed

##### Scenario: the contract
- GIVEN `contracts/openapi.json` after Part 1
- WHEN it is searched
- THEN it has no `/api/v1/engine` path, no `Engine*` schema and no
  `external_workflow` value, and `just openapi-check` passes

#### Requirement: an unknown owner is not an engine
Reading an execution's owner SHALL keep text it does not recognise as unknown —
shown, never scheduled, never decided — rather than treating it as an engine
nobody configured.

##### Scenario: a record from somewhere else
- GIVEN a stored run whose owner reads `engine:flyte`
- WHEN the run is read
- THEN its owner is unknown, it is listed, and nothing claims or decides it

#### Requirement: a step may ask for a pod of its own
A workflow step MAY name a pod template. For an attempt of such a step the work
role SHALL create one Kubernetes Job from that template, with the step's image,
running the worker for that attempt and no other; the worker SHALL claim,
heartbeat and report through the protocol every worker uses. A step that names
no template SHALL be claimed by long-lived workers as today.

##### Scenario: a step that asked for a pod
- GIVEN a registered workflow whose `analyze` step names the template `import`
- WHEN its attempt is due
- THEN one Job exists for that attempt, its pod runs `analyze` alone, and the
  step completes from the pod's report

##### Scenario: a step that did not ask
- GIVEN the same workflow's `persist` step, naming no template
- WHEN its attempt is due
- THEN a long-lived worker on its queue claims it, and no Job is created

##### Scenario: a process that cannot start pods claims none
- GIVEN a work role with no cluster configured
- WHEN an attempt of a step naming a template is due
- THEN that process never claims it — the attempt waits for a process that can,
  and the panel says which binding it is waiting for

#### Requirement: only an allowed image runs, from a known template
A step naming a template SHALL name an image in that template's allowlist.
Registration SHALL refuse a step whose template does not exist or whose image
the template does not allow, naming both, before anything runs. Templates and
allowlists SHALL come from configuration — chart values — never from the
definition. A step SHALL NOT name a namespace, a node, a service account or a
secret.

##### Scenario: an image nobody allowed
- GIVEN the template `import` allowing `ghcr.io/planner/import`
- WHEN a workflow is registered with a step naming `import` and
  `docker.io/somebody/else`
- THEN registration is refused naming the image and the template, and nothing
  is stored

##### Scenario: a template nobody configured
- GIVEN no template called `train`
- WHEN a step naming it is registered
- THEN registration is refused naming `train`

##### Scenario: a definition that names its own cluster
- GIVEN a step carrying a `namespace` field
- WHEN it is registered
- THEN it is refused as an unknown field, not stored with the field ignored

#### Requirement: resources come from the template
A pod's requests and limits SHALL come from its template. A step MAY set
CPU and memory within ceilings the template declares, and registration SHALL
refuse a step asking above one.

##### Scenario: a stage that needs more memory
- GIVEN the template `import` with a memory ceiling of 8Gi
- WHEN `analyze` asks for 6Gi and `acquire` asks for nothing
- THEN `analyze`'s pod requests 6Gi and `acquire`'s the template's default

##### Scenario: a stage that asks for too much
- GIVEN the same ceiling
- WHEN a step asks for 16Gi
- THEN registration is refused naming the step, the amount and the ceiling

#### Requirement: the engine owns a pod's retries
A Job SHALL be created with `backoffLimit: 0` and `restartPolicy: Never`. A pod
that ends without reporting SHALL end its attempt, classified by how it ended,
and the retry budget SHALL decide what follows — never Kubernetes.

##### Scenario: one stage runs out of memory
- GIVEN four stages, each in its own pod, and `analyze` exceeding its limit
- WHEN its pod is killed
- THEN `analyze`'s attempt fails as `infrastructure`, the budget schedules its
  next attempt, and the other stages' results stand

##### Scenario: a pod that vanished
- GIVEN a running attempt whose pod is deleted from outside
- WHEN its lease lapses
- THEN the attempt is retried under the budget, and no Job for it is left
  behind

#### Requirement: a cancel reaches a running pod
Cancelling a run SHALL delete the Jobs of its running attempts, and those
attempts SHALL end cancelled rather than running to completion.

##### Scenario: cancel during a long stage
- GIVEN `analyze` running in its pod
- WHEN the run is cancelled
- THEN the Job is deleted, the attempt ends cancelled, and no later step starts

#### Requirement: a pod's log is kept
When a pod ends, its log SHALL be stored as the step's `Log` artifact, bounded
to its last part, and the step's view SHALL link it.

##### Scenario: reading why a stage failed
- GIVEN a pod that printed a traceback and exited 1
- WHEN its attempt has ended
- THEN the attempt has a `Log` artifact holding the traceback

### MODIFIED Requirements

#### Requirement: Phase 12's gate
Phase 12 SHALL be built on the owner's decision. (Previously: behind a gate
asking for a concrete need for one pod per stage, which §39.4 found planner did
not have. The owner reopened it on 2026-09-11; the need it names is isolation,
which one pod for four stages does not give.)

##### Scenario: the plan says so
- GIVEN `docs/PIPELINE_ARCHITECTURE.md` after this spec
- WHEN Phase 12 is read
- THEN it is reopened by AW-4 and Phase 9 is withdrawn

## Settled

- **A pod is opt-in per step** (the owner). A step names a template or does not;
  a workflow can mix both, and planner moves its four stages and nothing else.
- **Templates are chart values, each with its own allowlist** (the owner),
  §42 decision 13 as written: aiwatcher's chart holds them because the work role
  reads them, and planner supplies its own under a documented key.
- **GPU later** (the owner). Nothing in the first template set asks for one.
- **The removal starts on what no other session holds** (the owner). The panel,
  the contract and the execution variants follow the commits that hold them.
- **Pods are a feature, not a crate.** A `kube` feature of `aiwatcher-server`,
  where executors live — the precedent `laser` and `postgres` set, and 43.1's
  finding that the argument for a crate was untrue. §27's `aiwatcher-kube`
  crate is corrected with this spec.
- **A step never names a secret.** Secrets come from the template; a definition
  that could name one could name any secret the work role can read.
- **ADR_0016 is superseded, not deleted**, and the pods get an ADR before any
  code: which templates exist and who may name an image is expensive to reverse.

## Still open, for the job phase

- **How a pod claims exactly its attempt.** A claim filtered by attempt key
  under the template's queue-scoped token, or a one-attempt token minted when
  the Job is created — the second is tighter and is a new credential shape.
- **How a pod's end becomes a report** when it dies before reporting: watching
  the Job from the work role, or letting the lease lapse and reading the pod's
  status then. The second needs no watch and is slower by one lease.
- **The log bound**, and whether it is the last N kilobytes or the last N lines.

## Log
- 2026-09-11 12:35 — spec drafted on `main`: two requirements removed, nine added over twenty scenarios, one modified; the owner's four answers and three design points settled; three questions left to the job
