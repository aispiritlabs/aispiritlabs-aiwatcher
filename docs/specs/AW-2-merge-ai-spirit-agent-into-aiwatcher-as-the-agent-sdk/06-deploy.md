---
id: AW-2
step: deploy
status: done
branch: feat/agent-sdk-merge
repo: aiwatcher
created: 2026-09-14
updated: 2026-09-14
tags: [spec/AW-2, step/deploy, branch/feat-agent-sdk-merge, status/done]
---

`#spec/AW-2` · `#step/deploy` · `#branch/feat-agent-sdk-merge` · repo `aiwatcher`

# ⑥ Deploy — AW-2

## Shipped
- [x] Merged: [PR #2](https://github.com/aispiritlabs/aispiritlabs-aiwatcher/pull/2),
      `125ec93` on 2026-09-10, and on `origin/main` after it: the runtime move
      (`4816f1e`), a turn naming its prompt version (`5376e65`), the licence
      (`d15fd8d`), `ai_spirit_agent`'s MIT notice (`de62911`) and the GUI review
      (`70a1b53`)
- [ ] Released: no tag. `aiwatcher-agentic` and `aiwatcher-sdk` are built from
      this checkout; nothing publishes either wheel.
- [x] Rollout / flag: none on aiwatcher's side. `AGENTIC_TRANSPORT=laser` is
      refused by name.
- [ ] `ai_spirit_agent`'s half: **parked by the owner** on 2026-09-11 —
      `feat/agent-sdk-merge` at `2fd7db0`, not pushed; its path sources to this
      checkout still break its CI.

③ job, ④ tests and ⑤ review have no notes. The work ran as Phases A–F out of
the index's log, each with its gate recorded there, and the owner closed the
spec on that record rather than on notes written after the fact.

## Post-deploy checks
- [x] Behaviour matches the spec where it runs — the gates the log records:
      `just e2e-agent-join` 8/8, `e2e-agent-outbox` 11/11,
      `e2e-agent-standalone` 14/14, `e2e-agent-transport` 11/11,
      `e2e-agent-graph` 8/8, `e2e-agent-prompt` 6/6
- [x] No regression: CI's `sdks` job (`sdk/python`, `sdk/agentic`,
      `sdk/typescript`) is green on `1405292`, 2026-09-14. Not re-run for this
      note.

## Graduated decisions
- **Three distributions, split by failure policy; the engine is the standard
  library alone and declares ports for a model, a tracer, a prompt and
  settings; stored records keep their wire names** → `CLAUDE.md`, *Python SDK*
- **A hop between two agents is a run of the target's one-step workflow, on
  aiwatcher's claims** → `CLAUDE.md`, *Python SDK*
- **The repository is PolyForm Noncommercial 1.0.0, and a contribution grants a
  sublicensable licence** → `LICENSE`, `NOTICE`, `CONTRIBUTING.md`

## Archive
- ADDED:
  - a held-out split is dealt by group;
  - an agent is registrable without a graph;
  - a hop survives aiwatcher being unreachable (the DuckDB outbox);
  - agent messaging runs on aiwatcher's claims;
  - one distribution per failure policy;
  - the engine's records keep their wire names;
  - the old import path is the same code for one release;
  - the agent core names its ports and carries no model stack;
  - the runtime is handed what it read from the application;
  - a graph's turn is drawn against the shape it declared;
  - an agent's named prompt is ADR_0011's current version;
  - a turn names the prompt version it ran on.
- MODIFIED: the Python floor is 3.13; an agent's durable history is shared; an
  optimisation is recorded, not written to a file.
- REMOVED: Apache Iggy as the agent transport; the MLflow prompt registry.

Still open outside this spec, from the owner's park: `packages/workshops`,
which the `evaluation` scorers move; Phase B's unticked criterion (a candidate
admitted on held-out and one refused, both in the panel); a traversal joined to
the hosted execution it runs in; a summarizer fired outside `run` that declares
nothing; authoring an agent graph, a spec of its own; and an agent's
`before_tool_execute` asking through `TaskContext.ask`.

## Log
- 2026-09-14 11:46 — closed at the owner's ask: the aiwatcher half shipped through PR #2 and after it; `ai_spirit_agent`'s half stays parked
