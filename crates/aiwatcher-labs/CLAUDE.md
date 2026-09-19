# A workshop's labs

The rules for `aiwatcher-labs`. Read
[ADR_0034](../../docs/ADR/ADR_0034_WORKSHOP_LABS.md) first, because most of what
this crate is about is what it deliberately does **not** hold. Publishing obeys
the root file's one ordering rule — the version object before the head that
indexes it — and the project boundary is the root file's and ADR_0033's.

- **Never build a second copy of the tests, the submission or the mark.** A
  lab's tests are an `aiwatcher-evaluation` scorecard at a version and a cohort
  derived from a dataset version; the work handed in is a staged recording or
  answers a worker generated; a mark is a published `EvaluationResult`. All four
  had routes before this crate existed, all four are project-scoped already, and
  a lab that duplicated any of them would be a second answer to what a
  measurement is. The one authored thing here is the brief.
- **Never let a lab compute a result's home, or a reader compute a lab's.**
  `EvaluationContext::id` is the content address of the cases, the card, the
  scorer and the metric definitions together, so every result measured on one
  lab's pins lands under it with nobody registering anything.
  `LabTests::measurement` folds the pins into that context and the API answers
  the id; a browser that derived one would be a second implementation of a
  canonicalisation, and the day it disagreed a lab would show somebody else's
  marks. The precedent is `POST /api/v1/evaluation-approvals/address`.
- **Never pin a card by its head.** A lab names `name@version`, as a scoring run
  does: a head would let a rewrite change what an already-issued lab measures
  between one participant's submission and the next.
- **Never store a lab whose pins do not resolve.** The API route checks the card
  and the cohort in *this project's* evaluation registry before publishing, and
  refuses with `LabError::Unpinned`. The refusal has to reach the instructor
  while they are writing the lab, not the participant when they submit to a slot
  that measures nothing.
- **Never answer a context id for a measurement a lab does not pin.** A judge
  and a calibration set are declared per scoring *run*, so two submissions need
  not have been graded by the same one and their results share no context.
  `LabError::Unmeasurable` names the metric that asks rather than answering an
  id that quietly means less than it looks like it means.
- **Never hold the notebook a lab hands out, and never say where marimo is.**
  A lab names a notebook — a file in the notebook runtime, or a path in a
  workshop the participants checked out — and stores no source. Where a marimo
  for it runs is the *deployment's* answer or the *participant's*, and a URL
  carried in the document would be authored data deciding where a browser goes:
  the shape `AIWATCHER_WORKFLOW_RUNNER_URL` is configuration to avoid, read from
  the browser's side. What may be authored is the command that opens it and the
  port that command lands on, because those are facts about the exercise rather
  than about anybody's machine.
- **Never pin a digest for bytes this instance does not hold.** `revision` is
  present exactly when the notebook is one the notebook runtime keeps, and then
  `path` is that runtime's name for it — `services/ml_pipeline/CLAUDE.md`'s rule
  for a curation block, restated for the second thing to pin one. A file in
  somebody's checkout has none, and absence is the honest answer rather than a
  weaker one: a digest that cannot be checked is a promise that cannot be kept,
  and what holds that file still is the workshop's own version control.
- **Never reach a notebook from this crate or its routes.** The card and the
  cohort are resolved before a lab is stored because they are in this instance's
  own registry and the answer is a read; a notebook is another service's or
  another repository's, and a publish that called one would put a third party in
  the write path of an authored document. What is refused here is a description
  that could resolve *nowhere* — an absolute path or one that escapes upwards, a
  command longer than a line, a privileged port, a digest that is not one — and
  everything else is reported where the lab is **read**, beside the thing that
  could not be opened.
- **Never let a lab carry a mark, a score or a position in a ranking.** It holds
  what was authored. Progress, points and due dates have no contract in this
  instance; a workshop's timetable is already the grant's `GrantWindow`, which
  is where a deadline would belong if one is ever wanted.
