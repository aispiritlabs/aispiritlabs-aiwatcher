---
id: AW-2
title: Merge ai_spirit_agent into aiwatcher as the agent SDK
step: spec
status: doing
branch: main
repo: aiwatcher
created: 2026-09-10
updated: 2026-09-10
tags: [spec/AW-2, step/spec, branch/main, status/doing]
---
<!-- spec-card -->

`#spec/AW-2` · `#branch/main` · repo `aiwatcher`

# AW-2 — Merge ai_spirit_agent into aiwatcher as the agent SDK

> Branch `main`

## Phases
- [x] [① Investigation](01-investigation.md)
- [x] [② Spec](02-spec.md)
- [ ] [③ Job](03-job.md)
- [ ] [④ Tests](04-tests.md)
- [ ] [⑤ Review](05-review.md)
- [ ] [⑥ Deploy](06-deploy.md)

## Summary
`ai_spirit_agent`'s SDK layer becomes aiwatcher's, and agent-to-agent messaging
moves off Apache Iggy onto aiwatcher's hosted execution stream. §40 is the design;
this spec adds the repository dimension §40 does not cover — one distribution
boundary, one Python floor, a local outbox, and an agent that is registrable as a
workflow on its own.

## Log
- 2026-09-10 07:56 — spec opened on `main`
- 2026-09-10 07:57 — investigation written; six phases, three distributions, Phase A wires a seam already built on both sides
- 2026-09-10 08:01 — Phase B started in the SDK: `aiwatcher_sdk.optimization` holds the held-out split the DeepEval bridge had been leaving to every caller. `just sdk-check` green (336).
- 2026-09-10 08:07 — spec drafted: 6 added, 3 modified, 2 removed requirements over 15 scenarios
- 2026-09-10 08:11 — Phase D core landed: `aiwatcher_sdk.outbox` — the port, a memory and a SQLite adapter under one contract suite, and `drain`. `just sdk-check` green (362).
- 2026-09-10 08:27 — Phase A, in `ai_spirit_agent`: `agentic_graph.durable` closes the seam nothing had ever built — `build_compiled_graph_system(execution_id=…)` now resolves a hosted join. 71 graph tests, 1079 unit tests green. `declare_graph` still unwired.
- 2026-09-10 08:39 — The SDK pin in `ai_spirit_agent` switched from rev `770eb97` to the sibling path, and the root now asks for `agentic-runtime[aiwatcher]` so it installs. The agent reads the working tree's SDK; the durable-join test that was skipping now runs. Costs CI until the repositories are one.
- 2026-09-10 08:46 — Phase B complete: `evaluation.splits` groups a corpus by conversation (flows and prefill chains, as connected components), `evaluation.measurement` is the one way a golden is asked, and `optimize_and_record` searches on dev and reports both sides through ADR_0011. 21 new tests; 1101 unit tests green.
