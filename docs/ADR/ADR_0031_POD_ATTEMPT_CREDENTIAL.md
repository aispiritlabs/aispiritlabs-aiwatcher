# ADR_0031: A pod authenticates as its attempt, with a credential the launcher mints

- **Status**: proposed. It builds the stricter mode ADR_0029 left for later and
  makes it the only mode. AW-7 is the change
  ([`docs/specs/AW-7-…`](../specs/AW-7-a-pod-holds-its-own-attempt-and-the-pod-gates-run-in-ci/02-spec.md))
- **Date**: 2026-09-12

## Context

ADR_0029 gave a pod its template's worker token: a queue-scoped ingest token in a
Secret the template names, handed to the pod as `AIWATCHER_TOKEN`. It left a
stricter mode for later, a credential minted per Job and signed over the attempt,
and named the trigger that would make it necessary: a leaked pod token used to
settle another pod's attempt.

That trigger has not fired. What changed is where a pod can run. ADR_0029's
third amendment added two backends on the work role's own host, and neither can
hold that token honestly:

- **Neither can read a Secret.** Both refuse `envFrom` and every `valueFrom` but
  the downward API's own name, because running the program without the
  credential it was written to hold would fail somewhere else and say nothing.
  `process` also withholds every `AIWATCHER_*` variable of the server's own
  environment from a step, so a step never runs holding the server's credential.
- **So under any auth mode but `none`, the only way left is a literal value.**
  The token has to be written into `AIWATCHER_POD_TEMPLATES`. That puts a
  long-lived `Editor` secret in a configuration file, shared by every pod of
  the template, and readable through `docker inspect` or `/proc`. Under `local`
  it is worse: that mode accepts only the local token, and the local token is
  `admin`.
- **Nothing has said so.** `just e2e-docker` and `just e2e-processes` both run
  with `AIWATCHER_AUTH_MODE=none`.

Five more facts shape the answer.

1. **A pod's worker calls six things, all through `AIWATCHER_TOKEN`:**
   - the five worker routes under its own attempt's key — a claim naming that
     attempt, the heartbeat, the result, inputs and outputs;
   - `POST /api/v1/events`, which carries the step's spans and
     `TaskContext.record_evaluation`.

   Nothing else in `aiwatcher_sdk.worker` calls the API.
2. **A token is an `Editor` everywhere, and most reads check no role.** An ingest
   token is `Editor` on every route. Authentication is the only gate in front of
   runs, spans and the live stream. So any credential the authentication layer
   accepts can read all of them, whatever its role says.
3. **The signing already exists.** `aiwatcher-auth`'s `Signer` seals a value with
   an expiry under HMAC-SHA256 and checks the signature before parsing a byte.
   The session cookie is sealed with it today.
4. **Outside a cluster, nothing enforces a Job's deadline.** A cluster kills a pod
   at `activeDeadlineSeconds`. `docker` and `process` never read that field, and
   the watch leaves a live pod alone once its attempt is not claimable, so a
   stuck step runs until it exits on its own.
5. **Nobody has deployed a template yet.** planner's commit naming one on its four
   stages is still to be written, and the chart's token example is a comment. Now
   is the cheapest moment this credential will ever have to change.

## Decision

**The launcher mints one credential per attempt, and a pod holds nothing else of
aiwatcher's.** Under any auth mode but `none`, the manifest's environment gains
`AIWATCHER_TOKEN`, sealed by the launcher for that attempt alone.

- `AIWATCHER_TOKEN` joins `pods::OWNED_ENV`, so a template that sets it is refused
  at start, naming the template and the variable, as the other three are.
- Under `none` nothing is minted, because nothing would check it.
- The SDK does not change. `run-attempt` and the telemetry client already read
  `AIWATCHER_TOKEN`, and the worker already names its attempt when it claims.

**What the credential is.** A new `Credential::Attempt` in `aiwatcher-auth`,
sealed with `Signer` and written as `aw-attempt.<payload>.<signature>`. The
prefix lets it be recognised without trying every bearer as an HMAC, and keeps it
away from the JWT verifier.

- **What it carries.** The payload is the attempt as three plain fields
  (execution, step and attempt number) plus the attempt's queue. The fields are
  plain so that `aiwatcher-auth` gains no dependency on the execution crate.
- **When it expires.** Its expiry is the moment it was minted, plus the
  template's start allowance, plus the step's timeout, plus one lease
  (`aiwatcher_jobs::LEASE_SECONDS`). That is the Job's own deadline, with room
  for the report that follows it.
- **Who it authenticates as.** Subject `attempt:<execution>/<step>/<n>`, role
  `Editor`, and the attempt's queue as its only queue. The attempt it names rides
  on `Identity` as an optional `attempt`. A retry is a new attempt number, so it
  gets a new credential with a new Job.
- **Its key is derived, never the secret itself.** The key is HMAC-SHA256 over
  the secret with the label `aiwatcher pod attempt credential v1`. A value sealed
  as a session never opens as a pod credential, and a pod credential never opens
  as a session, even when an operator gave both the same secret.

**It opens two doors, and the authentication layer shuts the rest.** An attempt
credential is accepted in exactly two places:

- **Its own attempt's worker routes.** A claim must name that attempt in
  `ClaimRequest.attempt`. `held` and `recorded_result` compare the path's key with
  the credential's before anything else. The queue scope stays as well, so two
  checks still have to agree.
- **`POST /api/v1/events`, as a producer.**

Every other path refuses it in `auth::authenticate`, through an `admits_attempt`
beside `is_public`, before any handler runs. It has to be the layer, because most
read routes check no role: a credential the layer let through would read every
run whatever its role said.

An allowlist of two doors fails closed. A route added later refuses an attempt
credential without anybody remembering to make it. This is not the path table
the guardrail warns against. That table decides *which role* a route needs, and
it drifts towards permitting. This one decides whether this credential is a
caller of this API at all, and a missing entry refuses.

**Every mode that authenticates accepts it.** In `oidc` and `proxy` it sits
beside the ingest tokens. In `local` it sits beside the local token, which is
otherwise the only credential that mode takes, so a process pod on a
single-user install stops needing `admin`.

**The key is `AIWATCHER_POD_CREDENTIAL_SECRET`.** The role that launches
(`work`, or the one combined process) mints with it. The role that serves the
worker routes (`serve`, or the same process) checks with it.

- **Unset in one process:** the process signs with a key generated at start-up
  and warns. After a restart every running pod's reports are refused, and each of
  those attempts ends at its lease, which the retry budget decides.
- **Unset in a split deployment:** when auth is on and templates are configured,
  both roles refuse to start and name the variable. This is §43.11's rule for the
  backends a split binary has to share, applied to one more thing it has to share.

**Where the credential is held depends on the backend, and the manifest does
not.** The launcher always writes the credential into the manifest as a plain
`env` value. Each backend then decides where it lives:

| backend | held in | readable by |
|---|---|---|
| `kubernetes` | a Secret the Job owns, read by `secretKeyRef` | whoever may read Secrets in that namespace |
| `docker` | the container's environment (`--env`) | whoever holds the engine, which is root on that host |
| `process` | the child's environment | the server's own user |

**On `kubernetes` the credential is a Secret, because Kubernetes' `view` role reads
Jobs and pods and never Secrets.** In the pod spec, anybody who may look at the
namespace could read a live attempt's credential and settle it with a result they
made up.

- **Moving it is the cluster backend's job.** `KubeCluster` rewrites the value
  into a Secret named from the Job and owned by it, so it is collected with the
  Job. The launcher still sends one manifest, and the other two backends read it
  unchanged.
- **The Job is created first and its Secret second.** The pod waits a few seconds
  for the Secret, in `CreateContainerConfigError`. A crash between the two leaves
  a Job with no Secret, which its start allowance ends. It never leaves a Secret
  with no owner.
- **A Job that already exists is given its Secret anyway.** When a pass meets a
  Job that already exists, it still makes sure the Secret exists. If a second
  launcher got there first, that launcher's credential stands, and it is just as
  valid.
- **The launcher's Role gains one grant: `secrets: create`.** It gains nothing
  that reads a Secret, and the gate's `can-i` keeps asserting that it cannot.

**The deadline holds on every backend.** Once a live pod's Job is past its
deadline — its creation, plus the start allowance, plus the step's timeout — the
watch stops it. The pod's attempt, if still unfinished, is ended as
`Infrastructure` with the reason `DeadlineExceeded`, which is the cluster's own
word for the same thing. A cluster already does this itself, and the watch
repeating it costs one delete. The other two backends never did, and without it
an expired credential would only make a runaway step's calls fail while the step
kept its memory. The timeout rides on the manifest as an annotation, so every
backend's listing returns it.

## Alternatives considered

**Keep the template token, and let the two host backends resolve `secretKeyRef`
from a directory** laid out as a mounted Secret is
(`<dir>/<secret>/<key>`). One mechanism stays across all three backends. It
still leaves a long-lived `Editor` token shared by every pod of a template, able
to read every read route. It does nothing for `local`, whose only token is
`admin`. What it is right for is an *application's* secrets outside a cluster,
which this does not decide. See the costs below.

**Let `process` inherit `AIWATCHER_TOKEN` from the server.** A step would run
holding the server's own credential, which under `local` is `admin`. Withholding
`AIWATCHER_*` exists to prevent exactly that.

**Derive the key from `AIWATCHER_AUTH_SESSION_SECRET`.** No new variable. But the
chart renders that secret only under `oidc`, and there it is optional. Rotating
it, which signs everybody out, would also refuse every running pod's reports.
That puts two unrelated clocks on one secret.

**A client-credentials token from the identity provider, per template.** These
are real tokens with real revocation. They need a provider in every mode and a
service account per template, and neither `local`, `proxy` nor a laptop has one.
They also scope to a client, not to an attempt.

**Scope the credential by role checks in the handlers**, giving it `Viewer` and
refusing it in `Caller::require`. Most read routes call no `require`, so it would
read everything readable. That is default-allow wearing a scope.

**Leave the credential in the Job's environment on `kubernetes` too.** One path
for all three backends, no Secret, no new grant. It loses to the `view` role:
a namespace viewer is not somebody who may settle an attempt.

**Accept only events that name the attempt's execution.** A lifted credential
could then not write unrelated events. Every producer's ingest would pay for a
validation, and a false event decides nothing, because an attempt is settled only
through the worker routes. It is kept as a trigger below.

## Consequences

**What it costs:**

- **New things to keep:** a credential variant in `aiwatcher-auth`, a secret both
  roles of a split deployment must share, a Secret per Job and one more grant in
  the launcher's Role.
- **Step code loses aiwatcher's token for everything else.** It is good only for
  the step's own attempt and for events. A stage that reads a prompt by name, or
  an annotation export, needs a credential of its own from its template. Outside
  a cluster that still means a literal value in the templates file, because an
  application's secrets are not what this decides.
- **A one-process deployment with no secret set loses its running pods on a
  restart.** Their attempts end at their leases, and the budget decides.
- **A lifted credential can still publish events until it expires.** Those events
  go out under a subject naming the attempt, so they say whose they are.
- **Clock skew between the minting role and the checking role moves the
  expiry.** One lease of grace absorbs seconds, not minutes.
- **`Identity` gains a field, and it is in the contract.** `just openapi` has to
  run in the same commit, and it cannot while another session's evaluation work
  holds the generated files.
- **The chart's template example changes.** A template drops its token, and
  `AIWATCHER_TOKEN` in one is now a refusal. Nothing deployed carries it yet.

**What would make this wrong.**

- **Aiwatcher reads show up in most templates**, such as a prompt read by name or
  an export. Then two doors are too narrow, and the next field is a *read* scope:
  named routes, still refused by default. The template token does not come back.
- **Events forged with a lifted credential.** Then ingest narrows to events naming
  the attempt's execution.
- **Steps running more than a lease past their timeout become ordinary**, because
  nobody sets a timeout honestly. Then the expiry has to be renewed by the
  heartbeat, with a heartbeat answering a fresh credential. A longer fixed grace
  is not the answer.
- **The seconds a pod waits for its Secret turn out to matter**, say behind a
  slow admission webhook. Then the Secret is created first, and the Job adopts it
  afterwards.
