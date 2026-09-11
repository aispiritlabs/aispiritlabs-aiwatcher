---
id: AW-5
step: tests
status: doing
branch: main
repo: aiwatcher
created: 2026-09-11
updated: 2026-09-11
tags: [spec/AW-5, step/tests, branch/main, status/doing]
---

`#spec/AW-5` · `#step/tests` · `#branch/main` · repo `aiwatcher`

# ④ Tests — AW-5

## Verification matrix

Rust tests are named by module path; `http::` means
`crates/aiwatcher-api/tests/http.rs`, `sdk::` means `sdk/python/tests/`, and
`e2e #n` is the numbered check `scripts/e2e-optimise-prompt.py` prints for each
variant.

| Spec scenario | Test | Result |
|---|---|---|
| A report knows its step › recorded by a step | `evaluations::tests::a_report_a_step_recorded_names_its_execution_and_step`; `sdk::test_worker::test_a_step_files_its_evaluation_under_its_run_and_step_and_a_retry_lands_on_it`; e2e #2 | ✅ |
| A report knows its step › recorded by a script | `evaluations::tests::a_report_a_script_recorded_names_no_execution_and_no_step`; `sdk::test_worker::test_a_report_recorded_outside_a_task_names_no_run_and_no_step` | ✅ |
| A report knows its step › does not become a workflow execution | `readmodel::tests::a_report_a_step_recorded_starts_no_workflow_execution` | ✅ |
| A report knows its step › a retried evaluation step | `evaluations::tests::a_retried_step_that_records_again_lands_on_its_own_report`; the same id twice in the SDK test above | ✅ |
| An optimisation names its reports › names its reports | `aiwatcher_prompts::tests::an_optimisation_keeps_the_reports_its_held_out_scores_came_from`; `http::production_on_a_rejected_candidate_is_refused_as_a_promotion` (the fields round-trip over HTTP); `sdk::test_prompts::test_an_optimisation_names_the_reports_its_held_out_scores_came_from`; e2e #3 | ✅ |
| An optimisation names its reports › recorded before this change | `aiwatcher_prompts::tests::a_record_written_before_the_report_references_still_reads` | ✅ |
| An optimisation names its reports › a named report does not decide | `aiwatcher_prompts::tests::a_report_an_optimisation_names_does_not_decide_its_verdict` | ✅ |
| `production` never names a rejected candidate › promoting one by hand | `aiwatcher_prompts::tests::production_refuses_a_candidate_the_verdict_turned_down`; `http::production_on_a_rejected_candidate_is_refused_as_a_promotion` (422 `promotion_refused`, naming the optimisation) | ✅ |
| `production` never names a rejected candidate › trying one on purpose | `aiwatcher_prompts::tests::staging_may_hold_a_candidate_the_verdict_turned_down`; the `staging` half of the HTTP test | ✅ |
| `production` never names a rejected candidate › a version a person wrote | `aiwatcher_prompts::tests::production_takes_a_version_a_person_published` | ✅ |
| `production` never names a rejected candidate › admitted without `promote` | `aiwatcher_prompts::tests::production_takes_an_admitted_candidate_recorded_without_promote` | ✅ |
| One managed run › an admitted candidate is approved | e2e `approved` #1, #4, #5, #6 | ✅ |
| One managed run › an admitted candidate is kept back | e2e `kept` #1, #5, #6 | ✅ |
| One managed run › a rejected candidate is never put to anybody | e2e `rejected` #1, #4, #5, #6 | ✅ |
| One managed run › an editor may not answer an admin's question | `http::a_question_a_worker_asked_for_an_admin_is_refused_to_an_editor` (403 to an editor, the step still `awaiting_input`, 200 to an admin) | ✅ |
| One managed run › answering does not record twice | e2e `approved` and `kept` #3 (one optimisation after the park and the resumed attempt) and #2 (two reports) | ✅ |
| One managed run › the candidate's text stays out of the store | e2e #7 in all three variants: absent from the run's view and every history page, present as the registry version | ✅ |
| Phase 15's scope in the plan › the plan says what exists | `grep` over `docs/PIPELINE_ARCHITECTURE.md`: `EvaluationSuite` appears only as withdrawn (§28, §35 rule 6), `RedisStreamsTransport` only as gone (§40.6), and Phase 15 is marked delivered and points at AW-5 | ✅ |

Beyond the spec, two refusals the job decided on, each with a test:
- `aiwatcher_prompts::tests::production_refuses_a_candidate_whose_verdict_was_never_written`
- `aiwatcher_prompts::tests::publishing_a_rejected_candidates_text_onto_production_is_refused_too`

## Commands run
```
cargo test -p aiwatcher-prompts -p aiwatcher-projector -p aiwatcher-core --lib
  — core 88, projector 96, prompts 44 passed; 0 failed
cargo test -p aiwatcher-api --test http -- production_on_a_rejected a_question_a_worker_asked_for_an_admin
  — 2 passed; 158 filtered out
uv run pytest tests/test_worker.py tests/test_prompts.py   (sdk/python)
  — 69 passed; ruff format/check clean, mypy --strict clean on the three changed modules
cargo clippy -p aiwatcher-{core,prompts,projector,api} --all-targets --all-features -- -Dwarnings
  — no findings
./scripts/e2e-optimise-prompt.py   (just e2e-optimise)
  — approved 7/7, kept 7/7, rejected 7/7: "all three runs hold"
just check
  — 18 of 19 PASS (cargo fmt, clippy, cargo test, openapi contract is current,
    panel build + typecheck, panel tests, python sdk, agentic engine, helm
    chart, cargo deny, …); FAIL comments: five comment blocks over 25 lines in
    apps/panel (search.ts, navigation.ts, query-builder.ts, workflow-layout.ts,
    attribute-picker.tsx), every one last touched by ea2dfe7, the panel
    rebuild's commit — AW-5 changed nothing in apps/panel but the generated
    client
just sdk-check
  — ruff format and check clean, mypy --strict clean over 72 files, 422 passed
```

## Issues found & resolutions
- **The link was dropped at the envelope.** The job's decision was that a
  report carries no `workflow_id`. `RecordedMetadata.workflow_run_id` is kept
  only beside a `workflow_id` (`EventEnvelope::workflow_run`), so the fold saw
  `None`. The first run of the projector test caught it. → The report carries
  both, and the decision is amended in `03-job.md`. The suite fallback that
  reads `workflow_id` never fires, because a report always names its suite.
- **A publish could move `production` around the refusal.** A version is its
  text, so publishing a rejected candidate's text with `label: production`
  landed on the candidate without passing `set_label`. → `check_admitted` runs
  there too, with its own test. This is within the requirement as written
  ("moving the `production` label SHALL be refused…"), so the spec does not
  move.
- **The first code commit failed and the e2e commit landed before it**
  (`181c47c`, then `1d5b0ca`). An unquoted list of paths did not split in
  zsh. Nothing was lost, and the script commit touches no compiled code, but
  `181c47c` on its own names a keyword the SDK did not yet have.

## Quality checks
- [ ] The project's verification command passes — **not fully**: 18 of 19,
      and the one red step (`comments`) is five panel files from `ea2dfe7`,
      which this change did not touch. Left to the session that owns the panel
      rather than rewritten from here; every step this change could move is
      green.
- [x] Every spec scenario has a test that fails without the change — except
      three regression guards over behaviour that held before and must keep
      holding: the report that starts no workflow execution (the `eval.*`
      routing predates this), `staging` on a rejected candidate, and
      `production` on an authored version
- [x] No backward-incompatible migration in a single release — no migration;
      every new field is optional and absent from what stored records hold

## Log
- 2026-09-11 12:33 — `just check` 18/19 — `comments` red on five panel files from `ea2dfe7`, not this change; `just sdk-check` green, 422 passed
