---
id: AW-3
title: DataFusion and DuckDB as query and curation engines
step: deploy
status: done
branch: feat/agent-sdk-merge
repo: aiwatcher
created: 2026-09-10
updated: 2026-09-11
tags: [spec/AW-3, step/deploy, branch/main, status/done]
---
<!-- spec-card -->

`#spec/AW-3` · `#branch/feat-agent-sdk-merge` · repo `aiwatcher`

# AW-3 — DataFusion and DuckDB as query and curation engines

> Branch `feat/agent-sdk-merge`

## Phases
- [x] [① Investigation](01-investigation.md)
- [x] [② Spec](02-spec.md)
- [x] [③ Job](03-job.md)
- [x] [④ Tests](04-tests.md)
- [x] [⑤ Review](05-review.md)
- [x] [⑥ Deploy](06-deploy.md)

## Summary
DataFusion, and DuckDB after it, beside Flow PHP in the Query tab, the curation
blocks and the corpus integrations, chosen per deployment. `benchmarks/curation`
measured the gap over 5 GB: Flow 548.8 s, DataFusion 1.68 s at 280 MiB, DuckDB
2.02 s at 677 MiB. Polars was as fast and peaked at 5.4–5.5 GiB, so it is not one
of them.

## Log
- 2026-09-10 13:13 — spec opened on `feat/agent-sdk-merge`
- 2026-09-10 13:22 — investigation written
- 2026-09-10 13:31 — spec drafted: engine chosen at deployment, Polars as Python, services/query
- 2026-09-10 14:56 — spec amended: DataFusion as the third engine option
- 2026-09-10 15:05 — DataFusion in Rust measured at 93 MiB over 5 GB; in-process execution noted for the job phase
- 2026-09-10 15:14 — spec amended: DataFusion as Python, SQL in no engine
- 2026-09-10 15:20 — decided by the user: three engines beside Flow — polars, datafusion, duckdb — each through its own Python API
- 2026-09-10 15:26 — decided by the user: Polars dropped for its peak memory; three options — flow, datafusion, duckdb last as the addition; spec retitled from "Polars as a second query and curation engine"
- 2026-09-10 15:42 — job planned on `feat/agent-sdk-merge`: five phases, 29 tasks
- 2026-09-10 16:24 — phase 1 of the job complete (tasks 1.1–1.9): the query engine generalised with Flow as its only implementation and no behaviour change
- 2026-09-10 18:16 — phase 2 tasks 2.1–2.3 landed: the shared Python contract, the fork-server sandbox and strict admission, proven against an Arrow-only test engine; the work continues on `main`, where PR #2 merged phase 1
- 2026-09-10 18:24 — phase 2 of the job complete (tasks 2.1–2.5): the contract every Python engine shares, its sandbox, strict admission, and a conformance suite Flow passes — 14 of 29 tasks ticked
- 2026-09-10 19:22 — 3.1–3.2 landed: the DataFusion engine answers conformance q1–q4 as Flow does; a macOS fork-safety crash in the shared child fixed; 16 of 29 tasks ticked
- 2026-09-10 19:47 — 3.3, 3.5 and 3.6 landed: the compiler's DataFusion step and its refusal, five seed variants, the image, proven on Linux; 19 of 29 tasks ticked
- 2026-09-10 19:51 — 3.7–3.8 landed: DataFusion measured through the service at 5 GB (the query's child at most 234 MiB) and run end to end on the server, the Flow variant refused with a 422; 21 of 29 tasks ticked. 3.4, the panel, waits on the panel's uncommitted restructure
- 2026-09-10 20:23 — 3.4 landed: the panel follows the deployed engine — Build mode, compilePython, recipe and pipeline content, other engines' content shown and not run — proven in the browser on DataFusion; phase 3 complete, 22 of 29 tasks ticked
- 2026-09-11 11:20 — verification run: every AW-3 scenario green, the timeout one after the chart pairing the user chose; verification red: just check's comment lint, on two files outside AW-3 (conversations/payload.rs, server/conversations.rs)
- 2026-09-11 11:21 — correction: the second just check run flags one file outside AW-3, not two — crates/aiwatcher-conversations/src/payload.rs; server/conversations.rs and sdk/agentic/pyproject.toml match HEAD and passed it
- 2026-09-11 11:40 — verification green: payload.rs's module doc trimmed, the user's call; just check 19 of 19
- 2026-09-11 11:57 — review recorded: approved — seven findings fixed (a URL query's engine, the image trap, the timeout refusal, stranding documented, Flow wording, example labels, a README default), two follow-ups deferred
- 2026-09-11 11:59 — just check after the review's fixes: 18 of 19, cargo test 41 suites 1102 passed; typos red on 04-tests.md quoting the earlier typo, reworded, and typos then clean over the repository

## Shipped
- 2026-09-11 — see [⑥ Deploy](06-deploy.md). Committed on `main` as `4d736b2` (AW-3 outside the panel), `ea2dfe7` (the panel, with the restructure it sits in) and `fac6a8c` (the justfile); not pushed. The decision is [ADR_0028](../../ADR/ADR_0028_QUERY_ENGINES.md). The Flow-era aliases go in the release after this one — the list is in the deploy note.
- 2026-09-11 12:06 — shipped: committed on main as 4d736b2, ea2dfe7 and fac6a8c, not pushed; ADR_0028 graduated; the card is Done
