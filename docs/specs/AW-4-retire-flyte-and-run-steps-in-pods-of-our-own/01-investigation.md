---
id: AW-4
step: investigation
status: doing
branch: main
repo: aiwatcher
created: 2026-09-11
updated: 2026-09-11
tags: [spec/AW-4, step/investigation, branch/main, status/doing]
---

`#spec/AW-4` · `#step/investigation` · `#branch/main` · repo `aiwatcher`

# ① Investigation — AW-4

## Problem

The owner's decision, 2026-09-11: Flyte leaves aiwatcher, and aiwatcher's own
workflow engine starts the pods its steps run in. Two questions follow, and the
owner asked the first one outright: **does aiwatcher, together with what planner
now does, have everything Flyte gave planner** — and what does removing Flyte,
then starting pods, actually touch.

## What planner used Flyte for

Read from planner's history (`7530a19` brought Flyte in on 2026-08-28,
`d6ea5b3` added the `PipelineRunner` port and the aiwatcher adapter, `8fa2eb8`
removed Flyte — 51 files — on 2026-09-09) and from its tree today.

| Flyte capability | planner's use | planner today | Gap |
|---|---|---|---|
| **One pod per stage** | the core of it: `acquire → normalize → analyze → persist`, each in a pod, plus a controller | all four stages in one import-worker pod, as §39.4 decided | **yes** — one stage's OOM now kills all four |
| Resources | one `Resources(cpu=("250m",4), memory=("1Gi","6Gi"))` for every task, no GPU | the worker Deployment carries the same numbers | none for planner; aiwatcher has no per-step resources at all |
| Image | the import service's image for every task | the worker's own image | none for planner; nothing could run a second image |
| Secrets | five `secretKeyRef`s in a hand-written pod template | the worker pod's environment | none for planner |
| Data between stages | JSON strings inline, files on the RWX volume | `context.write_artifact` through the API, files on the same volume | none |
| Retries, caching, launch plans, schedules, map, dynamic, notifications | **not used** (`cache="disable"`, no `retries=`) | aiwatcher's default retry budget; planner's own extraction cache | none |
| Console, history | flyteconsole behind Authentik; run labels copied into logs | the panel's Workflows view, the execution store, `JobRecord` | per-step **logs** |
| Invocation | `flyte.run(...).wait()` from an RQ job | Iggy/Laser job queue (`d566b3c` removed RQ); `AiwatcherRunner` serves the pool in the same process | none |

**For what planner used, yes — except one pod per stage.** Nothing else Flyte
did for planner is missing. The house import runs through aiwatcher on every
non-local profile, `flyte` is a refused value of `importOrchestrator`, and
planner's charts, locks and compose files name no Flyte.

## What aiwatcher has, measured against Flyte in general

| | Capabilities |
|---|---|
| **Yes** | versioned, content-addressed definitions; a pinned code revision per run (`name@version`, claimed only by a worker holding it); retries with separate budgets for code that failed and a runtime that was unreachable; human gates, including a task that parks mid-attempt; lineage per output; pause and resume |
| **Partial** | typed interfaces (outputs are named row tables, parameters are untyped JSON); timeouts (cooperative in a Python task — nothing on the server ends an attempt that hangs until its lease lapses); caching (the key and lookup exist, and the workflow compiler hard-codes `CachePolicy::Never`, `definition/mod.rs:323`); data passing (content-addressed, proxied, JSON rows only — no weights, no files); fan-out (parallel branches of a static graph, no map); dynamic graphs (hosted mode only); schedules (hourly, daily, weekly, with catch-up, no cron, empty parameters); cancel (cooperative — nothing tells a running worker task, `api/worker.rs`); recovery after a failure (the failed step retries from its pinned inputs, and whether the steps it skipped then run is untested); the UI (graph and controls, no logs) |
| **No** | per-step CPU, memory or GPU; a per-step image; **a pod per step**; secrets injected into a step; task log collection; conditional branches; notifications; launch plans with default inputs; multi-tenancy |

The *no* row is, almost item for item, what a pod of our own would bring or make
cheap: resources, image and secrets come from its template, the pod's logs can
be read back when it ends, and deleting the Job is a cancel that reaches work
already running. Conditionals, notifications, launch plans and tenancy are
separate and nobody has asked for them.

## The Flyte surface in aiwatcher

Self-contained. `aiwatcher-pipeline` has one dependent, the server;
`core::engine` has three, the API, the server and that crate; nothing in the
execution engine ever constructs its engine variants.

| Layer | What | Size | Held by another session? |
|---|---|---|---|
| core | `crates/aiwatcher-core/src/engine.rs` — `WorkflowEngine` and fifteen types around it | 648 lines, 5 tests | no |
| pipeline | `crates/aiwatcher-pipeline` — `FlyteEngine` (also a `WorkflowRunner`, for rerun), the literal encoder, `tests/admin.rs` | ~2 600 lines, 12 tests | no |
| execution | `ExecutionOwner::Engine` (`state.rs:176-203`, never constructed; `parse` turns *any* unknown text into it); `RuntimeBinding::ExternalWorkflow` and its spec and kind (declared, never constructed), four match arms and a postgres fallback | small | **yes** — `plan.rs`, `cache.rs`, `facts.rs`, `context.rs`, `store/postgres/mod.rs` carry AW-3's uncommitted changes |
| api | `engine.rs`, five routes under `/api/v1/engine`, `AppState.engine`, three error variants, the OpenAPI entries, ~10 tests with a `RecordingEngine` | 324 lines + tests | no |
| server | `EngineKind`, `WorkflowRunnerKind::Engine` (accepts `flyte`), eleven `AIWATCHER_FLYTE_*` settings, `build_engine`, `tests/engine_end_to_end.rs` | ~150 lines + 819 of tests | no |
| contract | five paths, twelve schemas, `external_workflow` values, the `engine` tag | — | **yes** — `contracts/openapi.json` has AW-3's diff |
| panel | `shared/components/engine-launcher.tsx` and its two call sites (Experiments, the curation recipe); the generated client's five functions | 587 lines | **yes** — untracked, the panel rebuild's |
| deploy | the Helm `engine:` block, its schema and helpers, a `status.sh` probe row, a `Dockerfile` COPY line | — | no |
| sdk | `aiwatcher_sdk/integrations/flyte.py` and its test, README sections — nothing imports them | 296 lines | no |
| docs | ADR_0016, and mentions in ADR_0012, 0018, 0024, 0025, 0026; `CLAUDE.md` decisions 12, 13, 22, the crate table, the owner row, three guardrails; `INSTALL.md`'s pipeline-engine section; §24, §27, §37–§39 here; README, EXAMPLES, `.env.example` | 89 mentions in the plan alone | `CLAUDE.md`, `INSTALL.md`, ADR_0024, the ADR index and `.env.example` are modified by others, not on Flyte's lines |

`just run-flyte` is already gone; the `flyte_*` variables it read are still in
the justfile, and README still points at it.

## What exists of pods of our own

Nothing in code. No `ContainerJob` binding, no `kube` dependency or feature, no
pod template, no image allowlist (`RuntimeBinding` has eight variants). The
design is §37 and Phase 12: a `ContainerJob` is a `PythonTask` whose worker is
started for it — a Kubernetes Job per attempt from a **named** pod template in
configuration, an image allowlist, the lease held by heartbeat, `backoffLimit: 0`
because the owner of an execution owns its retries. Its foundation is built: the
claim, lease, heartbeat and result protocol, `aiwatcher_sdk.worker`, and the
reactor split in which only `perform` crosses a seam.

## Constraints

- **Two sessions hold files this touches.** AW-3 holds the execution crate's
  plan, cache, facts, context and postgres files and the contract; the panel
  rebuild holds the launcher. Removing the panel, contract and execution parts
  waits for their commits, or is agreed with them first — the generated client
  and `contracts/openapi.json` must change in one commit (`just openapi-check`,
  the panel build).
- **A removed variable must not fail silently.** Configuration ignores unknown
  variables, so a deployment with `AIWATCHER_ENGINE=flyte` would start, lose its
  engine tab and say nothing. AW-2 refused `AGENTIC_TRANSPORT=laser` by name for
  the same reason; `AIWATCHER_ENGINE` and the `flyte` runner kind deserve the
  same for a release, and a Helm values file with `engine:` set should fail to
  render with the reason.
- **`ExecutionOwner::parse` needs a new rule.** Unknown text reads as an engine
  "so a record written by a newer build still reads". No stored row says
  anything but `local` or `worker`, so the variant can go; what unknown text
  becomes is a decision, not a deletion.
- **Pods are an adapter, not a crate.** §27 names a new `aiwatcher-kube` crate.
  This repository's precedent is the other way round — `laser` is a feature of
  `aiwatcher-bus`, and `postgres` and `duckdb` are features of
  `aiwatcher-execution` after 43.1 found the crate argument untrue. A pod
  launcher is an executor, and executors live in the server's `execution/`
  (`CLAUDE.md`, the crate table): a module behind a `kube` feature there.
- **The guardrails already say most of the design.** A process claims only work
  it can perform — the claim filter comes from the executor registry, so a
  process with no cluster configured registers no pod executor. A plan never
  names its executor's address — a step names a template, never a host or a
  namespace. The owner of an execution owns its retries — `backoffLimit: 0`.
  A bucket is never presigned to a process outside the cluster — pods are
  inside it, which makes a presigned path possible and still not required.
- **Local development.** Tilt refuses any non-local context, and a pod executor
  needs a cluster to be tested against; the contract can be a stub API server,
  the way `just test-pipeline` stubbed Flyte's admin.
- **The record.** ADR_0016 is superseded, not deleted, and the pods get an ADR
  of their own, because which templates exist and who may name an image is
  expensive to reverse.

## Options

**A. Remove Flyte and stop.** The removal is mechanical and every commit can
compile. Planner keeps one worker pod, as §39.4 accepted. Small — and it leaves
the owner's second sentence undone.

**B. Remove Flyte, then build `ContainerJob`** *(recommended)*. Two deliveries
under this spec. First the removal, ordered so every commit compiles: the SDK
integration, the server wiring and deploy values, the crate, the API routes and
the contract, the core port, the execution variants, the docs. Then pods: the
work role, for a `ContainerJob` attempt it claims, creates a Job from a named
template with the step's image (allowlisted) and resource overrides; the pod
runs `aiwatcher_sdk.worker` scoped to exactly that attempt, heartbeats and
reports as any worker does; a cancel deletes the Job; the pod's log is read back
into the step's `Log` artifact when it ends. planner's house import is the
first user — four pods, the isolation back — and GPU training the second, which
needs the image and the GPU request and nothing else new.

**C. Keep Flyte for map tasks and GPU work**, as §39.5 argued. Withdrawn by the
owner.

## Recommendation

Option B, removal first. The removal can start today on everything no other
session holds — the SDK integration, the server, the crate, the API and core
behind a commit that regenerates the contract once AW-3 has committed its diff
— and it closes Phase 9 for good. The pods are the part to design before
building: a spec with scenarios for a claimed attempt becoming a Job, a lost pod
becoming a lease that lapses, a cancel reaching a running pod, and an image
nobody allowed being refused by name.

## Open questions for the owner

1. **Granularity.** Every step of a workflow bound to `ContainerJob` in its own
   pod, or opt-in per step, with the rest still run by a long-lived worker?
2. **Templates and images.** Templates as Helm values in aiwatcher's chart, which
   §42 decision 13 already says, with planner supplying its own under a
   documented key — and the allowlist per template or global?
3. **GPU now or later.** A GPU request and a Kueue label in the first template
   set, or after planner's four pods run?
4. **What the pods do not change.** Should a cancel that deletes a pod, and logs
   read back from one, stay pod-only — or is the same cooperative-cancel gap in a
   long-lived worker worth closing here too?

**Answered by the owner, 2026-09-11:** a pod is **opt-in per step** — a step
that names a template runs in a Job of its own, and every other step stays
with a long-lived worker; templates are **Helm values in aiwatcher's chart**,
planner supplying its own under a documented key, each with **its own image
allowlist**; **GPU later**, once training exists to need it; and the removal
**starts now** on what no other session holds, the panel, the contract and the
execution variants following their commits. Question 4 stays with the spec:
pod-only unless the spec finds the worker's half is cheap.

Also found, for their owners: §38 still describes planner's worker as RQ and
`importOrchestrator` as `direct | flyte | aiwatcher`; planner's own
`docs/floor-plan-model-kickoff.md` and `docs/optimization-ml-plan.md` still
describe training in Flyte, and `deploy/.env.k3s` keeps three `FLYTE_*`
variables nothing reads.

## Log
- 2026-09-11 11:55 — investigation written: planner lost only one pod per stage; aiwatcher's *no* column is what a pod of our own brings; the Flyte surface is self-contained, three layers of it held by other sessions; recommend removal, then `ContainerJob`
- 2026-09-11 12:10 — owner's answers: a pod is opt-in per step; templates in the chart with a per-template image allowlist; GPU later; removal starts on what no other session holds
