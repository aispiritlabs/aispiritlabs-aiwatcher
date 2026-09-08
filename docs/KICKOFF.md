# Kick-off — the next work, in delivery order

- **Status:** active backlog after the 2026-09-08 review. Every item below is
  open until its acceptance evidence is recorded; this update changes the plan,
  not the implementation.
- **Audience:** whoever picks this up next, in a session that starts cold.
- **Last updated:** 2026-09-08

Managed Flow and marimo runs, execution controls, retention, schedules and the
chart exist. Recovery, scheduler concurrency and upgrade compatibility still
have gaps. Do not infer production readiness from “Phases 0–7 built” or from a
green happy-path suite. The authoritative delivery order and acceptance gates
are in [architecture §28](PIPELINE_ARCHITECTURE.md#28-migration-plan).

## Read in this order

1. This file: pick the first unfinished item below.
2. [Architecture §28](PIPELINE_ARCHITECTURE.md#28-migration-plan): scope,
   dependencies and the exit for that item. Original phase numbers remain
   reference labels, not the order of the remaining work.
3. [Review](PIPELINE_REVIEW_2026-09-08.md): the matching finding and reproduction.
   `R1–R7` are findings in the pending changes; `A1` predates them.
4. `CLAUDE.md`, then the relevant ADR: 0024 for curation blocks, 0025 for
   managed execution, 0026 for facts on the log. Section 43 records earlier
   reasoning; §43.35 qualifies the guarantees the review disproved.

## Start here — 1. Make upgrades compatible (R4)

**The next implementation task.** Resolve migration 0003 before releasing this
change. The old binary still names the columns it drops, and workers use
RollingUpdate. NULL values do not make the old SQL compatible with their removal.

Prefer a release that stops using the columns before a later release drops
them. Account for databases already at schema 4: do not rewrite an applied
migration and assume that repairs them. If a coordinated stop is required,
document the exact upgrade and rollback procedure in `docs/INSTALL.md`.

**Exit:** tests cover upgrade from schema 2/3, reopening schema 4, old/new
process compatibility under the chosen deployment procedure, and rollback
behaviour. Include migration 0004's preservation of `awaiting_input`. A fresh
schema test alone does not close this item.

## 2. Make local execution recover after partial writes (A1)

Choose the local commit/recovery mechanism, then implement and test it before
building more scheduler state on top of `WorkflowStore`. A durable input ID
must not suppress repair of a missing outbox or attempt row.

**Exit:** inject a failure after each write in one decision, retry and reopen;
there is recoverable work or an explicit terminal state, never a silently stuck
run. Test SIGKILL/restart and stale-lock handling. Upgrade an old `attempts.json`
by retiring terminal rows while preserving live and waiting attempts.

Keep `WorkflowStore::attempt()` for the contract suite and document that purpose
on the trait while working here; it is not a prerequisite design debate.

## 3. Make the scheduler reliable (R1, R2, R3, R5, R7)

Work in this order, following §28's scheduler gate:

1. Specify slot lifecycle, activation/edit boundaries, DST policy, catch-up,
   overlap scope and `run_now` request identity. Turn the review's examples
   into regression tests with supplied time and controllable I/O boundaries.
2. Implement transactional admission and durable per-slot processing. An
   asynchronous read model must not decide whether a run may start. Separate
   versioned schedule configuration from slot outcomes so a tick cannot undo
   an edit or DELETE.
3. Implement transient retry, restart/replay and edit-safe catch-up against
   those contracts. Verify the rule against both memory/file and PostgreSQL
   where each adapter's capabilities permit it.

**Exit:** two work replicas with a delayed projector cannot violate `skip`;
transient failures lose no slot; replay creates no duplicate; DELETE stays
applied; no slot predates activation; daily 02:30 in Warsaw has the same result
for one interval and many ticks across DST. A repeated `run_now` request has
one result even when the retry occurs in a later second.

Do not start Phase 8 to fix overlap. Transactional admission is execution state;
the product's history list still comes from the log's fold.

## 4. Make the panel report the server's answer (R6)

Handle SDK errors in pause/resume/cancel/retry/answer/delete mutations. Treat
404 as absence and show other read errors as failures. Preserve form state
when DELETE is refused. Keep the server's allowed actions authoritative.

**Exit:** UI tests cover 403, 409, 503 and successful 204; failures do not invoke
the success flow or present existing data as deleted. Record the API response
change from `RunProjection` to `RunView` for clients.

This is independent of storage implementation and may be completed earlier;
it does not remove the release gates in items 1–3.

## 5. Finish historical context, then the canvas (Phases 4, 6, 7)

1. Preserve notebook source as an immutable artifact and execute it by digest.
   Checking the current file against an old SHA detects drift but cannot recover
   the historical source.
2. Add editor sessions that resolve the pinned code, input and parameters after
   a reload, with permissions and expiry as in §16.3.
3. Serve the authored-block-to-plan-step mapping from the server. Light canvas
   blocks only when the draft matches the run's pinned revision; show drift
   otherwise. The browser must not reconstruct the mapping or execution rules.

**Exit:** change the notebook head, reload an old execution and open its exact
source and input; retry uses the pinned source. A matching canvas shows step
states, and an edited draft is explicitly marked as different.

Whole-execution `mode: "preview"` is outside the current delivery scope. The
ad-hoc editor path and bounded step previews remain; revisit simulation only
when a named use case requires execution without publication.

## 6. Deliver one worker integration (Phase 10 → Phase 11 Level 2)

Implement worker identity, claim/heartbeat/report, artifact access and the Python
SDK around one real task. Then move Planner's four stages onto that boundary
and compare their output with the direct path. Level 0 observation can precede
this because it does not hand execution ownership to aiwatcher.

**Exit:** a two-step worker plan survives worker death; the four-stage import
runs with Flyte off and produces byte-identical review artifacts. Run the
integration's own tests in its repository against the pinned SDK.

## After these gates

- **ContainerJob (Phase 12):** after item 6 and a concrete need for one pod per
  stage; preserve the byte-identical result gate before removing Flyte.
- **Hosted decider (Phase 13):** after item 6, for the agent graph's durable
  join. It does not require container jobs first.
- **Human input (Phase 14):** add an authored approval gate when needed; the
  existing Answer route is not an end-to-end feature. A curation approval can
  follow items 1–5 without waiting for hosted mode; in-turn agent approval
  depends on Phase 13.
- **Engine ownership (Phase 9):** only for a consumer that needs a unified
  launch API; it is not a prerequisite for the Python worker.
- **Phase 8 / Phase 15:** retain their own measurement/use-case gates.
- Other definition schedules wait for a second compiler. Flow `join` waits for
  a concrete sub-pipeline use case. Measure artifact/staging growth before
  designing retention or GC that could remove referenced data.

## Verify the item you changed

Use the repository command convention:

```bash
rtk just check
rtk just test-postgres
```

Use the dedicated test database (`rtk just postgres-up` if it is not running).
`just check` does not cover PHP or Python; run `rtk just flow-check` and
`rtk just ml-pipeline-check` when those paths change. Route/type changes require
`rtk just openapi` and both the contract and generated panel client.

The review recorded 334 Rust tests, 5 PostgreSQL tests, 101 Flow tests, 41 Python
tests and panel typecheck passing. It did not run full `just check`, browser
acceptance or rolling upgrade tests. That is baseline evidence, not proof that
any item above is closed. Record the new regression/acceptance evidence in §28
when an item is complete; keep the review as the dated finding record.
