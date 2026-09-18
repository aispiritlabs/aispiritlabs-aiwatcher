# Execution, the workflow store and the scheduler

The rules for `aiwatcher-execution`, its reactors in
`aiwatcher-server/src/execution`, and the store adapters behind `WorkflowStore`.
Read [ADR_0025](../../docs/ADR/ADR_0025_MANAGED_EXECUTION.md),
[ADR_0026](../../docs/ADR/ADR_0026_ENGINE_AS_PRODUCER.md) and
[ADR_0029](../../docs/ADR/ADR_0029_POD_PER_STEP.md) before changing this area;
the root `CLAUDE.md` holds the rules that reach past it, including the one
ordering every durable write here obeys.

## Claiming, attempts and the worker seam

- **Never let a process claim work it cannot perform.** The claim filter is
  built from the `ExecutorRegistry`, so a process with no `AIWATCHER_QUERY_URL`
  takes no `flow_php` attempt. A runtime whose client would not build is one
  this process claims nothing for, rather than one it fails every attempt of.
- **Never let a worker decide anything but its own function.** The loop is claim
  → load the plan → cache lookup → `step.started` → **perform** → re-check the
  lease → record → report (`Reactor::take`, `settle`, `resume`), and only
  `perform` crosses the HTTP seam. A worker re-implementing which cache entry
  answers, when a retry is due or whether its lease still holds is the drift the
  seam prevents — and the last is one a claimant cannot check about itself.
- **Never authorise a worker route by the name it claimed under.** Worker names
  are not secret, so `worker::held` checks the lease *and* the token's queue
  scope together.
- **Never let a queue widen a token.** `name[queue other]=secret` only ever
  *narrows* what may be claimed; the role stays hard-coded `Editor`. A
  producer's token, a session and an OIDC identity name no queue and claim
  nothing — claiming takes a lease something renews. The queues are in the label
  because the secret is split with `split_once` and may contain `=`.
- **Never accept an artifact reference a worker described rather than wrote.**
  Rows go through the attempt's own `outputs/{name}` route, which digests what
  it stored, and the result route checks every reported output exists before
  settling. A completed step pointing at an object that 404s is the one failure
  nothing downstream catches.
- **Never presign a bucket to a process outside the cluster.** A worker runs on
  somebody's laptop, and a presigned URL is a bearer credential for the store
  holding prompts, datasets, annotations, conversations and training.
  `AttemptArtifacts` proxies, because the route can check that the caller holds
  the lease on the attempt.
- **Never derive a message id from less than what it identifies.** A reactor's
  report id *is* the inbox key, so it names execution, step, attempt and which
  fact it is; from execution and event name alone a two-step plan's second
  `step_started` reads as a redelivery and the step sits `pending` for ever. A
  command id needs the run's version (`RunProjection::last_message_version`) or
  a Pause after a Resume is deduplicated as the first Pause, with success on the
  wire and the run still going.
- **Never store a finished attempt as a row.** A settlement is the row ceasing
  to exist (`AttemptWrite::Retire`); a terminal row describes nothing and left
  the `file` adapter `fsync`ing the whole history on every claim.
- **Never leave a parked attempt holding its lease.** `AttemptWrite::Park` keeps
  the row — the shape `awaiting_input` reserves, a question not being an ending
  — and drops the lease, or the next claimant redoes the work having been told
  nothing about the question. Only a step the plan *dispatched* is parked; a
  `HumanInput` step never reaches the claim table.
- **Never let an answer leave the row it parked.** An answer *is* an ending:
  `InputProvided` retires the row in the same decision that dispatches attempt
  *n+1*, or the table gains a row per park for ever.
- **Never retire an attempt by a number the event does not carry.**
  `StepSkipped` names no attempt; read as attempt `0` it retired a key that
  never existed and left the real row claimable for ever. The number comes from
  the run, and a step at attempt `0` or one never dispatched gets no write at
  all.
- **Never ask the plan what only the question knows.** A `PythonTask` spec has
  no `on_timeout`, so the policy rides on `InputRequest`, copied from the spec
  when the gate is scheduled, with the plan only as the fallback for older
  streams; resolved from the plan alone, a lapsed deadline was `NoSuchStep` and
  the run stayed parked for ever. The cancel side reads the decision's own
  facts, because `evolve` clears `awaiting` on the events a cancel follows.
- **Never let an answer be a step's result unless the step was the question.**
  `ProvideInput` completes a `HumanInput` step; an attempt that stopped mid-work
  gets attempt *n+1* instead and stays immutable. The resumed attempt re-runs
  from the beginning, so answers accumulate on the **step**.
- **Never decide what an answer does in two places.** A person's `ProvideInput`
  and a deadline's `OnTimeout::Answer` both go through `answer_lands`; written
  twice, the timeout completed the step and left a parked attempt `Completed`
  with no outputs. `continue_after` belongs to the arm that ended the step.
- **Never cache a step somebody answered.** A human decision is addressed by
  nothing, so `cache_key` is `None` rather than a key meaning "probably the
  same".
- **Never let a step run past its run or its deadline.** `Watch` sets
  `ActivityContext::stop`, calls `cancel` and abandons an executor that has not
  returned within the grace, reporting `Policy` or `Timeout`; an executor holds
  `Committing` across what must finish and the reactor waits. Engines are told
  through `…/executions/{key}/cancel`, a refusal of the stopped request reports
  the stop (`StopSignal::or_stopped`) rather than a 409 read as user code, and a
  worker hears 409 `execution_stopping` at its next heartbeat.
- **Never spend the budget for attempts at the work on a runtime that declined
  it.** `Transient` ran nothing and gets ten attempts; `Timeout` and
  `Infrastructure` may have done the work and keep three. Counted by kind, or
  one budget of three kills a run over a forty-second outage.
- **Never retry the same work in two places.** The owner of an execution owns
  its retries — Rust for a `local` run, the store for a hosted decider's
  attempt, never the worker's loop as well. A `ContainerJob` sets `backoffLimit:
  0`.
- **Never let a lookup's two answers become one question.** Only the runtime
  knows whether it is *still executing* a key; only the receipt says what a
  finished attempt *produced*. `done` with no receipt means the rows never
  landed. And the comparison that means something is the runtime's digest
  against the one *in the receipt* — never against the artifact's, which is a
  different encoding by a different language.
- **Never let a runtime's own knowledge be assumed by the caller.** Whether a
  query named `now()`, whether a notebook samples or asks a model — only what
  ran it knows, and both report it for `ActivityResult::cacheable`. A notebook
  says `deterministic = False`; absent means true.

## Schedules and the tick

- **Never let a scheduler decide what is due.** The tick supplies an *interval*
  and `Schedule::slots_between` answers what fell in it, purely — which is where
  catch-up comes from, and why the tick rate is operational rather than a
  correctness choice.
- **Never resolve a local time by the offset of the instant that found it.**
  Twice a year a wall-clock time is ambiguous or absent, and an offset from the
  sampling instant made `slots_between` depend on the tick rate: daily 02:30 in
  Warsaw fired once over one interval and twice over the same span in
  twenty-minute ticks. Candidates are enumerated by **local calendar date**,
  resolved by a policy per cadence — daily and weekly take the first of an
  ambiguous pair and the instant the clocks reach for a skipped one; hourly
  lives a repeated hour twice and a skipped one not at all.
- **Never work out in the panel when a schedule next fires.** `next_run` is
  `Schedule::next_after`, so the hour a card shows and the hour the tick fires
  cannot differ; a second implementation would have its own idea of when the
  clocks change.
- **Never let a new schedule version reach into the past.** One written during a
  three-day outage would run three days of slots at once. `effective_from`
  bounds it, and it is **not** `updated_at`: `fires_the_same_as` compares
  cadence, timezone and enabled only, so switching `overlap` at 08:59 keeps nine
  o'clock.
- **Never decide whether a scheduled run may start from the read model.** It is
  empty in the `work` role where the tick runs, so `overlap = skip` never
  skipped there. `admit_slot` asks the workflow store, in the transaction that
  takes the slot.
- **Never write a transient failure down as a decision.** A refused compile says
  the same thing next tick; an unreachable store does not. Written as `refused`
  with the cursor advanced, ten seconds of unavailability cost the day's run and
  left a note claiming it had been refused. `SlotSettlement::TryAgain` leaves
  the slot due.
- **Never let the tick write a schedule's configuration.** It wrote the whole
  object back, so a DELETE landing in between came back enabled — and a `get`
  before the `put` does not close it, because an object store offers no
  compare-and-set. Configuration and slot outcomes have different writers and
  different places; the tick is handed a `ScheduleReader`.
- **Never let a schedule fire without writing down what happened.** The slot,
  the outcome and the reason go on the schedule head, or a schedule refused
  every morning for a week looks like a working one. It records the
  *scheduler's* decision and never the run's outcome, which is the log's answer
  one click away.
- **Never give a scheduled run an id that is not its slot's.** Derived from the
  definition and the slot, so two workers that both find 09:00 due produce one
  execution and one conflict — never from the compiled plan, which two workers
  reading the head a moment apart derive differently. The cursor advances
  *after* the starts commit.
- **Never let a schedule and its tick reach different compilers.** Both go
  through `executions::compile_head`, so a name nobody saved is a 404 today
  rather than a failure at nine tomorrow. The kind comes from the route's path,
  never from a body.
- **Never let a schedule mint a definition revision.** It is a mutable head
  under its own `schedules/` prefix: changing nine to ten is not a new pipeline,
  and the prefix is its own because `aiwatcher-datasets` owns `pipelines/`.
- **Never put a clock tick on the event log.** The only subscriber is the
  scheduler. What is durable instead is the tick's *cursor*, a Unix second in
  `processor_checkpoints`.
- **Never let a measurement cost a run.** The scheduler reports lateness and
  backlog *after* it has started the slots, and a sink that is down is a warning
  rather than a failed tick. Lateness is measured to the tick that *found* the
  slot, and reported beside the backlog from the same tick: one slot four
  minutes behind and forty of them are the same lateness and very different
  news.

## The workflow store

- **Never split the six writes of one workflow decision.** Deduplicate the
  input, check the expected version, append the outputs, update the projection,
  write the outbox, advance the checkpoint — one transaction, or the dual-write
  gap: an outbox row with no decision publishes a `step.completed` the store
  does not consider complete, and a decision with no outbox row is a run the
  panel never sees finish.
- **Never publish a decision to the log.** Facts about work go on it —
  `workflow.declared`, `step.*`, `artifact.produced`, `execution.*` — and
  commands, scheduled retries, leases and heartbeats stay in the store. An
  engine publishing per decision would flood the log it observes.
- **Never let two parties publish one attempt's `step.*`.** `data.published_by`
  makes a second publisher visible; the resolution is that a managed step's
  producer code does not open its own `node()` scope.
- **Never write the derived files of one decision without journalling it
  first.** The `file` adapter touches five — stream, projection, outbox,
  attempts, checkpoint — and a filesystem writes one at a time: a crash after
  the stream left the input's message id recorded with none of its consequences,
  so the retry was answered `Duplicate` over a run with no outbox row and no
  attempt. `PendingCommit` is written whole and `fsync`ed first, the rename is
  the commit point, everything after it is idempotent, and the record is deleted
  once applied. `recover` runs at `open` **and** at the top of every `append`.
- **Never hold a single-process lock by a file's existence.** The lock is the
  operating system's, `File::try_lock` on the open file, because the kernel
  releases it however the process ends — `create_new` plus a `Drop` is exactly
  the code a `SIGKILL` does not run. The file is never unlinked while held, or
  the next process creates a second inode and locks that.
- **Never run a managed execution that needs two processes on the `file` or
  `duckdb` store.** `StoreCapabilities::multi_process` is `false` and the
  refusal names `AIWATCHER_WORKFLOW_STORE`, at `open` rather than by
  interleaving writes that each look fine alone.
- **Never remove in one release what the release before it names.** The schema
  is applied at start-up by whichever replica gets there first, workers roll,
  and a rollback runs the old binary against the new schema — so a removal is
  two releases, one that stops using the column and a later one that drops it.
  NULL in every row makes the *data* safe to lose and says nothing about the
  query still naming it. An applied migration is never edited: what withdraws
  one is another migration.
- **Never let one adapter decide what "finished long enough ago" means.**
  `store::prunable` is the rule; `last_activity` is all an adapter supplies. The
  same reason `StateType::TERMINAL` exists rather than a list of states written
  out in SQL.
- **Never let retention decide a run has died.** `prune` takes terminal
  executions only: a run with no end is `Running`, and age tells an OOM kill
  from a twenty-minute think in neither direction. It deletes the whole
  execution together — a kept projection whose stream is gone is a run the panel
  lists and cannot open.
- **Never delete an execution's history by default.**
  `AIWATCHER_WORKFLOW_RETENTION_DAYS` is unset unless a deployment says
  otherwise and `0` means keep. The stream is the *explanation* of a run and the
  one thing the event log does not carry. The window has a floor nothing in the
  crate can check: the inbox goes with the stream, so it must outlast the log's
  retention or a redelivery is decided again instead of recognised.
- **Never keep an outbox row the log has accepted.** `mark_published` deletes —
  the fact is on the log, and the `file` adapter rewrites the whole outbox on
  each publish, so remembering was quadratic.
- **Never put a run's timings in the workflow store.** When an execution started
  and ended is the log fold's answer, and an attempt's is the span assembler's.
  `RunProjection` and `AttemptRecord` carried four such fields written and read
  by nothing; the projection is for accepting the next command and for the run's
  own page.
- **Never build a second live view of one run.** A managed run's facts carry the
  execution as `workflow_run_id`, so `/api/v1/workflow-executions/{id}/stream`
  has streamed them since the first one ran. A dedicated
  `/executions/{id}/stream` was struck rather than built.
- **Never split the binary in two without sharing all three backends.**
  `postgres` for the workflow store, `laser` for the log, `s3` for the object
  store one role writes and the other reads. `Config::validate` refuses each by
  name, because two fail silently and the third fails three attempts later with
  "holds no object".
- **Never put the projector in the `work` role.** It *is* the read model the API
  answers from, in process, under `AIWATCHER_MAX_SPANS_TOTAL`. A `serve` role
  without it answers every read from an empty fold.

## Pods, plans and caching

- **Never let a plan know what a pod is.** A `container_job` step names a
  template and an image on that template's list; whether that becomes a Job, a
  container or a bare process is `AIWATCHER_POD_RUNTIME`, and the plan, the
  `plan_id`, the derived name and the claim are identical for all three. Every
  backend is sent the same manifest and *reads* it (`pods::manifest`), so a
  backend outside a cluster **refuses** what it cannot keep — `envFrom`, a
  volume, an environment value from anywhere but the downward API's
  `metadata.name` — because ignoring one runs the program without the credential
  it was written to hold. What it may ignore is what only shapes the room: a
  node, a service account, and for `process` the image and the limits too.
- **Never let a plan name its own executor's address.** `AIWATCHER_QUERY_URL`,
  `AIWATCHER_ML_PIPELINE_URL`, the pod's service account. A `PlanStep` names a
  binding and its parameters, never a host — and for the same reason the rerun
  target is configuration.
- **Never leave a process's crypto provider to the features.** rustls asks the
  *process* and refuses to guess when two are compiled in, which happens
  whenever reqwest picks `aws-lc-rs` and iggy picks `ring`. Left unsaid, every
  call to a cluster panicked inside rustls and the launcher's task died with it,
  in a build that compiled, clippied and passed every test.
  `KubeCluster::connect` installs one; an `Err` there means somebody already
  did.
- **Never add a runtime a managed step reaches without opening the path to it.**
  A policy that admits the panel is right while the only client proxies a
  person's query; a managed step is aiwatcher reaching that service directly,
  and the refusal arrives as a refused connection the retry budget spends ten
  attempts on. Admit whichever pod holds the reactor: `server` combined,
  `worker` split.
- **Never make `plan_id` depend on where a block sits.** The authored revision
  digests the whole request, positions included; `plan_id` digests the
  executable fields only, so a canvas tidy-up invalidates no cache and starts no
  different run.
- **Never cache a step whose inputs are not all addressed.** A moving window, an
  unpinned notebook, a `file://` with no digest — `cache_key` returns `None`
  rather than a key meaning "probably the same", and caching is opt-in because
  that claim is wrong often enough to be worth saying out loud. A pinned window
  counts, because `POST /query/query` takes `window_from`/`window_to` and the
  API's windowed routes take `as_of`.
- **Never decide from the request what only the runtime can answer.** Whether a
  cache key is *well defined* is `cache_key`'s question; whether the run
  happened under those conditions is the executor's, on
  `ActivityResult::cacheable`. An older query service that never learnt `as_of`
  reads a drifting window and says so by omission.
- **Never let a window mean "the last hour" to one reader and a span to
  another.** `as_of` is absent for every panel query, which keeps a shared link
  meaning the hour it is opened in; a managed step pins it, and only then is the
  window a closed span two reads agree on.
- **Never let a policy field have no reader.** `CachePolicy::ByContent` sat on
  every compiled Flow step while `cache_key` was called only by its own tests —
  a claim the code did not keep, and a latent unsoundness no test could catch.
  Wiring it is what found the bug.
- **Never put rows, notebook source, a prompt, a completion or an agent's
  inter-node text in a workflow message.** A step hands data on as an
  `ArtifactRef` and its answer as a bounded inline value; the last of those is
  conversation content and belongs in the archive with a retention clock
  (ADR_0021).
- **Never let a Flow PHP block read past something it cannot read.** A Flow step
  names a dataset in the catalog, so the one query a chain compiles to ends at
  the first block that is neither a source nor a transform — a notebook, or an
  approval. A transform behind either would silently run against the *source*
  again. One rule rather than one per kind, and the refusal says which of the
  two is in the way.
- **Never let a step's edge and its data binding be one cursor.** An approval is
  in the chain without being in the data — it reads the rows before it, produces
  nothing, and the block after it reads those same rows — so `resolved_inputs`
  resolves by step id rather than by adjacency. One cursor bound the publisher
  to an output no step declares: a dataset version over no rows.
- **Never make a managed execution depend on an open browser tab** (ADR_0025).
  The panel authors, commands, links and renders; it does not compile, sequence,
  retry, resume or publish. A managed run's Flow script is compiled in Rust —
  the browser may show the same text, and what runs is what the server produced.
- **Never make `produced_by` part of a dataset version's identity.** A block
  dragged across the canvas is a new pipeline revision and the same rows. It is
  provenance, like the recipe name — and it matters because the Flow script
  alone does not describe an execution a notebook ran after.

## Gates and human input

- **Never author one gate twice.** A curation block and a registered workflow's
  step both put a question to a person, compile to one
  `RuntimeBinding::HumanInput` and are answered through one route, so what a
  valid question *is* lives once in `aiwatcher_core::human_input`. The panel
  keeps the rule from the other end: one `AnswerGate` for both surfaces.
- **Never let an authored gate name a role the answer route does not check.** A
  gate only ever *raises* the floor: `provide_input` holds the editor floor and
  then the role the **question** named, read from the pinned plan through the
  step's own `awaiting` — which cannot live on the route, because which role
  answers is a fact about the step. An authored gate admits `editor` and `admin`
  and refuses anything weaker **by name**, or it offers buttons to somebody the
  floor will refuse. The panel's `useRoleDecision` keeps "nobody has answered
  yet" apart from "no".
- **Never give `decide` a vocabulary for a timer.** A deadline is a consequence
  of a question having been asked: `InputRequested` carrying one schedules the
  row and anything ending that step retires it, both derived in the handler and
  written in the transaction that holds the decision. `decide` resolves only the
  *instant*, from the clock in its input, so a replay reaches the same moment.
  The row's id names the step **and the attempt**, or the first attempt's row
  fires on the second, and only a step the plan gave a clock to is cancelled.
- **Never let the engine's own delivery go through the caller's door.** The
  effect-command guard in `handle` is about *who is asking*: no caller may post
  an `ExecuteStep`, a `RequestInput` or a `TimeoutInput`. The engine firing a
  deadline it scheduled uses `ExecutionHandler::deliver` — crate-private, one
  call site — rather than a widened guard that would make a timeout postable
  over HTTP.
- **Never record a timeout as an answer somebody gave.** `on_timeout: skip`
  completes the step with **no** `InputProvided`; `answer` records one
  attributed to `aiwatcher/timeout`. And `skip` is not `StepSkipped`, which
  marks what will not run because a parent failed — a run whose steps are not
  all `Completed` never completes.
- **Never list an action whose command would be refused.**
  `ContextSnapshot::allowed` carries `Retry` only from `Failed` or `Crashed` and
  not while cancelling, and `Answer` only while a step holds a question, each
  mirroring `decide`'s own precondition. Cancel, pause and resume are not there
  at all: they are done to a run, and that is a block's context.
