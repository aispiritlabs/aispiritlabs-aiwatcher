---
id: AW-7
step: spec
status: done
branch: main
repo: aiwatcher
created: 2026-09-12
updated: 2026-09-12
tags: [spec/AW-7, step/spec, branch/main, status/done]
---

`#spec/AW-7` · `#step/spec` · `#branch/main` · repo `aiwatcher`

# ② Spec — AW-7

## Proposal

**Intent.** Everything AW-4 built for a step's pod holds on every backend and in
every auth mode, and a machine other than the developer's checks it on every
push.

- **A pod authenticates as its attempt and as nothing else.** Code in one pod
  cannot settle another's attempt, read the runs list or start an execution.
- **No aiwatcher secret goes into a templates file.**
- **A `docker` backend runs on a Linux engine.**

**In scope.**
- **A.** `--add-host=host.docker.internal:host-gateway` on every container.
- **B.** A CI job running `e2e-processes` and `e2e-docker` on every push, and a
  scheduled `e2e-pods` on kind, with the gate taught to load its image into kind
  and route to the runner.
- **C.** The credential ADR_0031 decides, all of it:
  - the credential type, its key and its check in every authenticating mode;
  - the layer's two doors;
  - the worker routes' key check;
  - minting in the launcher;
  - a Secret per Job on `kubernetes`, with one more grant;
  - the deadline held by the watch;
  - a gate phase run with auth on;
  - the documents: ADR_0029 amended, ADR_0031 accepted, `CLAUDE.md`,
    `.env.example` and the chart.

**Out of scope.**
- **An application's own secrets outside a cluster.** A template's database
  password stays a literal. A host-side secret source is a separate change.
- **A read scope for the credential.** ADR_0031 names the observation that would
  add one.
- **Narrowing ingest to the attempt's execution.** Same.
- **Renewing the credential on heartbeat.** Same.
- **planner's own commit naming a template**, which now also means no token in
  that template.
- **Closing AW-4's spec-flow phases 04–06.** That is AW-4's card.

**Approach.** A and B first, in one change, because the Linux job is what
proves A. Then C in six slices, each compiling and green on its own:

1. **auth:** the credential and its key;
2. **api:** the doors and the key check;
3. **server:** minting and the split refusal;
4. **kubernetes:** the Secret and the grant;
5. **watch:** the deadline;
6. **gates and documents.**

The contract moves in slice 2, because `Identity` gains a field. That slice
waits for the generated files to be free of another session's in-flight work.

**Success criteria:**
- [ ] `just e2e-docker` passes on a Linux runner and still passes on OrbStack
- [ ] CI runs `e2e-processes` and `e2e-docker` on every push and `e2e-pods` on a
      schedule, and a broken launcher fails the job naming the phase
- [ ] With auth on, the four stages complete on all three backends with no
      token in any template
- [ ] A credential lifted from one pod is refused on another attempt's routes
      and on every read route, and accepted on its own and on ingest
- [ ] On `kubernetes` no credential appears in a Job or pod spec, and the
      launcher still cannot read a Secret
- [ ] A step running past its timeout is stopped on `docker` and `process` as
      it is on a cluster

## Spec (delta)

### ADDED Requirements

#### Requirement: a container on any engine reaches the host by one name
The `docker` backend SHALL start every container with
`host.docker.internal` resolving to the host, so that one
`AIWATCHER_POD_API_URL` works on a Linux engine and on a desktop one.

##### Scenario: a Linux engine
- GIVEN a Docker Engine on Linux with no desktop in front of it
- WHEN a step's container starts with `AIWATCHER_URL=http://host.docker.internal:<port>`
- THEN its claim reaches the server and the attempt runs

##### Scenario: a desktop engine does not regress
- GIVEN OrbStack
- WHEN `just e2e-docker` runs
- THEN all six phases pass as they did before the change

#### Requirement: the host pod gates run on every push
CI SHALL run `just e2e-processes` and `just e2e-docker` on every push and pull
request, in one job sharing one build.

##### Scenario: a broken launcher path fails CI
- GIVEN a change under which a pod's attempt is never claimed
- WHEN CI runs the pods job
- THEN the job fails, and its output names the phase that did not hold

#### Requirement: the cluster pod gate runs on a schedule, on kind
CI SHALL run `just e2e-pods` against a kind cluster on a schedule and on demand,
and the gate SHALL run on kind without hand-editing.

##### Scenario: the scheduled run
- GIVEN the scheduled workflow
- WHEN it creates a kind cluster and runs `just e2e-pods`
- THEN the worker image is loaded into the cluster, the pods reach the API on the runner, and every phase passes, the memory phase included

#### Requirement: a pod is given a credential for its own attempt, and nothing else of aiwatcher's
When the server authenticates (any `AIWATCHER_AUTH_MODE` but `none`), the
launcher SHALL put a credential scoped to the attempt into the pod's
`AIWATCHER_TOKEN`. `AIWATCHER_TOKEN` SHALL be one of the variables aiwatcher
owns on a pod.

##### Scenario: four stages authenticate with no token in a template
- GIVEN auth on, and a template that sets no `AIWATCHER_TOKEN`
- WHEN a four-stage run starts on `kubernetes`, `docker` or `process`
- THEN every pod claims, heartbeats, reads its inputs, writes its outputs, publishes its events and reports, and the run completes

##### Scenario: nothing is minted when nothing checks
- GIVEN `AIWATCHER_AUTH_MODE=none`
- WHEN the launcher builds a pod's manifest
- THEN its environment carries no `AIWATCHER_TOKEN`

##### Scenario: a template may not bring its own
- GIVEN a template whose container sets `AIWATCHER_TOKEN`
- WHEN the server starts
- THEN it refuses, naming the template and the variable

#### Requirement: an attempt credential opens its own attempt's routes and ingest, and nothing else
An attempt credential SHALL be accepted only on the worker routes for the
attempt it names and on `POST /api/v1/events`, and the authentication layer
SHALL refuse it on every other path, including paths added later.

##### Scenario: another attempt's route
- GIVEN a credential for `run-1/analyze/1`
- WHEN it is presented on the heartbeat, result, inputs or outputs of `run-1/persist/1`
- THEN the answer is 403 and that attempt's row is unchanged

##### Scenario: a claim for anything but its attempt
- GIVEN a credential for `run-1/analyze/1`
- WHEN it claims with no `attempt`, or with another one
- THEN the answer is 403 and nothing is claimed

##### Scenario: a read route
- GIVEN a credential for any attempt
- WHEN it is presented on `GET /api/v1/runs`, `GET /api/v1/spans` or the live stream
- THEN the authentication layer answers 403 before any handler runs

##### Scenario: every other path, including one added later
- GIVEN every path in the OpenAPI document
- WHEN an attempt credential is presented on each one except the worker routes and `POST /api/v1/events`
- THEN every one refuses it

##### Scenario: ingest
- GIVEN a credential for any attempt
- WHEN it posts events to `/api/v1/events`
- THEN they are accepted, under a subject naming the attempt

##### Scenario: every mode that authenticates
- GIVEN `oidc`, `proxy` or `local`
- WHEN a pod presents its credential on its own heartbeat
- THEN it is accepted — in `local` beside the local token, which is otherwise the only credential that mode takes

#### Requirement: an attempt credential expires with its attempt's deadline
The credential SHALL expire at its minting, plus the template's start
allowance, plus the step's timeout, plus one lease.

##### Scenario: an expired credential
- GIVEN a credential past its expiry
- WHEN it is presented on its own heartbeat
- THEN the answer is 401, and the attempt ends when its lease lapses

#### Requirement: no other sealed value opens as an attempt credential
The credential SHALL be sealed under a key derived for this purpose alone. A
session SHALL NOT open as an attempt credential, and an attempt credential SHALL
NOT open as a session, even when both are given the same secret.

##### Scenario: a session presented as a bearer
- GIVEN a valid session cookie's value, and `AIWATCHER_POD_CREDENTIAL_SECRET` equal to `AIWATCHER_AUTH_SESSION_SECRET`
- WHEN it is presented as `Authorization: Bearer aw-attempt.<value>`
- THEN the answer is 401

##### Scenario: an attempt credential presented as a cookie
- GIVEN a valid attempt credential
- WHEN it is set as the session cookie
- THEN the answer is 401

#### Requirement: the roles that mint and check share the key, or the split is refused
The key SHALL come from `AIWATCHER_POD_CREDENTIAL_SECRET`. A split deployment
that authenticates and has templates SHALL refuse to start without it, and a
single process SHALL start without it on a key of its own, and warn.

##### Scenario: a split deployment with no secret
- GIVEN `AIWATCHER_ROLE=serve` or `work`, auth on, templates configured, and no `AIWATCHER_POD_CREDENTIAL_SECRET`
- WHEN the process starts
- THEN it refuses, naming the variable

##### Scenario: one process with no secret
- GIVEN the combined role, auth on, templates configured, and no secret
- WHEN it starts
- THEN it runs, and warns that a restart ends every running pod's attempt at its lease

#### Requirement: on a cluster, the credential is a Secret the Job owns
On `kubernetes`, the credential SHALL be held in a Secret named from the Job and
owned by it, read through `secretKeyRef`, and SHALL NOT appear in the Job or pod
spec. The launcher SHALL be able to create a Secret and SHALL NOT be able to read
one.

##### Scenario: a Job and its Secret
- GIVEN a pod's attempt on a cluster
- WHEN the launcher creates its Job
- THEN a Secret named from the Job exists with an owner reference to it, and the container's `AIWATCHER_TOKEN` is a `secretKeyRef` to that Secret

##### Scenario: the Secret goes with the Job
- GIVEN a Job whose log has been kept
- WHEN the launcher deletes it
- THEN its Secret is collected with it

##### Scenario: a Job that already existed
- GIVEN a Job created by a pass that crashed before its Secret
- WHEN the next pass meets that Job already existing
- THEN it creates the Secret, and the pod starts

##### Scenario: the grant
- GIVEN the chart's launcher Role
- WHEN `kubectl auth can-i` is asked as its service account
- THEN `create secrets` is yes, and `get`, `list` and `watch` on secrets are no

#### Requirement: a pod past its deadline is stopped on every backend
The watch SHALL stop a live pod whose Job is past its creation, plus the start
allowance, plus the step's timeout. The attempt, if still unfinished, SHALL end
as `Infrastructure` with the reason `DeadlineExceeded`.

##### Scenario: a step that overruns on the host
- GIVEN a step with a 5-second timeout that sleeps for 60 seconds, on `docker` or `process`
- WHEN the deadline passes
- THEN its container or process is gone within a pass or two, its attempt ends as `infrastructure` naming `DeadlineExceeded`, and the budget decides the next

### MODIFIED Requirements

#### Requirement: what a launched pod holds (ADR_0029)
~~A pod holds its template's worker token, from a Secret the template names.~~
A pod holds a credential the launcher minted for its attempt (ADR_0031). A
template no longer needs an aiwatcher token and may not set one. What it attaches
for the application stays the template's.
(Reason: the two host backends cannot read a Secret, so the template token forced
a literal secret into the templates file — an `admin` one under `local` — and a
shared `Editor` token let one pod settle another's attempt and read every run.)

## Settled

- **The credential is decided in ADR_0031** — mint per attempt, two doors, a
  derived key, a Secret on a cluster — with the alternatives it beat.
- **`--add-host` is unconditional.** A name every engine should answer is not a
  setting.
- **The host gates run on every push and the cluster gate on a schedule.** The
  cluster gate costs a second feature build and a cluster. What it catches beyond
  the host gates is the cluster client, and a day's delay on that is acceptable.

## Still open, for the job phase

- **The exact prefix, and the error body for a refused door.** A 403 naming the
  credential rather than a role, since `Forbidden { needed, held }` would read
  "needs editor, holds editor".
- **Where the timeout rides.** A new `aiwatcher.dev/timeout` annotation, or the
  Job's `activeDeadlineSeconds` read back. The second exists only on a cluster,
  so it is probably the first.
- **What kind routes to the runner as the API host.** Probably the kind network's
  gateway, and the gate needs an option that asks Docker for it.

## Log
- 2026-09-12 10:40 — spec drafted on `main`: three parts, ten requirements added over twenty-three scenarios, one modified; the credential decided in ADR_0031 (proposed); three details left to the job
