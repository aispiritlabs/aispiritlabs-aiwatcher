# aiwatcher-scorers

The scorer service behind a scorecard's **external metrics**: DeepEval's and
Opik's metrics today, and any framework an adapter wraps tomorrow, behind one
contract aiwatcher's work role speaks. aiwatcher never imports a framework and
no scorecard carries code — a card names an adapter, a metric and its
parameters, and this service runs it.

```
aiwatcher serve role ── pins a card against ──► recorded catalog ◄── records ── work role
                                                                                 │
                                         POST /scorers/score, one case at a time  │
                                                                                 ▼
                                                        aiwatcher-scorers ──► DeepEval / Opik
                                                                          └─► the graded metrics' model
```

## The contract

Two routes, and no framework's name in either (`aiwatcher_scorers/contract.py`,
and `aiwatcher_evaluation::external` on the Rust side; the fixtures in
`contracts/fixtures/scorers-v1/` are read by both test suites).

- `GET /scorers/catalog` — every adapter at the release installed, the model
  its graded metrics ask, and each metric with its unit, which way is better
  (`higher | lower`), how a result folds it (`mean | rate`), what it reads
  (`input | answer | expected`), whether a model grades it, its range and the
  parameters it takes.
- `POST /scorers/score` — one metric over up to 64 cases, answered in order
  with `{"value": …}` or `{"failed": "…"}`. The request carries what the card
  pinned (`declared`): a release or a model other than the ones running here is
  a **409** naming both, never a number from something the card did not measure
  with. Unknown parameters, missing required ones and ones of the wrong kind are
  one **422** listing every problem.

**A reply never carries a model's words.** A framework's `reason` is a model's
text and can repeat the answer it was shown, so the contract has no field for
it, and a failed case is reported by the exception's class and a fixed phrase.

## Why the direction is not the author's

Publishing a card in aiwatcher copies the catalog's description of each external
metric into the card version — the release, the model, the unit, the direction.
A later catalog changes no published card; a run holds the service to the card;
and upgrading DeepEval is publishing the card again, which is a new suite
version and therefore a result that does not compare with the old one.

A graded metric's number is a model's word: the result says it is not
reproducible by re-reading, and the declaration warns that nothing measured how
often the model agrees with people (a rubric judge's calibration does that).

## Adding a framework

One module in `aiwatcher_scorers/adapters/` with four members — `name`,
`version`, `model` and a `metrics()` table of `Implemented(Metric, score)` — one
line in `adapters.KNOWN`, and an extra in `pyproject.toml`. Import the framework
inside the adapter, build every metric with its tracing off, and raise rather
than return a number the framework marked as failed. Nothing in aiwatcher
changes.

## Running it

```bash
just scorers-install
just scorers-serve          # 127.0.0.1:8083, both adapters, heuristics only
AIWATCHER_SCORERS_MODEL_URL=http://127.0.0.1:19086/v1 \
AIWATCHER_SCORERS_MODEL=gemma-4-e2b AIWATCHER_SCORERS_MODEL_REVISION=ud-q4-k-xl \
AIWATCHER_SCORERS_MODEL_PROFILE=llamacpp just scorers-serve   # and graded metrics
```

Then point aiwatcher's work role at it with `AIWATCHER_SCORER_URL=http://127.0.0.1:8083`.

| Variable | Meaning |
|----------|---------|
| `AIWATCHER_SCORERS_ADAPTERS` | Which adapters to load, `deepeval,opik` by default. One whose extra is not installed is left out with a warning. |
| `AIWATCHER_SCORERS_MODEL_URL`, `…_MODEL`, `…_MODEL_REVISION` | The one OpenAI-compatible model every graded metric asks. All three or none; none offers no graded metric. |
| `AIWATCHER_SCORERS_MODEL_PROFILE` | `openai`, or `llamacpp` to ask a thinking model not to think. |
| `AIWATCHER_SCORERS_MODEL_TOKEN` | A bearer credential for that model. |
| `AIWATCHER_SCORERS_HOST`, `…_PORT` | `127.0.0.1:8083`. |

**Never expose it.** It has no authentication and it is sent the cases it
scores — over a conversation cohort, the archive's words. Both frameworks phone
home by default; `adapters.quiet()` turns DeepEval's telemetry, Opik's tracing
and LiteLLM's price-table fetch off before either is imported.

`just scorers-check` runs `ruff format --check`, `ruff check`, `mypy --strict`
and `pytest` with both frameworks installed. Like the notebook runtime it is not
part of `just check`, and CI runs it in its own job.
