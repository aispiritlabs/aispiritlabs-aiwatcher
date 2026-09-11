---
id: AW-3
step: review
status: doing
branch: main
repo: aiwatcher
created: 2026-09-11
updated: 2026-09-11
tags: [spec/AW-3, step/review, branch/main, status/doing]
---

`#spec/AW-3` · `#step/review` · `#branch/main` · repo `aiwatcher`

# ⑤ Review — AW-3

**Reviewer:** Claude Code, three read-only passes in parallel, at Mateusz Kubaszek's request
**PR:** none yet — AW-3 sits uncommitted on `main`
**Outcome:** approved, with the fixes below made and re-verified, and the deferred items named

## How it was reviewed

Three passes over the uncommitted tree, each against `02-spec.md`, `03-job.md` and
`CLAUDE.md`'s guardrails: the Python engines (the contract, DataFusion and DuckDB, with
`strict` admission probed against the real engines), Rust and deploy (execution, the
server, the chart, Compose, CI and the justfile), and the panel and the docs. The tree also
holds AW-4's uncommitted Flyte retirement and the panel restructure. The Rust pass separated
them file by file, and anything found in AW-4's files is left to AW-4.

## Checklist
- [x] Every spec requirement is implemented; nothing unspecified was added — the one gap, a
      query in a URL with no engine (finding 1), was a SHALL and is closed
- [x] The decisions in `03-job.md` are the ones the code took — one `RuntimeKind` per engine,
      `FlowPhp`/`flow_php` untouched, the fork server, derived `strict` admission, the
      compiler's `df = (…)` script, only the deployed engine's executor registered
- [x] The repository's guardrails hold for the paths this change touched
- [x] Tests cover the spec scenarios and fail without the change (`04-tests.md`), plus
      `shared/lib/query.test.ts` for finding 1
- [x] Generated contracts and clients regenerated — `just check`'s *openapi contract is
      current*; committed with the change, which is not committed yet
- [x] No secrets; no backward-incompatible migration in one release — no migration, every
      stored digest and Flow `plan_id` kept, the Flow-era names read for one release
- [x] A decision worth keeping is queued for the decision record — ADR_0028 is written
      (5.1), and finding 4 adds a cost to it

## Findings

Fixed in this review, each re-verified:

1. **A query in a URL carried no engine.** A `?q=` on the Query tab or the recipe editor
   was run by whatever engine was deployed, so a Flow link opened on a DuckDB deployment came
   back a syntax error — against *content written for another engine SHALL be shown, not
   run*. → `writtenFor` on both routes' search, written wherever the page writes `q`;
   `linkedEngine` reads a link without it as Flow, the rule stored content already lives
   under, and `shared/lib/query.test.ts` pins it. In the browser against a DuckDB engine: a
   Flow link read-only with the sentence and Run, Test, Simulate, Execute and Save disabled;
   a DuckDB link editable and run; Write recording `writtenFor=duckdb`, kept through a Run.
2. **A copied `.env` put another engine's image on a switched release.** `.env.example`
   sets Flow's image, and the release summary printed two live `AIWATCHER_QUERY_IMAGE`
   lines, the last winning; with `AIWATCHER_QUERY_ENGINE=datafusion`, the DataFusion slot
   ran Flow's image, went Ready and failed every step. → `install.sh` refuses an image named
   for another engine (Flow's with `datafusion` refused, a matching or custom name admitted);
   the summary prints one live line and the other engines' commented out.
3. **The chart rendered a query ceiling at or below the step's.** `query.timeoutSeconds:
   120` against a 300 s step rendered, and the engine would fail the step with a 422 at two
   minutes — the spec's MUST, unkept in the one place that can keep it. →
   `aiwatcher.queryTimeoutSeconds` fails naming both values, and both go through `int`,
   because a `--set` of a million rendered `1e+06`. Renders: 300 → 360, 7200 → 7260, 900
   kept, 120 and 300 refused, 1 000 000 → 1 000 060; `just chart-check` green.
4. **Switching engines strands in-flight attempts without a word.** An attempt of the old
   engine still pending or retrying is claimed by nothing afterwards; ADR_0028 said the
   stranding was visible. → ADR_0028's costs and `INSTALL.md` say so, and say to switch
   between runs. A start-up warning counting such attempts is deferred.
5. **Flow's name on paths all three engines share.** `flow-preview.tsx` ("The Flow service
   stopped responding", `just flow-serve`), `hub-discovery.tsx`, the Query tab's install
   hint (`query.engine=flow`, two images of three), `query-builder.ts`'s header, the Query
   route's doc and `README.md`'s `just flow-serve`. → engine-neutral.
6. **The DuckDB example pipelines titled their transform "DataFusion: …"**, and both
   `titanic-survival` variants called themselves "one Flow PHP query". → corrected; the
   seed rebuilt, and `build_seed.py --check` passes.
7. **`services/query/README.md` gave `AIWATCHER_QUERY_CONCURRENCY` no default**, where
   `config.py`'s is 4. → 4.

Accepted:

- **The Query tab's ceiling is 360 s wherever managed execution is on**, where it was the
  engine's 30 s. It is the one value a person's query and a step both run under, paired as
  the user decided on 2026-09-11; with execution off it stays 30 s, and
  `query.timeoutSeconds` sets it outright.

Deferred:

- **The panel's shipped engine content has no committed `strict` test.** The examples,
  starters and cheat sheets in `shared/lib/duckdb.ts` and `datafusion.ts` were admitted once
  by hand in phases 3 and 4, which is the run that found `coalesce`; Build mode, the
  promotion and the hub import are pinned by `test_*panel_shapes.py`. A follow-up task.
- **A start-up warning for stranded attempts** (finding 4). A follow-up task; not built here
  because the server's wiring is under AW-4's uncommitted edits.
- Nits: CI's `datafusion` and `duckdb` matrix entries each re-run the workspace's
  `query-contract-check`; `just query-serve` derives 360 s with no managed execution too;
  Compose fixes 360 and says to raise the two together.
- Left to AW-4, whose uncommitted edits hold the files: `aiwatcher-server/Cargo.toml`'s
  "the Flow query service" comment, and `docs/decisions/EXECUTION.md` still calling
  ADR_0016 unchanged.
- Found in passing and older than AW-3 (the same in `HEAD`): the Datasets promotion's and
  the dataset explorer's links into Data Curation target `/data-curation`, whose index
  redirects to the pipeline canvas and drops `q`. A follow-up task.

Checked and sound: `strict` admission — a name the query assigns cannot reach anything
`DECLINED`, text reaches no relation method by any of the routes probed, and table functions
are refused through `FunctionExpression`; the DuckDB lock is applied in the child before any
query line; the fork server never runs a query; `deterministic` and HUGEINT; a Flow plan's
`plan_id`; cache keys by kind; the 422 at start before any run exists; the claim filter; the
NetworkPolicy for every engine and both roles; the image names across the chart, the release
workflow and `Dockerfile.query`; the OpenAPI diff; Build mode's DuckDB output admitted under
`strict`, and dropping rather than refusing; ADR_0028 and the three amendments.

## Verification after the fixes
```
npx vitest run (apps/panel)          — 178 passed
npm run build (apps/panel)           — built; prettier clean on every touched file
just chart-check                     — lint 0 failed; default 17, planner 25, valid
helm template (9 cases)              — as finding 3
the install.sh guard (6 cases)       — as finding 2; bash -n clean
python3 examples/build_seed.py --check — current
typos · scripts/lint-comments.py     — clean
the panel, in the browser            — as finding 1
just check                           — re-run after this note; the result is in the log
```

## Log
- 2026-09-11 11:57 — review recorded on `main`: approved — seven findings fixed and re-verified, one accepted, the rest deferred by name
- 2026-09-11 11:59 — just check after the review's fixes: 18 of 19, cargo test 41 suites 1102 passed; typos red on 04-tests.md quoting the earlier typo, reworded, and typos then clean over the repository
