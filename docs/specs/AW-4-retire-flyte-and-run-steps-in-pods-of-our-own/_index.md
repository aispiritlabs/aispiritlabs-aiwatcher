---
id: AW-4
title: Retire Flyte, and run workflow steps in pods of our own
step: investigation
status: todo
branch: main
repo: aiwatcher
created: 2026-09-11
updated: 2026-09-11
tags: [spec/AW-4, step/investigation, branch/main, status/todo]
---
<!-- spec-card -->

`#spec/AW-4` · `#branch/main` · repo `aiwatcher`

# AW-4 — Retire Flyte, and run workflow steps in pods of our own

> Branch `main`

## Phases
- [ ] [① Investigation](01-investigation.md)
- [ ] [② Spec](02-spec.md)
- [ ] [③ Job](03-job.md)
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
