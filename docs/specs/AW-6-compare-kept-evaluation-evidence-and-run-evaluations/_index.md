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

- **B3 — comparing kept evidence.** Its evidence side is **done** (plan section
  20): `GET /api/v1/evaluation-results/{id}/comparison?baseline=…`, with the
  candidates from `?context_id=`. The rule turned out to be the matching
  context hash — that address already covers the cohort, split, suite, scorer,
  judge and schemas — *plus* the two things it does not carry: whether the
  measurement succeeded, and whether the evidence behind each number can still
  be read. The case-level diff followed in plan section 23 —
  `/comparison/cases`, a merge of two streams both sides sorted by `case_id` at
  publish, narrowed by the server and bounded in how far one request walks.
  What is left of B3 is the judge's fourth ADR_0030 condition and the variant
  context on observations.
- **B4 — assessments.** One trace, span, session or case measurement, with a
  rubric version, a typed value, an author and a rationale. Human and judge
  assessments coexist.
- **AR3 — the compile-and-start use case out of the HTTP module.** **Done**
  (plan section 21): `aiwatcher_execution::start` holds it, `AppState::executions`
  assembles it, and the route, `run_now` and the tick all go through it. The
  scheduler asks the refusal `says_the_same_next_time` rather than reading the
  HTTP status an `ApiError` carries — which was wrong for the three refusals
  that are 5xx by number and permanent by meaning, each leaving a slot due and
  retried every minute for ever. Its one limitation is closed too (plan section
  22): the registry a *registered workflow* is read from answers with three
  errors instead of one string, so a corrupt stored definition is no longer
  indistinguishable from a store having a bad moment.
- **C0 — running an evaluation rather than recording one.** `score_existing`
  first, and it is what makes the approval resource earn its keep: a managed run
  publishes evidence with no step on the server's host.

Both follow-ups that sat inside the first are done (plan section 19): the
catalogue has an index and a published order, so its page costs 103 requests
rather than 152 and the screen carries a period again; and an approval bundle is
staged through the API, so a new variant needs nothing on the server's host.
The comparison itself followed in section 20.

## Log
- 2026-09-12 — card opened for the remainder of FTI; stage B closed by plan section 18
- 2026-09-12 — four packages off the limitation list (plan section 19): gaps
  reported by the pass that already knew, a catalogue index with a published
  order, bundle upload over the API, and approvals on the screen
- 2026-09-12 — B3's evidence side delivered (plan section 20): comparability is
  `context_id` equality plus readability, `Comparability` moved to
  `aiwatcher_core` so both halves speak one vocabulary, candidates come from the
  catalogue narrowed by context, and the panel draws one control
- 2026-09-12 — AR3 delivered (plan section 21): compile-and-start is
  `aiwatcher_execution::start`, three callers share it, and whether a slot stays
  due is the refusal's own answer rather than a status code's. `TargetKind`
  deleted in favour of `DefinitionKind`; every HTTP status unchanged
- 2026-09-12 — AR3's own limitation closed (plan section 22): `DefinitionError`
  replaces a flattened `StoreError::Backend`, in the dataset registry's own
  three words, so an unreachable store, a corrupt stored definition and a
  definition that does not compile are three answers. Here the statuses *do*
  change, because the classification was the thing that was wrong: 500, 502 and
  422 where there was one 503
- 2026-09-12 — B3's case-level diff delivered (plan section 23): a route of its
  own, because the header comparison costs two summaries and this is a full read
  of both sides. Five words rather than two lists — a case can be better at one
  thing and worse at another — decided from the directions the pinned context
  declares, never in the browser. An unverified pair keeps its rows, which is
  the one place the two halves of a comparison part company: the cases that
  failed are what somebody opens it to read
