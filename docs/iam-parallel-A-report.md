# Stream A — durable execution ownership and execution isolation (IAM-01)

Date: 2026-09-18. Branch: `iam-01/stream-a`, from `iam-01/snapshot` (`4769fff`),
a snapshot commit of the shared checkout's whole working tree — tracked
modifications and the new untracked files of the previous stages included, not
`main`'s HEAD alone.

**Delivered: the first independently receivable stage — durable, trusted
execution ownership, with every old global path refusing to handle a scoped
execution. Not delivered: the project dispatcher and project `/start`, which
remain closed. IAM-01 as a whole is still open.**

---

## 1. Decisions

The reasoning is in
[docs/iam-parallel-A-decision-execution-ownership.md](iam-parallel-A-decision-execution-ownership.md)
— a stream document rather than an ADR, because an ADR number chosen here would
collide with one chosen by B, C or D. The integrator promotes it with a number.

In one paragraph: an execution now has an `ExecutionOwnership { scope,
principal, definition }` written **in the transaction that creates it** and
never again; a global execution has no record and absence *is* the unscoped
side, so nothing existing is migrated; the scope binds the **store**
(`WorkflowStore::for_project`) rather than each call, which is what makes the
unscoped store refuse a project's execution without the reactor, worker,
launcher, timer tick, outbox publisher or retention sweep changing a line; and
a repeated start naming a different principal is a refusal rather than a
duplicate.

---

## 2. Changed files

### New

| Path | What |
|---|---|
| `crates/aiwatcher-execution/src/scope.rs` | `ExecutionScope`, `ExecutionOwnership`, `OwnedDefinition`, `ProjectStart`, `ScopeBinding` — the types and the one rule every adapter applies |
| `crates/aiwatcher-execution/migrations/0011_execution_ownership.sql` | the PostgreSQL table; additive, no backfill |
| `crates/aiwatcher-execution/tests/scope.rs` | the start path, the handler and the reactor, end to end |
| `docs/iam-parallel-A-decision-execution-ownership.md` | the decision |

### Modified — `aiwatcher-execution` (stream A's own)

| Path | What |
|---|---|
| `src/lib.rs` | module and re-exports |
| `src/error.rs` | `StoreError::{OutOfScope, OwnershipConflict, NotInThisScope}`, all `says_the_same_next_time == true` |
| `src/store/mod.rs` | `WorkflowStore::{scope, for_project, ownership}`; `AppendRequest::ownership`; the scope rule on the trait's own docs |
| `src/store/memory.rs` | binding + ownership map; guards and filters |
| `src/store/file.rs` | binding + `ownership/` files journalled in `PendingCommit`; `FileWorkflowStore` split into a shared `Held` so a binding shares the lock |
| `src/store/postgres/mod.rs` | binding + `execution_ownership`; `for update` on the record in `append`; a scope predicate on every cross-execution query |
| `src/store/postgres/schema.rs` | migration 0011 registered |
| `src/store/duckdb/{mod,schema}.rs` | binding + `ownership` table, same rules |
| `src/handler.rs` | `ExecutionHandler::start(…, ownership)`; the duplicate short-circuit now checks ownership |
| `src/start.rs` | `StartRun::project`; `RunIdentity::of(plan, scope)`; `Executions::start` builds the record |
| `src/hosted.rs`, `src/outbox.rs` | `ownership: None` on the appends they compose |
| `src/testing.rs` | 13 new shared contract properties + two fixtures exposed for adapter-specific tests |
| `tests/{store_contract,handler,hosted,postgres_upgrade}.rs` | the new property, the trait forwards on the two test doubles, `APPLIED` to 11 |

### Modified — outside stream A (mechanical, and one judgement call)

| Path | What | Note for the integrator |
|---|---|---|
| `crates/aiwatcher-api/src/{executions,schedules,scoring}.rs` | `project: None` on the three existing `StartRun` literals | forced by the field; `scoring.rs` is shared with C — one line, inside an existing literal |
| `crates/aiwatcher-server/src/{conversations,execution/mod,execution/stranded}.rs` | `ownership: None` on three `AppendRequest` literals, all in `#[cfg(test)]` fixtures | forced by the field |
| `crates/aiwatcher-server/src/execution/scheduler.rs` | `project: None` on the tick's `StartRun` | forced by the field |
| `crates/aiwatcher-api/src/error.rs` | the three scope refusals render **404 `not_found`**, not 503 | **a judgement call, shared with C.** 503 promises a retry for a boundary that never moves — the defect `says_the_same_next_time` exists to prevent — and a run somebody may not reach is a run they do not have. Revert it to `HandleError::Store(_)` if C would rather own that arm; nothing else depends on it |

`contracts/openapi.json` and `apps/panel/src/api/generated/` are **unchanged**:
no route, no body and no schema moved. Verified by regenerating and diffing.

No registry, migration module or UI file was touched. `crates/aiwatcher-iam/`
and `docs/ux-migration-plan-2026-09-14.md` were not edited, as instructed.

---

## 3. Contract for B, C and D

### The types (in `aiwatcher_execution`, re-exported from the crate root)

```rust
pub enum ExecutionScope { Global, Project(aiwatcher_iam::ProjectScope) }

pub struct ProjectStart {                 // the trusted half of a start
    pub scope: aiwatcher_iam::ProjectScope,
    pub principal: aiwatcher_iam::Principal,   // exact (provider, subject)
}

pub struct OwnedDefinition {              // what was started
    pub kind: DefinitionKind,
    pub name: String,
    pub revision: DefinitionRevision,
    pub plan_id: PlanId,
}

pub struct ExecutionOwnership {           // the durable record
    pub scope: aiwatcher_iam::ProjectScope,
    pub principal: aiwatcher_iam::Principal,
    pub definition: OwnedDefinition,
}
```

`ProjectStart` deliberately has **no** `Deserialize`: a type that can arrive over
the wire is a type somebody can send. `ExecutionOwnership` is serialized only by
the store adapters.

### The port (`WorkflowStore`)

```rust
fn scope(&self) -> ExecutionScope;
fn for_project(&self, scope: ProjectScope) -> Result<Arc<dyn WorkflowStore>>;
async fn ownership(&self, execution: &ExecutionId) -> Result<Option<ExecutionOwnership>>;
```

* `for_project` shares the backing data and narrows what may be reached.
  Rebinding to the scope it holds is idempotent; to a second one it is
  `StoreError::NotInThisScope`.
* Every per-execution method refuses `StoreError::OutOfScope` before touching
  the backend when the execution is not this store's.
* Every cross-execution query (`due_timers`, `pending_outbox`, `mark_published`,
  `claim_attempt`, `attempt`, `unclaimed_attempts`, `claimable_attempts`,
  `prune`) answers for this store's side only.
* `checkpoint`, `advance_checkpoint`, `admit_slot`, `settle_slot` and
  `recent_slots` are instance-wide and are `StoreError::NotInThisScope` on a
  bound store.

### How a project execution is started (once a route may)

```rust
let store   = global_store.for_project(scope)?;          // Arc<dyn WorkflowStore>
let handler = ExecutionHandler::new(store);
let started = Executions { handler: Some(&handler), .. }.start(
    plan,
    StartRun { project: Some(ProjectStart::new(scope, principal)), .. },
).await?;
```

`Executions::start` derives the scoped id, builds the record and hands it to
`ExecutionHandler::start`. **A caller never writes the record itself**, and the
store refuses one from an unscoped handle.

### What each stream can rely on

* **B (catalog / lineage / cache / retention).** The scope a catalog needs is
  `store.ownership(&execution)?.map(|o| o.scope)`, read from the store bound to
  it — never from a plan, a parameter or a caller. `ExecutionScope::key()` is
  `<organization>/<project>` (two uuids, `/`-separated, empty for global) and is
  the string the SQL adapters store; it is a stable key for a catalog's own
  index if one is wanted. Do not wire a scoped byte store to the global catalog:
  the boundary the store keeps is undone by a catalog that does not.
* **C (judge / external scorers).** `ProjectAuthority` is unchanged and still
  takes its scope, principal, execution and declaration explicitly. What stream A
  adds is the place those come from: `ownership()` on the bound store yields the
  scope and the exact principal, and `ownership.definition` says what the run
  was started on — so a future dispatcher never reads them from the plan, the
  request or the worker's name. No scoring route was opened.
* **D (migration).** Nothing was migrated and nothing needs to be for existing
  data: an execution with no record *is* a global execution, which is what every
  stream in every deployment already is. If a migration ever wants to move an
  existing execution into a project it must write the ownership record, and the
  store will then refuse every unscoped path to it — including the outbox
  publisher, so its unpublished facts would stop reaching the log. Do not move a
  running execution.

---

## 4. Commands run, and their results

All in the stream's own worktree, against its own `target/`.

| Command | Result |
|---|---|
| `cargo check --workspace --all-features --all-targets` | pass |
| `cargo test --workspace --all-targets` | **1712 passed, 0 failed** |
| `cargo test -p aiwatcher-execution --test store_contract` | 7 passed (memory and file against the full suite, including the 13 new scope properties) |
| `cargo test -p aiwatcher-execution --test scope` | 5 passed |
| `cargo test -p aiwatcher-execution --features duckdb --test duckdb` | 4 passed (DuckDB against the same suite) |
| `cargo test -p aiwatcher-execution --features postgres --test postgres -- --ignored --test-threads=1` | 5 passed |
| `cargo test -p aiwatcher-execution --features postgres --test postgres_upgrade -- --ignored --test-threads=1` | 7 passed |
| `cargo clippy --workspace --all-targets --all-features -- -Dwarnings` | clean |
| `cargo fmt --all --check` | clean |
| `git diff --check` | clean |
| `python3 scripts/check-rust-boundaries.py` | pass |
| `python3 scripts/lint-comments.py` | one finding, `aiwatcher-projector/src/pipeline.rs:1` — **pre-existing**, reported identically on the snapshot before any change here |
| `cargo run --bin aiwatcher-openapi` + diff against `contracts/openapi.json` | identical; no regeneration needed, panel untouched |

**PostgreSQL** ran against a **disposable** database of this stream's own — a
container named `aiwatcher-iam-a-postgres` on `127.0.0.1:5436`, created for the
run and removed after it. Not `just postgres-up`'s 5433, not any project
database, not a cluster. The upgrade suite is the one that matters for migration
0011: it applies historical schemas, upgrades, re-applies, and checks that the
**previous release's own SQL** still works against the upgraded database — which
is the rolling-upgrade and image-rollback case.

**Not run, and why:**

* the Laser integration tests, the Kubernetes stack, the panel build, the
  Python/TypeScript SDKs and the query services — untouched by this change and
  each needs something running;
* `just check` as a whole, for the same reason; its Rust half (fmt, clippy,
  workspace tests, boundaries, comments, openapi) was run directly, above;
* the `laser` cargo feature: `--all-features` covers it in check and clippy, and
  its tests need a broker.

---

## 5. Limitations, stated rather than hidden

1. **No project execution can be created by any production caller.** Every
   `StartRun` in the binary passes `project: None`. The scoped store is
   constructed only by tests. This is deliberate: the integration gate is not
   met.
2. **The dispatcher is not built.** See §6.
3. **A project execution is never pruned by the unscoped retention sweep**, and
   no scoped sweep is wired. Keeping is the safe direction and the default
   window already keeps everything, but a deployment that turns retention on
   will not reclaim a project's history until a scoped sweep exists.
4. **A project execution's facts never reach the event log**, because the global
   publisher does not see its outbox rows. That is the isolation asked for and
   also why a project run currently has no live view, no span and no fold.
5. **Schedules and processor checkpoints are instance-wide.** A project-bound
   store refuses both by name. Project schedules are a later decision; nothing
   here half-implements one.
6. **This is not authorization.** The store says which executions a caller may
   touch; whether a principal still holds a grant is IAM's answer and must be
   asked fresh. `ProjectAuthority` already does that for the recording executor.
7. **Revocation is unchanged and unproven on a real execution path.** The
   existing commit boundary stands: a revocation during a read prevents
   publication, an admitted commit finishes, and the next attempt must pass
   again. Nothing new was claimed about it because nothing new runs.
8. **`MemoryWorkflowStore::outbox()`** is an adapter inspection method (like
   `PostgresWorkflowStore::pool()`) and is not scoped. It is a test surface; it
   is not reached by any production path.
9. No IAM/object-store transaction is claimed anywhere. The record and the run
   land together because they are in **one store's** transaction; IAM is a
   separate system and is not in it.

---

## 6. The gate to the next stage

The dispatcher was deliberately not built, and the reason is not effort: it
would have to be written against contracts that do not exist yet. What it needs,
precisely:

**A. From this stream — done.**
A trusted place to read the scope and principal from: `store.ownership(&id)` on
a bound store, with the plan binding beside them.

**B. From stream B — required before a dispatcher runs anything.**
An artifact **catalog**, lineage and cache index that are scoped the same way
the byte store already is. A dispatcher that resolved a cache hit through the
global catalog would answer a project's step with another scope's rows, and the
reactor's cache lookup happens *before* any authorization — which the crate
README already names as the reason the project executor is registered nowhere.

**C. From stream C — required before a project measurement runs.**
Judge and external-scorer paths that are project-scoped, including the scorer
service's catalog. Until then a project scoring run has half a vocabulary.

**D. Still unowned, and each is a hole if skipped.**
1. A **project reactor**: a claim loop holding a bound store, resolving the
   authority from `ownership()`, checking the current grant when it takes the
   work *and* again before publication, and treating an IAM failure as
   `Transient` (never as consent).
2. A **scoped outbox publisher, ingest and projection** path, or an explicit
   decision that a project run's facts stay off the instance log. Today they
   stay off it by construction, which is safe and also invisible.
3. **Scoped live reads** (SSE/WebSocket) or an explicit absence.
4. A **scoped retention sweep**.
5. The **grant checks on every read, command, artifact and worker route** that
   the migration plan's own acceptance criteria list.

**Only when B, C and D1–D5 are integrated and tested may a project `/start` be
registered.** A library test is not that permission; nor is a URL prefix or a
panel filter. And if writing a project run could already wake the global outbox
or reactor — it cannot, as of this stream — no production call that writes one
should be added.

---

## 7. Working notes for the integrator

* The snapshot branch `iam-01/snapshot` (`4769fff`) holds the shared working
  tree as it stood when this stream began, including the other stages'
  uncommitted and untracked files. It was built with a temporary index, so the
  shared checkout's own index and working tree were never touched.
* `prompt1.md`–`prompt4.md` are in the snapshot because they were in the working
  tree; they are the four briefs and are not this stream's work.
* This stream did not edit `docs/ux-migration-plan-2026-09-14.md` or
  `crates/aiwatcher-iam/README.md`. Suggested wording for both, when the reports
  are merged, is §1–§3 of this document; the plan's next-gate sentence should
  read that durable ownership and the refusal of every old global path are done,
  and that the dispatcher, the scoped catalog, the live path and `/start` are
  not.
* The disposable PostgreSQL container was removed after the run. Nothing of this
  stream is left running.
