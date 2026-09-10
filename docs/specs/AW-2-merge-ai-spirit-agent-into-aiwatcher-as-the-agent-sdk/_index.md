---
id: AW-2
title: Merge ai_spirit_agent into aiwatcher as the agent SDK
step: spec
status: doing
branch: feat/agent-sdk-merge
repo: aiwatcher
created: 2026-09-10
updated: 2026-09-10
tags: [spec/AW-2, step/spec, branch/feat-agent-sdk-merge, status/doing]
---
<!-- spec-card -->

`#spec/AW-2` · `#branch/feat-agent-sdk-merge` · repo `aiwatcher`

# AW-2 — Merge ai_spirit_agent into aiwatcher as the agent SDK

> Branch `feat/agent-sdk-merge`

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
- 2026-09-10 09:04 — Committed on `feat/agent-sdk-merge` in both repositories, three commits each, by path. Not pushed.
- 2026-09-10 11:51 — Phase A proven on the wire: `just e2e-agent-join` runs three worker processes against a live server, SIGKILLs the first after two completions, and the join fires once from the stream with the words kept out of history. 8/8.
- 2026-09-10 12:21 — Phase D wired: with an outbox, `AiwatcherEventStore` writes a hop (an append checking no version) down before sending it, reads through an outage from what it last saw plus its waiting hops, and refuses a decision while its own hops wait — safe because facts are monotone and every decision is arbitrated. `python -m aiwatcher_sdk.outbox list|drain|requeue` is the operator door; `durable_join` opens one SQLite outbox from `AIWATCHER_OUTBOX`. `just e2e-agent-outbox` 11/11 on the live server, `just e2e-agent-join` still 8/8; SDK 394, agent 1103.
- 2026-09-10 13:18 — Two decisions from the user. A claim waits for aiwatcher — kept as it was, now a scenario. The durable outbox is DuckDB: `SqliteOutbox` is gone and `DuckdbOutbox` opens a connection per operation, because DuckDB locks its file exclusively and this file has an operator and several workers as readers (≈7 ms against 0.4 ms held). `duckdb` is an SDK extra that the agent's `aiwatcher` extra asks for. SDK 396, agent 1103, `e2e-agent-outbox` 11/11, `e2e-agent-join` 8/8.
- 2026-09-10 13:50 — Phase E: an agent is a workflow of its own. `aiwatcher_sdk.integrations.agentic.agent_workflow` turns a text-in, text-out turn into a one-step `Workflow` whose reply goes to a `PayloadStore` — the result carries reference, digest and size — and whose turn is tried once, because its tools write. `agentic_runtime.hosted.hosted_runtime` hands an `AgenticRuntime` to the SDK's `Runtime`, which registers each agent, answers it through the targeted-turn path so the router is never asked, and closes it through the new `on_close`. `just e2e-agent-standalone` 11/11 on the live server: started as the panel's launcher starts one, again from a schedule's `run_now` answering the registered default, no `agentic_graph` imported, the reply's words in no response. SDK 409, agent 1113, join 8/8, outbox 11/11.
- 2026-09-10 14:16 — Phase E, the three follow-ups the user settled. **Fine-tuning data**: `agent_workflow(archive=…)` records every turn in the conversation archive as one exchange joined to the run, before the step reports — unreachable is `infrastructure` and retried, a refusal is `policy` and a person decides. **Spans**: the worker publishes the attempt in `current_attempt`, and a tracer built before any attempt defers to it — no root run per turn, `agent.*` under the step. **At least once, under control**: the server's default budget back in place of one attempt; `TaskContext.step_key` is the same on every attempt and becomes the message's idempotency key and the archive's message ids, so a retry overwrites and a keyed tool writes once; `retry=` per agent. `just e2e-agent-standalone` now starts its own server with the archive on and passes 14/14: a first attempt lost after its tool wrote is retried and the tool writes once, both attempts' spans nest under the step, the exchange is archived once, approved and exported as one `sft` row. SDK 418, agent 1116, join 8/8, outbox 11/11.
- 2026-09-10 15:29 — Phase F: Apache Iggy is out. A hop between two agents is a run of the target agent's one-step workflow (`<prefix>.<agent>`, step `hop`, its own queue), started under the message id as `Idempotency-Key`, so a message sent twice is one run; aiwatcher's claim, lease, retry budget and failed step are the consumer group, `XAUTOCLAIM` and the dead-letter topic, with no Rust change — a hosted run never schedules an attempt, so a hosted stream had nothing to claim. A client's replies go to its mailbox, one hosted execution per address. The registry is the definitions aiwatcher holds. `laser-sdk`, `loop.py` and the Laser tests are deleted; `AGENTIC_TRANSPORT=laser` is refused by name. Found on the way: lab 6's four messages had never survived `deserialize_record`, so its pipeline stopped at the first hop on every transport — fixed, with a test on the real classes. `just e2e-agent-transport` 11/11; SDK 418, agent 1127, standalone 14/14, join and outbox still green.
- 2026-09-10 16:07 — Phase C: the engine is `aiwatcher-agentic` in `sdk/agentic` — imported as `aiwatcher_agentic.workflow`, no runtime dependencies, 3.13 — and `aiwatcher-sdk` is on 3.13 with it. What wraps a concrete agent (`builder`, the LLM reactors) stayed in `agentic`; the engine declares `WorkflowTracer` and `AgentRun`/`ToolRun`/`AgentPrompt`/`ModelUsage` instead of importing them, and imports nothing from `providers`. `agentic.workflow` registers each moved module as the engine's own object, so one class and one monkeypatch whichever path. Records keep `agentic.workflow.…` as their wire name: the moved engine writes byte-for-byte what it wrote before, from a fixture made by the pre-move serializer. `mypy` at 3.13 found `uuid.uuid7`, 3.14-only, in `WorkflowRuntime`. `just agentic-check` 164, `just sdk-check` 418 on 3.13, agent repository 997 (969 + 28); standalone 14/14 and transport 11/11, join and outbox green against a server of their own because :8080 was down.
