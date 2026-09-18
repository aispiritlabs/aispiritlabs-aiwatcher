# IAM-01 — where it stands, and what is left

Date: 18.09.2026. **The control plane and the storage boundary are done; nothing
runs inside a project yet.** No dispatcher, no project `/start`, no selector, no
cutover.

This is the handover after four parallel streams landed on `main`. Their working
reports are gone — what survived them is here, in
[ADR_0033](ADR/ADR_0033_PROJECT_SCOPED_STORAGE.md) (the boundary),
[`crates/aiwatcher-iam/README.md`](../crates/aiwatcher-iam/README.md) (the
contract), [`iam-migration-runbook.md`](iam-migration-runbook.md) (the operator's
procedure) and the code. Read those before this; this file is what to do next.

---

## 1. What already holds

**An execution has a durable owner.** `ExecutionOwnership { scope, principal,
definition }` is written in the transaction that creates the execution and never
again. It is immutable: a repeated start naming a different principal is refused
before the inbox. It never comes from the plan, a parameter, `requested_by`, a
worker's name or a declaration's author — only from `ProjectStart`, which has no
`Deserialize` on purpose.

**The scope binds the store, and the unscoped handle refuses from the other
side.** `WorkflowStore::for_project`, `ObjectArtifactCatalog::for_project`,
`Artifacts::for_project`, `Registry::for_project_evidence`. That is why the
reactor, the worker routes, the pod launcher, the timer tick, the outbox
publisher and the retention sweep needed no change at all.

**Absence is the global side.** No execution and no artifact was migrated; every
one this build has written is a global one, and a project store may not adopt it.

**A project may declare judged and framework measurements** over its own
rubrics, cards, people, calibrations and kept replies. The scorer catalog stays
the deployment's — read from a project, written only by the `work` role. The
conversation archive is closed on both sides, by name.

**Four registries can be migrated by hand** with `aiwatcher-migrate`: prompts,
datasets, training, annotations.

---

## 2. The one gate: a dispatcher

Everything else waits on this, and it has five rules that are not negotiable.
Four of them were written by the stream that owns the code each protects.

1. **Scope and principal come from `store.ownership(&execution)`** on the bound
   store — never from the plan, a parameter, a worker's name or a declaration's
   author.
2. **Check the current grant when the work is taken *and* again before
   publication.** An IAM failure is `Transient`, never consent.
3. **Check it before the cache lookup.** A hit is an answer about a project's
   data whether or not any work follows, and `Reactor::cached` turns a catalog
   error into `None` with a `warn` — so a `Policy` refusal there would be
   invisible and the work would run and be stored globally.
4. **Build both halves of artifact storage together** (`ProjectArtifacts::bind`).
   A scoped byte store paired with the global catalog describes a project's
   outputs in the deployment-wide index, where a cache lookup for anybody
   answers with them.
5. **Never register a project executor in the global `ExecutorRegistry`** before
   1–4 hold.

### The two changes stream C is waiting on

Both are in files stream A owns, both are safe only after the gate above:

- `crates/aiwatcher-server/src/execution/scoring/project.rs` —
  `ProjectAuthority::authorize` admits `RuntimeBinding::ScoreEvaluation` only.
  Judged measurements compile to `JudgeEvaluation` and framework metrics to
  `ExternalEvaluation` (`aiwatcher-api/src/scoring.rs`, `plan_for`). Admit all
  three, still comparing `spec.declaration` with the pinned declaration.
- `crates/aiwatcher-server/src/execution/scoring.rs` — the first guard in
  `ScoreExecutor::execute` refuses when `project.is_some()` and a judge, a
  scorer **or** `artifacts` is set. The judge and the scorer are deployment
  clients rather than somebody else's data, so those two refusals may go; the
  `artifacts` one may not, because that reader is global. Suggested shape:
  refuse on `self.artifacts.is_some()` alone.

`prepare` already calls `authority.authorize(...)` before reading the
declaration, and the second IAM check before `Committing` is already there. The
rest of `execute` needs nothing.

---

## 3. After the dispatcher, before `/start`

| What | Why it is not optional |
|---|---|
| A **scoped outbox/ingest/projection** path, or an explicit decision that a project's facts stay off the event log | Today they stay off *by construction*, which is safe and invisible: a project run has no live view, no span and no fold |
| **Scoped live reads** (SSE/WebSocket) or a stated absence | The panel's two most important routes; a browser can set no headers on either |
| A **scoped retention sweep** | A project's execution is pruned by nothing today |
| An **artifact collector** for the scoped area | Refused deliberately: there is no scoped source of truth about reachability while project execution history does not exist, and a pass deleting what no *global* history names would delete a project's bytes for the absence of a global run |
| **Grant checks on every read, command, artifact and worker route** | The migration plan's own acceptance criteria |
| A **retention policy for declarations, judge settings and kept replies** | They grow with the number of questions per declaration and have no policy of their own |

Only when those are integrated and tested may a project `/start` be registered.
A library test is not that permission; nor is a URL prefix or a panel filter.

---

## 4. Migration: what each owner still owes

`aiwatcher-migrate` needs one function per registry:

```rust
pub async fn inventory(&self, target: ProjectScope)
    -> Result<aiwatcher_core::migration::Inventory, InventoryError>;
```

`target_key` comes from that registry's own `for_project`, plus the `skipped`
sub-prefixes it deliberately leaves behind. Adding the family to
`crates/aiwatcher-migration/src/family.rs` is then two lines — the entry and a
`match` arm in `plan::inventory_of`, and the `match` is the dispatch on purpose:
a family declared supported with no arm does not compile.

| Prefix | Owner | Why it is not just an adapter |
|---|---|---|
| `artifacts` | `aiwatcher-server` execution artifacts | Outputs and receipts are addressed by a canonical key the catalog validates on read, and the URI lives *inside* manifests, cache entries, receipts, execution streams, `artifact.produced` events and dataset versions. A plain directory copy yields records the reader refuses. **Recommendation: do not migrate historical artifacts in the first cutover** — the global space stays readable and nothing falls out of it |
| `workflows`, `schedules` | `aiwatcher-execution` | A schedule is a mutable head whose tick writes slot outcomes; copying one starts a second writer for the same intention |
| the seven `evaluation*` | `aiwatcher-evaluation` | Approvals and withdrawal markers are authoritative decisions, not a derivable index — losing one silently un-hides withdrawn evidence — and publication races collection at one immutable key |
| `variant-observations` | `aiwatcher-projector` | A projector fold's periods, written create-only from a log position. Rebuilt by folding; a copied period would be counted beside the one the fold writes |
| `conversations` | `aiwatcher-conversations` | **Blocked, not queued.** Content is sealed with the object's own key path as the HKDF `info` and the AEAD associated data (ADR_0021), so the same ciphertext under a project key does not open. Moving it is a re-seal: a decryption, a new retention clock, a new erasure surface |

Three families stream C added under the project prefix are inventoried but have
no adapter yet: `evaluation-judges/settings/<sha256>.json` (content-addressed,
create-only), and `evaluation-judges/replies/` and `evaluation-scorers/replies/`
keyed `<declaration>/<sha256 of the question>`. **The replies are not a
derivable cache** — losing one changes the bytes a re-measurement publishes.
`evaluation-scorers/catalog.json` stays one deployment object and must **not**
be copied into a project.

---

## 5. Cutover: what no tool can clear

1. Eleven families have no adapter and one is blocked. A project populated today
   does not hold the application's data.
2. The legacy routes still serve legacy data. Copying is additive; nothing here
   switches a route or removes a source.
3. Rollback of data is a snapshot restore, never re-enabling a global API —
   which would hand every project's data to everybody rather than undoing a copy.
4. The freeze is a person. The tool detects a source that moved and refuses,
   which is a failed migration rather than a prevented one.
5. **No IdP group or email maps to anything.** Who may read a project after a
   copy is exactly who could before it. Membership and grants are a separate
   decision nobody has made.
6. S3/RustFS is unverified for migration; the CLI reads a filesystem snapshot
   only.

---

## 6. What has never been run

Authentik, S3/RustFS, a Kubernetes cluster, workers, or any end-to-end. Every
stream's PostgreSQL work used a disposable container of its own. Seven tests are
`#[ignore]` because they need a database or a bucket. No user data was touched
and no deployment was changed.

Four module headers are over `scripts/lint-comments.py`'s 25-line limit; one of
them, `crates/aiwatcher-projector/src/pipeline.rs`, predates all of this.
`ADR_0032` is deliberately unused — another line of work reserved it.

---

## 7. Prompt for the next session

> Kontynuujesz IAM-01 w repozytorium AIWatcher, po zamknięciu czterech
> równoległych strumieni. Przeczytaj najpierw `docs/iam-01-kickoff.md`, potem
> `docs/ADR/ADR_0033_PROJECT_SCOPED_STORAGE.md`, sekcję o wykonaniu w
> `crates/aiwatcher-iam/README.md` i instrukcje repozytorium. Zweryfikuj stan w
> kodzie — dokumentacja opisuje etap zamknięty 18.09.2026 i mogła się
> zdezaktualizować.
>
> Zadanie: zbuduj **projektowy dispatcher** — jedyną bramkę, na której czeka
> wszystko inne. Ma pobierać scope i principala wyłącznie z
> `store.ownership(&execution)` na związanym magazynie, sprawdzać aktualny grant
> przy podejmowaniu pracy **i ponownie przed publikacją**, sprawdzać go **przed
> lookupem w cache**, budować obie połowy magazynu artefaktów przez
> `ProjectArtifacts::bind`, i nie rejestrować projektowego executora w globalnym
> `ExecutorRegistry`, dopóki to wszystko nie zachodzi. Błąd IAM to `Transient`,
> nigdy zgoda. Sekcja 2 kickoffu ma pełną listę z uzasadnieniami.
>
> Dopiero gdy dispatcher stoi i jest przetestowany, zdejmij dwie blokady, na
> które czeka strumień C (sekcja 2, „dwie zmiany"): `ProjectAuthority::authorize`
> dopuszczający `JudgeEvaluation` i `ExternalEvaluation`, oraz pierwszy strażnik
> `ScoreExecutor::execute` zawężony do samego `artifacts`.
>
> **Nie otwieraj projektowego `/start`** i nie rejestruj niczego w produkcyjnym
> reactorze, dopóki nie są zintegrowane i przetestowane pozycje z sekcji 3
> kickoffu. Test biblioteczny nie jest zgodą na otwarcie trasy, a prefiks URL ani
> filtr w panelu nie zastępują izolacji. Jeśli pełen zakres przekracza bezpieczną
> iterację, dostarcz dispatcher z testami i nazwij pozostałe blokady.
>
> Pracuj na osobnej gałęzi z `main`. Nie zmieniaj istniejących migracji SQL —
> nowe są addytywne, z rolling upgrade i rollbackiem starego binarium. Nie
> regeneruj kontraktu HTTP bez rzeczywistej zmiany API. Na koniec uruchom
> `cargo test --workspace --all-targets`, clippy z `-D warnings`,
> `cargo fmt --all --check`, `git diff --check`,
> `python3 scripts/check-rust-boundaries.py` i
> `python3 scripts/lint-comments.py`; PostgreSQL testuj wyłącznie na osobnej,
> jednorazowej bazie. Jawnie wypisz, czego nie uruchomiłeś.
