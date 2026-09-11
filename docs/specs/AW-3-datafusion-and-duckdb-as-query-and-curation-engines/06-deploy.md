---
id: AW-3
step: deploy
status: done
branch: main
repo: aiwatcher
created: 2026-09-11
updated: 2026-09-11
tags: [spec/AW-3, step/deploy, branch/main, status/done]
---

`#spec/AW-3` · `#step/deploy` · `#branch/main` · repo `aiwatcher`

# ⑥ Deploy — AW-3

## Shipped
- [x] Committed on `main`, by path, as the user chose on 2026-09-11:
      `4d736b2` AW-3 outside the panel (the engines, the Rust steps, the chart, Compose,
      CI, the examples and seed, ADR_0028 and its amendments, the docs — 111 files);
      `ea2dfe7` the panel restructure with AW-3's panel half inside it (154 files — the
      restructure is not AW-3's, and its panel edits could not be split from it by path);
      `fac6a8c` the justfile's query recipes, which `4d736b2` left out by mistake — its CI
      job calls `query-contract-check` and `query-conformance`, so the two go out
      together; and `cd18692`, `payload.rs`'s module doc, which is not AW-3's and was
      trimmed at the user's call to turn `just check` green
- [x] Pushed by the owner on 2026-09-11, with `5e4df79` on top; `origin/main` holds every
      AW-3 commit. The push runs `release-images.yml`, which now also publishes
      `aiwatcher-query-datafusion` and `aiwatcher-query-duckdb`
- [x] Rollout: no flag. `AIWATCHER_QUERY_ENGINE` defaults to `flow` and `query.engine` to
      `flow`, so a deployment that sets nothing runs as it did; switching is that one value

## Post-deploy checks
- [ ] Behaviour matches the spec where it now runs — it runs nowhere but this checkout
      yet. What was verified is the working tree these commits came from (`04-tests.md`,
      and `05-review.md`'s re-run); the first runs outside it are CI's `query` job per
      engine and the image build after the push
- [ ] No regression in the signals this change could move — the same: after the push,
      CI green for every engine, and a Flow deployment's managed runs unchanged
- **CI on the push** (run 34589791754): every job green but the three Python ones, which
  failed on one test, `test_a_query_naming_a_clock_is_not_deterministic` — DataFusion's
  `now()` on Linux carries nanoseconds, and `rows_of` could not make a `datetime` of
  them; a macOS clock ticks in microseconds, so no local run saw it. Fixed in the working
  tree: `rows_of` casts nanosecond times to microseconds, the answer's precision, pinned
  by `test_a_nanosecond_time_is_answered_to_the_microsecond_on_any_clock`, which fails
  without the fix on any clock; `just query-contract-check` 187 passed
- Left uncommitted and not in these commits: `sdk/agentic/tests/test_gemma_prompt.py`
  and `sdk/python/aiwatcher_sdk/outbox.py`, other work in the same checkout

## Graduated decisions
- A deployment chooses one query engine; content names its engine; a typed query is
  admitted (`strict`) or runs where code runs (`open`) → [ADR_0028](../../ADR/ADR_0028_QUERY_ENGINES.md),
  with what switching strands added by the review
- The query surface is no longer Flow alone → [ADR_0008](../../ADR/ADR_0008_FLOW_QUERY_SURFACE.md),
  [ADR_0014](../../ADR/ADR_0014_DATA_CURATION.md) and [ADR_0024](../../ADR/ADR_0024_CURATION_BLOCKS.md),
  each amended and pointing at ADR_0028
- The smaller rules this change made live beside their code rather than in an ADR:
  the chart's timeout pairing (`_helpers.tpl`, `docs/INSTALL.md`), `writtenFor` on a URL
  (`shared/lib/query.ts`), and the guardrails `CLAUDE.md` gained

## Archive
- ADDED: a deployment chooses one query engine, `flow | datafusion | duckdb`; one query
  contract, proved against every engine by one conformance suite; a DataFusion query is
  ordinary DataFusion Python, and a DuckDB query ordinary DuckDB Python; running typed
  Python has a stated boundary, `open` or `strict`; the Python engines' catalog is Flow's;
  a managed step runs on the deployed engine, and a plan for another is a 422 naming both;
  content names the engine it was written for; the panel follows the deployed engine
- MODIFIED: the engine's address is `AIWATCHER_QUERY_URL`; the query limits are the
  engine's, and the chart keeps the engine's above the step's; the engines live side by
  side under `services/query/`; the deployment values name the engine; a query surface is
  admitted, or runs where code runs
- REMOVED: nothing in this release. In the release after it, the Flow-era names read as
  aliases: `AIWATCHER_FLOW_URL` (server config, the Vite proxy, the justfile);
  `install.sh`'s `AIWATCHER_FLOW` and `AIWATCHER_FLOW_IMAGE`; the `flow-install`,
  `flow-serve`, `flow-test`, `flow-lint`, `flow-fmt`, `flow-fmt-check`, `flow-check` and
  `flow-query` recipes; the `/flow/*` routes Flow answers beside `/query/*`, with the Flow
  executor's `/flow`, Vite's `/flow/` proxy, nginx's `/flow/` location and rewrite and the
  chart's `/flow/healthz` probe; the chart's `flow.*` values, `panel.flowUpstream`,
  `execution.flowUrl` and the schema's `flow` block

## Log
- 2026-09-11 12:06 — shipped from `main`: committed as 4d736b2, ea2dfe7 and fac6a8c (with cd18692 beside them), not pushed — the owner's step; ADR_0028 and three amendments graduated
- 2026-09-11 12:34 — pushed by the owner (origin/main at 5e4df79); CI run 34589791754 and release-images run 34589791847 started
- 2026-09-11 12:39 — CI red on the push, one test on Linux's nanosecond clock; fixed in the working tree (rows_of to microseconds), query-contract-check 187 passed, not yet committed
