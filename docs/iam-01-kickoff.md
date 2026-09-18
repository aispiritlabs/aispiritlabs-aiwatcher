# IAM-01 — where it stands, and what is left

Date: 18.09.2026. **The control plane, the storage boundary and the dispatcher
are done; nothing runs inside a project in a deployment yet.** No production
wiring builds a dispatcher, no project `/start`, no selector, no cutover.

This is the handover after four parallel streams landed on `main`, and after the
gate they all waited on. What survived their working reports is here, in
[ADR_0033](ADR/ADR_0033_PROJECT_SCOPED_STORAGE.md) (the boundary),
[`crates/aiwatcher-iam/README.md`](../crates/aiwatcher-iam/README.md) (the
contract), [`iam-migration-runbook.md`](iam-migration-runbook.md) (the operator's
procedure) and the code. What to do next is here and, staged, in
[IAM-02](iam-02-data-plane.md).

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

**One loop runs a project's work**, `ProjectDispatcher`, and nothing in `spawn`
builds one. Section 2 is what it does and why each step is where it is.

---

## 2. The gate, closed

`aiwatcher_server::execution::project::ProjectDispatcher` exists, with
`aiwatcher_execution::ExecutionAuthority` as the port the reactor asks through.
The five rules are properties of the code now:

1. **Scope and principal come from `store.ownership(&execution)`** on the bound
   store. The authority is handed that record and nothing else, so it has no
   plan, parameter, worker name or declaration author to read either out of.
   `ProjectGrant` refuses an execution nobody owns.
2. **The current grant is checked when the work is taken and again before
   publication.** `editor_grant` is the one policy the reactor's authority and
   `ProjectAuthority` both call. An IAM failure is `Transient`: before the work
   it leaves the attempt claimable and spends no retry budget; before
   publication it is reported, because the lease is that pass's. A `Policy`
   refusal fails the step rather than letting a run be claimed and dropped
   every poll for ever.
3. **Checked before the cache lookup** — before the stream is loaded at all,
   which is the earliest point at which an execution id is known and what
   `OwnedDefinition` is readable-without-the-stream for.
4. **Both halves of artifact storage are built together** by
   `ProjectArtifacts::bind`; the dispatcher holds the pair and the reactor
   records through its catalog.
5. **No project executor is in a process-wide `ExecutorRegistry`.** It names one
   execution and one declaration, so it is built per claimed attempt from that
   attempt's owner.

`Reactor::resume` answers `None` under an authority: a claim rebuilt across two
requests has no pass to ask one in.

Tests: `aiwatcher-execution/tests/authority.rs` (the reactor's ordering, with a
stub) and `aiwatcher-server/tests/evaluation/project_dispatcher.rs` (the whole
chain, against the real memory IAM policy and a filesystem object store).

### The two changes stream C was waiting on — done

- `ProjectAuthority::authorize` admits `ScoreEvaluation`, `JudgeEvaluation` and
  `ExternalEvaluation`, still comparing the binding's declaration with the
  pinned one.
- `ScoreExecutor::execute` refuses a project registry beside a global
  `artifacts` reader **alone**. A judge and a scorer service are the
  deployment's own clients answering a question composed from the project's
  card; the artifact reader resolves an `object://` in the deployment's
  namespace, which is somebody else's bytes.

A project recording therefore still cannot read generated answers: that step's
rows come through the artifact store, and only a global one exists on this path.

## 3. What `/start` is still waiting on

| What | Why it is not optional |
|---|---|
| A **scoped outbox/ingest/projection** path, or an explicit decision that a project's facts stay off the event log | Today they stay off *by construction*, which is safe and invisible: a project run has no live view, no span and no fold |
| **Scoped live reads** (SSE/WebSocket) or a stated absence | The panel's two most important routes; a browser can set no headers on either |
| A **scoped retention sweep** | A project's execution is pruned by nothing today |
| An **artifact collector** for the scoped area | Refused deliberately: there is no scoped source of truth about reachability while project execution history does not exist, and a pass deleting what no *global* history names would delete a project's bytes for the absence of a global run |
| **Grant checks on every read, command, artifact and worker route** | The migration plan's own acceptance criteria |
| A **retention policy for declarations, judge settings and kept replies** | They grow with the number of questions per declaration and have no policy of their own |

Only when those are integrated and tested may a project `/start` be registered,
and only then may `spawn` build a dispatcher. A library test is not that
permission; nor is a URL prefix or a panel filter.

**The staged plan for all of it is [IAM-02](iam-02-data-plane.md)**, whose M1
gate is the smallest useful thing: sign in, see only your own projects, watch
only your own streams. It turns on one decision — `EventEnvelope` carries an
optional `ProjectScope`, written at ingest from the credential and never
honoured from a producer — because today a project's facts stay off the log by
construction, so a project run has no live view, no span and no fold.

### What sharing a project needs

Separate from `/start`, and mostly not blocked by it. The scoped route families
already take a **real** grant decision (`ProjectAuthorization`, fresh per
operation, rechecked after a write's body arrives), so the authored half —
prompts, datasets, annotations, training, evaluations, workflow definitions,
reviews, cohorts, recordings, bundles, approvals, calibrations, declarations —
is ready to be shared as soon as three things exist:

1. **Invitations.** `Command::Grant` takes the exact `(provider, subject)` pair,
   and nobody knows that before the invitee's first sign-in. An invitation is a
   one-time, expiring token bound to `(scope, role, window)`, redeemed after SSO
   in the transaction that creates the grant for the redeemer's principal. An
   email address is a delivery hint and never an identity key; a replay and a
   different recipient must both be refused.
2. **A panel.** No panel source file mentions organizations or projects today —
   the generated client holds 82 scoped entries and there is no UI at all. An
   organization/project switcher, a members-and-grants page and a share dialog.
   `/account` already renders IdP groups read-only and says they are not teams.
3. **Revocation that reaches a live stream.** The session cookie's TTL *is* the
   revocation window today. A stream that lives for hours has to ask again and
   close on refusal, and a `Last-Event-ID` resume has to ask **before** it
   replays — otherwise resuming is a way to read after access was taken away.

Sharing the *observability* half — runs, spans, metrics, the live stream — needs
IAM-02's E1–E4 as well. **No organization or project selector is activated
before the M1 gate.**

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

## 6. Known rough edges

Six module headers are over `scripts/lint-comments.py`'s 25-line limit; one of
them, `crates/aiwatcher-projector/src/pipeline.rs`, predates all of this.
`ADR_0032` is deliberately unused — another line of work reserved it.

Seven tests are `#[ignore]` because they need a database or a bucket. No user
data was touched and no deployment was changed.

---

## 7. What has never been run, still

Authentik, S3/RustFS, a Kubernetes cluster, workers, or any end-to-end for a
project. The dispatcher has been exercised against the memory IAM policy, the
filesystem object store and the memory workflow store, and against a disposable
PostgreSQL for the store contract and the IAM control plane — never against a
deployment.

---

## 8. Prompt for whenever IAM-02 is picked up

IAM-01 is closed and **IAM-02 is deliberately last** — after the rest of the
roadmap, not next. [`iam-02-data-plane.md`](iam-02-data-plane.md) is its plan
and says what that costs; the decision this prompt used to ask for — whether a
project's facts reach the event log — is **made** there, in its section 2.

Until it is picked up, one rule stands for everything built in the meantime:
**a new authored resource gets its scoped route family at birth.** The pattern
is `ProjectAuthorization` plus `<prefix>/scopes/<organization>/<project>/registry/`,
and writing it once is the same code that retrofitting it later is not.

> Zaczynasz IAM-02 w repozytorium AIWatcher. IAM-01 jest zamknięte: dispatcher
> stoi (`aiwatcher_server::execution::project::ProjectDispatcher`), obie blokady
> strumienia C są zdjęte, ale **nic w `spawn` go nie buduje i nie ma
> projektowego `/start`**. Przeczytaj `docs/iam-02-data-plane.md` (plan i
> decyzje), potem `docs/iam-01-kickoff.md`,
> `docs/ADR/ADR_0033_PROJECT_SCOPED_STORAGE.md`, sekcję o dispatcherze w
> `crates/aiwatcher-iam/README.md` i instrukcje repozytorium. Zweryfikuj stan w
> kodzie — dokumentacja opisuje etap zamknięty 18.09.2026.
>
> Zadanie: **E1 — projekt na kopercie**. `EventEnvelope` niesie opcjonalny
> `ProjectScope`; trasa ingestu **zawsze nadpisuje** go wartością z
> poświadczenia, a wartość od producenta jest odrzucana. W przeciwieństwie do
> `published_by` to pole **musi być serializowane**, bo czyta je projektor z
> szyny, a nie ta sama trasa. `IngestToken` zyskuje zakres, rola zostaje twardo
> `Editor`, zakres tylko zawęża. Brak pola to strona globalna, więc nic
> istniejącego się nie rusza. Zaktualizuj `contracts/envelope.schema.json`;
> SDK nie zmieniaj — nigdy tego pola nie wysyłają.
>
> Jeśli E1 zmieści się z zapasem, weź E2 (jeden fold z kluczem zakresu w
> wierszu — **nigdy fold per tenant**) i przebiegnij `just load-test`, bo
> `AIWATCHER_MAX_SPANS_TOTAL` jest kontraktem pamięciowym.
>
> **Nie otwieraj projektowego `/start`, nie rejestruj dispatchera w produkcyjnym
> `spawn` i nie aktywuj selektora organizacji/projektu.** Bramka M1 wymaga E1–E4
> zweryfikowanych przez rzeczywiste HTTP; test biblioteczny nie jest zgodą na
> otwarcie trasy, a prefiks URL ani filtr w panelu nie zastępują izolacji.
>
> Pracuj na osobnej gałęzi z `main`. Nie zmieniaj istniejących migracji SQL —
> nowe są addytywne, z rolling upgrade i rollbackiem starego binarium. Kontrakt
> HTTP regeneruj `just openapi` **tylko** jeśli rzeczywiście zmieniłeś API. Na
> koniec uruchom `cargo test --workspace --all-targets`, clippy z `-D warnings`,
> `cargo fmt --all --check`, `git diff --check`,
> `python3 scripts/check-rust-boundaries.py` i
> `python3 scripts/lint-comments.py`; PostgreSQL testuj wyłącznie na osobnej,
> jednorazowej bazie. Jawnie wypisz, czego nie uruchomiłeś.
