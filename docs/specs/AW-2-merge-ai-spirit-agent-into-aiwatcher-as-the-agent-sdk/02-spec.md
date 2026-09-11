---
id: AW-2
step: spec
status: doing
branch: feat/agent-sdk-merge
repo: aiwatcher
created: 2026-09-10
updated: 2026-09-11
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

**In scope.** `agentic`, `agentic_runtime` and the port `providers`
implements become aiwatcher's (the adapters stay — see *Settled*); agent-to-agent messaging moves onto the hosted execution stream; a
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
- [x] The agent core is in that distribution, an agent runs on any model
      source, and importing it loads no model stack. (`Agent`, tools,
      messages and prompt builders are `aiwatcher_agentic`'s, and `agentic`
      hands back the same modules; `test_agent_port` runs an agent on a
      one-method double and imports the core in a fresh interpreter with no
      provider, MLflow, transformers or pydantic loaded.)
- [x] The runtime is in that distribution too, handed its tracer and settings,
      and the application's agents run on it unchanged. (`AgenticRuntime`, its
      stores, `hosted` and the distributed transport are
      `aiwatcher_agentic.runtime`'s; `agentic_runtime` hands back the same
      modules and composes the SDK's runtime with MLflow, `.env`, its model
      client and its `data/`. `test_runtime_port` imports it with no SDK,
      MLflow or pydantic loaded. `just agentic-check` 355, the agent
      repository 865; standalone 14/14, transport 11/11, join 8/8, outbox
      11/11.)
- [x] A composed graph's turn is drawn against the shape it declared: a node
      it never reached is `Pending` and its hand-offs are messages.
      (`just e2e-agent-graph`, 8/8, on a server of its own: a routed graph's
      two passed-over searchers stay `Pending`, the dispatch and the answer to
      the join are messages between agents, and the searcher's span nests
      under its node though its tracer publishes through a client of its own.)
- [x] The MLflow prompt registry is gone: an agent's named prompt is what
      ADR_0011's registry resolves — an admitted, promoted candidate once there
      is one — and nothing asks MLflow for a prompt. (Against a server of its
      own, 9/9: `make registry` publishes six, a real builder reads the
      admitted candidate, a dev-only win is refused, a republish moves no
      label.)
- [x] A turn's model call names the prompt version it ran on, and the
      reference resolves. (`just e2e-agent-prompt`, 6/6, on a server of its
      own: the promoted candidate named and answered by the registry, the
      authored version when the builder has no registry, nothing for text
      given directly, and none of the words on the log or the run.)

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

#### Requirement: the agent core names its ports and carries no model stack
The agent core — `Agent`, its tools, messages, prompt builders, structured
output and usage limits — SHALL ship in `aiwatcher-agentic` beside the engine,
and SHALL reach a model, a tracer and a prompt named by `external_prompt_name`
only through ports it declares: `ModelSource`, `LLMTracer` and
`PromptSource`. Importing it SHALL NOT load a provider stack, MLflow,
transformers or pydantic. `agentic`'s old names SHALL resolve to the same
module objects for one release, and a builder named by `external_prompt_name`
SHALL read the application's prompts there — from ADR_0011's registry when one
is configured.

##### Scenario: an agent runs on anything that lends it a model
- GIVEN a double whose only method is `session()`, yielding a model
- WHEN an `Agent` built on it runs a turn
- THEN it answers, with the model's usage on the result

##### Scenario: a model that did not load is refused with the reason
- GIVEN a source whose session yields no model and whose `get_load_error`
  says why
- WHEN the agent runs a turn
- THEN it raises with that reason in the message

##### Scenario: importing the core loads no model stack
- GIVEN a fresh interpreter
- WHEN it imports `aiwatcher_agentic.agent`, `tools` and `prompts`
- THEN none of `providers`, `mlflow`, `transformers`, `torch`, `mlx`,
  `openai`, `httpx`, `pydantic` or `agentic` is loaded

##### Scenario: a named prompt with nowhere to be read from is refused by name
- GIVEN no source handed to the builder and none named by the application
- WHEN a builder is given `external_prompt_name="sage"`
- THEN it raises naming `'sage'` and `use_prompt_source`

##### Scenario: the application's registry answers
- GIVEN `ai_spirit_agent`'s `agentic` imported
- WHEN a `QwenPromptBuilder(external_prompt_name=…)` is built
- THEN its text is what `registry.get_prompt` returned

##### Scenario: what an agent returns is what the engine reads
- GIVEN an `AgentResult`, a `ToolRunResult`, a `PromptSnapshot` and a
  `RequestUsage`
- WHEN each is assigned to the engine's `AgentRun`, `ToolRun`, `AgentPrompt`
  and `ModelUsage`
- THEN `mypy --strict` accepts all four

#### Requirement: the runtime is handed what it read from the application
`AgenticRuntime`, its stores, the hosted runtime and the distributed transport
SHALL be `aiwatcher_agentic.runtime`'s, handed their tracer and settings
through ports rather than reading an application's, and SHALL import no
backend — no `aiwatcher-sdk` until `hosted` or the transport is used. A store
handed no path SHALL go under the working directory, never beside the code.
The application's composition — its tracer, settings, model client and data
directory — SHALL stay the application's, and a caller that builds its runtime
SHALL get what it got before the move.

##### Scenario: a runtime answers a turn on what it was handed
- GIVEN an `AgenticRuntime` with one workflow, a router, `RuntimeConfig`
  stores and `NoopLLMTracer`
- WHEN a message targeted at that workflow is handled
- THEN the workflow's reply comes back, and nothing reached MLflow or a model
  client

##### Scenario: importing the runtime loads no backend
- GIVEN a fresh interpreter
- WHEN it imports `aiwatcher_agentic.runtime`, `hosted` and `distributed`
- THEN none of `aiwatcher_sdk`, `httpx`, MLflow, `providers`, pydantic or
  pydantic-settings is loaded

##### Scenario: an application's settings are the port
- GIVEN an application's settings object, with plain attributes of the same
  names
- WHEN it is handed to the runtime or to discovery
- THEN it type-checks as `RuntimeSettings` and `DistributedSettings`, and
  every service that discovery creates is held to its delivery budget

##### Scenario: a store nobody placed stays out of the code's directory
- GIVEN a runtime handed no store paths
- WHEN it opens its stores
- THEN both are under the working directory's `.data/`, and nothing is
  written beside the installed modules

##### Scenario: the application's runtime is composed as before
- GIVEN `agentic_runtime.runtime.AgenticRuntime` built with the application's
  `Settings`
- WHEN it starts
- THEN it is the SDK's runtime, with the tracer `create_tracer` returned, the
  OpenAI-compatible client configured from `Settings`, its stores in the
  application's `data/` when `.env` names none — and a patch of
  `init_tracing`, `create_tracer` or `SQLiteMessageStore` through that module
  reaches the code that runs

##### Scenario: the runtime's records keep their wire names
- GIVEN `AgentRegistration` and `AgentHeartbeat` as `ai_spirit_agent` wrote
  them before the move
- WHEN the moved runtime writes and reads them
- THEN the bytes are the same, `agentic_runtime.distributed.contracts`
  included

#### Requirement: a graph's turn is drawn against the shape it declared
A compiled `agentic_graph`, when `AIWATCHER_URL` is set, SHALL declare the
nodes a turn can reach — its agents and its outputs — and the connections
between two of them before the turn's first node runs; SHALL publish every node
it runs as a step of that turn's execution; and SHALL record every hand-off — a
dispatch, and an answer handed to a join — as a message between the agents that
made it. One turn SHALL be one execution, under the graph's own id. A span an
agent's tracer opens inside a node SHALL be that node's child however the two
are published, and the tracer SHALL open no run of its own there. Without
`AIWATCHER_URL`, or without the SDK, the graph SHALL run as before and declare
nothing.

##### Scenario: a node the turn never reached is Pending
- GIVEN a graph whose entry routes to one of three searchers
- WHEN one turn runs
- THEN its execution holds the six reachable nodes, all declared; the four it
  reached succeeded, the two the router passed over are `Pending`, and the
  execution succeeded with two nodes pending

##### Scenario: a provider or an integration is not a stage
- GIVEN a graph whose agents are wired to a model provider and search
  integrations
- WHEN its shape is declared
- THEN neither is a node of it, and no connection touching one is an edge

##### Scenario: the hand-offs are messages
- GIVEN the same turn
- WHEN it runs
- THEN the entry's dispatch to the searcher and the searcher's answer to the
  summarizer are two messages between those agents, and neither is a declared
  edge

##### Scenario: an agent's spans nest under its node
- GIVEN an agent whose tracer publishes through a client of its own
- WHEN it runs inside a node
- THEN its span's parent is the node's step, and the tracer opens no run of its
  own

##### Scenario: a node that fails fails the turn
- GIVEN a searcher that raises
- WHEN the turn runs
- THEN its step failed, the entry it ran inside failed, the turn's run failed,
  and the error reaches the caller

##### Scenario: without aiwatcher nothing is declared
- GIVEN no `AIWATCHER_URL`
- WHEN a turn runs
- THEN nothing is published and the answer is what it was

#### Requirement: an agent's named prompt is ADR_0011's current version
An application's prompt named by `external_prompt_name` SHALL be read from
aiwatcher's prompt registry when `AIWATCHER_URL` names one — the version
`production` points at, or the newest when none has been promoted — and SHALL
be the text the application authored when it names none. With a registry
configured, a read that cannot be answered SHALL raise and SHALL NOT fall back
to the authored text. Publishing the authored prompts SHALL be idempotent on
their text and SHALL move no label. Nothing SHALL ask MLflow for a prompt.

##### Scenario: an admitted optimisation is what an agent runs
- GIVEN a prompt published to aiwatcher, and an optimisation of it admitted on
  its held-out score and promoted
- WHEN an agent's builder names that prompt
- THEN its text is the candidate's, even with a newer draft stored beside it

##### Scenario: with nothing promoted, the newest version is read
- GIVEN two versions of a prompt and no `production` label
- WHEN a builder names it
- THEN its text is the newer one's

##### Scenario: without aiwatcher, the authored text
- GIVEN `AIWATCHER_URL` unset
- WHEN a builder names `sage`
- THEN its text is `SAGE_PROMPT`, and a name nobody authored is refused by name

##### Scenario: a configured registry that cannot answer is never replaced
- GIVEN `AIWATCHER_URL` set, and aiwatcher unreachable or started without a
  prompt store
- WHEN a builder names a prompt
- THEN it raises — `registry_disabled` for the second — and no authored text is
  served in the registry's place

##### Scenario: a prompt the registry was never given names the fix
- GIVEN `AIWATCHER_URL` set and nothing published
- WHEN a builder names `sage`
- THEN it raises naming aiwatcher's address and `make registry`

##### Scenario: publishing twice is one version each, and deploys nothing
- GIVEN `production` on a candidate of `sage`
- WHEN `make registry` runs twice
- THEN each prompt has one authored version, `sage` keeps its first author, and
  `production` still names the candidate

##### Scenario: nothing imports MLflow for a prompt
- GIVEN a fresh interpreter
- WHEN it imports `registry`
- THEN neither `mlflow` nor an HTTP client is loaded, and `registry` declares no
  MLflow dependency

#### Requirement: a turn names the prompt version it ran on
An agent's model call SHALL carry the registry reference of the prompt its
builder read by name — `prompt_name` and `prompt_version` on its `llm.*`
events, which aiwatcher writes as `aiwatcher.prompt.name` and
`aiwatcher.prompt.version_id` on the span. The version SHALL be `sha256` of the
text the builder read, so that it is the id the registry gave that text, and
SHALL NOT be the hash of the rendered prompt, which names no version. A builder
that read nothing by name, or whose text or name has changed since it read,
SHALL name no version. The text SHALL NOT be carried.

##### Scenario: a promoted candidate is named on the span
- GIVEN `production` on a candidate of `sage`
- WHEN an agent whose builder names `sage` runs a turn
- THEN its LLM span names `sage` and the candidate's version, and the registry
  answers that reference with the candidate's text

##### Scenario: without a registry, the authored version
- GIVEN no `AIWATCHER_URL` for the builder, and the catalogue published by
  `make registry`
- WHEN the same agent runs a turn
- THEN its span names the authored version, and that reference resolves

##### Scenario: the template's version, not the rendered prompt's
- GIVEN a template with `{tools}` and whitespace around it, and a capability
  that adds instructions
- WHEN a turn renders it
- THEN the version is the template's `sha256`, and differs from `prompt_hash`

##### Scenario: text given directly names no version
- GIVEN a builder handed its text as well as a name
- WHEN a turn runs
- THEN its span carries no prompt reference — the name alone would point at a
  text the model was not given

##### Scenario: the reference, never the words
- GIVEN a turn on a named prompt
- WHEN its events and its run are read back
- THEN neither holds any of the prompt's text

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

- **The agent core moves; the model stack does not** (2026-09-10). `agent`,
  `tools`, `message`, `prompts`, `capabilities`, `exceptions`, `history`,
  `memory`, `usage`, `response_parser` and `structured_output` are
  `aiwatcher_agentic`'s, under their old module names, and `agentic`
  registers each as that module object. What builds a concrete
  `providers.ModelProvider` stayed — `CoreAgentic`, `LLMCall`, `VLMCall`, and
  with them the LLM reactors and `WorkflowBuilder` — as did the MLflow tracer,
  the specialized agents (one reads Qdrant), the search integrations, voice
  and the git tracer. `Agent` only ever called `session()` on its provider, so
  that is the port: `ModelSource`, with `TextModel` and `ModelResponse` moved
  outright and re-exported by `providers`.
- **`providers` does not move whole** — the question this spec deferred to
  here. The port moved with `Agent`; the adapters (MLX, vLLM, SGLang,
  transformers, ONNX, the OpenAI-compatible client, the model server) stay with
  the application that picks one, which also keeps `mlx-audio` out of a Linux
  worker's install.
- **A named prompt is read from a source the application names.** The builder
  imported the MLflow `registry` itself, an application package. It now reads
  `external_prompt_name` through `PromptSource` — handed to it, or set once
  with `use_prompt_source`, which `agentic` does on import with the registry,
  imported lazily as before. Retiring that registry is one line there.
- **The core brings `structlog` and `orjson`.** Kept rather than rewritten onto
  `logging` and `json`: a tool call parsed differently, or a log line shaped
  differently, is a behaviour change, and a move is not the place for one. The
  engine's modules still import nothing third-party.
- **No wire name moved with the core.** Nothing in either repository writes a
  core type as a record — what is registered is the engine's messages and the
  applications' own events — so `WIRE_PREFIX` stays the engine's.

- **The runtime moves; the application's composition does not** (2026-09-10).
  `AgenticRuntime`, the conversation store, the fine-tuning export, `hosted`
  and nine of the ten `distributed` modules are `aiwatcher_agentic.runtime`'s,
  under their old names, and `agentic_runtime` registers each as that module
  object. The investigation left `distributed/` behind because it was the Iggy
  transport; since Phase F it is the aiwatcher one, which is what this spec is
  about. What stayed names this application: `settings` (pydantic-settings,
  `.env`, its model names), `trace` (MLflow), `users`, `workspaces` and
  `slugs` (`~/.aispiritagent`), `deciders` (a third `passthrough_decider`),
  and `DistributedAgenticRuntime`, which answers in Polish and names chat and
  image modes.
- **The runtime is handed a tracer and settings; the application subclasses
  it.** `AgenticRuntime(tracer=, settings=)`, where `RuntimeSettings` is a
  protocol the application's pydantic `Settings` satisfies by having the
  fields, and `_configure_providers` is a hook. `agentic_runtime.runtime`'s
  `AgenticRuntime` is that subclass — MLflow, `.env`, the OpenAI-compatible
  client, its `data/` — rather than a re-export, so the places that patch
  `init_tracing`, `create_tracer` or `SQLiteMessageStore` through it still
  reach the code that runs. `AgenticServiceDiscovery.from_settings` takes the
  settings it used to import, and its two callers pass them.
- **`aiwatcher-sdk` arrives, as an extra.** The dependency the distribution
  requirement names: `hosted` and `distributed.aiwatcher` import the SDK when
  they are used, so `aiwatcher-agentic[aiwatcher]` installs it and an
  application whose agents share one process installs nothing.
- **A store's default was the checkout** (found here; broken since Phase C).
  `SQLiteEventStore` took its default directory from its own file,
  `parents[4] / "data"`, which after the engine moved named the root of the
  aiwatcher checkout — an agent run with no `EVENT_STORE_PATH` wrote
  `workflow_event_store.sqlite3` there, and the moved message store would have
  written into `sdk/`. Both default to the working directory's `.data/`, and
  `ai_spirit_agent` names its own `data/`, which is where both were before
  either moved. The stray file is deleted.
- **Two record types keep `agentic_runtime`.** `_WIRE_NAMES` extends
  `WIRE_PREFIX`'s promise to the runtime package, so `AgentRegistration` and
  `AgentHeartbeat` are written as before. Nothing has stored them since
  Phase F; the fixture keeps it true if something does again.
- **A graph declares what a turn can reach** (Phase A's last piece). Agents
  and outputs are nodes; a model provider and a search integration are what an
  agent is configured with, and nothing hands a turn to one — declared, they
  would sit `Pending` in every execution for ever. The connections between two
  reachable nodes are the edges.
- **A turn is its own execution, never the hosted one.** The turn id
  `CompiledGraphSystem.run` mints is the `workflow_run_id`. The engine already
  declares a hosted execution's shape as the definition that started it, so a
  graph declared under that id would have every node flagged undeclared beside
  it. Joining a graph's traversal to a hosted execution it ran inside waits for
  a caller that runs one there: the join e2e's workers never call `run`, and a
  summarizer a deadline fires outside `run` has no traversal open and says
  nothing.
- **A node names its own span.** The declaration and the agent's tracer
  publish through two clients, which are two queues, so "the node is still
  open" cannot be inferred from arrival order. `WorkflowContext.node` takes a
  `span_id` and the scope it yields carries it as the parent;
  `Scope.correlation` is what the tracer is handed through `current_attempt`,
  which also keeps it from opening a root of its own. Minted once per node and
  carried by every event of it, so a redelivered envelope lands on the span it
  opened — the rule `AiwatcherTracer` already keeps.
- **In process a hand-off nests.** The bus runs the handler that starts the
  next node before the source's turn returns, so the target's step is inside
  the source's, and a target that raises fails its source too. That is the
  call stack, and the waterfall draws it rather than two stages side by side.
- **Hand-offs are messages by alias, of two kinds.** `dispatch` for a node
  starting another, `completion` for an answer handed to the join that waits
  for it. The panel draws messages between agents and a node's step names its
  alias as the agent, so the two meet on one name.
- **The catalogue stays with the application; only MLflow left it.**
  `registry` keeps its name, its `Prompts` and its texts, now `CATALOGUE`,
  because what an agent is told is the application's. `get_prompt` reads
  ADR_0011's registry when `AIWATCHER_URL` names one, so the builders' source
  in `agentic` is unchanged — the one line the agent-core bullet above
  anticipated turned out to be none.
- **Without aiwatcher, the authored text; with it, never.** Unset, there is no
  registry to disagree with and the authored text is the only answer. Set, a
  registry that cannot answer raises: serving the authored text in its place
  would run a version the registry may already have replaced, with nothing in
  the trace to say so — the failure the removed requirement names.
- **Publishing is not deploying.** `make registry` stores drafts and moves no
  label, so a promoted candidate survives every republish, and an edited text
  runs while nothing is promoted over it or once somebody moves `production`.
- **A prompt nobody published names the fix.** A 404 is raised as
  `UnknownPromptError` naming aiwatcher's address and `make registry`, rather
  than published on read: a read that writes would attribute a version to
  whichever process happened to ask first.

- **A turn's prompt version is derived, not looked up** (2026-09-11). The
  builder records `sha256` of what its source returned, which is the
  registry's own content address, so the id on the span is the registry's by
  construction and nothing asks the registry a second time — `make registry`'s
  six versions are the `sha256` of their texts, and the span's id resolves.
  Not `prompt_hash`: that is the *rendered* prompt, after `{tools}` is filled,
  the template stripped and a capability's instructions added, and it names no
  version of anything.
- **A reference only while it is true.** Text handed to the builder directly,
  a text replaced after it was read and a builder renamed since all name no
  version. The tracer sends the name and the version independently and
  `PromptRef` keeps nothing without a version, so a name alone never reaches a
  span as a claim about a text the model was not given.
- **The channel is the attribute the agent already wrote.** The agent passes
  `agentic.prompt_version` beside `agentic.prompt_name` in `extra_attributes`,
  which the MLflow tracer sets on its span verbatim; aiwatcher's tracer, which
  dropped both, writes them as `prompt_name` and `prompt_version` on both
  `llm.*` events. `LLMTracer`'s protocol is unchanged. The capability path
  rebuilt `PromptArtifacts` field by field, and would have dropped the new
  field; it is `dataclasses.replace` now.
- **The licence is PolyForm Noncommercial 1.0.0, for the whole repository**
  (the owner, 2026-09-11). Apache-2.0 was the first answer the same morning and
  did not survive the second question: the owner wants the code used for
  nothing commercial, which a permissive licence cannot say. The root `LICENSE`
  — proprietary, granting nothing — and the Apache-2.0 that `Cargo.toml`, both
  SDKs and `ml_pipeline` declared had disagreed since before AW-2; they are now
  one licence, its text verbatim under a `Required Notice:` line, and `NOTICE`
  says commercial use needs a separate licence. Each SDK carries its own
  `LICENSE`, because a wheel or a package does not ship the repository's, and
  `sdk/agentic/NOTICE` keeps the MIT notice the engine, the core and the
  runtime came under. The crates are `publish = false`, and `deny.toml` skips
  unpublished workspace crates rather than allowing PolyForm — allowed, it
  would admit a noncommercial *dependency* with nobody deciding so. Unlike the
  text it replaces, it does not forbid training a model on the code. Left for
  their owners: `services/query/flow` still declares MIT, and AW-3's three
  `services/query` projects Apache-2.0. `ai_spirit_agent` followed the same
  day, on the same terms: MIT on top of a noncommercial dependency said
  something no user could act on, and copies received under the earlier MIT
  licence stay under it, as they must. And `CONTRIBUTING.md` asks every
  contribution for a licence AI Spirit Labs may sublicense on any terms —
  without it a commercial licence could not cover the whole of the code, and
  that is not something to repair after the first outside contribution.

- **The GUI is reviewed, and nothing of it moves** (2026-09-11,
  [gui-review.md](gui-review.md)). The run overlay the investigation counted on
  is not live — every node lights before a run and after it — and the panel's
  Workflows view already draws a turn node by node; the Events tab is the
  panel's run and live feeds after the fact; a turn started from the Gradio
  Runner is already a declared execution in aiwatcher when `AIWATCHER_URL` is
  set. Authoring an agent graph is the one real gap, and it is a spec of its
  own. The panel's own gaps for agent graphs — the prompt a span ran on, a node
  that says what it was, a composer — follow the panel rebuild in progress
  rather than race it.

## Still open, deferred to the phase that can answer them

- **Which `evaluation` scorers move.** The DeepEval bridge is already
  structural here; the MLflow scorers read completions, and this plan moves no
  content.

## Log
- 2026-09-10 08:07 — spec drafted on `main`; two open questions settled, two deferred
- 2026-09-10 15:29 — messaging requirement rewritten for Phase F: a hop is a run of the target agent's workflow, a reply is a row in the client's mailbox; four scenarios added
- 2026-09-10 16:07 — Phase C: two requirements added (the engine's wire names, the old import path for one release) over four scenarios; the engine's cut, its wire names, the one 3.14-only call and the `Generator` spelling settled; `providers` deferred to the phase that moves `Agent`
- 2026-09-10 16:30 — agent core: one requirement added over six scenarios (33 in all); the core's cut, `providers` keeping its adapters, the prompt source, the core's two dependencies and why no wire name moved, settled; the deferred `providers` question closed
- 2026-09-10 18:27 — agent runtime: one requirement added over six scenarios (39 in all); the runtime's cut, the tracer and settings ports with the application's subclass, `aiwatcher-sdk` as an extra, a store default found broken since Phase C, and the runtime's two wire names, settled
- 2026-09-10 20:05 — Phase A finished: `declare_graph` wired. One requirement added over six scenarios (45 in all), one success criterion ticked; the shape a turn can reach, the turn as its own execution, a node's own span id, nesting in process and the two message kinds, settled
- 2026-09-10 20:04 — the MLflow prompt registry retired: one requirement added over seven scenarios (52 in all), the agent core's prompt clause reworded, one success criterion ticked; the catalogue staying with the application, the authored text without aiwatcher and never with it, publishing not deploying, and a 404 naming the fix, settled; a turn's prompt version left open
- 2026-09-11 10:10 — a turn's prompt version: one requirement over five scenarios (57 in all), one success criterion added and ticked; the version derived from what was read, a reference only while it is true, the attribute channel, and the licence (Apache-2.0, the owner) settled; the root `LICENSE` contradiction recorded, not changed
- 2026-09-11 10:25 — the licence re-decided by the owner: PolyForm Noncommercial 1.0.0 for the whole repository, replacing the Apache-2.0 settled an hour earlier and the proprietary root `LICENSE` with it
- 2026-09-11 10:40 — `ai_spirit_agent` licensed the same way, and `CONTRIBUTING.md` asks contributions for the rights a commercial licence needs
- 2026-09-11 11:00 — the GUI reviewed ([gui-review.md](gui-review.md)): nothing moves; the investigation's run overlay corrected; authoring an agent graph left for a spec of its own
