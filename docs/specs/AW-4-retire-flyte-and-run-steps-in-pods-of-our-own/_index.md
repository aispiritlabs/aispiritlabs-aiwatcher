---
id: AW-4
title: Retire Flyte, and run workflow steps in pods of our own
step: job
status: doing
branch: main
repo: aiwatcher
created: 2026-09-11
updated: 2026-09-12
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
- 2026-09-11 14:45 — Part 2 designed: ADR_0029 accepted — templates, images, the pod as a worker for one attempt, the lease deciding and the Job explaining; 2.2 is next
- 2026-09-11 15:13 — 2.2 built (`c4fc124`): a step may ask for a pod, and registration refuses what its template does not allow; nothing launches one yet, and 2.3 is next
- 2026-09-12 00:32 — 2.3 built (inside another session's `26be55e`): the work role starts one Job per pod's attempt and claims none of them, a pod claims its own attempt by key and no long-lived worker can take it, and the chart grants the launcher Jobs and pod logs in one namespace; 2.4 is next — a cancel deleting the Job, the watch ending a dead pod's attempt, and the log
- 2026-09-12 01:10 — 2.4 built (`c713a1e`): the launcher watches what it started — a pod that died holding its attempt ends it now, with the cluster's own reason rather than a lapsed lease's silence; a Job no pod claimed in time is ended and deleted; a cancel deletes a running pod's Job and the run reaches `cancelled` in seconds; and a pod's last 256 KiB is kept against its attempt in the catalog before its Job goes. A cancel that had been leaving dispatched attempts in the claim table for ever is fixed with it. 2.5 is next — planner's four stages on a local cluster, byte-identical
