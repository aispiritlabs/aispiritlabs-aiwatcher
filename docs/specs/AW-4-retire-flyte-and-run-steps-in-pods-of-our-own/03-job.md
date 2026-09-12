---
id: AW-4
step: job
status: doing
branch: main
repo: aiwatcher
created: 2026-09-11
updated: 2026-09-11
tags: [spec/AW-4, step/job, branch/main, status/doing]
---

`#spec/AW-4` · `#step/job` · `#branch/main` · repo `aiwatcher`

# ③ Job — AW-4

## Technical approach

Part 1 is three passes, cut where another session's uncommitted work sits, so
that nothing here waits on anybody and nothing of theirs is swept into a commit.

- **1a, now.** Everything no other session holds: the server stops building an
  engine and refuses to be asked for one; the crate goes; the chart, the
  justfile, the environment example, the SDK integration and the documents
  that name the crate or its recipes follow. The API keeps its engine routes for
  this pass, and with no engine wired they answer 501 — the panel's launcher,
  which the rebuild holds, keeps compiling against a contract that has not
  moved.
- **1b, after the panel rebuild drops the launcher.** The five routes, their
  error variants and `AppState.engine`, `core::engine`, the contract and the
  generated client in one commit, and the launcher's two call sites.
- **1c, after AW-3 commits `plan.rs` and its neighbours.** `ExecutionOwner::Engine`
  becomes an unknown owner; `RuntimeBinding::ExternalWorkflow`, its spec and its
  kind go; the contract is regenerated again. Then the documents that describe
  the engine as a design rather than as a crate: `CLAUDE.md`'s decisions and
  guardrails, `INSTALL.md`, §24, §37–§39, the ADR cross-references.

Part 2 starts with its ADR and is planned in full once Part 1 is through; the
outline below is what the spec already fixes.

## Decisions

- **A refusal names the removal, in a variant of its own.** `ConfigError::Removed`
  says what was set, what it switched on, that it was removed, and what to do
  instead. The plan was to reuse `Invalid`, whose message reads *"…which is not
  a valid {expected}"* — so the refusal came out as "not a valid none — the
  engine was removed", which is a sentence nobody should have to parse at start
  time. `none`, `off` and `disabled` keep parsing, so a deployment that spelled
  the default out is not refused.
- **The `AIWATCHER_FLYTE_*` variables are ignored, not refused.** Only the switch
  that turned the engine on can mislead anybody; the rest configured nothing
  unless it was set.
- **The chart refuses `engine` with `fail`, not with the schema.** The schema
  has no `additionalProperties: false` at its root, and adding one would turn
  every stray key in every installation into a refusal. One guard, in the
  template that already carries the release's other guards.
- **`engine_end_to_end.rs` goes with the crate.** It drives the API through a
  real `FlyteEngine` against a stand-in admin; the API's own `RecordingEngine`
  tests stay until 1b removes the routes they cover.
- **An unknown owner is `Unknown(String)`** (1c): kept for display, matched by
  nothing that schedules or decides. No stored row holds anything but `local`
  or `worker`, so this changes no read of existing data. It keeps the text
  verbatim, so `engine:flyte` still reads `engine:flyte`. Nothing needed to
  change beyond the type: no scheduling or deciding code ever read the owner.
- **A runtime kind the postgres adapter cannot name is refused** (1c, decided
  while building). It used to fall back to `external_workflow`, "the binding
  this process never runs". With that binding gone there is no honest
  fallback. The kind is now decoded through `RuntimeKind`'s own serde spelling,
  and an unknown one is `StoreError::Encoding`, which is what the `memory`,
  `file` and `duckdb` adapters already do when they decode. A claim never
  meets one, because its SQL selects only the claimant's kinds. What does meet
  one is a lookup by key.
- **The Experiments page keeps only its Comparison placeholder** (1b). The
  stage buttons and the period control fed nothing but the launcher, and a
  control that changes nothing is a button that does not work. The `stage`,
  `engine` and `engineFind` search parameters go with them. Nothing linked in
  with them.
- **Part 2's design is [ADR_0029](../../ADR/ADR_0029_POD_PER_STEP.md)** (2.1),
  and it answers the spec's three open questions:
  - **The pod claims its attempt by key**, under its template's queue-scoped
    token. A key-only rule in `ClaimFilter` keeps long-lived workers off a pod's
    row, and a one-attempt credential is deferred as the stricter mode.
  - **The lease decides how an attempt ended**, and the Job's watch explains it
    sooner, with the pod's own reason.
  - **The log is the last 256 KiB**, in bytes rather than lines, recorded in
    the catalog against the attempt.

  Two points move from §37: an image matches a repository exactly rather than
  by prefix, and the launcher creates Jobs without claiming anything — one per
  attempt, under a name derived from its key.
- **The templates file is a JSON map from name to template** (2.2), and each
  template has these fields:
  - `images`;
  - `resources`, holding `requests`, `limits` and a required `max`;
  - `command`;
  - `start_allowance_seconds`, 300 by default;
  - `pod`.

  Unknown fields are refused, every problem is reported at once, and a malformed
  file fails the start. A template's name is a DNS label, because the Jobs it
  starts will carry it.
- **What aiwatcher fills in is refused in a template, by name** (2.2):
  - on the pod: `restartPolicy`, `activeDeadlineSeconds`, and the Job's
    `backoffLimit` and `ttlSecondsAfterFinished`;
  - on the container: `image`, `resources`, `command`, `args`, and the three
    variables aiwatcher sets — `AIWATCHER_ATTEMPT`, `AIWATCHER_URL` and
    `AIWATCHER_WORKER_NAME`.

  `command` and `args` go further than the ADR's list: the template has a
  `command` of its own, and two places for one argv are two answers.
  `pod.containers` is absent or holds exactly one container.
- **Docker's defaults are written out before an image is compared** (2.2):
  `python:3.13` is `docker.io/library/python`, so an entry and a step that spell
  one image two ways agree. The match is still exact, by repository.
- **Until 2.3, a `container_job` row carries no queue** (2.2), so nothing claims
  it and it waits. Writing its queue before the key-only rule exists would let
  any worker holding that queue take a pod's attempt and run it outside the
  pod, which is the hole the ADR names.
- **The work and combined roles refuse templates in every build until `kube`
  exists** (2.2, `ConfigError::Unusable`). 2.3 makes the refusal depend on the
  feature. The serve role reads templates in any build.
- **A pod's step has a cache key only with an image pinned by digest** (2.2). A
  tag names whatever was pushed under it last. The workflow compiler sets
  `never` anyway, so this is only `cache_key`'s own answer.
- **The SDK's `WorkflowStep` takes `pod=PodRequest(…)`** (2.2), and sends it only
  when it is set, so a step without one registers the same revision.
- **The key-only rule is one list, `RuntimeKind::CLAIMED_BY_KEY`** (2.3).
  `ClaimFilter::matches` reads it, and the PostgreSQL claim binds it
  (`runtime <> all($9)` unless a key was named). A pod's row now carries its
  queue and task, so a claim naming the key still needs the token's queue and
  the code.
- **The launcher's read is `WorkflowStore::claimable_attempts`** (2.3): one
  runtime's rows that may be taken now, in four adapters and one contract
  property. A retry inside its delay and a row a pod holds are both left out.
  One pass reads 1000 rows, because an attempt whose pod was asked for stays
  claimable until the pod claims it, and a small bound would let those starve
  the rows behind them.
- **A Job is named `aiwatcher-` plus 32 hex characters of `sha256(key)`** (2.3),
  because the name is also a label, at most 63 characters. The attempt rides as
  the annotation `aiwatcher.dev/attempt`. The labels
  `app.kubernetes.io/managed-by: aiwatcher` and
  `app.kubernetes.io/component: step` are what the NetworkPolicy admits the pods
  by.
- **The loop and the manifest are in every build** (2.3). The manifest is JSON,
  and kube-rs reads it back into a typed Job before sending it. Only
  `execution/pods/kubernetes.rs` needs `kube`, and the loop is tested against a
  stand-in cluster.
- **A launch that cannot happen is reported the way a reactor reports** (2.3): a
  `StepFailed` through `handle`, under an id derived from the attempt.
  - A template or an image no longer allowed fails as `UserCode`.
  - A manifest the cluster refuses (400 or 422) fails as `Validation`, naming
    the template.
  - Anything else — a missing grant, a quota, a 5xx, a connection — leaves the
    attempt waiting for the next pass.
- **`AIWATCHER_POD_API_URL` is required wherever pods are launched** (2.3). The
  chart sets it to the server Service's fully qualified name.
  `AIWATCHER_POD_NAMESPACE` defaults to the client's own namespace. The
  `Unusable` refusal now depends on `cfg!(feature = "kube")`.
- **The SDK worker reads its attempt and its name from the environment** (2.3):
  `AIWATCHER_ATTEMPT` when `--ref` is absent, and `AIWATCHER_WORKER_NAME`. A
  template's command is the same for every attempt.
- **Found while building: the result route acknowledged only a `PythonTask`'s
  report** (2.3). `recorded_result` read the queue with a `let-else` on that
  one binding, so a pod's committed report came back as a 409. It now reads the
  queue from either binding.
- **Left to 2.4: a pod that dies after claiming** (2.3). Its row is claimable
  again, the launcher is told the Job already exists, and the attempt waits.
  2.4's watch ends it with the pod's reason; until then only the Job's
  one-hour TTL and a restart of the launcher start a second pod, as a retake.
- **The watch is a second half of the same pass, and the cluster is its
  record** (2.4). A launcher that restarted still has to end the attempt of a
  pod that died while it was away, so what exists is read back from the
  cluster — one Jobs listing and one pods listing per pass, never a read per
  Job — rather than remembered here. A listing that could not be read leaves
  the watch for the next pass and does not stop the launch: a create is
  idempotent by name, and an attempt with no pod is the one thing waiting on
  the loop.
- **The attempt rides as three annotations rather than one key** (2.4).
  `aiwatcher.dev/execution`, `aiwatcher.dev/step` and `aiwatcher.dev/attempt`,
  because the watch reads it *back*: an execution id comes from a request and a
  step id from a canvas, and neither is checked against a path grammar
  anywhere, so `<execution>/<step>/<attempt>` is a string to write and not one
  to parse. `manifest::attempt_of` is the inverse, beside the writer.
- **The Job says whether a pod ended; the pod says why** (2.4). A pod may be
  deleted after its Job finished, and then the Job is the only thing left that
  knows it did — so `status.succeeded`/`failed` decides `Ended`, and the pod's
  own word (`OOMKilled`, `Evicted`, `DeadlineExceeded`, a terminated reason
  with its exit code) fills in the reason. A dead pod's attempt fails as
  `Infrastructure`, which is the class whose words are "a killed pod" and whose
  budget is the tighter one.
- **A start allowance is read off the Job's template label** (2.4), not off the
  plan: the plan would be a stream read per Job. A template no longer
  configured falls back to `DEFAULT_START_ALLOWANCE_SECONDS`, because this is
  the clock that decides a pod never got going and an absent one would decide
  "never". The Job's own `activeDeadlineSeconds` cannot answer it — that is the
  allowance *plus* the step's timeout.
- **A cancel is `Policy`, and the launcher asks the run rather than the row**
  (2.4). `StateType` has no `Cancelling`: a cancel is `Running` with a reason,
  so the reason became `RunState::cancelling()` and `is_cancelling()` — the
  launcher reads a fact instead of matching the string a badge is drawn from,
  and nothing had to be migrated into the projection. One keyed projection read
  per execution per pass, memoised, because a fan-out is many pods of one run.
  The class is `Policy` — whose own words are "cancelled" — and the one thing
  the loop depends on is that nothing retries it: a retry would be a second pod
  for a run that is stopping. What the *run* ends as stays the decider's, since
  a failure while cancelling is an `ExecutionCancelled`.
- **A run that already ended stops its pods too** (2.4), by the same branch. A
  step that failed for the last time skips what is downstream of it and ends
  the run while a sibling's pod is still working, and that pod's work is
  nobody's. Left alone its row would also sit in the claim table for ever,
  read by every pass.
- **A parked attempt's pod ending is not an ending** (2.4). A question keeps its
  row and the answer dispatches attempt *n+1*, so the watch ends an attempt
  only when its row is neither terminal nor `awaiting_input`.
- **The log is bytes, kept after the attempt, and the Job goes once it is
  stored** (2.4). 256 KiB of the *end*, because a traceback is printed last and
  one line of a structured logger can be megabytes; the stored object's first
  line says how many bytes came before. `Tail` is the ring buffer and compiles
  in every build, because the cluster can only bound the *first* bytes of a log
  stream. The attempt ends first and is read after — a settlement never waits
  for its log — and a log that could not be read or stored leaves the Job for
  the next pass, where the same bytes store once because an artifact is named
  by its own hash. `ttlSecondsAfterFinished` is the backstop, and a deployment
  with no object store keeps no log and deletes the Job anyway.
- **Found while building: a cancel left a dispatched attempt in the claim
  table** (2.4). `StepSkipped` names no attempt, so the handler retired
  attempt `0` — a key that had never existed. The row stayed claimable for
  ever: a reactor took a cancelled run's pending attempt and did the work, and
  a pod's row was read by every launcher pass. The number now comes from the
  run (`dispatched_attempt`), which is where `dispatched` was already read
  from, and `StepSkipped` leaves `settled()`.
- **The reader half of the log is a route per noun, not per kind** (2.4).
  `GET /executions/{id}/artifacts` answers from `ArtifactCatalog::produced_by`
  — every artifact a run produced, each carrying the execution, the step and
  the attempt that made it — and `GET /executions/{id}/artifacts/{digest}`
  reads one of them as text. A route that listed *logs* would need a second
  one the first time a step's view wanted its rows; the catalog does not sort
  by kind and neither does this.
- **The digest is the whole address, and the execution is what makes that
  safe** (2.4). The content route checks the digest against what *this run*
  produced before it reads anything, so it is not an oracle over a store that
  also holds prompts, datasets, annotations, conversations and training: a
  digest no row of this run names is a 404 whatever is under it. The bytes go
  through `AttemptArtifacts::read_bytes` — the proxying port the worker's rows
  already use — rather than a presigned URL, for the reason ADR_0025's
  guardrail gives, with the panel in the place of the laptop.
- **Text, never the stored content type** (2.4). Serving bytes back under a
  type the object store was told about is how a stored `text/html` becomes a
  page on this origin. It is decoded lossily, because a pod's stdout is not
  promised to be UTF-8 and a traceback is worth reading with a byte mangled in
  it, and refused over 1 MiB — four times the only thing it is expected to be
  asked for, checked against the declared size before the read and against the
  object after it.
- **A 501 rather than an empty list** (2.4), naming `AIWATCHER_PROMPT_STORE`.
  The catalog and the store are `Some` exactly together, so their absence is
  one fact with one fix — `StepArtifactsDisabled`, separate from the worker's
  because the reader is a person on a step's view and a code naming a worker
  would send them looking at one. The panel keeps the other half of R6: a 501
  is rendered as the sentence the server sent and never as "this step printed
  nothing", which is what the two would look like drawn the same way.
- **One `StepLog`, in both places a managed run is watched** (2.4) — the
  curation pipeline's run card and the Workflows view, for `AnswerGate`'s
  reason. It lists **every** attempt's log, newest first, because the retry
  that succeeded is the uninteresting one; it fetches the list with the step's
  context and the bytes only on a click; and it is not conditional on the
  binding, because whether an attempt left a log is the catalog's answer and a
  list of runtimes in the panel would be a second opinion about it.
- **`aiwatcher_execution::Provenance` is `ArtifactProvenance` in the contract**
  (2.4). An OpenAPI components block is one global namespace and a
  conversation turn already has a `Provenance` in it; two crates may call their
  own noun the same thing and the document may not. Aliased on the new one, so
  no stored shape and no generated type moved.
- **The chart needed nothing for 2.4** (2.4): 2.3's Role already carries jobs
  `delete`, pods `get/list/watch` and `pods/log` `get`, because the ADR named
  them. The one thing the client had to say out loud is
  `DeleteParams::background()` — without a propagation policy the pod is
  orphaned by the delete and keeps running, which is the one thing a cancel is
  for.
- **The chart** (2.3):
  - one ConfigMap of templates, mounted by the server and the worker;
  - a `-launcher` ServiceAccount on the pod that holds the reactors, the only
    pod in the release with a token;
  - a Role and a RoleBinding in the pods' namespace;
  - a NetworkPolicy entry for the step pods;
  - a schema that wants quantities as strings;
  - a `fail` unless the store is `postgres`.

  The release image and `build-images.sh` build with `aiwatcher-server/kube`.
- **The crypto provider is the process's to name, and the features cannot**
  (2.5, and the thing 2.5 was for). reqwest's `rustls` feature selects
  `aws-lc-rs`; kube was asked for `ring`. Two providers in one rustls means
  rustls will not name a process-level default, so *every* call to a cluster
  panicked inside it — in a build that compiled, clippied and passed every test,
  because until now nothing had ever opened a connection to one. kube's feature
  is `aws-lc-rs` now, and the client installs the provider explicitly as well,
  because `iggy` brings `ring` back whenever `laser` is on.
- **The gate runs the launcher on the host and the pods in the cluster** (2.5).
  What had never been exercised is `kubernetes.rs` against a real API server,
  and a server started here with a real kubeconfig exercises all of it for the
  price of a `cargo build` rather than an image. What that leaves out is an
  in-cluster client reading its own service account, which is kube-rs's code —
  and the grant such a client would hold is asked about directly instead.
- **The server the gate starts sees one cluster** (2.5). `try_default` reads
  whichever context is current and this kubeconfig has production EKS in it, so
  the gate hands it a minified copy holding the one local context. ADR_0006's
  rule, a layer further down than the Tiltfile's.
- **An unreachable API is refused by a probe pod** (2.5), before anything is
  registered. A host address that is right here and wrong in the cluster
  otherwise arrives as four attempts that never claimed, one start allowance
  later, and reads as a broken launcher.
- **The four stages are aiwatcher's own, in planner's shape** (2.5):
  `acquire → normalize → analyze → persist`, three artifact edges, a review at
  the end, in `sdk/python/examples/pod_stages.py` and an image of their own.
  The spec scoped planner's commit out, and a gate that needed planner's image
  could not run anywhere else — what it costs is stated rather than hidden: the
  review compared is this workflow's, not the house import's.
- **Byte-identity is the digest the route computed over the stored bytes**
  (2.5), per stage rather than only at the end. And *who* ran each stage is a
  separate artifact, asserted to **differ** — four distinct pod names against
  one worker's — because two identical reviews would also pass a comparison of
  one path with itself.
- **The long-lived worker is up for every phase** (2.5), registered for all four
  tasks and polling the queue the pods' attempts are on. Nothing but the
  key-only claim rule keeps it off them, so a gate that stopped it while the
  pods ran would be testing the launcher with that rule switched off.
- **A log is named by the hash of its own bytes, so stored logs cannot be
  counted** (2.5). The first version of this assertion counted objects and found
  two for seven pods: the stages printed nothing, and every empty log is one
  object. Each stage prints its own name now, and what is asserted is that a
  pod's words are in the store after the pod is gone.
- **The chart's Role is asked about rather than read** (2.5). The gate's server
  is an administrator through the kubeconfig, so the Role is exactly the thing
  that would be wrong in a deployment and cannot be wrong here: the chart's RBAC
  is applied and `kubectl auth can-i` asked, as that service account, for each
  call the launcher makes and for four it must never make.
- **The gate builds its own binary** (2.5, found by running it). The launcher is
  not in the default build, and any plain `cargo build` or `cargo test` in this
  repository replaces `target/debug/aiwatcher` with one that has no launcher —
  after which the gate fails at start-up with a refusal about a variable nobody
  set.
- **What a pod *is* is the deployment's, so the backend is a setting** (2.6, the
  owner's ask). `AIWATCHER_POD_RUNTIME` is `kubernetes` or `process`; the plan,
  the `plan_id`, the derived name and the claim are identical either way. The
  alternative considered was a second binding — a `LocalProcess` runtime kind —
  and it was wrong for the reason ADR_0012 gives: a definition would then name
  where it runs, and the same import could not move between a laptop and a
  release without being re-authored.
- **The two backends are sent one manifest, not two descriptions** (2.6). A
  neutral `Launch` struct beside the Job would have been two authored shapes of
  one thing, free to drift. The pod spec already says what to run, with what
  environment, in which directory, so the process backend *reads* the manifest
  — the same way `KubeCluster` reads it back into a typed `Job` before sending
  it, and the same way `attempt_of` is the inverse of `annotations`.
- **A host refuses what the program would read and ignores what only shapes the
  room** (2.6). `envFrom`, a volume and an environment value from anywhere but
  the downward API's own name end the attempt naming themselves; the image, the
  limits, the security context, the service account and the node selector are
  ignored and named once in the module rather than per pod. The split is what a
  wrong answer costs: a program without its credential fails somewhere else
  entirely, while one without a cgroup is honestly slower and unbounded.
- **The queue is the start allowance, not a refusal** (2.6). Over the host's
  limit, a `create` is accepted and the attempt is `Live` with the cluster's own
  kind of reason, so the launcher's start allowance decides about it exactly as
  it decides about a pod nothing will schedule. Refusing the create instead
  would have been a warning per waiting attempt per two seconds, and a
  concurrency answer written twice.
- **The gate gets a backend rather than a second gate** (2.6). `--runtime
  process` runs the same five phases and skips the memory one, saying why. What
  a `kubectl get jobs` answers in a cluster is answered there by the launcher's
  own log, in JSON, because a step's process lives inside the server and nothing
  outside it can list one — and the two assertions that are about a process
  being *gone* ask `pgrep`.

## Tasks

### Part 1a — now
- [x] 1.1 Config: drop `EngineKind` and the `flyte_*` settings, their parsing and
      validation; refuse `AIWATCHER_ENGINE` other than `none` and the runner kinds
      `engine`/`flyte` by name. Tests for both refusals. — *a removed engine is
      refused by name*
- [x] 1.2 Wiring: drop `build_engine` and the runner's engine arm;
      `AppState.engine` is `None`. — *no build carries Flyte*
- [x] 1.3 Delete `crates/aiwatcher-pipeline`, its workspace entries, the server's
      dependency, the `Dockerfile` COPY line and `tests/engine_end_to_end.rs`;
      let `Cargo.lock` follow. — *no build carries Flyte*
- [x] 1.4 Chart: drop the `engine` values, schema entry and helper block and the
      `status.sh` row; a values file that still sets `engine` fails to render
      naming it. — *a removed engine is refused by name*
- [x] 1.5 SDK: drop `integrations/flyte.py`, its test and the README sections. —
      *no build carries Flyte*
- [x] 1.6 The justfile's `test-pipeline` and `flyte_*` variables, `.env.example`'s
      engine block, README's recipe row, `CLAUDE.md`'s crate row and commands,
      the workspace header; ADR_0016 marked superseded.
- [x] 1.7 Verify: `just check`, `just sdk-check`, `just chart-check`.

### Part 1b — after the panel rebuild
- [x] 1.8 API routes, error variants, `AppState.engine`, `core::engine`; the
      contract and the generated client regenerated; the launcher and its call
      sites removed. — *no build carries Flyte*

### Part 1c — after AW-3
- [x] 1.9 `ExecutionOwner::Unknown`; `RuntimeBinding::ExternalWorkflow` and its
      spec and kind removed; the contract regenerated. — *an unknown owner is not
      an engine*
- [x] 1.10 The documents that describe the engine as a design. — *no build
      carries Flyte*

### Part 2 — pods (outline)
- [x] 2.1 ADR: templates, allowlists, who may name an image, the `kube` feature —
      [ADR_0029](../../ADR/ADR_0029_POD_PER_STEP.md)
- [x] 2.2 `RuntimeBinding::ContainerJob` and the step's `pod` field; the
      templates file both roles read; registration's refusals — *only an
      allowed image runs*, *resources come from the template*
- [x] 2.3 The launcher behind `kube`: the store's read of claimable rows by
      runtime, a Job per attempt under a name derived from its key,
      `backoffLimit: 0`, the key-only claim rule; the chart's RBAC, templates
      and network rule, and `kube` in the release image — *a step may ask for a
      pod*, *the engine owns a pod's retries*
- [x] 2.4 Cancel deletes the Job; the watch ends a dead pod's attempt with its
      reason; the log is read back into the catalog — *a cancel reaches a
      running pod*, *a pod's log is kept*
- [x] 2.5 planner's four stages on a local cluster, byte-identical — *the shape,
      in aiwatcher's own gate (`just e2e-pods`); naming a template on planner's
      own four is planner's commit*

### Part 3 — the other backend (the owner's ask, 2026-09-12)
- [x] 2.6 `AIWATCHER_POD_RUNTIME=process`: the same launcher against this host,
      one process per attempt, proven by `just e2e-processes` — *what a pod is
      is the deployment's*, *a host refuses what it cannot keep*

## Log
- 2026-09-11 12:50 — job planned: Part 1 in three passes cut where other sessions' work sits, five decisions, ten tasks; Part 2 outlined
- 2026-09-11 13:40 — Part 1a built: the server refuses the engine by name (`ConfigError::Removed`, two tests), the crate and its end-to-end test are gone, the chart refuses `engine`, the SDK integration and the recipes are gone, ADR_0016 superseded; `cargo clippy -Dwarnings` and the config tests green, `just chart-check` and `just sdk-check` (419) green
- 2026-09-11 13:55 — verified: `just check` 19/19 on the working tree, other sessions' work included
- 2026-09-11 12:47 — Parts 1b and 1c built (`8154041`): the five routes, `core::engine`, `AppState.engine`, the three error variants, the launcher on the recipe and Experiments pages, `ExecutionOwner::Unknown`, `ExternalWorkflow` and its spec and kind; the contract and the panel's client regenerated with no engine left in them; clippy clean, every touched crate's tests and the panel's 178 green
- 2026-09-11 12:54 — 1.10 done: the docs pass landed inside `5f9996b`; after it, §28's Phase 12 bullet no longer says AW-4 is investigating, and three unwrapped `CLAUDE.md` lines are wrapped; `just check` 18/19 on HEAD, `comments` failing on the same five panel blocks from `ea2dfe7`
- 2026-09-11 14:45 — 2.1 done: ADR_0029 — a step opts in with `pod` and compiles to `container_job`; templates and per-template image lists are chart values, matched by exact repository; the launcher reads claimable rows and creates one Job per attempt by a derived name, never claiming; the pod claims its attempt by key under its template's queue token (a one-attempt credential deferred as the strict mode); the lease decides, the watch explains; the log is the last 256 KiB in the catalog. Answers the spec's three open questions; 2.2–2.4 reworded to match
- 2026-09-11 15:13 — 2.2 built (`c4fc124`): `RuntimeBinding::ContainerJob` under `container_job`; the step`s `pod` field, absent from every older digest; `aiwatcher_execution::pods` with the templates file, the exact-repository image match, Kubernetes quantities and the refusals of fields aiwatcher fills in; a 422 at registration naming step, value and template; `AIWATCHER_POD_TEMPLATES` read by the serve role and refused where no launcher exists; the SDK `PodRequest`. Clippy -Dwarnings and the three crates suites green, `just sdk-check` 423, the panel build green, the contract regenerated
- 2026-09-12 00:32 — 2.3 built, and swept into another session's `26be55e` before it could be committed on its own: the key-only claim rule in `ClaimFilter` and in the PostgreSQL claim, `claimable_attempts` in four adapters with a contract property, the launcher and its Job manifest in `aiwatcher-server/src/execution/pods` — the loop and the manifest in every build, kube-rs behind `kube` — a refused launch reported as a `StepFailed` the way a reactor reports, `PERFORMABLE` and `recorded_result` widened so a pod's report is acknowledged, the chart's templates file, launcher RBAC, mounts and network rule, `AIWATCHER_POD_API_URL` and `AIWATCHER_POD_NAMESPACE`, the SDK reading `AIWATCHER_ATTEMPT` and `AIWATCHER_WORKER_NAME`, and `aiwatcher-server/kube` in the release image. Clippy -Dwarnings with every feature, the execution, API and server suites, `just sdk-check` 424, `just chart-check` plus renders with templates set, the postgres and duckdb store contracts, `cargo deny` and `just openapi-check` green
- 2026-09-12 01:10 — 2.4 built (`c713a1e`): the launcher's pass gained a watch — the cluster's own Jobs and pods listing, read once each per pass — which ends a dead pod's attempt as `Infrastructure` with the cluster's own reason (`OOMKilled`, `DeadlineExceeded`, an exit code), ends and deletes a Job no pod claimed within its template's start allowance, and stops a cancelling or already-ended run's pods as `Policy` so the cancel completes rather than waiting out a lease; the attempt rides as three annotations so it can be read back; the log is the last 256 KiB through a `Tail` that compiles in every build, stored under the artifacts prefix and recorded in the catalog against the attempt, after which the Job is deleted (`DeleteParams::background()`, or the pod outlives it); a log that could not be read or stored leaves the Job for the next pass. `RunState::cancelling()`/`is_cancelling()` replace matching a badge's string, and the cancel that left a dispatched attempt in the claim table for ever — `StepSkipped` retiring attempt `0` — is fixed in the handler. Clippy `-Dwarnings --all-features`, execution 160 + 13 + 23 + 14 + 15 + 12, server 93 + 38 and 93 with `kube`, API 156 + 24, the postgres (5 + 7) and duckdb (4) store contracts, `just chart-check`, `cargo deny` and `just openapi-check` green
- 2026-09-12 07:55 — 2.5 built (`0ec11aa`, `409ec5d`), and AW-4's exit is green on a real cluster: `just e2e-pods` runs an import's four stages as four pods on the local Kubernetes and the same four through one long-lived worker, and compares them — every stage's output has one digest across both paths, while the artifacts saying who ran each stage differ, four pod names against one worker's. Beside it: the chart's Role covers each call the launcher makes and refuses four it must not, a cancel deleted `analyze`'s Job and reached `cancelled` in 2.3s with `persist` never started, a stage taking 512MiB against its 192Mi limit failed twice as `infrastructure` with the cluster's own `OOMKilled (exit 137)` while the stages before it stood, and every pod's log was in the store with no Job left behind. It found the bug that mattered: two rustls crypto providers in one process meant the launcher panicked on its first call to any cluster, in a build that was green everywhere else. Clippy `-Dwarnings --all-features`, `cargo test --workspace --all-targets` (47 suites), server 93 + 38 with `kube`, `just sdk-check` 450, `just chart-check`, `just openapi-check` and `cargo deny` green
- 2026-09-12 09:34 — 2.6 built (`c77a997`, `aaf09fc`): the launcher's backend is a setting — `AIWATCHER_POD_RUNTIME=kubernetes|process` — and `ProcessCluster` is the second `Cluster`, one process per attempt from the same manifest, with a slot limit that queues rather than refuses, both streams into one 256 KiB tail, a delete that kills, a `Drop` that stops what is left, and refusals for `envFrom`, a volume and any environment value but the downward API's own name. Templates in the work role no longer need the `kube` feature under this runtime, and a namespace beside it is refused. 15 new tests (12 on the backend against real processes, 3 on the config); `just e2e-processes` green on a binary built with no cargo feature, no image and no kubeconfig — four distinct pod names, one digest per stage against the worker, a cancel in 2.0s, every log in the store — and `just e2e-pods` still green on orbstack, memory phase included
