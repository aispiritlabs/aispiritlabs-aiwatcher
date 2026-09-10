# aiwatcher-agentic

What an agent is built from, in two layers, both moved here from
`ai_spirit_agent` (AW-2):

- **`aiwatcher_agentic.workflow`** — the durable workflow engine: messages,
  deciders, event stores, processors, sagas, and the consumer and turn loops
  that drive them. It was `agentic.workflow` (Phase C).
- **the agent core** — `Agent`, tools and toolsets, chat messages, prompt
  builders, structured output, usage limits and the retry policy. It was the
  rest of `agentic`'s top level.

`ai_spirit_agent` re-exports both under their old names for one release, as
the same module objects rather than copies.

```python
from aiwatcher_agentic.agent import Agent
from aiwatcher_agentic.workflow import Decider, InMemoryEventStore, handle_command
```

## A distribution of its own, carrying no model stack

`aiwatcher-sdk` holds two failure policies — telemetry swallows and counts,
the registry clients raise — and imports into an agent's process. This is the
third thing an agent imports. The engine is the standard library and nothing
else, SQLite included; the core adds `structlog` and `orjson`, which it logged
and parsed tool calls with before it moved. What neither brings is a model:
`test_agent_port` imports the core in a fresh interpreter and fails if
`providers`, MLflow, torch, transformers, `httpx` or pydantic came with it.
Nothing in `aiwatcher-sdk` imports this.
`aiwatcher_sdk.integrations.agentic.AiwatcherEventStore` implements the
engine's `EventStore` structurally and names its conflict error by module
path, so each side still installs without the other.

## Ports, and what stayed in `ai_spirit_agent`

Where this code needs something that would bring a stack with it, it declares
what it calls instead of importing the thing:

| Port | Module | Satisfied by |
|---|---|---|
| `ModelSource` lends a model for one unit of work; `TextModel` answers a prompt with a `ModelResponse` | `model` | `providers.ModelProvider` and its adapters — MLX, vLLM, SGLang, OpenAI-compatible, ONNX |
| `LLMTracer` — workflow, agent and step spans, and a model call | `tracer` | `agentic.observability.MlflowLLMTracer`, `aiwatcher_sdk.integrations.agentic.AiwatcherTracer`, `NoopLLMTracer` |
| `PromptSource` — a prompt's text by name, for `external_prompt_name` | `prompts` | the MLflow prompt registry, which `agentic` names with `use_prompt_source` when it is imported |
| `WorkflowTracer` — the engine's half of `LLMTracer` | `workflow.tracer` | any `LLMTracer` |
| `AgentRun`, `ToolRun`, `AgentPrompt`, `ModelUsage` — what the engine reads off a turn | `workflow.agent_run` | `AgentResult`, `ToolRunResult`, `PromptSnapshot`, `RequestUsage` |

The last row was written while the engine could not import the types it
describes; with both layers here, `test_agent_port` holds it to `mypy`.

What wraps a concrete model stack stayed with the application that chooses
one: `CoreAgentic`, `LLMCall` and `VLMCall` build a `providers.ModelProvider`,
the LLM reactors and `WorkflowBuilder` wrap `CoreAgentic`, and
`agentic.observability` keeps the MLflow tracer. The application's own agents,
search integrations, voice and git tracer stay too. `ModelResponse` and
`TextModel` moved outright, and `providers` re-exports the same objects for
the adapters that build them.

`tools.build_hf_json_repairer` is the one place that reaches for a model. It
imports transformers when it is called, never at import, and installs nothing.

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

The floor every distribution here shares. Both layers ran only on 3.14 before
they moved. The engine called `uuid.uuid7`, which 3.13 does not have — `ids.uuid7`
is the standard library's where there is one and RFC 9562's layout where there
is not. `mypy` runs with `python_version = "3.13"`, which is what found it.

## Checking it

```bash
just agentic-install
just agentic-check   # ruff format --check, ruff check, mypy --strict, pytest
```
