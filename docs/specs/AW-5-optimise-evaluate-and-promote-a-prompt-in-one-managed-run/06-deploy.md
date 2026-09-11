---
id: AW-5
step: deploy
status: done
branch: main
repo: aiwatcher
created: 2026-09-11
updated: 2026-09-11
tags: [spec/AW-5, step/deploy, branch/main, status/done]
---

`#spec/AW-5` · `#step/deploy` · `#branch/main` · repo `aiwatcher`

# ⑥ Deploy — AW-5

## Shipped
- [x] Merged: on `main` — `1d5b0ca` (the rules, the SDK, the contract),
      `181c47c` (`just e2e-optimise`), and the spec and document commits
- [ ] Released: **not pushed.** Pushing is the owner's decision, and nothing in
      this change is released until they make it.
- [x] Rollout / flag: none. Every field is additive and optional. The one
      behaviour change, the `production` refusal, applies from the first start
      on the new binary.

## Post-deploy checks
- [x] Behaviour matches the spec where it runs: `just e2e-optimise` against a
      server built from this commit — approved, kept and rejected, 7 of 7 each
- [x] No regression in the signals this change could move: the contract is
      current, the panel builds and its tests pass, `cargo test` and the Python
      SDK (422) are green. `just check`'s only red step is `comments`, on five
      panel files from `ea2dfe7`.

## Graduated decisions
- **A report names the run and step that measured it; an evaluation is a
  worker task, not a binding** →
  [ADR_0010, amendment 2026-09-11](../../ADR/ADR_0010_EVALUATION_REPORTS.md)
- **`production` answers to the verdict; an optimisation names its held-out
  reports and never resolves them** →
  [ADR_0011, amendment 2026-09-11](../../ADR/ADR_0011_PROMPT_REGISTRY.md)
- **Never let `production` name a candidate the verdict turned down** →
  `CLAUDE.md`, Guardrails
- **Why there is no `EvaluationSuite`** → `docs/PIPELINE_ARCHITECTURE.md` §35,
  rule 6

## Archive
- ADDED:
  - a report knows the step that produced it;
  - an optimisation names the reports behind its held-out scores;
  - `production` never points at a rejected candidate;
  - one managed run optimises, evaluates and promotes.
- MODIFIED: Phase 15's scope in the plan — delivered; distributed mode is
  recorded as AW-2 Phase F's, with its two departures.
- REMOVED: the `EvaluationSuite` binding.

Still open outside this spec:
- planner adopting it, as planner's own ticket;
- the panel's Promote button on a rejected candidate;
- the five over-long panel comments;
- Option C — the server deriving scores from reports — behind its gate.

## Log
- 2026-09-11 12:35 — shipped to `main` locally; not pushed
