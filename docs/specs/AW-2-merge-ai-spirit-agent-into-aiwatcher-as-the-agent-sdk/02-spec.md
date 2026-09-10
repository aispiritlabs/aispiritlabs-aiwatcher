---
id: AW-2
step: spec
status: doing
branch: feat/agent-sdk-merge
repo: aiwatcher
created: 2026-09-10
updated: 2026-09-10
tags: [spec/AW-2, step/spec, branch/feat-agent-sdk-merge, status/doing]
---

`#spec/AW-2` · `#step/spec` · `#branch/feat-agent-sdk-merge` · repo `aiwatcher`

# ② Spec — AW-2

## Proposal

**Intent.** One repository holds the agent SDK and the system that observes it,
and a hop between two agents is a row in a history aiwatcher owns rather than a
message on a second broker. What this buys is not tidiness: it is a shared join
that survives a worker restart, a panel that draws a fan-out as it happens, and
a promotion that rests on a held-out score rather than on the score an optimiser
maximised.

**In scope.** `agentic`, `agentic_runtime` and `providers` become
aiwatcher's; agent-to-agent messaging moves onto the hosted execution stream; a
local outbox keeps an agent working while aiwatcher is unreachable; an agent is
registrable and schedulable on its own; prompt optimisation reports through
ADR_0011; `laser-sdk` and the MLflow prompt registry leave.

**Out of scope.** The application — `personal_assistant`, `chat`, `cli`,
`knowledge_base`, `agentic_graph`, `workshops` — stays in
`ai_spirit_agent`. `agentic_graph`'s Gradio and PixiJS surface is reviewed
for capability and not ported. The MLflow tracer stays teed. Sealed payloads
stay §40.4's deferred option.

**Approach.** Six phases, each ending at something that runs: wire the seam that
is already built on both sides; move prompt optimisation; move the engine; build
the outbox; merge the two composition roots and add the standalone wrapper;
replace the transport. §40 is the design and is unchanged by this; what this
spec adds is the repository dimension §40 does not cover.

**Success criteria:**
- [x] A fan-out of three joins across a worker restart, with no broker running.
      (`just e2e-agent-join`: three processes, the first SIGKILLed after two
      completions, the join fires once from the stream. Since Phase F there is
      no broker left in the agent SDK to run.)
- [x] An agent starts from `POST /api/v1/executions` with no graph around it.
      (`just e2e-agent-standalone`, 14/14 — the call the panel's launcher makes,
      and again from a schedule's `run_now`; the host never imports
      `agentic_graph`. It starts its own server with the archive on: a lost
      first attempt is retried and its tool writes once, the agent's spans
      nest under the run, and the exchange is archived once and exported as an
      `sft` row.)
- [x] aiwatcher stopped mid-conversation: the agent keeps answering and every
      hop arrives exactly once when it returns. (`just e2e-agent-outbox`, 11/11
      — through a proxy the script shuts, so the dev server itself is not
      stopped; a claim is the one thing that waits for it.)
- [ ] A prompt candidate admitted on a held-out score, and one refused by name.
- [x] `just sdk-check` and `make test-agentic` both green; `laser-sdk` is
      absent from the agent SDK's dependencies. (SDK 418, the agent repository
      1127. `laser-sdk` is gone from `agentic_runtime` and from `uv.lock`, and
      `just e2e-agent-transport`, 11/11, runs lab 6 with it uninstalled: a lost
      hand-off, an agent with no worker, a refused hop and SIGTERM on the way.)
- [x] The engine is a distribution of its own, on the 3.13 floor, and every
      stored record still reads. (`aiwatcher-agentic` in `sdk/agentic`, no
      runtime dependencies, `just agentic-check` green; `agentic.workflow` is
      the same modules under their old names; a record is written as the same
      bytes the engine wrote before it moved.)

## Spec (delta)

### ADDED Requirements

#### Requirement: A held-out split is dealt by group
The SDK SHALL provide the dev/held-out split that
`PromptRegistry.record_optimization` requires, dealing whole **groups** to one
side, deterministically in the group and a salt and in nothing else. It SHALL
refuse a split that leaves either side empty.

##### Scenario: every phrasing of one question lands together
- GIVEN sixty questions, each with three phrasings
- WHEN they are split with the question as the group key
- THEN no question has phrasings on both sides

##### Scenario: adding a question moves no existing question
- GIVEN a split over forty questions under one salt
- WHEN twenty questions are added and the corpus is split again under that salt
- THEN every original question is on the side it was on before

##### Scenario: an empty side is refused, and the refusal counts groups
- GIVEN six phrasings of a single question
- WHEN they are split at the default share
- THEN the call raises, naming the empty side and reporting
  `1 group(s) across 6 case(s)` — because the fix is more questions, and a
  case count would hide that

#### Requirement: an agent is registrable without a graph
The SDK SHALL turn one agent into a `WorkflowDefinition` that
`POST /api/v1/executions` starts and a schedule fires, without the caller
authoring a graph. It SHALL use the existing `DefinitionKind::Workflow`, whose
own documentation already reads *"A declared agent, search or ML workflow"*, and
SHALL NOT introduce a third definition kind.

##### Scenario: one agent, started from the panel
- GIVEN an agent registered through the wrapper
- WHEN `POST /api/v1/executions` names it with `target.kind = "workflow"`
- THEN it runs, and the Workflows view draws it with no graph document anywhere

##### Scenario: a turn is delivered at least once and its tool writes once
- GIVEN a hosted agent whose tool keys its write by the message's idempotency key
- WHEN its first attempt is lost after the tool wrote
- THEN the server retries it, the run completes, and the write happened once

##### Scenario: a turn becomes fine-tuning data
- GIVEN an agent hosted with a conversation archive
- WHEN a turn completes, on its first attempt or a later one
- THEN the archive holds its exchange once, joined to the run's trace, and an
  approved `sft` export holds a row whose completion is the reply

##### Scenario: a hosted agent's spans belong to its run
- GIVEN an agent runtime whose tracer was built before any attempt existed
- WHEN a worker runs one of its turns
- THEN its `agent.*` and `llm.*` spans carry the execution's run id under the
  step's span, and no run of the agent's own is opened

##### Scenario: the schedule and the tick reach one compiler
- GIVEN a schedule saved against that registered agent
- WHEN the slot comes due
- THEN the tick compiles it through `executions::compile_head`, the same
  function that agreed to save the schedule

#### Requirement: a hop survives aiwatcher being unreachable
Agent-to-agent messaging SHALL persist locally before it is sent and SHALL drain
when aiwatcher returns. It SHALL NOT drop on overflow as telemetry does, and it
SHALL NOT raise into the agent's own path as a registry client does. A row SHALL
be deleted only after aiwatcher has accepted it, and a human SHALL be able to
list and drain the store by hand.

##### Scenario: a conversation continues through an outage
- GIVEN two agents mid-conversation and aiwatcher stopped
- WHEN the first hands off to the second three times
- THEN the agent keeps answering, and when aiwatcher returns all three hops are
  on the stream, once each

##### Scenario: the process dies between the send and the acknowledgement
- GIVEN a hop written locally and sent, with the acknowledgement lost
- WHEN the process restarts and drains
- THEN the hop is re-sent under the same message id and aiwatcher recognises the
  redelivery rather than appending beside it

##### Scenario: an operator can see what is stuck
- GIVEN an outbox holding undrained rows
- WHEN an operator lists it
- THEN every row shows its execution, its message id and how many attempts it
  has had

##### Scenario: a claim waits for aiwatcher
- GIVEN a fan-in whose last completion was written while aiwatcher was
  unreachable
- WHEN the summarizer's claim is asked for
- THEN it is refused while those completions wait, and granted once they have
  drained — a claim is never queued and never decided offline

#### Requirement: agent messaging runs on aiwatcher's claims
A hop SHALL be a run of the target agent's one-step workflow. Publishing SHALL
start that run with the message id as its `Idempotency-Key`; consuming SHALL be
a worker claim whose queue is the target agent; acknowledgement SHALL be the
claim's result; a lost claimant SHALL be recovered by lease expiry; a dead
letter SHALL be the step's failure. A reply to a client that is not an agent
SHALL be an append to that client's **mailbox** — one hosted execution per reply
address — read from a cursor, as the Iggy topic was. A hop's text SHALL NOT
reach the event log, a run's parameters or a stream row: each carries sender,
recipient, kind, a reference, a digest and a size, and the content follows
§40.4's `external` policy.

This replaces the first draft of this requirement, which made publishing an
append to one hosted execution's stream. A hosted run never schedules an
attempt — `dispatch_ready` returns before it, so that no reactor claims what the
worker is also deciding — which leaves a hosted stream with nothing a queue can
claim. See *A hop is a run* below.

##### Scenario: three workers share one history
- GIVEN three agent workers and no broker
- WHEN a fan-out of three completes across them
- THEN the join fires once, from the stream, whichever worker sees the last
  completion

##### Scenario: the log carries the fact and never the words
- GIVEN a hop whose text is a user's message
- WHEN it is published, claimed and answered
- THEN no run's parameters, history or projection holds the text — each holds
  the sender, the recipient, the kind, a reference, a digest and a size

##### Scenario: a hop waits for its agent
- GIVEN no worker running for the summary agent, and no broker
- WHEN the search agent hands off to it
- THEN the hop waits in the summary's queue, and is answered when a summary
  worker starts

##### Scenario: a hop whose acknowledgement was lost is recognised
- GIVEN a worker whose start of the next hop reached aiwatcher and whose answer
  did not
- WHEN the server retries that worker's attempt
- THEN the next hop is started again under the same message id, is one run, and
  is handled once

##### Scenario: a hop that cannot be handled reaches the sink
- GIVEN a handler that raises `PermanentMessageError`
- WHEN its hop is claimed
- THEN the step fails and is not retried, and the client is answered with the
  failure rather than waiting out its timeout

##### Scenario: the registry is what was registered
- GIVEN three agents registered with their capabilities
- WHEN a fresh process asks for `web-search`
- THEN it is the search agent, read from the definition aiwatcher holds

#### Requirement: one distribution per failure policy
The agent SDK SHALL ship as a distribution of its own, depending on
`aiwatcher-sdk` while nothing in `aiwatcher-sdk` depends on it, so that
importing telemetry does not import a provider stack. The outbox SHALL live in
`aiwatcher-sdk`, because a worker reporting a step result wants the same
durability and the Rust side has one outbox rule with two callers.

##### Scenario: telemetry stays cheap to import
- GIVEN a process that only publishes spans
- WHEN it installs and imports `aiwatcher_sdk`
- THEN neither the provider stack nor an agent dependency is installed

#### Requirement: the engine's records keep their wire names
A record SHALL be written under the same type name it was written under before
the engine moved, and a record written then SHALL read back as the engine's
type. A type an application defines SHALL keep being named by its own module
path.

##### Scenario: the same record, the same bytes
- GIVEN a `UserMessage`, an `Event` and an `LLMResponse` serialised by the
  engine at `ai_spirit_agent`'s last commit with the engine in it
- WHEN the moved engine serialises the same three records
- THEN it writes exactly those bytes, `agentic.workflow.messages:UserMessage`
  included

##### Scenario: an old row reads back
- GIVEN a row that pre-move engine wrote
- WHEN the moved engine reads it
- THEN it is the engine's own type, and writing it again gives the same bytes

##### Scenario: an application's type is its own
- GIVEN an event type defined outside the engine
- WHEN it is registered, written and read
- THEN its wire name is its own module path, not the engine's old prefix

#### Requirement: the old import path is the same code for one release
`agentic.workflow` SHALL resolve every submodule that moved to the engine's own
module object, not a copy, for one release.

##### Scenario: one class, whichever path imported it
- GIVEN `agentic.workflow.messages` and `aiwatcher_agentic.workflow.messages`
- WHEN both are imported
- THEN they are one module, and a monkeypatch through the old name reaches the
  code that runs

### MODIFIED Requirements

#### Requirement: the Python floor
Every Python distribution in this repository SHALL require 3.13 or newer.
(Previously: `aiwatcher-sdk` required 3.11 and CI ran that floor because
planner did; the agent packages pinned `==3.14.*`.) Nothing 3.14-only was
found, so this is a policy choice; it moves planner's five SDK pins and the
SDK's CI matrix, and that cost is accepted rather than discovered.

##### Scenario: the gate runs the floor
- GIVEN the CI matrix after this change
- WHEN the SDK job runs
- THEN it runs on 3.13, and `requires-python` says the same

#### Requirement: an agent's durable history is shared
A workflow's history SHALL be readable by every worker that may act on it.
(Previously: `agentic.workflow` kept one SQLite store per process, so a
fan-out whose worker restarted between the second and third completion had no
store its join could live in.) The per-process SQLite store SHALL remain as the
local backend for a single-process run.

##### Scenario: the join outlives the worker that opened it
- GIVEN a fan-out of three with two completions recorded
- WHEN the worker holding them is killed and another takes over
- THEN the third completion fires the join, and it fires once

#### Requirement: an optimisation is recorded, not written to a file
The prompt optimiser SHALL search on the dev side only, measure baseline and
candidate on both sides, and report through `record_optimization`, letting
ADR_0011's verdict decide. (Previously: it searched over every golden, wrote the
winning text to `optimized_prompt.txt` and reported no scores at all — so the
registry would have refused every candidate it sent.)

##### Scenario: a candidate is admitted on evidence it did not select on
- GIVEN a candidate that improves the held-out score
- WHEN the optimisation is recorded
- THEN the registry admits it, and `overfit_gap` is on the record

##### Scenario: a candidate that stopped reading its input is refused by name
- GIVEN a candidate whose text no longer interpolates a variable the baseline used
- WHEN the optimisation is recorded
- THEN it is refused for `variables_lost`, checked before the scores, so the
  reason says it stopped reading its input rather than inviting more iterations

### REMOVED Requirements

#### Requirement: Apache Iggy as the agent transport
(Reason: its whole job here — publish, consume as a group, ack one entry,
reclaim what an idle consumer never acked — is what the claim protocol already
does, over a history the panel can draw. `laser-sdk` leaves the agent SDK's
dependencies. `aiwatcher-bus`'s own `laser` feature is untouched: that is
aiwatcher's event log, a different question.)

#### Requirement: the MLflow prompt registry
(Reason: ADR_0011 is the same registry with content-addressed versions, labels
and a verdict the client cannot fake. Two registries for one prompt is two
answers to "which version did this run use". MLflow keeps tracing, teed.)

## Settled from the investigation's open questions

- **A standalone agent's kind.** No new kind. `DefinitionKind` has two
  variants and `Workflow`'s doc comment already covers an agent; a third would
  be a `match` arm in two places, which is the failure the "never let a
  schedule and its tick reach different compilers" guardrail names.
- **Where the outbox lives.** `aiwatcher-sdk`, not the agent distribution —
  see the distribution requirement above.
- **A claim waits for aiwatcher** (the user, 2026-09-10). A fan-in reached
  during an outage keeps its completions and fires its summarizer when
  aiwatcher is back. Deciding offline was the alternative and it risks two
  workers firing one summarizer.
- **The durable tier is DuckDB** (the user, 2026-09-10), as Phase D first said —
  replacing the SQLite adapter this spec's first implementation shipped. Its
  file lock is exclusive for a connection's life, so the outbox opens one per
  operation (≈7 ms against 0.4 ms held): an operator and several workers must be
  able to open the file while an agent runs, which the Rust store, one process
  owning its database, never needed.

- **One composition root, two classes.** `AgenticRuntime` stays where an
  application composes its agents; `hosted_runtime` hands it to the SDK's
  `Runtime`, which registers each agent, hosts its turns and closes it through
  `on_close`. Folding the classes together would put a router, SQLite stores
  and a provider stack into the distribution telemetry imports.

- **A hosted agent prepares fine-tuning data** (the user, 2026-09-10). Its
  message stays a run parameter — an instruction to an agent preparing data,
  not somebody's turn — and each turn is recorded in the conversation archive
  as one exchange joined to the run, for review and an `sft` or `dpo` export
  (ADR_0021). It is recorded before the step reports; an archive that cannot be
  reached fails the turn as `infrastructure`, which is retried, and one that
  refuses it fails it as `policy`, which a person decides.
- **A hosted agent's spans nest under its run** (the user, 2026-09-10). The
  worker publishes the attempt in a context variable, and a tracer that was not
  handed a run of its own defers to it: no root run per turn, `agent.*` under
  the step's span.
- **At least once, under control** (the user, 2026-09-10). A turn takes the
  server's default budget for attempts that may not have finished. Everything
  it records is filed under the step — `TaskContext.step_key`, the archive's
  message ids, the message's idempotency key — so a retry overwrites rather
  than duplicates and a keyed tool writes once. `retry=` sets the budget per
  agent, and a failure in the agent's own code is `user_code`, which only a
  person retries, from the panel.

- **A hop is a run, not a row** (Phase F, 2026-09-10). §40.6 maps consume to a
  worker claim on the target's queue, and only a compiled step reaches the claim
  table: a hosted run's `dispatch_ready` returns before scheduling anything, on
  purpose. Phase E had already made an agent a one-step workflow, so a hop is a
  start of that workflow — the message id is the `Idempotency-Key`, which makes
  a re-sent hop the same run — and the server's claims, leases, retry budget and
  failed-step sink are the consumer group, `XAUTOCLAIM` and the dead-letter
  stream, with no change on the Rust side. A reply to a client has no agent to
  claim it and every client tails it, so it goes to a mailbox: one hosted
  execution per reply address, found by a fixed `Idempotency-Key`, appended to
  and read from a cursor. The registry is the definitions aiwatcher holds, with
  capabilities in the step's parameters; a hop to an agent with no worker waits
  in its queue instead of being refused, which is what liveness was for.

- **The engine is a distribution of its own, and depends on nothing** (Phase C,
  2026-09-10). `aiwatcher-agentic` in `sdk/agentic`, imported as
  `aiwatcher_agentic.workflow` — named like `aiwatcher_sdk`, and not `agentic`,
  because `ai_spirit_agent` still has an `agentic` and has to re-export from
  it. All 26 files of `agentic.workflow` were read for what they import; what
  wraps a concrete agent — `builder` and the two LLM reactors, which build a
  `ToolMessage` and enforce `UsageLimits` — stayed in `agentic`. Where the
  engine reads an agent it now declares a protocol instead: `WorkflowTracer`
  (a workflow and a step span, nothing that calls a model) and `AgentRun`,
  `ToolRun`, `AgentPrompt`, `ModelUsage`. `Description`, `TraceSnapshot`,
  `TracingContext` and `build_trace_snapshot` are plain values and moved
  outright. The distribution requirement's "depending on `aiwatcher-sdk`" is
  deliberately not true yet: nothing in the engine imports it, and the
  dependency arrives with the first module that does.
- **A wire name is not an import path** (Phase C). `serialize_record` names a
  record's type by its module path, and stored rows say `agentic.workflow.…`;
  moving the module would have changed every record written afterwards and the
  `contract_name` column. `serialization.WIRE_PREFIX` keeps the engine's own
  types under the old prefix.
- **One 3.14-only call** (Phase C). The investigation found nothing 3.14-only;
  `mypy` at `python_version = "3.13"` found `uuid.uuid7` in `WorkflowRuntime`,
  which would have raised on 3.13 the first time a runtime minted an id.
  `ids.uuid7` is the standard library's on 3.14 and RFC 9562's layout below it.
- **`Generator[T, None, None]` stays the house spelling.** At 3.13 the
  one-argument form works and ruff's `UP043` would rewrite 22 sites; the reason
  for the full form went with 3.11, the rule did not, so `UP043` is ignored and
  the spelling is a separate decision.

## Still open, deferred to the phase that can answer them

- **Whether `providers` moves whole.** Not Phase C's question after all: the
  engine imports nothing from it — only `LLMTracer.llm` named `ModelResponse`,
  and the engine's tracer protocol has no `llm`. It belongs to the phase that
  moves `Agent`, where `mlx-audio` being darwin-only still argues for moving
  the port and leaving the MLX adapter.
- **The licence.** `ai_spirit_agent` declares MIT; `aiwatcher-agentic` was
  given the Apache-2.0 of the distributions beside it. The owner's call.
- **Which `evaluation` scorers move.** The DeepEval bridge is already
  structural here; the MLflow scorers read completions, and this plan moves no
  content.

## Log
- 2026-09-10 08:07 — spec drafted on `main`; two open questions settled, two deferred
- 2026-09-10 15:29 — messaging requirement rewritten for Phase F: a hop is a run of the target agent's workflow, a reply is a row in the client's mailbox; four scenarios added
- 2026-09-10 16:07 — Phase C: two requirements added (the engine's wire names, the old import path for one release) over four scenarios; the engine's cut, its wire names, the one 3.14-only call and the `Generator` spelling settled; `providers` deferred to the phase that moves `Agent`
