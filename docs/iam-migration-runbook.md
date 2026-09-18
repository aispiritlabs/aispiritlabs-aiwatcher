# `aiwatcher-migrate` — the operator's runbook

What this tool is: a way to take the authored object store a deployment has
been writing since before projects existed, and put a named subset of it under
one IAM project's keys — reviewably, resumably, and without overwriting
anything.

What it is **not**, and will refuse to pretend to be: it does not stop a
deployment, change a route, change a chart, delete a source, create an
organization, a team, a membership or a grant, read an identity provider's
groups, rewrite a stored byte, or decide that a cutover has happened.

**Nothing in this document has been run against production data.** Everything
below was exercised against disposable fixtures and a disposable PostgreSQL.

---

## 1. What it moves, and what it does not

| Family | Prefix | Owner | Status |
| --- | --- | --- | --- |
| prompts | `prompts` (or `AIWATCHER_PROMPT_PREFIX`) | `aiwatcher-prompts` | **supported** |
| datasets | `datasets` | `aiwatcher-datasets` | **supported** |
| training | `training` | `aiwatcher-training` | **supported** |
| annotations | `annotations` | `aiwatcher-annotations` | **supported** |
| conversations | `conversations` | `aiwatcher-conversations` | **blocked** |
| evaluations, evaluation-bundles, evaluation-judges, evaluation-reviews, evaluation-scopes, evaluation-scorers, evaluation-variant-artifacts | as named | `aiwatcher-evaluation`, `aiwatcher-server` | unsupported |
| artifacts | `artifacts` | `aiwatcher-server` | unsupported |
| workflows, schedules | as named | `aiwatcher-execution` | unsupported |
| variant-observations | `variant-observations` | `aiwatcher-projector` | unsupported |

Three words with three meanings, and the manifest keeps them apart:

* **empty** — a supported family, and there is nothing under its prefix.
* **unsupported** — no adapter in this build. The objects are *counted* and
  left where they are. This is never reported as zero.
* **blocked** — copying the bytes would break them. The conversation archive
  is blocked because content is sealed with the object's own key path as the
  HKDF `info` and the AEAD associated data (ADR_0021): the same ciphertext
  under a project key does not open. Moving it needs a re-seal — a decryption,
  a new retention clock and an erasure surface — which is the archive's own
  protocol and not a byte copy.

Two more states exist and both stop a run:

* **damaged** — an object at a key the owner recognises, holding something
  that is not that owner's document.
* **foreign** — a key under a family's prefix that its adapter does not
  recognise at all. Neither migrated nor ignored.

A top-level prefix belonging to no family at all is reported as an **unknown
prefix**, which blocks a cutover without blocking a copy.

## 2. The four commands

```sh
aiwatcher-migrate plan   --store fs:DIR --org UUID --project UUID \
                         [--prefix family=value]... [--out manifest.json]
aiwatcher-migrate verify --store fs:DIR --manifest manifest.json \
                         [--iam SPEC --principal PROVIDER:SUBJECT]
aiwatcher-migrate apply  --store fs:DIR --manifest manifest.json \
                         --checkpoint ck.json --iam SPEC \
                         --principal PROVIDER:SUBJECT --confirm [--exclusive-access]
aiwatcher-migrate resume --store fs:DIR --manifest manifest.json \
                         --checkpoint ck.json --iam SPEC \
                         --principal PROVIDER:SUBJECT --confirm [--exclusive-access]
```

`plan` and `verify` write nothing at all — not even a probe object. `apply`
and `resume` are the same operation; the two words exist so that an operator
saying which one they believe they are doing gets told when they are wrong:
`apply` refuses to run over an existing checkpoint, and `resume` refuses to run
without one.

`--store` takes `fs:DIRECTORY`, an existing snapshot directory. There is no S3
selector: see §8.

`--iam` is `postgres://…` in a deployment and `fixture:FILE` for a rehearsal.
A fixture is **not an authority** — every receipt it admits carries
`non_authoritative_iam`, and no cutover may be declared on one. Build with
`--features postgres` for the PostgreSQL adapter; a build without it says so by
name rather than falling back.

`--principal` is `PROVIDER:SUBJECT`, the exact pair the control plane compares.
Never an email address and never a group name.

`--prefix` belongs to `plan`. An execution reads the prefixes out of the
manifest it was given, and says so rather than accepting a flag it would
ignore.

## 3. Reading a dry run

`plan` prints `{"manifest": …, "survey": …}` and, with `--out`, writes the
manifest alone to a file.

The **manifest** is a pure function of the snapshot and the configuration. Its
`manifest_id` is the SHA-256 of its own body, so planning an unchanged snapshot
twice produces byte-identical files. Read, at minimum:

* `body.families` — every family, with one of the states above;
* `body.snapshot.objects` / `.bytes` — what would be copied;
* `body.blockers` — sorted, each `refuses_execution` or `refuses_cutover`;
* `body.references.internal_missing` — every object naming something in its own
  registry that is not in the snapshot. These are **copied as they stand**: the
  source was already inconsistent, and rewriting a reference to make the copy
  look tidy is changing history.
* `body.identity` — the statement that no grant is created, no IdP group is
  read and no stored byte is rewritten.

The **survey** is what the target held when somebody looked: `absent`,
`identical`, and every `conflict` named with both digests. It is deliberately
*not* part of the manifest's identity, because a half-finished copy changes the
target and must not change the plan.

## 4. The cutover protocol

The tool performs step 3 and nothing else. Every other step is a person.

1. **Freeze.** Stop writes and ingest to the registries being moved. The tool
   cannot do this and will not pretend to: it detects a source that moved and
   refuses, which is a failed migration rather than a prevented one.
2. **Snapshot.** Take a copy of the object store, and run against the copy.
   Record where it came from — the `--store` string is recorded in the manifest
   and compared on resume.
3. **Plan, review, apply.**
   ```sh
   aiwatcher-migrate plan --store fs:/snapshots/2026-09-18 \
     --org "$ORG" --project "$PROJECT" --out manifest.json
   # read it, then:
   aiwatcher-migrate apply --store fs:/snapshots/2026-09-18 \
     --manifest manifest.json --checkpoint migration.checkpoint.json \
     --iam "$IAM_URL" --principal oidc:"$SUBJECT" --confirm
   ```
   On a failure, fix the cause and `resume` with the same manifest and the same
   checkpoint.
4. **Count and check.** The receipt carries `written`, `already_identical`,
   `resumed` and `complete`. Re-run `verify`: `absent` must be 0 and
   `conflicts` empty. Compare `body.families[*].categories` with what the
   product shows for that project.
5. **Resume writes.** Only after step 4.
6. **Rollback** is restoring the snapshot, or deleting the project's prefixes
   and starting again. It is *not* re-enabling a global API: the legacy routes
   still serve the legacy data, which this tool never touched, and turning
   project scoping off would hand every project's data to everybody rather
   than undoing a copy.

A run is `complete` when every object in the manifest is in the target. It is
`cutover_ready` only when `complete` **and** no blocker of any kind stands —
which, for every store this build can read today, it will not be, because the
unsupported families are still there. That is the honest answer and the tool
prints it on stderr.

## 5. What stops a run, and what it leaves behind

Every refusal below leaves the target exactly as it was found.

| Situation | What happens |
| --- | --- |
| The manifest was edited | Refused on its own digest, before anything is read |
| The manifest was edited and re-sealed | The executor plans again from the live store and refuses on the difference |
| A source object changed or vanished | `snapshot changed`, before any write |
| An object under a family's prefix is foreign or damaged | Refused, named, nothing written |
| A target key holds different bytes | Refused, both digests reported, **not overwritten** |
| A target key holds identical bytes | A no-op, counted as `already_identical` |
| A write reports success and stores something else | Caught by the read-back, refused |
| The store cannot publish create-only | Refused, unless `--exclusive-access` is stated |
| The checkpoint belongs to another plan, target or store | Refused by name |
| No IAM was reached | A dry run may say so; an execution refuses |
| IAM does not admit the operator on that project | Refused, and the refusal says "it does not exist or you hold no live grant" — the control plane does not distinguish them, so neither does this |

## 6. Crash safety

The copy order is the registries' own: content, then records, then indexes — so
an interrupted copy leaves an unindexed object rather than an index whose rows
404. Per object the loop is: read the source and check its digest, read the
target, write (create-only), read the target back, **then** checkpoint.

A checkpoint written after the read-back can only lag. A lagging checkpoint
costs a repeated copy of bytes that are already identical, which is a no-op. A
checkpoint that ran ahead would lose an object, which is why it is never
written first.

## 7. The `--exclusive-access` statement

`ObjectStore::create` publishes an object only when its key is absent. The
filesystem, memory and S3 adapters in this workspace all implement it; an
adapter that does not returns a refusal, and this tool **refuses rather than
falling back to read-then-write**, because read-then-write claims a safety
against concurrent writers that it does not have.

`--exclusive-access` is the operator stating that this run has the store to
itself. The receipt records `publication: exclusive_access` so nobody later
reads that run as proof of something it never had.

## 8. Known limits

* **The CLI reads a filesystem snapshot only.** The library is written against
  the `ObjectStore` port, so an S3 store works through it; the `fs:` selector
  is the only one wired, and a bucket is migrated by snapshotting it to a
  directory first. Against S3 directly, nothing here has been verified.
* **Prefix nesting.** Configuring one family's prefix inside another's (say
  `datasets` and `datasets/prompts`) makes each adapter see the other's keys as
  foreign, and the run is refused. That is loud rather than silent, and it is
  not otherwise handled.
* **Large objects.** The `ObjectStore` port reads whole objects, so an
  annotation blob is held in memory while it is hashed and copied. There is no
  streaming path.
* **One checkpoint per run.** The checkpoint is a local file. Two operators
  running the same plan against one store from two machines each keep their
  own; create-only is what keeps that safe, not the checkpoint.
* **No adapter is a blocker, not a gap in the numbers.** Read §1 before
  telling anybody a project holds its data.
