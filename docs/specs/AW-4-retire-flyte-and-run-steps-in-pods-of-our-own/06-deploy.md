---
id: AW-4
step: deploy
status: done
branch: main
repo: aiwatcher
created: 2026-09-14
updated: 2026-09-14
tags: [spec/AW-4, step/deploy, branch/main, status/done]
---

`#spec/AW-4` · `#step/deploy` · `#branch/main` · repo `aiwatcher`

# ⑥ Deploy — AW-4

## Shipped
- [x] Merged: on `main` and pushed. Part 1 — `8154041`, `a34c378`, `5f9996b`;
      2.2 `c4fc124`; 2.3 inside `26be55e`; 2.4 `c713a1e`; 2.5 `0ec11aa`,
      `409ec5d`; 2.6 `c77a997`, `aaf09fc`, `4542239`; 2.7 `29c2978`, `ff3df98`,
      `167349b`; a pod killed mid-attempt `dfb70f9`
- [x] Released: images built by the `Release images` workflow, green on
      `1405292`; no tag
- [x] Rollout / flag: a deployment still setting `AIWATCHER_ENGINE` is refused
      at start by name; a step opts into a pod by naming a template;
      `AIWATCHER_POD_RUNTIME` is `kubernetes`, `docker` or `process`
- [ ] planner naming a template on its four stages — planner's own commit, not
      this spec's

④ tests and ⑤ review have no notes. The evidence is the gates each task
recorded in `03-job.md` — `just e2e-pods`, `just e2e-processes`,
`just e2e-docker`, `just e2e-pod-death` — and AW-7 put them in CI.

## Post-deploy checks
- [x] Behaviour matches the spec where it now runs: CI's `pods` job
      (`e2e-processes`, `e2e-docker`) and the `Pod gate (cluster)` workflow
      (`e2e-pods` on kind) are green on `1405292`, 2026-09-14
- [x] No regression in the signals this change could move: CI green on
      `1405292`. Not re-run for this note.

## Graduated decisions
- **Flyte leaves aiwatcher and the pipeline engine is removed** →
  [ADR_0016](../../ADR/ADR_0016_PIPELINE_ENGINE.md), superseded; `CLAUDE.md`
  decision 13; `docs/INSTALL.md`, *The pipeline engine*
- **A step that needs a pod names a template, and the pod is a worker for one
  attempt; what a pod is — cluster, container, process — is the deployment's**
  → [ADR_0029](../../ADR/ADR_0029_POD_PER_STEP.md) and its amendments
- **Never let a plan know what a pod is; never leave a process's crypto
  provider to the features** → `CLAUDE.md`, Guardrails
- **What a pod holds** → superseded by
  [ADR_0031](../../ADR/ADR_0031_POD_ATTEMPT_CREDENTIAL.md) (AW-7)

## Archive
- ADDED:
  - a removed engine is refused by name;
  - no build carries Flyte;
  - an unknown owner is not an engine;
  - a step may ask for a pod of its own;
  - only an allowed image runs, from a known template;
  - resources come from the template;
  - the engine owns a pod's retries;
  - a cancel reaches a running pod;
  - a pod's log is kept.
- MODIFIED: Phase 12's gate — reopened by this spec, and closed by it.
- REMOVED: the pipeline engine (ADR_0016); engine-owned executions (Phase 9).

Beyond the spec, at the owner's ask: the `process` (2.6) and `docker` (2.7)
backends behind the same `Cluster` port.

## Log
- 2026-09-14 11:46 — closed at the owner's ask; shipped on `main`, gates green in CI
