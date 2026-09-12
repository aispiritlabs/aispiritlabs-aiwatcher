---
id: AW-7
title: A pod holds its own attempt, and the pod gates run in CI
step: job
status: todo
branch: main
repo: aiwatcher
created: 2026-09-12
updated: 2026-09-12
tags: [spec/AW-7, step/job, branch/main, status/todo]
---
<!-- spec-card -->

`#spec/AW-7` · `#branch/main` · repo `aiwatcher`

# AW-7 — A pod holds its own attempt, and the pod gates run in CI

> Branch `main`

## Phases
- [x] [① Investigation](01-investigation.md)
- [x] [② Spec](02-spec.md)
- [x] [③ Job](03-job.md)
- [ ] [④ Tests](04-tests.md)
- [ ] [⑤ Review](05-review.md)
- [ ] [⑥ Deploy](06-deploy.md)

## Summary
What AW-4's last two backends left open, found while reviewing what is left
after them. `docker` and `process` cannot hold a pod's worker token honestly, so
under any auth mode but `none` it has to be written into the templates file. No
gate noticed, because every gate runs unauthenticated and none of them runs in
CI. And the `docker` backend has only ever run where OrbStack defines
`host.docker.internal`.

Three parts, cheapest first:
- **A.** A container on a Linux engine reaches the API.
- **B.** The three pod gates run in CI.
- **C.** The launcher mints each pod a credential for its own attempt, which is
  the stricter mode ADR_0029 deferred and
  [ADR_0031](../../ADR/ADR_0031_POD_ATTEMPT_CREDENTIAL.md) decides.

## Log
- 2026-09-12 10:40 — spec opened on `main` after AW-4's 2.7; investigation, spec and job written together with ADR_0031 (proposed), at the owner's ask for "an ADR and a plan"; nothing built
