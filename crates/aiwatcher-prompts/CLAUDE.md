# The prompt registry

The rules for `aiwatcher-prompts`. Read
[ADR_0011](../../docs/ADR/ADR_0011_PROMPT_REGISTRY.md). Publishing obeys the
root file's one ordering rule — the version object before the head that indexes
it — and what a *trace* may carry about a prompt is in the root file too.

- **Never let a client decide whether an optimisation was an improvement.**
  `OptimizationRecord::verdict` computes it in `aiwatcher-prompts`, from the
  held-out scores and `variables_lost`, and the API returns what it decided
  rather than what was sent. An optimiser selected its candidate by maximising
  the number it then reports; a registry that took its word is a filing cabinet.
- **Never admit a candidate on a dev score.** The dev split is what the search
  ran against, so a gain there is a hypothesis. An optimisation with no held-out
  measurement is recorded and refused a promotion, and `overfit_gap` is the
  number worth watching across a series.
- **Never promote a candidate that dropped a variable.** An optimiser that has
  stopped interpolating `{{ page }}` can score arbitrarily well on a harness
  that fed it fixed inputs. The bar is checked *before* the scores in `verdict`,
  so the reason says "it stopped reading its input" rather than inviting
  somebody to raise the iteration count.
- **Never let `production` name a candidate the verdict turned down.**
  `Registry::check_admitted` refuses it on `PUT /labels/production` and on a
  publish carrying `label: production` — a version is its text, so publishing a
  rejected candidate's text lands on the candidate. A candidate whose record is
  missing is refused too: it is written first, and an unwritten verdict is not
  an admission. `staging` and a version a person wrote stay free.
