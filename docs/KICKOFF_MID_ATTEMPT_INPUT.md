# Kickoff: an attempt that stops to ask (the rest of Phase 14)

Written 2026-09-09, after the authored gate landed. The design is
[architecture](PIPELINE_ARCHITECTURE.md) §41 and the scope is what Phase 14 has
left; this document is what a session needs to start, not a second design.

This session is in **aiwatcher** — the execution crate, the worker protocol, the
Python SDK. It shares files with the open `ai_spirit_agent` session, and the
overlap is real rather than theoretical: `decide.rs`, `handler.rs`, `hosted.rs`
and `message.rs` are the same four files Phase 13's timers went through. Agree
who holds them before either side starts, and **commit by path, never `-a`** —
a `git commit -a` has already once swept one session's work into the other's
commit under the wrong message.

## Where the tree is

41 files are uncommitted at `3b930bc`, mixing both sessions' work. `just check`
is green except `typos`, which fires only on the untracked
`docs/diagrams/out/*.html` a Google Fonts `unicode-range` upsets. Land what is
there before starting.

## What Phase 14 already has, checked in the tree

A gate is authored on **both** surfaces and compiles to one binding. A curation's
`BlockSpec::Approval` and a registered workflow's `WorkflowTask::approval` both
become `RuntimeBinding::HumanInput`; `aiwatcher_core::human_input` owns what a
valid question is, so the two cannot come to disagree. `decide` parks the step,
`ContextSnapshot::allowed` carries `Answer` only while it holds a question, and
`POST …/steps/{step}/input` requires the role the *question* named on top of the
editor floor — read from the pinned plan, because a route cannot know it in
advance.

A gate may carry a deadline. `decide` resolves the instant from the clock in its
input, the handler derives the timer row from the fact, and the ten-second tick
that Phase 13 built delivers it once. `on_timeout` is `fail | skip | answer`,
authored with the deadline and refused without one.

One panel control answers all of it — `AnswerGate`, used by the pipeline's run
card and by the Workflows view.

Read 43.36, 43.37, 43.38 and 43.39 before changing any of it. They are four
short entries and each one records a thing that was wrong on the first attempt.

## What is missing, and it is one thing

**A step either *is* a gate or is not.** §41 also wants a worker to park
**mid-attempt** — the tool call an `AbstractCapability.before_tool_execute` hook
wants approved — releasing its lease, leaving the attempt `awaiting_input`, and
being resumed by an answer as a *new* attempt of the same step.

Three facts make the size of that clear:

- `WorkReport` has exactly two shapes, `Completed` and `Failed`
  (`worker.rs:177`). There is no third, and adding one is the whole of the
  protocol change.
- `AttemptWrite` has `Dispatch` and `Retire` (`claim.rs:80`). A parked attempt
  is neither — §43.34 already decided that `awaiting_input` **keeps its row**,
  because it is not an ending, so the shape is there and nothing produces it.
- `StateType::AwaitingInput` is non-terminal and already folds correctly. The
  state machine is not the gap.

## Decide this before writing code

**Hosted and compiled are two different things wearing one name.** §41 says "a
worker may `await` from inside an attempt" as if it were one feature. It is not.

For a **hosted** run the worker owns the decision: `dispatch_ready` returns early
on `ExecutionMode::Hosted`, and a park is a message the worker appends to its own
stream. Nothing in the protocol has to change, and building something here would
be a second decider.

For a **compiled** run's `PythonTask` attempt the worker holds a lease it has to
renew, and the engine decides. That is where the gap is, and it is the one worth
building. Settle this first, because it decides whether `WorkReport` grows a
variant or `hosted.rs` does — and the answer this document expects is
`WorkReport`.

**And decide what the answer resumes.** §41 says a *new* attempt of the same
step, with the response in its inputs, so the attempt that asked stays
immutable. That is right and it has a consequence worth stating: the resumed
attempt re-runs the work from the beginning. A worker that wants to keep what it
did before the question has to hand it back as an artifact and read it again —
which is the same rule every retry already lives under, and not a special case.

## The work, in an order that keeps a gate at each step

**1. `WorkReport::Parked`.** A third shape carrying the question — prompt, role,
choices, and an optional deadline, which is
`aiwatcher_core::human_input::question_problems`'s vocabulary and must be
validated by it rather than by a second rule set. The route settles the attempt
by parking it: the claim row stays, the lease is released, and the step becomes
`awaiting_input` with an `InputRequest` the same shape a `HumanInput` step
produces.

*Exit:* a claimed attempt reports `Parked`, the run's page shows the question,
and no worker can claim that attempt while it waits.

**2. The answer resumes it.** `ProvideInput` on a parked *attempt* rather than a
parked step: `decide` schedules attempt *n+1* of the same step with the response
bound as an input, and the attempt that asked keeps its record.

*Exit:* the answer produces a new claimable attempt; the old one is still
readable with its question and who answered it.

**3. The deadline follows for free, or it does not.** The timer row is derived
from `InputRequested` carrying a deadline, and a mid-attempt park emits the same
fact — so a parked attempt with a deadline should already work. Prove it rather
than assume it, and if it does not, the reason is worth an entry in §43.

*Exit:* a parked attempt whose deadline lapses is decided by its `on_timeout`,
once, with the claim row retired.

**4. The Python SDK's side.** `aiwatcher_sdk.worker` gains the way to ask —
whatever a capability hook calls to park. Keep the telemetry client's rule: this
half raises, because a worker that cannot ask must not silently carry on.

*Exit:* a task that asks parks, and the same task resumed with an answer sees it
in its inputs.

## What this session should not build

**The first control message on `/api/v1/live`,** unless it re-argues the gate
first. It was specified when the panel had no other way to hear about a
question. It now re-reads the run on every stream frame, so the question appears
on its own, and the answer goes by a REST route that is tested on both surfaces.
An inbound control channel would be a *second* way to send one command, which is
the shape this repository refuses everywhere else. Build it when something has
to answer without making a request — not because §41 named it.

**Approval inside an agent turn.** That is Phase 13's `agentic_graph` work, in
`ai_spirit_agent`, and it is the last item on that list.

## If this session is short, or the four shared files are held

Two things are unblocked, need none of those files, and are the gate for other
work:

- **Measure slot lateness and backlog, and artifact and staging growth.**
  Nothing in the workspace measures any of it, and it is the stated gate for
  designing observability and a reference-aware GC. Workflow retention does not
  clean every artifact or staged context, so the second number decides whether
  that matters yet.
- **The Notebook view draws a gate as "Not run".** It reads only
  `outcome.status === 'running'` and `result`
  (`pipeline-notebook.tsx:52`), so it misses the note the canvas already shows.
  One line, and it is the last place the two views disagree about a block.

## Commands

```bash
just check                 # everything CI runs
just test-worker-protocol  # the protocol's own gate — where a new report shape belongs
just test-worker-runtime   # the recovery gate the SDK side has to keep passing
just test-postgres         # the storage contract, if the claim row's shape moves
just openapi               # after any route or type change; commit contract and client together
```

## Traps

- **Never let a worker decide anything but its own function.** The loop is claim
  → load → cache → `step.started` → perform → re-check the lease → record →
  report, and only `perform` crosses the wire. A park is a *report*, so it goes
  through `settle`; a worker that decided what its own park meant would be the
  drift the seam exists to prevent.
- **Never let a parked attempt be claimable.** The claim filter is what stops a
  second worker picking up a question the first is holding. §43.34 kept the row
  for exactly this and nothing has ever written one.
- **Never record a timeout as an answer somebody gave.** `skip` completes with no
  `InputProvided`; `answer` attributes it to `aiwatcher/timeout`. A mid-attempt
  park inherits both rules rather than inventing its own.
- **Never give `decide` a vocabulary for a timer.** The deadline row is derived
  in the handler from the fact. If a parked attempt needs a timer, it needs an
  `InputRequested` carrying a deadline — not a new call.
- **Never validate a question in two places.** `aiwatcher_core::human_input` is
  the one rule set, and a `WorkReport::Parked` is a third authored surface for
  the same question.
