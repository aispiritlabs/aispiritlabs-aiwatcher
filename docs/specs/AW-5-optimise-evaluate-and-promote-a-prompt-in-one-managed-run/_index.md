---
id: AW-5
title: Optimise, evaluate and promote a prompt in one managed run
step: deploy
status: done
branch: main
repo: aiwatcher
created: 2026-09-11
updated: 2026-09-11
tags: [spec/AW-5, step/deploy, branch/main, status/done]
---
<!-- spec-card -->

`#spec/AW-5` · `#branch/main` · repo `aiwatcher`

# AW-5 — Optimise, evaluate and promote a prompt in one managed run

> Branch `main`

## Phases
- [x] [① Investigation](01-investigation.md)
- [x] [② Spec](02-spec.md)
- [x] [③ Job](03-job.md)
- [x] [④ Tests](04-tests.md)
- [x] [⑤ Review](05-review.md)
- [x] [⑥ Deploy](06-deploy.md)

## Summary
Phase 15 of the pipeline plan, chosen by the owner on 2026-09-11. Its exit: an
optimise → evaluate → promote definition runs end to end with the verdict
computed on the server, as ADR_0011 requires. Its other half, distributed mode,
was delivered by AW-2 Phase F. So what is left is the joins between an
evaluation report, an optimisation and a label, rather than a new runtime.

## Log
- 2026-09-11 12:02 — spec opened on `main` for Phase 15
- 2026-09-11 12:02 — investigation written: distributed mode is already delivered, and the verdict is already computed on the server; recommend no new binding, three joins, one refusal and an end-to-end run
- 2026-09-11 12:08 — spec drafted: four requirements added over seventeen scenarios, one modified, the `EvaluationSuite` binding removed; the owner's three answers settled
- 2026-09-11 12:13 — job planned: nine decisions, twelve tasks; the three open questions settled
- 2026-09-11 12:34 — verification run: every scenario covered; `just e2e-optimise` 21/21; `just check` 18/19 — `comments` red on panel files from `ea2dfe7`
- 2026-09-11 12:34 — review recorded: approved; the panel's Promote button and the panel comments deferred to their own tasks
- 2026-09-11 12:34 — shipped to `main` locally, not pushed; decisions graduated into ADR_0010, ADR_0011 and `CLAUDE.md`

## Shipped
- 2026-09-11 — see [⑥ Deploy](06-deploy.md)
