---
id: AW-5
title: Optimise, evaluate and promote a prompt in one managed run
step: spec
status: doing
branch: main
repo: aiwatcher
created: 2026-09-11
updated: 2026-09-11
tags: [spec/AW-5, step/spec, branch/main, status/doing]
---
<!-- spec-card -->

`#spec/AW-5` · `#branch/main` · repo `aiwatcher`

# AW-5 — Optimise, evaluate and promote a prompt in one managed run

> Branch `main`

## Phases
- [x] [① Investigation](01-investigation.md)
- [x] [② Spec](02-spec.md)
- [ ] [③ Job](03-job.md)
- [ ] [④ Tests](04-tests.md)
- [ ] [⑤ Review](05-review.md)
- [ ] [⑥ Deploy](06-deploy.md)

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
