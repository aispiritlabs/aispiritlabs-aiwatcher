# Specs

One folder per unit of work, moving through six phases:

> **investigation → spec → job → tests → review → deploy**

[`BOARD.md`](BOARD.md) is the board — one column per phase, one card per spec.
Each spec folder holds `_index.md` (status, branch, rolled-up log) and one file per
phase: `01-investigation.md`, `02-spec.md`, `03-job.md`, `04-tests.md`,
`05-review.md`, `06-deploy.md`. The spec files follow OpenSpec conventions — a
proposal, a delta of ADDED / MODIFIED / REMOVED requirements, RFC-2119 keywords, and
`GIVEN/WHEN/THEN` scenarios a test can be derived from.

**This folder is the record of a change; it is not the record of a decision.**
Something that would be expensive to reverse graduates to [`docs/ADR/`](../ADR),
and the spec folder links it. Two copies of one piece of reasoning are two copies
free to disagree, and the day they do, nobody can say which is current — the same
rule `.claude/skills/README.md` states for vendored skills and `CLAUDE.md` states
for itself.

## Driving it

The flow ships with the repository: the contract is
[`.claude/skills/spec-flow/SKILL.md`](../../.claude/skills/spec-flow/SKILL.md) and
the eight commands are in [`.claude/commands/`](../../.claude/commands). A clone
needs no per-machine setup, and a change to how a phase behaves arrives in a pull
request like any other. The same skill is also installed globally on the machines
that use this flow in other repositories; here the project-scoped copy is the one
that loads (see [AW-1](AW-1-ship-the-spec-flow-with-the-repository/_index.md)).

```
/spec-new [ID] <title>      folder, index, card in Backlog
/spec-investigate [ID]      ① what is true now, and the options
/spec-propose [ID]          ② the behaviour contract
/spec-job [ID]              ③ the approach, the decisions, the tasks
/spec-tests [ID]            ④ the verification matrix
/spec-review [ID]           ⑤ the diff against the spec
/spec-deploy [ID]           ⑥ ship, graduate the decisions, archive
/spec-board [init|sync|show]
```

## Project hook

The facts the flow cannot guess about this repository. Everything here **points at**
the file that holds the rule; nothing here restates one.

- **ID prefix:** AW
- **Verification:** `just check` — everything CI runs. It deliberately covers neither
  the PHP service nor the Python toolchains, so a change that touches one runs its
  own gate too: `just sdk-check` (`sdk/python`), `just flow-check` (`services/query/flow`),
  `just ml-pipeline-check` (`services/ml_pipeline`). CI runs all three in their own
  jobs. A single test by name: `just test-one <pattern>`.
- **Contract regeneration:** `just openapi` after any change to an axum route or to a
  type that appears in one; commit `contracts/openapi.json` **and**
  `apps/panel/src/api/generated`. CI fails if either is stale.
- **Decision record:** [`docs/ADR/`](../ADR), written from
  [`docs/ADR/template.md`](../ADR/template.md); [`docs/decisions/`](../decisions)
  groups them into four readings. An ADR's value is its *Consequences* — what this
  costs and what would make it wrong.
- **Guardrails:** [`CLAUDE.md`](../../CLAUDE.md) — *The decisions that explain most of
  the code*, and the *Guardrails* list beneath it. A review reads the guardrails for
  the paths the change touched; a change that contradicts one is a decision to raise,
  not one to make quietly.
- **Panel:** `cd apps/panel && npm run build` (vite build, then a full `tsc` project
  check) when the change reaches `apps/panel`.
- **Diagrams:** `just diagrams` when a change moves something a diagram draws.

## Conventions

- The phase notes are committed on the branch the work happens on, so the spec is
  reviewed in the same pull request as the code it describes.
- A spec folder on a branch nobody merged is a proposal that was not taken. Keep it.
- `## Log` sections are appended to, never rewritten.
- `04-tests.md` records what a command actually printed. A scenario that cannot pass
  as written is an issue and a decision, never a deleted row.
