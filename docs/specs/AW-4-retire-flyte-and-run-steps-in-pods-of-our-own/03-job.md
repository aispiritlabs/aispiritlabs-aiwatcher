---
id: AW-4
step: job
status: doing
branch: main
repo: aiwatcher
created: 2026-09-11
updated: 2026-09-11
tags: [spec/AW-4, step/job, branch/main, status/doing]
---

`#spec/AW-4` · `#step/job` · `#branch/main` · repo `aiwatcher`

# ③ Job — AW-4

## Technical approach

Part 1 is three passes, cut where another session's uncommitted work sits, so
that nothing here waits on anybody and nothing of theirs is swept into a commit.

- **1a, now.** Everything no other session holds: the server stops building an
  engine and refuses to be asked for one; the crate goes; the chart, the
  justfile, the environment example, the SDK integration and the documents
  that name the crate or its recipes follow. The API keeps its engine routes for
  this pass, and with no engine wired they answer 501 — the panel's launcher,
  which the rebuild holds, keeps compiling against a contract that has not
  moved.
- **1b, after the panel rebuild drops the launcher.** The five routes, their
  error variants and `AppState.engine`, `core::engine`, the contract and the
  generated client in one commit, and the launcher's two call sites.
- **1c, after AW-3 commits `plan.rs` and its neighbours.** `ExecutionOwner::Engine`
  becomes an unknown owner; `RuntimeBinding::ExternalWorkflow`, its spec and its
  kind go; the contract is regenerated again. Then the documents that describe
  the engine as a design rather than as a crate: `CLAUDE.md`'s decisions and
  guardrails, `INSTALL.md`, §24, §37–§39, the ADR cross-references.

Part 2 starts with its ADR and is planned in full once Part 1 is through; the
outline below is what the spec already fixes.

## Decisions

- **A refusal names the removal, in a variant of its own.** `ConfigError::Removed`
  says what was set, what it switched on, that it was removed, and what to do
  instead. The plan was to reuse `Invalid`, whose message reads *"…which is not
  a valid {expected}"* — so the refusal came out as "not a valid none — the
  engine was removed", which is a sentence nobody should have to parse at start
  time. `none`, `off` and `disabled` keep parsing, so a deployment that spelled
  the default out is not refused.
- **The `AIWATCHER_FLYTE_*` variables are ignored, not refused.** Only the switch
  that turned the engine on can mislead anybody; the rest configured nothing
  unless it was set.
- **The chart refuses `engine` with `fail`, not with the schema.** The schema
  has no `additionalProperties: false` at its root, and adding one would turn
  every stray key in every installation into a refusal. One guard, in the
  template that already carries the release's other guards.
- **`engine_end_to_end.rs` goes with the crate.** It drives the API through a
  real `FlyteEngine` against a stand-in admin; the API's own `RecordingEngine`
  tests stay until 1b removes the routes they cover.
- **An unknown owner is `Unknown(String)`** (1c): kept for display, matched by
  nothing that schedules or decides. No stored row holds anything but `local`
  or `worker`, so this changes no read of existing data. It keeps the text
  verbatim, so `engine:flyte` still reads `engine:flyte`. Nothing needed to
  change beyond the type: no scheduling or deciding code ever read the owner.
- **A runtime kind the postgres adapter cannot name is refused** (1c, decided
  while building). It used to fall back to `external_workflow`, "the binding
  this process never runs". With that binding gone there is no honest
  fallback. The kind is now decoded through `RuntimeKind`'s own serde spelling,
  and an unknown one is `StoreError::Encoding`, which is what the `memory`,
  `file` and `duckdb` adapters already do when they decode. A claim never
  meets one, because its SQL selects only the claimant's kinds. What does meet
  one is a lookup by key.
- **The Experiments page keeps only its Comparison placeholder** (1b). The
  stage buttons and the period control fed nothing but the launcher, and a
  control that changes nothing is a button that does not work. The `stage`,
  `engine` and `engineFind` search parameters go with them. Nothing linked in
  with them.
- **Part 2's design is [ADR_0029](../../ADR/ADR_0029_POD_PER_STEP.md)** (2.1),
  and it answers the spec's three open questions:
  - **The pod claims its attempt by key**, under its template's queue-scoped
    token. A key-only rule in `ClaimFilter` keeps long-lived workers off a pod's
    row, and a one-attempt credential is deferred as the stricter mode.
  - **The lease decides how an attempt ended**, and the Job's watch explains it
    sooner, with the pod's own reason.
  - **The log is the last 256 KiB**, in bytes rather than lines, recorded in
    the catalog against the attempt.

  Two points move from §37: an image matches a repository exactly rather than
  by prefix, and the launcher creates Jobs without claiming anything — one per
  attempt, under a name derived from its key.
- **The templates file is a JSON map from name to template** (2.2), and each
  template has these fields:
  - `images`;
  - `resources`, holding `requests`, `limits` and a required `max`;
  - `command`;
  - `start_allowance_seconds`, 300 by default;
  - `pod`.

  Unknown fields are refused, every problem is reported at once, and a malformed
  file fails the start. A template's name is a DNS label, because the Jobs it
  starts will carry it.
- **What aiwatcher fills in is refused in a template, by name** (2.2):
  - on the pod: `restartPolicy`, `activeDeadlineSeconds`, and the Job's
    `backoffLimit` and `ttlSecondsAfterFinished`;
  - on the container: `image`, `resources`, `command`, `args`, and the three
    variables aiwatcher sets — `AIWATCHER_ATTEMPT`, `AIWATCHER_URL` and
    `AIWATCHER_WORKER_NAME`.

  `command` and `args` go further than the ADR's list: the template has a
  `command` of its own, and two places for one argv are two answers.
  `pod.containers` is absent or holds exactly one container.
- **Docker's defaults are written out before an image is compared** (2.2):
  `python:3.13` is `docker.io/library/python`, so an entry and a step that spell
  one image two ways agree. The match is still exact, by repository.
- **Until 2.3, a `container_job` row carries no queue** (2.2), so nothing claims
  it and it waits. Writing its queue before the key-only rule exists would let
  any worker holding that queue take a pod's attempt and run it outside the
  pod, which is the hole the ADR names.
- **The work and combined roles refuse templates in every build until `kube`
  exists** (2.2, `ConfigError::Unusable`). 2.3 makes the refusal depend on the
  feature. The serve role reads templates in any build.
- **A pod's step has a cache key only with an image pinned by digest** (2.2). A
  tag names whatever was pushed under it last. The workflow compiler sets
  `never` anyway, so this is only `cache_key`'s own answer.
- **The SDK's `WorkflowStep` takes `pod=PodRequest(…)`** (2.2), and sends it only
  when it is set, so a step without one registers the same revision.

## Tasks

### Part 1a — now
- [x] 1.1 Config: drop `EngineKind` and the `flyte_*` settings, their parsing and
      validation; refuse `AIWATCHER_ENGINE` other than `none` and the runner kinds
      `engine`/`flyte` by name. Tests for both refusals. — *a removed engine is
      refused by name*
- [x] 1.2 Wiring: drop `build_engine` and the runner's engine arm;
      `AppState.engine` is `None`. — *no build carries Flyte*
- [x] 1.3 Delete `crates/aiwatcher-pipeline`, its workspace entries, the server's
      dependency, the `Dockerfile` COPY line and `tests/engine_end_to_end.rs`;
      let `Cargo.lock` follow. — *no build carries Flyte*
- [x] 1.4 Chart: drop the `engine` values, schema entry and helper block and the
      `status.sh` row; a values file that still sets `engine` fails to render
      naming it. — *a removed engine is refused by name*
- [x] 1.5 SDK: drop `integrations/flyte.py`, its test and the README sections. —
      *no build carries Flyte*
- [x] 1.6 The justfile's `test-pipeline` and `flyte_*` variables, `.env.example`'s
      engine block, README's recipe row, `CLAUDE.md`'s crate row and commands,
      the workspace header; ADR_0016 marked superseded.
- [x] 1.7 Verify: `just check`, `just sdk-check`, `just chart-check`.

### Part 1b — after the panel rebuild
- [x] 1.8 API routes, error variants, `AppState.engine`, `core::engine`; the
      contract and the generated client regenerated; the launcher and its call
      sites removed. — *no build carries Flyte*

### Part 1c — after AW-3
- [x] 1.9 `ExecutionOwner::Unknown`; `RuntimeBinding::ExternalWorkflow` and its
      spec and kind removed; the contract regenerated. — *an unknown owner is not
      an engine*
- [x] 1.10 The documents that describe the engine as a design. — *no build
      carries Flyte*

### Part 2 — pods (outline)
- [x] 2.1 ADR: templates, allowlists, who may name an image, the `kube` feature —
      [ADR_0029](../../ADR/ADR_0029_POD_PER_STEP.md)
- [x] 2.2 `RuntimeBinding::ContainerJob` and the step's `pod` field; the
      templates file both roles read; registration's refusals — *only an
      allowed image runs*, *resources come from the template*
- [ ] 2.3 The launcher behind `kube`: the store's read of claimable rows by
      runtime, a Job per attempt under a name derived from its key,
      `backoffLimit: 0`, the key-only claim rule; the chart's RBAC, templates
      and network rule, and `kube` in the release image — *a step may ask for a
      pod*, *the engine owns a pod's retries*
- [ ] 2.4 Cancel deletes the Job; the watch ends a dead pod's attempt with its
      reason; the log is read back into the catalog — *a cancel reaches a
      running pod*, *a pod's log is kept*
- [ ] 2.5 planner's four stages on a local cluster, byte-identical

## Log
- 2026-09-11 12:50 — job planned: Part 1 in three passes cut where other sessions' work sits, five decisions, ten tasks; Part 2 outlined
- 2026-09-11 13:40 — Part 1a built: the server refuses the engine by name (`ConfigError::Removed`, two tests), the crate and its end-to-end test are gone, the chart refuses `engine`, the SDK integration and the recipes are gone, ADR_0016 superseded; `cargo clippy -Dwarnings` and the config tests green, `just chart-check` and `just sdk-check` (419) green
- 2026-09-11 13:55 — verified: `just check` 19/19 on the working tree, other sessions' work included
- 2026-09-11 12:47 — Parts 1b and 1c built (`8154041`): the five routes, `core::engine`, `AppState.engine`, the three error variants, the launcher on the recipe and Experiments pages, `ExecutionOwner::Unknown`, `ExternalWorkflow` and its spec and kind; the contract and the panel's client regenerated with no engine left in them; clippy clean, every touched crate's tests and the panel's 178 green
- 2026-09-11 12:54 — 1.10 done: the docs pass landed inside `5f9996b`; after it, §28's Phase 12 bullet no longer says AW-4 is investigating, and three unwrapped `CLAUDE.md` lines are wrapped; `just check` 18/19 on HEAD, `comments` failing on the same five panel blocks from `ea2dfe7`
- 2026-09-11 14:45 — 2.1 done: ADR_0029 — a step opts in with `pod` and compiles to `container_job`; templates and per-template image lists are chart values, matched by exact repository; the launcher reads claimable rows and creates one Job per attempt by a derived name, never claiming; the pod claims its attempt by key under its template's queue token (a one-attempt credential deferred as the strict mode); the lease decides, the watch explains; the log is the last 256 KiB in the catalog. Answers the spec's three open questions; 2.2–2.4 reworded to match
- 2026-09-11 15:13 — 2.2 built (`c4fc124`): `RuntimeBinding::ContainerJob` under `container_job`; the step`s `pod` field, absent from every older digest; `aiwatcher_execution::pods` with the templates file, the exact-repository image match, Kubernetes quantities and the refusals of fields aiwatcher fills in; a 422 at registration naming step, value and template; `AIWATCHER_POD_TEMPLATES` read by the serve role and refused where no launcher exists; the SDK `PodRequest`. Clippy -Dwarnings and the three crates suites green, `just sdk-check` 423, the panel build green, the contract regenerated
