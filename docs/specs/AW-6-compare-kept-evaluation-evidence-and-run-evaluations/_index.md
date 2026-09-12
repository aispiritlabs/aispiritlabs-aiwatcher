---
id: AW-6
title: Compare kept evaluation evidence, and run evaluations from aiwatcher
step: backlog
status: open
branch: main
repo: aiwatcher
created: 2026-09-12
updated: 2026-09-12
tags: [spec/AW-6, step/backlog, branch/main, status/open]
---
<!-- spec-card -->

`#spec/AW-6` · `#branch/main` · repo `aiwatcher`

# AW-6 — Compare kept evaluation evidence, and run evaluations from aiwatcher

> Branch `main`

## Phases
- [ ] [① Investigation](01-investigation.md)
- [ ] [② Spec](02-spec.md)
- [ ] [③ Job](03-job.md)
- [ ] [④ Tests](04-tests.md)
- [ ] [⑤ Review](05-review.md)
- [ ] [⑥ Deploy](06-deploy.md)

## Summary
The rest of FTI, which until now has been run out of `docs/FTI_*.md` rather than
from this board. Stage B is closed: durable evidence has its own store, its own
admission and its own screen, and an instance holds many admitted pairs at once
(plan sections 9–18, ADR_0030). What is left is the work those sections were
building towards, and it is four things rather than one:

- **B3 — comparing kept evidence.** Two results of one context and different
  variants, compared on the server. Its inputs exist now; its rules are the ones
  ADR_0030 states — status, cohort, split, suite, scorer, judge, schemas and
  completeness, never a matching context hash alone.
- **B4 — assessments.** One trace, span, session or case measurement, with a
  rubric version, a typed value, an author and a rationale. Human and judge
  assessments coexist.
- **AR3 — the compile-and-start use case out of the HTTP module.** Independent
  of the rest, and still a precondition for C0: the scheduler classifies a
  failure by the HTTP status an `ApiError` carries.
- **C0 — running an evaluation rather than recording one.** `score_existing`
  first, and it is what makes the approval resource earn its keep: a managed run
  publishes evidence with no step on the server's host.

Both follow-ups that sat inside the first are done (plan section 19): the
catalogue has an index and a published order, so its page costs 103 requests
rather than 152 and the screen carries a period again; and an approval bundle is
staged through the API, so a new variant needs nothing on the server's host.
What is left inside B3 is the comparison itself.

## Log
- 2026-09-12 — card opened for the remainder of FTI; stage B closed by plan section 18
- 2026-09-12 — four packages off the limitation list (plan section 19): gaps
  reported by the pass that already knew, a catalogue index with a published
  order, bundle upload over the API, and approvals on the screen
