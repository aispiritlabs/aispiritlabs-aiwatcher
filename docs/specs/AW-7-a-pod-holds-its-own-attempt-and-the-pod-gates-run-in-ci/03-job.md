---
id: AW-7
step: job
status: todo
branch: main
repo: aiwatcher
created: 2026-09-12
updated: 2026-09-12
tags: [spec/AW-7, step/job, branch/main, status/todo]
---

`#spec/AW-7` · `#step/job` · `#branch/main` · repo `aiwatcher`

# ③ Job — AW-7

## Technical approach

Two passes. The first makes a machine other than this one check the pod path.
The second changes what a pod holds, and it is checked by the first.

- **Part A+B, one change.** The `docker` backend adds the host name. A CI job
  runs both host gates on a Linux runner, which is the only place the fix can be
  proven. A scheduled workflow runs the cluster gate on kind, once the gate can
  load its image there and route to the runner.
- **Part C, six slices.** Each one compiles, is green on its own, and is
  committed before the next starts:
  1. **C1 — `aiwatcher-auth`.** The credential, its derived key, and its check in
     `oidc`, `proxy` and `local`. Nothing mints one yet, so nothing changes for
     anybody.
  2. **C2 — `aiwatcher-api`.** The layer's two doors, the worker routes' key
     check, and the contract. `Identity` gains a field, so `just openapi` runs
     here. This slice waits until `contracts/openapi.json` and
     `apps/panel/src/api/generated` no longer hold another session's uncommitted
     work; regenerating over it is the one way to lose that work.
  3. **C3 — `aiwatcher-server`.** The secret, the split refusal, one set of
     credentials per process, and the launcher minting into the manifest. This
     is where behaviour changes, and only under auth. The gates run under
     `none` until C6, so they stay green through this slice.
  4. **C4 — the cluster backend.** The Secret the Job owns, and the grant.
  5. **C5 — the watch.** The deadline on every backend.
  6. **C6 — gates and documents.** The gates run with auth on and lift a
     credential the way leaked step code would. ADR_0031 is accepted, and
     ADR_0029, `CLAUDE.md`, the chart and the examples follow.

## Decisions

Beyond what ADR_0031 decides:

- **`--add-host=host.docker.internal:host-gateway` is always passed.** It is
  harmless where a desktop already answers the name, and it needs engine 20.10
  or later, the same floor the backend's `--pull` flag already sets. OrbStack is
  re-run to prove there is no regression, and CI proves Linux.
- **Docker gets the credential by name, never in an argument.** `run_arguments`
  passes every variable as `--env NAME=value`, and an argument list is readable
  through `ps` by every user on the host for as long as `docker run` is running.
  `AIWATCHER_TOKEN` goes as `--env AIWATCHER_TOKEN`, with its value in the
  environment of the `docker` process itself. The other variables can stay as
  they are, because none of them is a secret aiwatcher put there.
- **One set of attempt credentials per process, shared by the authenticator and
  the launcher.** With no secret set, the key is generated at start-up. Two
  instances in one process would be two keys, and every pod would be refused by
  the server that started it.
- **A refused door is a 403 of its own.** `Forbidden { needed, held }` would say
  "needs editor, holds editor". The body names the credential, the attempt it
  holds, and the two places it opens.
- **Public paths stay public to anyone.** `is_public` runs before
  authentication, so an attempt credential on `/livez` gets what no credential
  gets. The every-path test leaves those paths out, and says why.
- **The Secret has the Job's name, under key `token`, and the manifest rewrite
  is a pure function.** Secrets and Jobs are different kinds, so the names do
  not collide. `KubeCluster` takes the plain `env` value out, puts a
  `secretKeyRef` in its place, and creates the Secret with an owner reference
  after the Job. The rewrite is tested without a cluster, and the creation is
  proven by `e2e-pods`.
- **A pod waiting for its Secret is `Live`.** `unstarted_reason` already reports
  a waiting reason such as `CreateContainerConfigError` as a reason rather than
  an ending. If the Secret never comes, the start allowance still decides.
- **The timeout rides as `aiwatcher.dev/timeout`,** beside the three attempt
  annotations, so all three backends' listings return it. `activeDeadlineSeconds`
  exists only on a cluster.
- **The gate lifts a credential through a step's own output.** A test task writes
  `AIWATCHER_TOKEN` into an artifact, and the gate reads it back. That is how
  step code would leak it, and it works the same on all three backends, with no
  `docker inspect` and no `/proc`.
- **The gate runs in `proxy` mode.** The gate calls as an admin by header, and a
  lifted credential sends none, so it lands on the credential path. `local` is
  asserted once on `process`, the single-user case, because it refuses a server
  bound beyond loopback and `docker` and `kubernetes` need exactly that.
- **CI has two workflows.** `ci.yml` gains `pods` on `ubuntu-latest`: toolchain,
  `rust-cache`, `setup-uv`, `setup-just`, then `just e2e-processes` and
  `just e2e-docker` over one build. A new `pods-cluster.yml` runs nightly and on
  `workflow_dispatch`, with kind, helm and kubectl, then `just e2e-pods`. On
  failure both upload the gate's server log.

## Tasks

### Part A+B — the host name, and the gates in CI
- [ ] 7.1 **The host name.**
  - `docker::run_arguments` passes `--add-host=host.docker.internal:host-gateway`.
  - A unit test asserts it.
  - One sentence goes into ADR_0029's backend amendment.
  - `just e2e-docker` passes on OrbStack.
- [ ] 7.2 **The gate on kind.**
  - On a `kind-*` context, `e2e-pod-steps.py` runs
    `kind load docker-image <image> --name <cluster>` after building.
  - `--api-host` defaults to the `kind` Docker network's gateway, from
    `docker network inspect`. The probe pod still refuses an address that does
    not route before any stage runs.
- [ ] 7.3 **CI.**
  - `ci.yml` gains a `pods` job running `just e2e-processes` and
    `just e2e-docker`.
  - `pods-cluster.yml` runs `just e2e-pods` on a schedule and on demand.
  - Both upload the server log when they fail.
  - The runner answers `OOMKilled` on cgroup v2. If it does not, that phase is an
    *Issue* in `04-tests.md`, not a skipped assertion.
  - Green on a pushed branch before this part is called done.

### Part C1 — `aiwatcher-auth`
- [ ] 7.4 **The credential.**
  - An `attempt` module with `AttemptScope` (execution, step, attempt number,
    queue) and `AttemptCredentials::{new(secret: Option<&str>), mint(scope, ttl),
    open(token)}`, on a `Signer` over HMAC-SHA256(secret,
    `aiwatcher pod attempt credential v1`). Each value carries the `aw-attempt.`
    prefix.
  - `Credential::Attempt`, and `Identity.attempt: Option<AttemptScope>`.
  - `Identity::may_claim` holds for its own queue only.
  - Tests:
    - a credential opens back to its scope;
    - one past its expiry is refused;
    - one sealed under another secret is refused;
    - a session sealed under the *same* secret does not open;
    - a value without the prefix never reaches `open`.
- [ ] 7.5 **Accepted where it may be.**
  - `Authenticator::authenticate` accepts it in `oidc` before the ingest tokens,
    in `proxy` beside them, and in `local` beside the local token.
  - A test per mode.
  - An attempt credential set as the cookie is refused.
- [ ] 7.6 **The key's source.**
  - `AuthConfig` takes an `AttemptCredentials`, not a string, so the server can
    hand the launcher the same instance.
  - With no secret, the ephemeral key warns once, and only when pods launch
    under auth.

### Part C2 — `aiwatcher-api`
- [ ] 7.7 **The two doors.**
  - `auth::admits_attempt(path)` holds for `/api/v1/events` and for
    `/api/v1/worker/claims` and everything under it.
  - The layer refuses an attempt credential anywhere else, with its own 403 and
    error code.
  - Tests: `GET /api/v1/runs`, `/api/v1/spans` and the live stream refuse it.
- [ ] 7.8 **Its own attempt.**
  - `claim` refuses a request whose `attempt` is absent or is not the
    credential's.
  - `held` and `recorded_result` compare the path's key with the credential's
    before the lease and the queue.
  - `http.rs` covers the heartbeat, result, inputs and outputs of another
    attempt, and a claim without `attempt`. Each is a 403, with the row
    unchanged.
- [ ] 7.9 **Every path, now and later.**
  - One test walks `ApiDoc::document().paths` and presents an attempt credential
    on every operation that is neither public nor a door. Every one refuses.
- [ ] 7.10 **The contract.**
  - `just openapi`, once the generated files carry nobody else's uncommitted
    work.
  - Commit `contracts/openapi.json` and `apps/panel/src/api/generated` by path.

### Part C3 — `aiwatcher-server`
- [ ] 7.11 **Configuration.**
  - `AIWATCHER_POD_CREDENTIAL_SECRET` is read.
  - `Config::validate` refuses `serve` or `work` with auth on, templates set and
    no secret, naming the variable.
  - The combined role is not refused.
  - `.env.example` documents it.
- [ ] 7.12 **Minting.**
  - `pods::OWNED_ENV` gains `AIWATCHER_TOKEN`, so a template setting it is
    refused at start, with a test.
  - Under auth, the launcher mints for the attempt with an expiry of the start
    allowance plus the step's timeout plus `LEASE_SECONDS`, and the manifest's
    environment carries it.
  - Under `none` nothing is minted, with a test.
  - Wiring builds one `AttemptCredentials` and gives it to both the
    authenticator and the launcher.
- [ ] 7.13 **Held without being printed.**
  - `docker` passes the token by name, with its value in the client's
    environment.
  - `process` hands it to the child. A test shows it arrives while the server's
    own `AIWATCHER_TOKEN` still does not.
  - A test holds that no `ClusterError`, launch log line or `Debug` of a request
    carries the value.

### Part C4 — the cluster backend
- [ ] 7.14 **The Secret.**
  - A pure `credential_secret(manifest, job)` returns the rewritten manifest and
    the Secret. It is unit-tested: `secretKeyRef` in the container, no value
    anywhere in the Job, and an owner reference with the Job's uid filled in
    after creation.
  - `KubeCluster::create_job` creates the Job, then the Secret.
  - On `AlreadyExisted` it reads the Job's uid and creates the Secret, taking
    `AlreadyExists` as done.
- [ ] 7.15 **The grant.**
  - `pods.yaml`'s Role gains `secrets: [create]`.
  - The gate's `can-i` asserts `create secrets` is allowed, and `get`, `list`
    and `watch` on secrets are not.
  - `just chart-check`.
- [ ] 7.16 **The chart.**
  - `execution.pods.credentialSecret: {name, key}` is rendered into the server
    and the worker whenever templates are set and auth is on.
  - The chart `fail`s for a split release with auth, templates and no secret.
  - The `values.yaml` example drops its token and says why.

### Part C5 — the deadline
- [ ] 7.17 **The timeout annotation.**
  - The manifest carries `aiwatcher.dev/timeout`.
  - `Observed` gains the timeout, read back by all three backends.
  - `manifest::attempt_of`'s neighbours are tested.
- [ ] 7.18 **The watch stops it.**
  - A `Live` pod past its creation plus start allowance plus timeout is deleted.
  - Its unfinished attempt ends as `Infrastructure`, reason `DeadlineExceeded`.
  - A stand-in cluster test covers it, and so does the case where the attempt
    had already been reported, where nothing is reported twice.

### Part C6 — gates and documents
- [ ] 7.19 **The gates speak auth.**
  - `e2e-pod-steps.py` runs its server in `proxy` mode, with the gate calling as
    an admin by header, and no template carries a token.
  - A phase lifts a pod's credential through a test task's output. It asserts
    the credential is refused on another attempt's heartbeat and on
    `GET /api/v1/runs`, and accepted on `POST /api/v1/events`.
  - `process` additionally runs one pass under `local`.
- [ ] 7.20 **The deadline phase.**
  - A step with a 5 s timeout sleeping 60 s is gone within two passes on
    `docker` and `process`, ending `infrastructure` with `DeadlineExceeded`.
  - On a cluster the same words come from the cluster itself.
- [ ] 7.21 **The documents.**
  - ADR_0031 is accepted, with an amendment for anything built differently.
  - ADR_0029's status line and credential paragraph point at it.
  - `CLAUDE.md`: decision 13, and a guardrail next to *Never let an ingest token
    be more than an editor*: never give a pod a credential for more than its
    attempt.
  - The SDK worker README says a launched pod's token is aiwatcher's, and a
    long-lived worker's is still the operator's.
  - `docs/decisions/EXECUTION.md` moves 0031 from proposed to accepted.
- [ ] 7.22 **Verified.**
  - `cargo fmt --check`.
  - `cargo clippy --workspace --all-targets --all-features -- -Dwarnings`.
  - `cargo test --workspace --all-targets`.
  - `cargo test -p aiwatcher-server --features kube`.
  - `python3 scripts/lint-comments.py`.
  - `just chart-check` and `just openapi-check`.
  - All three gates locally, and both CI workflows green.
  - What they printed goes into `04-tests.md`.

## Risks

- **The contract is held by another session.** C2 cannot land while the
  evaluation work sits uncommitted in the generated files. C1 and C3–C5 do not
  depend on C2's contract change, only on its checks, so they can be written
  first. They are merged in order all the same.
- **`OOMKilled` on a hosted runner.** If the runner's cgroup setup does not
  report it, the memory phase fails in CI and passes on a desktop. That goes in
  as an *Issue* with the output, and the decision is the owner's.
- **Split deployments gain a required secret.** The chart refuses without it. A
  release that sets no templates is not affected.

## Log
- 2026-09-12 10:40 — job planned on `main`: A+B first (the host name, and the three gates in CI with kind for the cluster one), then C in six slices behind ADR_0031; ten further decisions made, the Docker argument list among them; nothing built
