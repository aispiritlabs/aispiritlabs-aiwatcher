# Stream D — the migration manifest and its executor

**No user data was migrated and nothing was deployed.** Everything below ran
against disposable fixtures, a temporary directory and a disposable PostgreSQL
container started for this work and stopped afterwards. No production store, no
cluster, no existing project database and no user data directory was touched.

Branch: `iam-stream-d`, in a worktree on a snapshot commit containing every
stream's uncommitted work at the time it started. The shared checkout was not
reset, stashed or formatted.

---

## 1. What was delivered

A versioned manifest, a survey, a checkpoint and a resumable executor, over
**four named stable registries**, with every other prefix in the store given an
explicit answer.

| Piece | Where |
| --- | --- |
| The neutral inventory vocabulary owners answer in | `crates/aiwatcher-core/src/migration.rs` |
| The prompt registry's inventory | `crates/aiwatcher-prompts/src/migration.rs` |
| The training registry's inventory | `crates/aiwatcher-training/src/registry/migration.rs` |
| The annotation registry's inventory | `crates/aiwatcher-annotations/src/registry/migration.rs` |
| The dataset registry's inventory, and the existing dry-run rebuilt on it | `crates/aiwatcher-datasets/src/scope.rs` |
| Family registry, manifest, planner, authority, executor, CLI | `crates/aiwatcher-migration/` |
| Operator runbook | `docs/iam-migration-runbook.md` |

This is **not** a full application migration, and the tool says so in every
receipt it writes. See §5.

## 2. The four decisions that shape it

### The owner supplies the keys; the tool composes

Each registry answers in one vocabulary — what it holds, the key each object
takes in a bound scope, in what write order, whether the bytes are its own JSON
or opaque, and what each object points at. `aiwatcher-migration` computes no
object key. The one piece of string work in the planner is deciding which
configured prefix a listed key falls under, so that a key under none of them is
*reported* rather than walked past.

This matters concretely. The training registry digests run ids and model names
into their key segments, and the annotation registry digests project names
because a project name may hold slashes. A generic copier would have to
re-implement both, and the day it drifted it would write bytes under a key
nothing reads.

The vocabulary went into `aiwatcher-core` rather than the migration crate
because the owners are all *below* the tool: a trait in the tool would mean the
registries depending on it, and four near-identical structs is the alternative.
It adds no dependency to `aiwatcher-core`.

### A manifest is a pure function of the snapshot; a survey is not

`Manifest::manifest_id` is the SHA-256 of the manifest's own body, which covers
the target scope, the store the operator named, the configured prefixes, every
family's state, every object's key pair and digest, the reference summary and
every blocker. Planning an unchanged snapshot twice gives byte-identical files.

What the *target* holds — absent, identical, in conflict — is a separate
`Survey`. That split is what makes resume possible: a half-finished copy changes
the target, and a manifest whose identity moved when the first object landed
would be a manifest no checkpoint could bind to. A test holds this: after
copying every object, the plan is still the same plan.

For the same reason the snapshot fingerprint excludes the project prefixes this
run writes into, and it is content — `key \t bytes \t sha256` for every
inventoried object — rather than counts and modification times.

### Nothing is trusted out of the manifest file

`execute` **plans again from the live store** and refuses unless it reaches the
same `manifest_id`. That single step does the path revalidation, the
source-change detection and the foreign-schema detection at once, and it is why
an edited target key cannot be written: the keys the executor uses are the ones
the registries just computed, not the ones in the file. Re-sealing a forged
manifest so its digest matches its own body does not help — the re-plan is what
it is compared against. Each key is then still checked against its family's own
declared destination prefix and against the segments no key may contain.

### Absence is an answer

`empty`, `unsupported`, `blocked`, `damaged`, `foreign` and `unknown prefix` are
six different states with six different meanings, and none of them is a count
of zero. A completed copy is `complete`; only a copy with no blocker at all is
`cutover_ready`, and for every store this build can read, it is not — because
the unsupported families are still there. The CLI prints that on stderr.

## 3. Families

Supported: **prompts**, **datasets**, **training**, **annotations**.

Blocked: **conversations**. Archived content is sealed with the object's own key
path as the HKDF `info` and the AEAD associated data (ADR_0021), so the same
ciphertext under a project key does not open. Moving it is a re-seal — a
decryption, a new retention clock, a new erasure surface — which is the
archive's own protocol and not a byte copy. It is reported as blocked *with its
object count*, so nobody reads it as nothing.

Unsupported, each by name with a reason: `artifacts`, `workflows`, `schedules`,
`variant-observations`, and the seven `evaluation*` prefixes. Three of those
groups are the other streams' and are named in §6.

`variant-observations` is worth calling out as a refusal on its own merits
rather than a queue item: it is a projector fold's periods, written create-only
from a position in the log. It is rebuilt by folding, and a copied period would
be counted beside the one the fold writes.

## 4. IAM, and what a migration is not

`execute` requires `TargetAuthority::admit` to return `Verified`. The
`IamAuthority` adapter asks `IamStore::access(scope, principal)` and requires
**admin** — not editor, because an ingest token is an editor by construction and
a token in an agent's environment must not be enough to land somebody's whole
registry somewhere. A dry run may be offline and says `not_checked`; a write may
not.

The refusal deliberately says "either it does not exist or this principal holds
no live grant on it", because the control plane returns one `NotFound` for both
and telling them apart would let anybody enumerate an organization's projects.

Every manifest carries an `identity` block stating, as data rather than as
prose in a runbook, that the run creates no organization, team, membership or
grant, reads no identity-provider group, and rewrites no stored byte, version id
or reference. Nothing in this tool maps a group name or an email address to
anything. **Mapping data into a project grants nobody access to it.**

`--iam fixture:FILE` exists for rehearsals and tests. It answers
`authoritative: false`, which puts a `non_authoritative_iam` blocker on every
receipt it admits, so a run authorized that way is structurally incapable of
reading as a cutover.

## 5. References

References are inventoried verbatim and **never rewritten**. Each is one of
three named kinds the owner assigns:

* **internal** — another object in the same registry, by its source key. The
  planner checks it is in the snapshot; one that is not is named individually in
  `references.internal_missing` and becomes a `refuses_cutover` blocker. It is
  still copied, faithfully: the source was already inconsistent, and rewriting a
  reference to make the copy look tidy is changing history.
* **foreign** — a resource another registry owns, named by that registry. A
  training run's `plans@sha256…` names `aiwatcher-annotations`; an
  optimisation's `evaluation_id` names the log and `aiwatcher-evaluation`.
* **opaque** — text this adapter does not interpret, with the reason. A
  checkpoint URI. A model name a run is training. **And query text**: a curation
  pipeline's Flow/DataFusion/DuckDB body and an authored block spec are reported
  as unanalysed rather than parsed. Declaring a dependency analysis of a
  language whose parser lives in a PHP service would be a claim this tool cannot
  keep, and a test asserts the reason says so.

A mutable dataset name (`plans`, no `@`) is opaque rather than foreign, using
the training registry's own `is_reproducible` rule rather than a second copy of
it.

## 6. What the other streams need to do

Nothing here edits another stream's files. What is needed from them, when their
layouts settle, is one function per registry with this signature:

```rust
pub async fn inventory(&self, target: ProjectScope)
    -> Result<aiwatcher_core::migration::Inventory, InventoryError>;
```

returning objects whose `target_key` comes from that registry's own
`for_project`, plus the `skipped` sub-prefixes it deliberately leaves behind.
Adding the family to `crates/aiwatcher-migration/src/family.rs` is then two
lines: the entry, and a `match` arm in `plan::inventory_of`. The `match` is the
dispatch on purpose — a family declared supported with no arm does not compile.

* **A** (`aiwatcher-execution`): `workflows` and `schedules`. A schedule is a
  mutable head whose tick writes slot outcomes; copying one starts a second
  writer for the same intention, so it needs more than a byte copy and is named
  unsupported rather than queued.
* **B** (`aiwatcher-server` execution artifacts): `artifacts`. Step outputs and
  attempt receipts are addressed by a canonical key the catalog validates on
  read; a copy made under a layout that has not landed would be a copy under the
  wrong layout.
* **C** (`aiwatcher-evaluation`): the seven `evaluation*` prefixes. Two things
  make this more than an adapter, and they are why it is named rather than
  attempted: **approvals and withdrawal markers are authoritative decisions**,
  not a derivable index, and losing one silently un-hides withdrawn evidence;
  and publication races collection at one immutable key, so a copy taken across
  that race can resurrect a claim a collector abandoned.

## 7. Verification

Everything below was run in this branch. Nothing is reported that was not run.

```
cargo test -p aiwatcher-core -p aiwatcher-prompts -p aiwatcher-training \
           -p aiwatcher-annotations -p aiwatcher-datasets -p aiwatcher-migration
  → 330 passed, 0 failed, 6 ignored

cargo clippy -p aiwatcher-core -p aiwatcher-prompts -p aiwatcher-training \
             -p aiwatcher-annotations -p aiwatcher-datasets -p aiwatcher-migration \
             --all-targets --all-features -- -D warnings
  → clean

cargo check --workspace --all-targets   → clean (with every stream's work in tree)
rustfmt --edition 2024 <this stream's 23 files>   → clean
git diff --check   → clean
```

`cargo fmt --all` was **not** run: it would rewrite the other streams' files.

Per crate: core 106, prompts 52, training 29, annotations 77, datasets 34,
migration 32. The pre-existing dataset dry-run tests still pass unchanged — the
old `migration_manifest` was rebuilt on the new classifier rather than left as a
second set of key rules.

### The migration crate's 32 tests

**Dry run (10).** Deterministic and read-only, writing nothing including no
probe. The manifest moves with the snapshot, the target and the configured
prefix. Empty / blocked / unsupported read differently, each with counts. A
foreign key and a damaged document each stop a run under their own blocker
kind. References are counted, named, and the query-text reason is asserted. A
dangling internal reference blocks a cutover without blocking the copy. Already
scoped keys are never a source, so a half-finished copy plans identically. Two
families configured onto one prefix, and an object at the store's root, are each
refused or named rather than read twice or walked past.

**Execution (15).** Byte-for-byte copy, digests preserved, sources untouched,
and both the prompt and training registries reopening their own objects under
the project with the same version ids and the same labels. Re-running writes
nothing and is not a conflict. A conflicting target stops the run and is not
overwritten — and the survey reported it first. Interruption **before a write,
after a write before the checkpoint, and after the checkpoint**, each resuming
without loss or double counting. A write that reported success and stored
something else, caught by the read-back. A store without create-only, refused
and then admitted under `--exclusive-access`. A source that moved, an object
that vanished, an edited manifest, a re-sealed forged manifest, a checkpoint
from another plan / target / store — each refused with nothing written. A dry
run may be offline; an execution may not. A target IAM does not admit, refused.
The control plane itself, over `MemoryIamStore`: a creator's explicit admin
grant admitted, a stranger refused, an **editor** refused, a project nobody
created refused. An empty store copies nothing and is still not cutover-ready.

**Command line (3), on a temporary file store with `FileObjectStore`.** plan →
apply → resume, with the objects read back by a registry that was never told a
migration happened; `--confirm` required; `apply` over an existing checkpoint
and `resume` without one each refused by name; a missing directory and a
non-`fs:` store refused by name; an edited manifest refused, with neither the
escaped key nor a checkpoint created.

**Owner adapters (12, in the owning crates).** Prompts: a label names the
version key that version really has; a scoped registry is never a source and its
keys are never inventoried; a non-prompt document is damaged while a stray file
is neither migrated nor declared skipped. Training: a model version names its
run by the digested key the registry really wrote; checkpoint URIs are opaque,
export references are foreign, a mutable dataset name is opaque. Annotations: a
picture is opaque bytes and is never parsed; an image head names its blob;
`exports/index.json` is an index and not one more export; the project head is
written last. Datasets: `name@revision` resolves to a pipeline revision while
query text is reported as unanalysed.

### PostgreSQL

Declared **verified**, not assumed. A disposable PostgreSQL 18.6 container on
127.0.0.1:5437 (its own database, not any existing one; started for this work
and stopped afterwards):

```sh
AIWATCHER_MIGRATION_TEST_POSTGRES_URL=postgres://…@127.0.0.1:5437/iam_stream_d \
  cargo test -p aiwatcher-migration --features postgres --test postgres -- --ignored
  → 2 passed
```

Both go through `PostgresIamStore`. The first admits a real organization and
project, refuses a project id nobody created and an organization nobody created,
and then runs a real copy admitted by that store with `authoritative: true`. The
second drives the **binary** with `--iam postgres://…`, asserts the receipt
records `postgres` / `authoritative: true` / `role: admin`, and shows a
principal with no grant refused at the command line with nothing written.

Ignored rather than skipped, so a run with no database reports that it did not
run rather than reporting a pass.

### Not verified

* **S3 / RustFS.** The library is written against the `ObjectStore` port and the
  S3 adapter implements create-only conditional writes, but nothing here was run
  against a bucket. The CLI reads `fs:` only. Named as unverified rather than
  claimed.
* **Anything at scale.** The largest fixture is a handful of objects. The
  `ObjectStore` port reads whole objects, so a large annotation blob is held in
  memory while it is hashed.
* **A real deployment's store.** By instruction and by design.

### Evidence from a real run

Against a scratch directory holding one prompt (head + version), one sealed
conversation object and one evaluation object:

* two `plan` runs produced **byte-identical** manifests;
* the plan named 2 objects, ordered `content` (the version) before `index` (the
  head), mapped under
  `prompts/scopes/<org>/<project>/registry/house.extract/…`;
* `conversations` read `blocked` with `objects: 1`; `evaluations` read
  `unsupported` with `objects: 1`; `datasets`, `training` and `annotations` read
  `empty`;
* `apply` wrote 2, reported `complete: true`, `cutover_ready: false` and
  `publication: create_only`, and printed *"this run is not a cutover: 13
  blocker(s) stand"*;
* a second `apply` refused and named `resume`; `resume` wrote 0 and resumed 2;
* putting foreign bytes on a target key made `apply` refuse with both digests,
  left those bytes untouched, and `verify` reported the conflict before anybody
  ran anything.

## 8. Cutover blockers

None of these is something this tool can clear.

1. **Eleven families have no adapter**, and one is blocked outright. A project
   populated by this tool does not hold the application's data.
2. **The conversation archive needs its owner's protocol**, not a copier.
3. **Approvals and withdrawal markers** in `evaluations/` are authoritative
   decisions; losing one un-hides withdrawn evidence.
4. **The legacy routes still serve legacy data**, and nothing here switches a
   route or removes a source. Copying is additive.
5. **Rollback of data is a snapshot restore**, never re-enabling a global API —
   which would hand every project's data to everybody rather than undoing a
   copy.
6. **The freeze is a person.** The tool detects a source that moved and refuses,
   which is a failed migration rather than a prevented one.
7. **No IdP group or email maps to anything.** Who may read a project after the
   copy is exactly who could before it. Membership and grants are a separate
   decision that nobody has made yet.
8. **S3 is unverified** and the CLI reads a filesystem snapshot only.

## 9. Files

New: `crates/aiwatcher-core/src/migration.rs`,
`crates/aiwatcher-prompts/src/migration.rs`,
`crates/aiwatcher-training/src/registry/migration.rs`,
`crates/aiwatcher-annotations/src/registry/migration.rs`,
`crates/aiwatcher-migration/**`, `docs/iam-migration-runbook.md`, this report.

Changed: `Cargo.toml` and `Cargo.lock` (the new crate),
`crates/aiwatcher-core/src/lib.rs` (one `pub mod`),
`crates/aiwatcher-prompts/src/lib.rs` and
`crates/aiwatcher-training/src/registry.rs` (one `mod` each),
`crates/aiwatcher-annotations/src/registry.rs` (one `mod`),
`crates/aiwatcher-annotations/src/store.rs` (a `prefix()` accessor),
`crates/aiwatcher-datasets/src/pipeline.rs` (`pipeline_version_key` made
`pub(crate)`), `crates/aiwatcher-datasets/src/scope.rs` (rewritten onto the
shared classifier; the public `migration_manifest` shape and its tests are
unchanged).

Not touched: any panel file, any route, any chart, `crates/aiwatcher-iam/README.md`,
`docs/ux-migration-plan-2026-09-14.md`, and the files owned by streams A, B
and C.
