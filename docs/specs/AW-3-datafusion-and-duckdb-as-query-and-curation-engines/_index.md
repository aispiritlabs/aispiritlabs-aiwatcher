---
id: AW-3
title: DataFusion and DuckDB as query and curation engines
step: job
status: doing
branch: feat/agent-sdk-merge
repo: aiwatcher
created: 2026-09-10
updated: 2026-09-10
tags: [spec/AW-3, step/job, branch/feat-agent-sdk-merge, status/doing]
---
<!-- spec-card -->

`#spec/AW-3` · `#branch/feat-agent-sdk-merge` · repo `aiwatcher`

# AW-3 — DataFusion and DuckDB as query and curation engines

> Branch `feat/agent-sdk-merge`

## Phases
- [x] [① Investigation](01-investigation.md)
- [x] [② Spec](02-spec.md)
- [x] [③ Job](03-job.md)
- [ ] [④ Tests](04-tests.md)
- [ ] [⑤ Review](05-review.md)
- [ ] [⑥ Deploy](06-deploy.md)

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
