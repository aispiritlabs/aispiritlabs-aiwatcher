# ADR_0029: A step that needs a pod names an operator's template, and the pod is a worker for one attempt

- **Status**: accepted; reopens Phase 12, which ADR_0016's supersession left unbuilt.
  One amendment below, from building the watch
- **Date**: 2026-09-11

The change is AW-4's Part 2
([`docs/specs/AW-4-…`](../specs/AW-4-retire-flyte-and-run-steps-in-pods-of-our-own/02-spec.md)):
its spec is the behaviour, its job note the build. This is the part that would be
expensive to reverse — which templates exist, who may name an image, what a pod
holds, and how a pod's end becomes a fact about its attempt.

## Context

Flyte gave planner one thing it misses: **one pod per stage**. planner removed Flyte
on 2026-09-09 and AW-4 removed aiwatcher's engine on 2026-09-11, so planner's four
house-import stages run as four `PythonTask` attempts inside one import-worker pod
(§39.4). What that pod cannot give is isolation: one stage running out of memory
kills the process holding all four.

What already exists is most of a pod's worker. The claim, the lease held by
heartbeat, the result route and the proxied inputs and outputs are the protocol
every worker speaks (§36); a claim can name one attempt (`ClaimRequest.attempt`),
and `aiwatcher_sdk.worker` has a `run-attempt --ref` mode that claims exactly that
attempt and exits. What does not exist is any Kubernetes client, any template, or
any per-step image or resources — `RuntimeBinding` has seven variants and none of
them starts anything.

Three facts shape the rest:

- **A definition is saved by anyone who holds `editor`.** A pod spec inside one
  would carry a `hostPath`, `privileged: true` or a secret's name from anybody who
  may save a workflow.
- **Whatever runs in a pod can read what the pod holds** — its environment, its
  mounted secrets, its token. The boundary that matters is which code meets which
  secrets.
- **A queued attempt is claimed by queue and task alone.** `ClaimFilter::matches`
  does not look at a queued row's runtime, so a pod's attempt on a queue a
  long-lived worker also serves would be taken by that worker and run outside any
  pod.

The owner settled four things on 2026-09-11: a pod is **opt-in per step**;
templates are **chart values**, planner supplying its own under a documented key;
each template has **its own image allowlist**; and **GPU is later**.

## Decision

**A step opts in with `pod`, and compiles to a binding of its own.** A workflow step
gains one optional field, `pod: {template, image, cpu?, memory?}`. It is nested
rather than flattened, because serde's `flatten` and `deny_unknown_fields` do not
compose, and it denies unknown fields, so a `namespace`, a `node`, a
`serviceAccount`, a `secret` or a `gpu` is refused by name rather than stored and
ignored. A step with `pod` compiles to `RuntimeBinding::ContainerJob` —
`PythonTaskSpec`'s `task_ref`, `queue` and `params` plus the pod request — under
`RuntimeKind::ContainerJob` (`container_job`). It is a kind of its own for ADR_0028's
reason: a claim filter tells it apart without loading the plan. A step without
`pod` is unchanged byte for byte. The field is absent from its serialisation, so no
stored revision changes its digest and no plan its `plan_id`. The template and the
image are executable fields: a different image is different code. A gate
(`approval`) refuses `pod` by name, because it runs nothing.

**Templates are the operator's, and each template owns its image list.** They live in
chart values under `execution.pods.templates.<name>`, and each has four parts:
- `images`: the repositories it runs;
- `resources`: default requests and limits, plus a ceiling for CPU and one for
  memory;
- `command`: what its container runs;
- `pod`: a pod-spec fragment the operator writes in full — volumes, env from
  secrets, a service account, node selectors, tolerations and security context.

They are rendered into one file that both roles read (`AIWATCHER_POD_TEMPLATES`),
and no route writes one. The list belongs to the template, not to the deployment,
because the template is where secrets and volumes are attached. That pairing is the
unit of trust: which code may reach this template's secrets is answered by this
template's list. A global list would let an image allowed for a scratch template run
under the template that mounts planner's database password.

- **An image matches a repository exactly.** The reference is parsed into its
  registry and repository, and they are compared with an entry; the tag or digest
  is free. There is no prefix match: `ghcr.io/planner/import` does not admit
  `ghcr.io/planner/import-debug`, which would be a permission invented by spelling —
  the corpus-name rule, applied to images. A pinned `task_ref` is what guarantees
  the code: a pod whose image lacks `name@version` cannot claim.
- **aiwatcher owns these fields and fills them in:** the container's image, its
  resources and the attempt's environment, `restartPolicy: Never`, and the Job's
  own fields. A template that sets any of them is refused at start, naming the
  template and the field. So is one whose pod has anything but exactly one
  container. Init containers and volumes are the operator's.
- **One namespace**, `AIWATCHER_POD_NAMESPACE`, defaulting to the release's. It
  never comes from a step or a template: the grant is a namespaced Role, and one
  namespace is the whole of it.

**Registration refuses, and a launch refuses again.** The serve role registers
definitions and reads the templates file, and it holds no cluster credential because
checking a name against a list needs none. A step is refused, with a 422 naming the
step, the value and the template, when:
- its template is unknown;
- its image is outside the template's list;
- its CPU or memory is not a Kubernetes quantity, or is above the ceiling.

Every problem is reported at once and nothing is stored. A step's `cpu` or `memory`
sets both the request and the limit for that resource; whatever it leaves out comes
from the template's defaults. The list is configuration and can change after
registration, so the launch checks again. An attempt whose template or image has
stopped being allowed fails as `UserCode`, naming both, and is not retried, because
every retry would get the same answer.

**The work role launches pods; the pod is the worker.** The launcher is a loop in the
role that holds the reactors: the work role when split, the one process otherwise.
It sits behind `kube`, in `aiwatcher-server`'s `execution/pods`. It reads claimable
`container_job` rows, without claiming them, and creates one `batch/v1` Job per
attempt with:
- `backoffLimit: 0` and `restartPolicy: Never`;
- `activeDeadlineSeconds` set to the template's start allowance plus the step's
  timeout;
- `ttlSecondsAfterFinished` as a backstop;
- the attempt reference, the API's address and the pod's own name as the worker
  name, all in the environment.

The template's `command` runs the application's own worker in `run-attempt` mode.
That worker claims **that** attempt by key, heartbeats, performs it and reports it
through the routes every worker uses. `Reactor::take` and `Reactor::settle` do not
change, and nothing about a pod's step is decided in the pod or in the launcher.

- **Exactly one Job per attempt, with no lease.** The Job's name is derived from the
  attempt key, so a second launcher's create comes back `AlreadyExists`.
  Kubernetes' name uniqueness serves as the compare-and-set — `TraceId::derive`'s
  rule, one layer further out. A retry is a new attempt number, so it gets a new
  name and a new Job.
- **A pod's attempt is claimable only by its key.** `ClaimFilter::matches` gains one
  rule: a `container_job` row matches a claim naming its key, and nothing else. A
  worker claiming without a key is unchanged, and so is every row that is not a
  pod's.
- **A process that cannot start pods launches none.** A build without `kube`, or a
  process without templates, holds no launcher, and a `container_job` attempt waits
  and is shown as pending on that binding. Templates configured in a build that
  lacks `kube` are refused at start in the work and combined roles, naming
  `AIWATCHER_POD_TEMPLATES`. The serve role reads templates in any build.

**A pod holds its template's worker token.** The template names a Secret holding an
ingest token scoped to a queue (`name[queue]=secret`), which is today's credential
with nothing new. The recommended configuration gives each template a queue of its
own and a token for that queue only, and the chart's example does it. The design
leaves a stricter mode for later: a credential minted per Job, signed over the
attempt key and a deadline and held in a Secret the Job owns, so that a leaked one
settles one attempt.

**The lease decides; the Job explains.** Correctness needs nothing from the launcher
once it has created the Job. A pod that vanished, was killed or never came back is a
lease that lapses, and the store's rule ends that attempt as it ends any worker's.
So a work role that is down loses no attempt — only the explanation. The launcher
watches the Jobs it created and adds three things:
- **A pod that ended while it still held its attempt is ended at once**, as
  `Infrastructure` and with the pod's own reason (`OOMKilled`, `DeadlineExceeded`,
  an exit code), rather than a lease later. This is delivered as the engine's own
  input through `ExecutionHandler::deliver`, never through the caller's door, and
  only while the lease is still that pod's.
- **A Job whose pod never claimed within the start allowance is deleted**, and its
  attempt fails as `Infrastructure`. That covers a refused image pull, an
  unschedulable pod and a crash before the claim: nothing ran, so the budget
  decides.
- **The log.**

**A pod's log is its last 256 KiB, recorded against its attempt.** When the Job ends,
the launcher reads the log stream through a ring buffer and stores it in the object
store. It records the log in the `ArtifactCatalog` as `ArtifactKind::Log`, with
`Provenance` naming the attempt; a header says how many bytes came before. The bound
is in bytes, not lines, because one line can be megabytes of JSON, and a bound in
lines bounds nothing. The log is never in the stream and never an output: the
attempt stays immutable once it is terminal, a settlement never waits for its log,
and a pod that reported first and printed last is still read in full. The launcher
deletes the Job once the log is stored. `ttlSecondsAfterFinished` (one hour) covers
the case where no launcher is left to do it.

**A cancel deletes the Job.** While a run is cancelling, the launcher deletes the
Jobs of its running `container_job` attempts. It needs no lookup, because the names
are derived. The attempt's state comes from the decider's cancel, not from the
deletion, and a report arriving from a dying pod is a lost lease.

**`kube` is a feature of `aiwatcher-server`**, off by default, as `laser` is:
`kube-rs` and `k8s-openapi` stay out of every build that does not ask for them. The
release image adds `aiwatcher-server/kube` to the features it already builds with,
so a chart turns pods on without a rebuild. The chart gains three things:
- a ServiceAccount and a namespaced Role — Jobs create, get, list, watch and delete;
  pods get, list and watch; `pods/log` get — bound to the pod that runs the
  launcher, which is the only pod with `automountServiceAccountToken: true`;
- a NetworkPolicy rule admitting launched pods to the API;
- the templates file.

The serve role holds no cluster credential (§31). A launched pod runs under its
template's service account, with no token mounted unless the template says so.
What passes between steps is still an `ArtifactRef` through the outputs route: a
volume a template mounts is the application's business and not an artifact.

## Alternatives considered

**A pod spec in the definition.** Anyone who may save a workflow could write a
`hostPath`, a privileged container or a secret's name. §37 refused it for that reason,
and nothing here has changed the reason.

**One allowlist for the whole deployment** (§37's `AIWATCHER_KUBE_IMAGE_ALLOWLIST`).
It is simpler, and it separates the images from the secrets they meet. The owner
chose per template.

**Matching by registry prefix**, as §37 wrote it. It admits every repository whose
name begins with an allowed one. A `/*` wildcard at a path boundary can be added
later without touching a stored definition; narrowing a rule already in use would
strand the definitions written under it.

**The launcher claims the attempt and hands its lease to the pod.** It gives
exactly-once through the store rather than through Job names, and it costs a lease
handover — a new state in four adapters — plus a pod reporting into an attempt
somebody else holds. The derived name gives exactly-once with neither.

**The launcher as an `ActivityExecutor`** that creates the Job, waits for it and
returns the result. A pod's inputs, outputs and report cross the worker routes
anyway, so the executor would be waiting on a report that goes to somebody else, and
two parties would settle one attempt — the case *never let two parties publish one
attempt's `step.*`* exists to prevent.

**A one-attempt credential now.** It is tighter: code in a pod could touch its own
attempt and nothing else. It costs a new credential shape in `aiwatcher-auth`, a
signing key both roles share, and a Secret per Job. Following the permissive
default, it waits as the stricter mode, and *What would make this wrong* names the
trigger.

**Only the lease, or only the watch.** The lease alone is slower by one lease and
reports "crashed" where Kubernetes knows "OOMKilled", and it keeps no log. The watch
alone loses attempts whenever the work role is down.

**An `aiwatcher-kube` crate** (§27). Section 43.1 found the argument for a separate
crate untrue for the store adapters, and `laser` is the precedent: an adapter is a
feature of the crate that uses it. Executors live in the server.

**Kueue, Argo or Flyte beneath it.** Each is a second scheduler beside an engine
that already schedules. Queueing for GPU work, when it arrives, is a Kueue label on
a template, not a second engine.

**Keeping the log's last N lines.** See above: a bound in lines bounds nothing.

## Consequences

**What it costs:**

- **The work role holds a cluster credential.** It is a namespaced Role, but a
  compromised work role can create Jobs in that namespace with any image, because
  **aiwatcher enforces the allowlist, not the cluster**. An admission policy in the
  cluster (a `ValidatingAdmissionPolicy`, Kyverno, Gatekeeper) is the second wall.
  It is not shipped, and an operator who wants it writes it from the same values.
- **A template is only as safe as the images on its list.** Any code in an allowed
  image runs with that template's secrets and volumes. The list is a statement of
  trust, not a scan.
- **Every pod of a template shares one token.** Code in one pod can claim another
  pod's attempt on the same queue by key and lie about its outcome. That is the
  trust any worker has had since §36.2, and no more: rows a pod cannot name are rows
  it cannot take, and long-lived workers' rows sit on other queues.
- **A pod per attempt pays a cold start.** Scheduling and an image pull happen once
  per attempt, so four stages cost four. That suits minutes-long stages like
  planner's and does not suit sub-second ones.
- **The launcher needs a read the store does not have:** the claimable rows of one
  runtime, by key. That is one more method in four adapters. It is bounded by the
  claim table, which holds live attempts only.
- **A log can be lost.** One whose Job outlived its TTL while no launcher was
  running is gone, and the kubelet's own rotation (10 MiB by default) bounds what
  there is to read in the first place.
- **The release image grows by a Kubernetes client, and the chart grows RBAC.**
- **Pods need a cluster.** Under `just run` there are no templates, so a step naming
  one is refused at registration. Development uses Tilt on a local Kubernetes
  (ADR_0006), and the Job manifests are tested as pure functions without one.

**What would make this wrong.**

- An allowed image run by people who should not have its template's secrets. Then a
  repository list is too coarse: the step's image has to be pinned by digest and
  its signature verified, not listed.
- A leaked pod token used to settle another pod's attempt. Then the one-attempt
  credential stops being optional.
- Cold starts dominating a run, such as seconds of work behind a minute of pulls.
  Then a warm pool of long-lived workers per template beats a pod per attempt, and
  the template becomes a queue rather than a Job.
- Jobs held pending by something the launcher cannot see — a quota, Kueue, an
  autoscaler that takes minutes. Then "never claimed within the start allowance"
  is the wrong failure, and the launcher should report *waiting for the cluster*
  rather than end the attempt.
- A step that needs a GPU, a second container or fan-out. The first is the next
  field — `gpu` is refused as unknown until then, and adding it is additive — and
  the other two are outside this decision.

## Amendment, 2026-09-12: what the watch turned out to be

Four things in the decision above are not what the watch was built as (AW-4's
2.4), and each is worth correcting where somebody will read it.

**A pod's ending goes through `handle`, not `deliver`.** The decision says the
ending is "delivered as the engine's own input through `ExecutionHandler::deliver`,
never through the caller's door". The door it names guards *effect commands* —
`ExecuteStep`, `RequestInput`, `TimeoutInput` — and what the launcher sends is a
`StepFailed`, which is a report: the same message a reactor and a worker send
through `handle`, under an id derived from the attempt so a pass that failed
after the append lands on the inbox entry rather than beside it. `deliver` is
crate-private and has one caller, the timer table, and widening it for this
would have made a launcher's report reachable from nowhere it needed to be.

**A cancel's attempt is ended by the launcher and the run's outcome is still
the decider's.** The decision says "the attempt's state comes from the
decider's cancel, not from the deletion" — but a cancel is cooperative and
*waits* for what is running, so something has to say the pod has stopped. The
launcher says it, as `FailureClass::Policy` — the class whose own words are
"cancelled", and the one thing this depends on is that nothing retries it,
because a retry would be a second pod for a run that is stopping. What the run
then ends as is `decide`'s: a failure while cancelling is an
`ExecutionCancelled`, so the run reaches `cancelled` in seconds rather than
waiting out a lease. Deleting the Job and reporting nothing would have left the
row claimable, and the next pass would have started another pod.

**A run that has already ended stops its pods too.** A step that failed for the
last time skips what is downstream of it and ends the run while a sibling's pod
is still working. That pod's work is nobody's, and its row would sit in the
claim table for ever being read by every pass — so the same branch covers both,
and "is this run still taking work" is one keyed projection read per execution
per pass, memoised, because a fan-out is many pods of one run. It reads
`RunState::is_cancelling`, which exists so that a reader asking for the fact
does not match the string a badge is drawn from.

**The attempt rides as three annotations.** `aiwatcher.dev/execution`,
`aiwatcher.dev/step` and `aiwatcher.dev/attempt`, because the watch reads it
*back* off the cluster: an execution id comes from a request and a step id from
a canvas, and neither is checked against a path grammar anywhere, so
`<execution>/<step>/<attempt>` is a string to write and not one to parse.
