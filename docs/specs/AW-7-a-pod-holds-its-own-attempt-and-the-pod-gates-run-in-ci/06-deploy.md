---
id: AW-7
step: deploy
status: done
branch: main
repo: aiwatcher
created: 2026-09-14
updated: 2026-09-14
tags: [spec/AW-7, step/deploy, branch/main, status/done]
---

`#spec/AW-7` · `#step/deploy` · `#branch/main` · repo `aiwatcher`

# ⑥ Deploy — AW-7

## Shipped
- [x] Merged: on `main` and pushed. Parts A and B — `184897f`, `21e83c9`,
      `e27dcf6`; part C — `e91febe`, `44a7409`, `9942814`, `1f87eac`, `020fb87`,
      `1405292`. `27277eb`, the log line recording it green, is local.
- [x] Released: images built by the `Release images` workflow, green on
      `1405292`; no tag
- [x] Rollout / flag: under any auth mode but `none` the launcher mints each
      pod a credential for its attempt, in a Secret its Job owns on a cluster; a
      split release refuses to start without `AIWATCHER_POD_CREDENTIAL_SECRET`;
      no template carries an aiwatcher token

④ tests and ⑤ review have no notes. Every task in `03-job.md` is ticked with
the gate it ran, and the gates are the CI jobs below.

## Post-deploy checks
- [x] Behaviour matches the spec where it now runs: on `1405292`, CI's `pods`
      job (both host gates, authentication on) and the `Pod gate (cluster)`
      workflow (kind) are green; the nightly schedule was green on `21e83c9`
      the same morning
- [x] No regression in the signals this change could move: CI green on
      `1405292`. Not re-run for this note.

## Graduated decisions
- **A pod authenticates as its attempt, with a credential the launcher mints,
  and the watch holds a Job's whole deadline on every backend** →
  [ADR_0031](../../ADR/ADR_0031_POD_ATTEMPT_CREDENTIAL.md), accepted 2026-09-14,
  superseding what a pod holds in
  [ADR_0029](../../ADR/ADR_0029_POD_PER_STEP.md)
- **Never give a pod a credential for more than its attempt** → `CLAUDE.md`,
  Guardrails

## Archive
- ADDED:
  - a container on any engine reaches the host by one name;
  - the host pod gates run on every push;
  - the cluster pod gate runs on a schedule, on kind;
  - a pod is given a credential for its own attempt, and nothing else of aiwatcher's;
  - an attempt credential opens its own attempt's routes and ingest, and nothing else;
  - an attempt credential expires with its attempt's deadline;
  - no other sealed value opens as an attempt credential;
  - the roles that mint and check share the key, or the split is refused;
  - on a cluster, the credential is a Secret the Job owns;
  - a pod past its deadline is stopped on every backend.
- MODIFIED: what a launched pod holds (ADR_0029).
- REMOVED: nothing.

## Log
- 2026-09-14 11:46 — closed at the owner's ask; shipped on `main`, CI and kind green on `1405292`
