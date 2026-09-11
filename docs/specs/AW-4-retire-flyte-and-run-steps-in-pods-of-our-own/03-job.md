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
  or `worker`, so this changes no read of existing data.

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
- [ ] 1.8 API routes, error variants, `AppState.engine`, `core::engine`; the
      contract and the generated client regenerated; the launcher and its call
      sites removed. — *no build carries Flyte*

### Part 1c — after AW-3
- [ ] 1.9 `ExecutionOwner::Unknown`; `RuntimeBinding::ExternalWorkflow` and its
      spec and kind removed; the contract regenerated. — *an unknown owner is not
      an engine*
- [ ] 1.10 The documents that describe the engine as a design. — *no build
      carries Flyte*

### Part 2 — pods (outline)
- [ ] 2.1 ADR: templates, allowlists, who may name an image, the `kube` feature
- [ ] 2.2 `RuntimeBinding::ContainerJob` and the step fields; registration's
      refusals — *only an allowed image runs*, *resources come from the template*
- [ ] 2.3 The pod executor behind `kube`: a Job per attempt, `backoffLimit: 0`,
      the claim filter from the registry — *a step may ask for a pod*, *the
      engine owns a pod's retries*
- [ ] 2.4 Cancel deletes the Job; the log is read back — *a cancel reaches a
      running pod*, *a pod's log is kept*
- [ ] 2.5 planner's four stages on a local cluster, byte-identical

## Log
- 2026-09-11 12:50 — job planned: Part 1 in three passes cut where other sessions' work sits, five decisions, ten tasks; Part 2 outlined
- 2026-09-11 13:40 — Part 1a built: the server refuses the engine by name (`ConfigError::Removed`, two tests), the crate and its end-to-end test are gone, the chart refuses `engine`, the SDK integration and the recipes are gone, ADR_0016 superseded; `cargo clippy -Dwarnings` and the config tests green, `just chart-check` and `just sdk-check` (419) green
- 2026-09-11 13:55 — verified: `just check` 19/19 on the working tree, other sessions' work included
