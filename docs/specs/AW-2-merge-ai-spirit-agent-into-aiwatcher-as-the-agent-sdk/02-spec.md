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
- [ ] A fan-out of three joins across a worker restart, with no broker running.
- [ ] An agent starts from `POST /api/v1/executions` with no graph around it.
- [ ] aiwatcher stopped mid-conversation: the agent keeps answering and every
      hop arrives exactly once when it returns.
- [ ] A prompt candidate admitted on a held-out score, and one refused by name.
- [ ] `just sdk-check` and `make test-agentic` both green; `laser-sdk` is
      absent from the agent SDK's dependencies.

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

#### Requirement: agent messaging runs on the execution stream
Publishing SHALL be an append to one hosted execution's stream under
`expected_version`; consuming SHALL be a worker claim whose queue is the
target agent; acknowledgement SHALL be the claim's result; a lost claimant SHALL
be recovered by lease expiry. A hop's text SHALL NOT reach the event log — the
fact carries sender, recipient, kind, digest and size, and the content follows
§40.4's `external` policy.

##### Scenario: three workers share one history
- GIVEN three agent workers and no broker
- WHEN a fan-out of three completes across them
- THEN the join fires once, from the stream, whichever worker sees the last
  completion

##### Scenario: the log carries the fact and never the words
- GIVEN a hop whose text is a user's message
- WHEN the fact reaches the event log
- THEN it carries `from`, `to`, `kind`, a digest and a size, and the text
  is not in it

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

## Still open, deferred to the phase that can answer them

- **Whether `providers` moves whole.** `agentic` imports it directly and
  `mlx-audio` is darwin-only, which has no business in a Linux worker image.
  Phase C decides against the real import graph; the likely shape is that the
  port moves and the MLX adapter stays.
- **Which `evaluation` scorers move.** The DeepEval bridge is already
  structural here; the MLflow scorers read completions, and this plan moves no
  content.

## Log
- 2026-09-10 08:07 — spec drafted on `main`; two open questions settled, two deferred
