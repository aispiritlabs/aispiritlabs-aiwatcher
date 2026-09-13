# A regression gate in CI

`aiwatcher-gate` measures the application at one commit against a published
baseline and exits by the server's verdict — the same comparison the
Evaluation page draws, held to a policy:

| Exit | Verdict | Meaning |
|------|---------|---------|
| 0 | `pass` | Nothing held got worse. |
| 1 | `regression` | A metric worse than its tolerance, or a critical case lost — even beside a better average. |
| 2 | `incomplete` | A scorer failed or a case went unanswered; a measurement that cannot say never passes. |
| 3 | `error` | Nothing admits the variant, the run failed, or the two results do not compare. |

It prints the verdict with the commit, the card version and the link to the
evidence, writes the same to `GITHUB_STEP_SUMMARY` when there is one, and
comments on nothing.

## Once, by an admin

1. Publish the cases as a curation dataset version (`POST /api/v1/datasets`,
   rows of `case_id`, `input.question`, `expected.answer`) and the card
   (`POST /api/v1/evaluation-scorecards`).
2. Measure `main` as the baseline — the first run of the experiment, admitted
   by hand or by the line below.
3. Admit the line: `POST /api/v1/evaluation-approval-lines` with any declaration
   of the experiment. Every variant of that `experiment_id` measured in that
   context — same cases, split, card, scorer — is then admitted when its run
   starts, from the bytes its job staged, and the approval names the line. A
   variant naming a model or a workflow is still admitted by hand, and a line
   over the conversation archive is refused.

## Every job, with an editor's token

`run.json` is a scoring run declaration with the pins the job fills in:
`--code-commit` pins `variant.code` to the commit, `--artifact
generation_config=path` stages a file the variant pins, and `--recording`
stages the answers `app.py` wrote. `policy.json` names what may not get worse:
`tolerance` per metric in its unit (a metric not named may not get worse at
all), `ignore`, and `critical_cases`.

`github-actions.yml` is the job. `just e2e-gate` runs the whole of this against a
server of its own and checks every exit code.

A regression suite a team can see is not a held-out measure of improvement:
passing it says nothing got worse on these cases, not that the variant is
better.
