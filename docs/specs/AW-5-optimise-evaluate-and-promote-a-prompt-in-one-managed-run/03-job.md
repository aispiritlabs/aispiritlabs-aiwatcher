---
id: AW-5
step: job
status: doing
branch: main
repo: aiwatcher
created: 2026-09-11
updated: 2026-09-11
tags: [spec/AW-5, step/job, branch/main, status/doing]
---

`#spec/AW-5` · `#step/job` · `#branch/main` · repo `aiwatcher`

# ③ Job — AW-5

## Technical approach

The build order is chosen so that every commit compiles and `just check` stays
green:
1. The three server-side rules: the evaluation fold, the optimisation record
   and the label.
2. The SDK halves that carry data to them.
3. The contract, regenerated once for all three.
4. The end-to-end run, which proves them together.
5. The documents.

Each rule is tested where it lives — the projector, the prompt registry, the
API — and the e2e proves only the joins, so it stays short. AW-3 and the panel
rebuild have both committed (`2dd84e9`, `ea2dfe7`), so nothing here is cut
around another session's work any more.

## Decisions

- **A report names its step in `data.step_id`, and its execution as the
  envelope's `workflow_run_id`** — *because* `node` is what a `step.*` fact
  calls a node of a declared graph, and a report is not one. A later reader
  keying on `node` would draw it. Alternative rejected: `data.node`, for
  symmetry with the facts.

  **Changed while building:** the report also carries the workflow's id. The
  envelope keeps a `workflow_run_id` only beside a `workflow_id`
  (`EventEnvelope::workflow_run`), so the plan of sending no `workflow_id`
  dropped the link at the envelope. A test caught it. The suite fallback that
  reads `workflow_id` never fires, because a report always names its suite.
- **A task's report id is `uuid5` over `step_key/suite/variant`** — *because*
  it keeps the shape `_new_id` already gives, and a retry lands on the report
  it already wrote. Alternative rejected: `step_key` itself, which is not
  unique once one step records two suites.
- **`record_evaluation` gains keyword-only `workflow_id`, `workflow_run_id` and
  `step_id`, and `TaskContext.record_evaluation` fills them** (the first
  arrived with the change above) — *because* the link is then
  public and explicit, and usable by a script that knows its run. Alternative
  rejected: a private helper the context calls across modules. That is ruff's
  `SLF001`, and a second path to the same four events.
- **The refusal is `RegistryError::NotAdmitted`, answered 422
  `promotion_refused`** — *because* it is the model registry's refusal of the
  same act, and `error.rs:421-431` already says why a refused promotion is a 422:
  it is a decision about content, not a conflicting write. Alternative
  rejected: 409.
- **A candidate whose optimisation record is missing is refused too.**
  `record_optimization` stores the version before the record, so a crash
  between the two leaves a candidate with no verdict. A verdict that was never
  written is not an admission.
- **The two evaluation references are validated no more than `evaluation_id`
  is** — *because* neither is a key in the store, and a report id is whatever
  its producer chose.
- **`promote` reads the verdict from `record`'s `verdict` artifact** —
  *because* the store already keeps it and hands it on as an input. A derived
  id would be one fact derived twice, in two tasks. The artifact holds the
  optimisation id, the outcome and the candidate's version id, never text.
- **The candidate is handed on as the `optimise` step's `candidate`
  artifact** — *because* it then goes through the attempt's outputs route,
  which digests what it stores, and stays out of every workflow message.
- **The e2e starts its own server for each variant, the `e2e-agent-*` way** —
  with the `memory` workflow store (a worker in another thread claims, and
  `file` refuses that), auth `none`, and the worker as an in-process
  `Runtime`.
- **The editor's refusal is proven in `http.rs`, not in the e2e** — *because*
  under auth `none` every caller is an admin. The API test uses the
  `behind_a_proxy` fixture against a question a worker asked mid-attempt,
  which no test covers yet. The e2e checks that the question named `admin`.
- **"The candidate stays out of the store" is checked through every read the
  API offers of the run** — its view and its history. The `memory` store has
  no disk to search, and those two are what a reader of the run can see.

## Changes

- **Files:**
  - `crates/aiwatcher-projector/src/evaluations.rs` (modified) —
    `EvaluationSummary.execution_id` and `.step_id`, and their fold.
  - `crates/aiwatcher-prompts/src/lib.rs` (modified) —
    `OptimizationRequest`'s two references, `set_label`'s refusal, and
    `RegistryError::NotAdmitted`.
  - `crates/aiwatcher-core/src/prompts.rs` (modified) — `OptimizationRecord`'s
    two references.
  - `crates/aiwatcher-api/src/error.rs` (modified) — the 422.
  - `crates/aiwatcher-api/tests/http.rs` (modified) — the label refusal and a
    parked question's role.
  - `sdk/python/aiwatcher_sdk/__init__.py` (modified) — `record_evaluation`'s
    two keywords.
  - `sdk/python/aiwatcher_sdk/worker/context.py` (modified) —
    `TaskContext.record_evaluation`.
  - `sdk/python/aiwatcher_sdk/prompts.py` (modified) — `record_optimization`'s
    two keywords and the record's two fields.
  - `scripts/e2e-optimise-prompt.py` (new) and a `justfile` recipe,
    `e2e-optimise`.
  - `docs/PIPELINE_ARCHITECTURE.md` §12, §28, §35 and §40.6, ADR_0010, ADR_0011
    and `CLAUDE.md` (modified).
- **Contract:** `EvaluationSummary`, `OptimizationRecord` and the optimisation
  request gain optional fields, all additive. `just openapi` regenerates
  `contracts/openapi.json` and `apps/panel/src/api/generated`, and both are
  committed.
- **Data / migrations:** none. Every new field is optional with a serde
  default, so stored records and replayed logs read unchanged.

## Tasks

### 1 — a report knows its step
- [x] 1.1 Projector: fold `execution_id` from the envelope's `workflow_run_id`
      and `step_id` from `data.step_id`. Tests: by a step, by a script, no
      workflow execution invented. — *a report knows the step that produced
      it*
- [x] 1.2 SDK: the two keywords on `record_evaluation`, and
      `TaskContext.record_evaluation` with the derived id. Tests: stamped, and
      the same id on a second attempt. — *a report knows the step that
      produced it*

### 2 — an optimisation names its reports
- [x] 2.1 Core and prompts: `baseline_evaluation` and `candidate_evaluation` on
      the request and the record. Tests: kept and read back, an old record
      reads, and a named report does not decide. — *an optimisation names the
      reports behind its held-out scores*
- [x] 2.2 SDK: the two keywords and the record's two fields. — *same*

### 3 — `production` and the verdict
- [x] 3.1 Prompts: `set_label` refuses `production` on a rejected candidate or
      one with no record. Tests: the four scenarios plus the missing record. —
      *`production` never points at a rejected candidate*
- [x] 3.2 API: `NotAdmitted` as 422 `promotion_refused`, with an HTTP test. —
      *same*
- [x] 3.3 API test: an editor may not answer an admin's question asked
      mid-attempt. — *one managed run…* (the editor scenario)

### 4 — the contract and the run
- [x] 4.1 `just openapi`; commit the contract and the generated client.
- [x] 4.2 `scripts/e2e-optimise-prompt.py` and `just e2e-optimise`: the three
      variants, the reports and their links, one optimisation per run, the
      candidate only in its artifact and the registry. — *one managed run
      optimises, evaluates and promotes*

### 5 — the record
- [x] 5.1 The plan's §12, §28, §35 and §40.6, the ADR_0010 and ADR_0011
      amendments, and `CLAUDE.md`'s guardrail. — *Phase 15's scope in the
      plan*, and the removed `EvaluationSuite` binding
- [x] 5.2 Verify: `just check`, `just sdk-check`, `just e2e-optimise`. — `just check`
      18/19, `comments` red on panel files from `ea2dfe7`; the rest green

## Log
- 2026-09-11 12:20 — job planned on `main`: nine design decisions and twelve tasks; the three open questions settled — `data.step_id`, 422 `promotion_refused`, the verdict read from `record`'s artifact
- 2026-09-11 12:28 — decision changed while building: a step's report carries `workflow_id` too, because the envelope keeps a `workflow_run_id` only beside one; the fold's test caught the dropped link
- 2026-09-11 12:28 — built: the fold, the record's two references, `check_admitted` on `set_label` and on a publish carrying `production` (a gap found on the way: a version is its text), 422 `promotion_refused`, the SDK halves, the contract; core 88, projector 96, prompts 44, the two new HTTP tests, sdk 69 green
- 2026-09-11 12:28 — `just e2e-optimise`: 21 of 21 checks over the three variants; the plan, ADR_0010, ADR_0011 and `CLAUDE.md` amended
