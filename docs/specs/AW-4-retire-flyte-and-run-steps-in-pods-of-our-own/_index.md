---
id: AW-4
title: Retire Flyte, and run workflow steps in pods of our own
step: job
status: doing
branch: main
repo: aiwatcher
created: 2026-09-11
updated: 2026-09-11
tags: [spec/AW-4, step/job, branch/main, status/doing]
---
<!-- spec-card -->

`#spec/AW-4` · `#branch/main` · repo `aiwatcher`

# AW-4 — Retire Flyte, and run workflow steps in pods of our own

> Branch `main`

## Phases
- [x] [① Investigation](01-investigation.md)
- [x] [② Spec](02-spec.md)
- [x] [③ Job](03-job.md)
- [ ] [④ Tests](04-tests.md)
- [ ] [⑤ Review](05-review.md)
- [ ] [⑥ Deploy](06-deploy.md)

## Summary
The owner's decision, 2026-09-11: Flyte leaves aiwatcher. planner already runs
its house import through aiwatcher's own workflow engine, so the engine
catalogue, the launch route and the Flyte adapter have no user left — and the
engine should start the pods itself, Phase 12's `ContainerJob`, rather than ask
an orchestrator to. The first question is the owner's: is everything Flyte gave
planner something aiwatcher now has.

## Log
- 2026-09-11 11:30 — spec opened on `main`; Phase 9 (engine-owned executions) withdrawn in its favour
- 2026-09-11 11:55 — investigation written: for planner's use only one pod per stage is missing; recommend removing Flyte, then `ContainerJob`
- 2026-09-11 12:35 — spec drafted: Part 1 removes the engine, Part 2 adds `ContainerJob`; nine requirements over twenty scenarios; three questions left to the job
- 2026-09-11 13:40 — job planned and Part 1a built: the server, the crate, the chart, the SDK integration and the recipes; the API, the contract, the panel and the execution variants wait for the sessions holding them
- 2026-09-11 12:54 — Part 1 done: 1b and 1c built (`8154041`, `a34c378`), the documents in `5f9996b`; Part 2 starts with its ADR
