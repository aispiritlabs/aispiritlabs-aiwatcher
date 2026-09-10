---
id: AW-1
title: Ship the spec flow with the repository
step: tests
status: doing
branch: main
repo: aiwatcher
created: 2026-09-10
updated: 2026-09-10
tags: [spec/AW-1, step/tests, branch/main, status/doing]
---
<!-- spec-card -->

`#spec/AW-1` · `#branch/main` · repo `aiwatcher`

# AW-1 — Ship the spec flow with the repository

> Branch `main`

## Phases
- [x] [① Investigation](01-investigation.md)
- [x] [② Spec](02-spec.md)
- [x] [③ Job](03-job.md)
- [x] [④ Tests](04-tests.md)
- [ ] [⑤ Review](05-review.md)
- [ ] [⑥ Deploy](06-deploy.md)

## Summary
The spec-driven flow that this repository's work runs through lives in `~/.claude`,
per machine. This puts the skill and the eight `/spec-*` commands under `.claude/`
beside the code, and teaches the vendoring check the difference between a skill this
repository authored and one it pinned from upstream.

## Log
- 2026-09-10 00:59 — spec opened on `main`
- 2026-09-10 01:00 — investigation written
- 2026-09-10 01:00 — spec drafted: 4 requirements, 9 scenarios
- 2026-09-10 01:00 — job planned: 8 tasks, none started
- 2026-09-10 01:07 — verified: 7 scenarios green, 2 not automatable, just check not run in full
