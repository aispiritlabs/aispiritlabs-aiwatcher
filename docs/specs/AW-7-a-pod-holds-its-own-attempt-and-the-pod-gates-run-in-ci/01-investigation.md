---
id: AW-7
step: investigation
status: done
branch: main
repo: aiwatcher
created: 2026-09-12
updated: 2026-09-12
tags: [spec/AW-7, step/investigation, branch/main, status/done]
---

`#spec/AW-7` · `#step/investigation` · `#branch/main` · repo `aiwatcher`

# ① Investigation — AW-7

## Problem

AW-4 finished on 2026-09-12 with three pod backends behind one port. The owner
asked what is left to do, and reading the code for an answer found three gaps.
None of them is a defect in anything the gates assert. Each one is a place where
the gates never looked.

## Findings

### 1. The `docker` backend only works where the desktop names the host

- The gate and `.env.example` both tell a container to reach the API at
  `http://host.docker.internal:<port>`.
- OrbStack and Docker Desktop define that name. A plain Docker Engine on Linux
  does not unless the container is started with
  `--add-host=host.docker.internal:host-gateway`.
- `run_arguments` in
  [docker.rs](../../../crates/aiwatcher-server/src/execution/pods/docker.rs)
  passes no `--add-host`.
- So the backend has run only on a Mac. On a Linux server, and on a CI runner,
  the claim would never arrive. The gate's probe container would say so before
  any stage ran, but nothing runs the gate there.

### 2. Nothing in CI runs a pod gate

- [ci.yml](../../../.github/workflows/ci.yml) has jobs for Rust, the contract,
  Laser, the workflow store, the object store and the query engines. It has none
  for `just e2e-pods`, `just e2e-docker` or `just e2e-processes`.
- These gates exist because a stand-in cluster opens no connection. They are what
  found the two rustls providers panicking on the first real call in a build that
  was green everywhere else, and they found the missing `label=` prefix, which
  left `jobs()` failing every pass while launches kept working.
- Nothing stops the next one of those from landing.

What each gate needs, read from
[e2e-pod-steps.py](../../../scripts/e2e-pod-steps.py):

| gate | builds | needs | runs where |
|---|---|---|---|
| `e2e-processes` | plain `cargo build` | `uv` | anywhere |
| `e2e-docker` | plain build and `deploy/Dockerfile.worker` | an engine | any Linux runner — once finding 1 is fixed |
| `e2e-pods` | `--features kube` and the worker image | a local cluster | a kind cluster. Needs `kind load docker-image` and an `--api-host` the kind network routes to the runner, neither of which the script does today |

### 3. A pod outside a cluster cannot hold its credential

ADR_0029 gave a pod its template's queue-scoped ingest token, from a Secret the
template names via `envFrom` or `secretKeyRef`. What happens to that token on the
two host backends:

- **Neither can read a Secret.**
  [manifest.rs](../../../crates/aiwatcher-server/src/execution/pods/manifest.rs)
  refuses `envFrom` and every `valueFrom` but `metadata.name`, on purpose: a
  program without the credential it was written to hold fails somewhere else.
- **`process` passes on nothing of aiwatcher's.**
  [process.rs](../../../crates/aiwatcher-server/src/execution/pods/process.rs)
  withholds every `AIWATCHER_*` variable of the server's own environment
  (`NOT_INHERITED`).
- **So under `oidc` or `proxy`, the token must be a literal in the templates
  file.** Every pod of the template shares that long-lived `Editor` secret, and
  it is readable through `docker inspect` or `/proc`.
- **Under `local` it is worse.** `Authenticator::authenticate` returns early and
  accepts only the local token, which is `admin`.
- **No gate has seen any of this.** The gate's `serve` strips every
  `AIWATCHER_*` variable from the shell and sets no auth mode, so its server
  runs as `none` and the question never comes up.

What a pod's worker actually calls, read from `aiwatcher_sdk.worker`:

- **The five worker routes**
  ([worker.rs](../../../crates/aiwatcher-api/src/worker.rs)): a claim naming its
  attempt, then `heartbeat`, `result`, `inputs/{name}` and `outputs/{name}`
  under that attempt's key.
- **`POST /api/v1/events`**, which carries the step's spans through the tracer
  and `TaskContext.record_evaluation`.

Both go through `AIWATCHER_TOKEN`, and nothing else in the worker calls the API.

What any credential the authentication layer accepts can reach:

- Most read routes check no role at all. `runs.rs`, `live.rs` and `stream.rs`
  take no `Caller`, so being authenticated is the whole check.
- An ingest token is `Editor` (`IngestToken::identity`) on every route that does
  check one. It can save a definition and start an execution.

Two smaller findings sit beside it:

- **Outside a cluster, nothing enforces a pod's deadline.** Neither `docker.rs`
  nor `process.rs` reads `activeDeadlineSeconds`. The watch leaves a live pod
  whose attempt is not claimable alone (`mod.rs`, the `_ => return Ok(false)`
  arm), so a step past its timeout runs until it exits.
- **Nobody has deployed a template.** planner's commit naming one is still to be
  written. The chart's `AIWATCHER_TOKEN` example in `values.yaml` is a comment.

## Constraints

The guardrails this touches, all in [CLAUDE.md](../../../CLAUDE.md):

- *Never let an ingest token be more than an editor*, and *never let a queue
  widen a token*. A new credential must narrow and never widen.
- *Never authorise a worker route by the name it claimed under.* The lease and
  the scope are checked together, and a new scope joins that check rather than
  replacing it.
- *Never let a route decide for itself whether it needs a caller.* The layer
  authenticates, and the handlers decide roles.
- *Never split the binary in two without sharing all three backends.* A signing
  key is a fourth thing a split binary would have to share.
- *Never leave a process's crypto provider to the features.* Signing stays on
  the `ring` HMAC `aiwatcher-auth` already uses.
- *Never let a plan know what a pod is.* The credential is the launcher's and the
  backend's. A step and its plan never see it.
- ADR_0029's *What would make this wrong* names a one-attempt credential as the
  answer to a leaked pod token.

## Options

**For finding 1:**
- **Always pass `--add-host=host.docker.internal:host-gateway`.** Harmless where
  the name already resolves. Needs an engine of 20.10 or later.
- **Make it configuration.** One more variable for a name every engine should
  just answer.

**For finding 2:**
- **One job per gate on every push.** Three cold builds, one of them a
  `kube`-featured server.
- **One `pods` job on every push running `e2e-processes` and `e2e-docker` over a
  single cargo build, plus a scheduled `e2e-pods` on kind.** The two host gates
  are minutes. The cluster gate adds a second feature build and a cluster, and
  what it catches beyond them is the cluster client itself.

**For finding 3:**
- **A host-side secret source.** The two host backends would read
  `secretKeyRef` from a directory laid out like a mounted Secret. One mechanism
  across all three backends. It keeps a shared `Editor` token per template, and
  it does nothing for `local`.
- **Inherit `AIWATCHER_TOKEN` for `process`.** A step would hold the server's
  credential, which is `admin` under `local`.
- **A credential minted per attempt, accepted on that attempt's routes and on
  ingest, refused everywhere else by the layer.** ADR_0029's deferred mode. It
  works on all three backends and in every auth mode, with no change to the SDK.

## Recommendation

- **Findings 1 and 2 go first and together.** The CI job on Linux is what proves
  the `--add-host` fix, and once it exists every later part is checked by
  somebody other than this machine.
- **Finding 3 gets the minted credential.** It is decided in
  [ADR_0031](../../ADR/ADR_0031_POD_ATTEMPT_CREDENTIAL.md), with the deadline
  held on every backend as part of it: an expiry nothing enforces only makes a
  runaway step's calls fail.
- **Application secrets outside a cluster are out of scope.** A step's own
  database password is the operator's and stays a literal. The host-side secret
  source above is the right answer to that question, and it is a separate one.

## Log
- 2026-09-12 10:40 — investigation written from the code after AW-4's 2.7: `docker` lacks `host-gateway`, no pod gate runs in CI, and a host pod's token can only be a literal in the templates file (admin under `local`); the host backends enforce no deadline either
