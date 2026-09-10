# aiwatcher-agentic

The workflow engine agents are built on: messages, deciders, event stores,
processors, sagas, and the consumer and turn loops that drive them. It moved
here from `ai_spirit_agent`'s `agentic.workflow` (AW-2, Phase C), and
`ai_spirit_agent` re-exports it under that name for one release.

```python
from aiwatcher_agentic.workflow import Decider, InMemoryEventStore, handle_command
```

## A distribution of its own, depending on nothing

`aiwatcher-sdk` holds two failure policies — telemetry swallows and counts,
the registry clients raise — and imports into an agent's process. The engine
is the third thing an agent imports, and its dependency list is **empty**:
every module here is the standard library, SQLite included. That is why it is
not folded into `aiwatcher-sdk` (whose registry half brings `httpx`) and why
the agent layer that will join it later — a provider stack, MLX on a Mac — is
not allowed to make it heavier. Nothing in `aiwatcher-sdk` imports this.
`aiwatcher_sdk.integrations.agentic.AiwatcherEventStore` implements this
engine's `EventStore` structurally and names its conflict error by module
path, so each side still installs without the other.

## What stayed in `agentic`

`agentic.workflow.builder` and the two LLM reactors in
`agentic.workflow.reactor` wrap a concrete `CoreAgentic`, its `ToolMessage`
and its usage limits, so they stayed with the agent they wrap. Where the
engine needs something from an agent it declares what it reads instead of
importing the type:

| Protocol | Module | Satisfied by |
|---|---|---|
| `WorkflowTracer` — a workflow span and a step span, nothing that calls a model | `tracer` | `agentic.observability.LLMTracer`, and `NoopWorkflowTracer` |
| `AgentRun`, `ToolRun`, `AgentPrompt`, `ModelUsage` — the fields a turn's result becomes messages from | `agent_run` | `agentic`'s `AgentResult`, `ToolRunResult`, `PromptSnapshot`, `RequestUsage` |

`Description`, `TraceSnapshot`, `TracingContext` and `build_trace_snapshot`
are plain values rather than protocols, so they moved outright and
`agentic.metadata` / `agentic.observability` re-export the same objects.

## A wire name does not follow the code

`serialize_record` writes a record's type into it — `__type__`,
`__record_contract__`, the metadata's `contract_name` — from the type's module
path, and stored rows say `agentic.workflow.messages:UserMessage`. Every type
this engine defines keeps that prefix on the wire (`WIRE_PREFIX`), so a record
written now is the same bytes as one written before the move, and an older
reader still recognises it. `tests/fixtures/records_before_the_move.jsonl` is
what the engine wrote before it moved, and the test holds it to those bytes.
A type an application defines is named by its own module path, as before.

## Python 3.13

The floor every distribution here shares. The engine ran only on 3.14 before
it moved, and it called `uuid.uuid7`, which 3.13 does not have — `ids.uuid7`
is the standard library's where there is one and RFC 9562's layout where there
is not. `mypy` runs with `python_version = "3.13"`, which is what found it.

## Checking it

```bash
just agentic-install
just agentic-check   # ruff format --check, ruff check, mypy --strict, pytest
```
