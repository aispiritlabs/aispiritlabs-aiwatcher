---
id: AW-5
step: review
status: doing
branch: main
repo: aiwatcher
created: 2026-09-11
updated: 2026-09-11
tags: [spec/AW-5, step/review, branch/main, status/doing]
---

`#spec/AW-5` · `#step/review` · `#branch/main` · repo `aiwatcher`

# ⑤ Review — AW-5

**Reviewer:** Claude. This is a self-review of the diff against the spec and
`CLAUDE.md`'s guardrails, and the owner's review is still to come.
**PR:** none. The commits are on `main` and not pushed: `1d5b0ca` (code), `181c47c`
(e2e) and the spec's own. The owner decides when to push.
**Outcome:** approved, with two findings deferred to their own tasks.

## Checklist
- [x] **Every spec requirement is implemented, and nothing unspecified was
      added.** Two refusals go beyond the scenarios: a candidate with no record,
      and a publish carrying `label: production`. Both fall inside the
      requirement's own words, "moving the `production` label SHALL be
      refused", and each has a test.
- [x] **The decisions in `03-job.md` are the ones the code took.** One changed
      while building: the report carries `workflow_id`. The change is recorded
      in the job with the test that forced it.
- [x] **The guardrails hold for the paths this change touched:**
  - no prompt in a workflow message — the candidate goes as an artifact, and
    e2e #7 reads every view of the run;
  - the verdict stays the server's;
  - a client cannot promote past it, now at the label route too;
  - a retry lands on its own report and on its own optimisation, both derived
    from `step_key`;
  - no report becomes a workflow execution — the routing test guards it;
  - ADR_0011's warning about scripts that call `set_label` is answered in its
    amendment: the promote step calls it only after a person answers.
- [x] **Tests cover the spec scenarios and fail without the change.** The
      three regression guards are named in `04-tests.md`.
- [x] **Generated contracts and clients were regenerated and committed.** They
      are in `1d5b0ca`, and `just check` reports "openapi contract is current"
      as passing.
- [x] **No secrets, and no migration.** Every new field is optional and absent
      from stored records.
- [x] **The lasting decisions are recorded:**
  - the ADR_0010 amendment (a report names its run and step);
  - the ADR_0011 amendment (`production` answers to the verdict);
  - `CLAUDE.md`'s new guardrail and decision 8;
  - the plan's §35 rule 6 (why there is no `EvaluationSuite`).

## Findings
- **The panel still offers Promote on a candidate the server now refuses.**
  `features/prompts/screens/detail/page.tsx` renders the 422's message as
  `promoteError`, so the refusal is named rather than silent. Offering a button
  that does not work still breaks the panel's own rule. → Deferred, because
  the panel is another session's area and the fix needs the version's verdict
  on the page. Offered as a separate task: "Hide Promote on a rejected prompt
  candidate".
- **`just check` is red on `comments`,** from five panel files in `ea2dfe7`,
  not from this change. → Deferred and offered as its own task: "Shorten five
  panel comment blocks lint rejects".
- **The e2e commit precedes the code commit** (`181c47c` before `1d5b0ca`).
  → Accepted. Nothing is pushed, and reordering history is the owner's call,
  not something a review does quietly.
- **Not carried over.** The TypeScript SDK's `recordEvaluation` has no
  run-and-step link, and the DeepEval bridge's `record_optimization` does not
  forward the two report references. → Out of scope by the spec. Both
  arguments are optional, so nothing breaks, and planner's adoption ticket is
  where the bridge would pick them up.
- **A linked report can be evicted from the evaluation projection** — it is
  bounded, and the optimisation record keeps its ids. → A stated cost
  (ADR_0011's amendment, Option C's gate). The log still holds the report for
  as long as retention does.

## Log
- 2026-09-11 12:35 — review recorded on `main`: approved; two findings deferred to their own tasks, three accepted with the reason
