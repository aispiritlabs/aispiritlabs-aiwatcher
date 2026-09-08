# Kick-off — the next work, in delivery order

- **Status:** active backlog after the 2026-09-08 review. Every item below is
  open until its acceptance evidence is recorded. Items 1, 2 and 3 are closed;
  items 4–6 are open.
- **Audience:** whoever picks this up next, in a session that starts cold.
- **Last updated:** 2026-09-08

Managed Flow and marimo runs, execution controls, retention, schedules and the
chart exist. Upgrade compatibility, local recovery and the scheduler are
closed; items 4–6 remain. Do not infer production readiness from “Phases 0–7 built” or from a
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

## 1. Make upgrades compatible (R4) — **done, 2026-09-08**

Staged removal, as this file preferred. Migration 0005 re-adds
`execution_runs.started_at` and `ended_at`; the drop returns in a later release,
once no binary that names them can still be running. 0003 stays applied and
unedited but for a comment pointing at 0005, because a recorded version is
skipped forever after. Rolling upgrade and image rollback both work with no
coordinated stop; `docs/INSTALL.md` has the procedure and `CLAUDE.md` carries
the rule.

Evidence: `crates/aiwatcher-execution/tests/postgres_upgrade.rs` — seven tests
over the previous release's own SQL, copied from `3117259`, each in a
PostgreSQL schema of its own. Upgrade from 2, from 3, reopening 4, fresh,
repeated `apply`, both binaries reading each other's rows, and 0004 keeping
`awaiting_input`. Removing 0005 from the runner fails six of the seven.
`just check` and `just test-postgres` green. Recorded in
[architecture §28](PIPELINE_ARCHITECTURE.md#28-migration-plan), work 1.

## 2. Make local execution recover after partial writes (A1) — **done, 2026-09-08**

An intent journal, not a transactional local adapter: the adapter's identity is
the write-ahead log's shape, and a local database would replace it rather than
repair it. `PendingCommit` holds the five writes one decision makes and is
`fsync`ed into `commits/` before any of them; that rename is the commit point,
`apply` is idempotent in every step, and the record is deleted only once it has
run. `recover` runs at `open` **and** at the top of every `append` — A1's
reproduction never restarted anything, so the repair has to come before the
duplicate check.

Three defects found by writing the tests this item asked for: a torn final line
was read as end-of-stream and then written behind (now truncated), the lock was
a file removed by `Drop`, which `SIGKILL` does not run (now `File::try_lock`,
released by the kernel), and an old `attempts.json` kept terminal rows (now the
file-store half of migration 0004, on `is_terminal`, keeping `awaiting_input`).
`WorkflowStore::attempt()` is kept, with its purpose — an observation point for
the contract suite, no production caller — documented on the trait.

Evidence: seven tests in `store/file.rs`'s own module, failures injected through
the filesystem the way the review reproduced the original. Each fix has its own
negative control. `just check` and `just test-postgres` green. Recorded in
[architecture §28](PIPELINE_ARCHITECTURE.md#28-migration-plan), work 2.

## 3. Make the scheduler reliable (R1, R2, R3, R5, R7) — **done, 2026-09-08**

R5 and R7 first, being the pure half: slots are enumerated by local calendar
date with a DST policy stated per cadence, and `effective_from` stops a new
schedule reaching into the past.

R1, R2 and R3 turned out to be one design rather than three fixes — all three
were the question of **where slot processing state lives**. Configuration stays
in the object store and the tick no longer writes it; admission and per-slot
outcome moved into the `WorkflowStore`. `admit_slot` creates the slot, refuses
it if somebody settled it or holds a live lease, and checks the definition for
a run that has not finished — all in one transaction, against the projection
this store already holds rather than the read model that is empty in the `work`
role. `settle_slot`'s `TryAgain` leaves a slot due after a transient failure,
with the caller drawing the line from the API's own status. R3 is closed by the
type system: the tick is handed a `ScheduleReader`, whose one method is `all`.

`run_now` gained a caller-supplied `request_id`, minted by the panel per press
and outside `mutationFn` — that function runs again on every retry.

Evidence: five properties in the storage contract suite, so all three adapters
prove the same things; two HTTP tests for the request identity; a negative
control for each fix. Migration 0006 adds a table, which is compatible by
construction under item 1's rule. `just check` and `just test-postgres` green.
Recorded in [architecture §28](PIPELINE_ARCHITECTURE.md#28-migration-plan),
work 3.

## Start here — 4. Make the panel report the server's answer (R6)

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
any item above is closed. Items 1–3 have since added seven PostgreSQL upgrade
tests, seven file-store fault tests, five storage-contract slot properties
proved on all three adapters, and eleven schedule-rule tests — with `just check`
and `just test-postgres` green. Record the new regression/acceptance evidence in §28
when an item is complete; keep the review as the dated finding record.
