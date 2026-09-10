# aiwatcher-agentic

What an agent is built from, in three layers, all moved here from
`ai_spirit_agent` (AW-2):

- **`aiwatcher_agentic.workflow`** — the durable workflow engine: messages,
  deciders, event stores, processors, sagas, and the consumer and turn loops
  that drive them. It was `agentic.workflow` (Phase C).
- **the agent core** — `Agent`, tools and toolsets, chat messages, prompt
  builders, structured output, usage limits and the retry policy. It was the
  rest of `agentic`'s top level.
- **`aiwatcher_agentic.runtime`** — where an application composes its agents
  (`AgenticRuntime`), the conversation store and the fine-tuning export read
  from it, the hosted runtime that puts each agent on aiwatcher as a workflow
  of its own, and the distributed transport on which a hop between two agents
  is a run of the target's workflow. It was `agentic_runtime`.

`ai_spirit_agent` re-exports all three under their old names for one release,
as the same module objects rather than copies.

```python
from aiwatcher_agentic.agent import Agent
from aiwatcher_agentic.workflow import Decider, InMemoryEventStore, handle_command
from aiwatcher_agentic.runtime import AgenticRuntime, RuntimeConfig
```

## A distribution of its own, carrying no model stack

`aiwatcher-sdk` holds two failure policies — telemetry swallows and counts,
the registry clients raise — and imports into an agent's process. This is the
third thing an agent imports. The engine is the standard library and nothing
else, SQLite included; the core adds `structlog` and `orjson`, which it logged
and parsed tool calls with before it moved. What neither brings is a model:
`test_agent_port` imports the core in a fresh interpreter and fails if
`providers`, MLflow, torch, transformers, `httpx` or pydantic came with it.
The runtime adds nothing to that list. It talks to aiwatcher through
`aiwatcher-sdk`, and only `hosted` and `distributed.aiwatcher` import it, when
they are used: that is the `[aiwatcher]` extra, and an application whose agents
all run in one process installs nothing. `test_runtime_port` imports the
runtime, `hosted` included, in a fresh interpreter and fails if the SDK, MLflow
or pydantic settings came with it. Nothing in `aiwatcher-sdk` imports this.
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
| `RuntimeSettings` — where the runtime's two stores are, and how it streams | `runtime.config` | `ai_spirit_agent`'s pydantic `Settings`, `RuntimeConfig` |
| `DistributedSettings` — the prefix, and what a delivery may cost | `runtime.config` | the same two |

The last row was written while the engine could not import the types it
describes; with both layers here, `test_agent_port` holds it to `mypy`.

What wraps a concrete model stack stayed with the application that chooses
one: `CoreAgentic`, `LLMCall` and `VLMCall` build a `providers.ModelProvider`,
the LLM reactors and `WorkflowBuilder` wrap `CoreAgentic`, and
`agentic.observability` keeps the MLflow tracer. The application's own agents,
search integrations, voice and git tracer stay too. `ModelResponse` and
`TextModel` moved outright, and `providers` re-exports the same objects for
the adapters that build them.

`AgenticRuntime` is handed its tracer (`tracer=`) and its settings
(`settings=`), and where it used to configure the OpenAI-compatible client it
calls `_configure_providers`, a hook that does nothing here — once the
workflows are registered and before the bus replays anything. The application
subclass that fills all three in stayed as `agentic_runtime.runtime`, with its
settings (read from `.env`, naming its models), its MLflow tracing, its users
and workspaces under `~/.aispiritagent`, and `DistributedAgenticRuntime`, the
chat front end lab 6 puts on the transport.

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
The runtime's two records — `AgentRegistration` and `AgentHeartbeat` — keep
`agentic_runtime` the same way, held by
`tests/fixtures/runtime_records_before_the_move.jsonl`.

## Where a store goes when nobody says

Under the working directory, in `.data/`: `SQLiteEventStore` and
`SQLiteMessageStore` both default there when handed no path. They used to take
the directory from their own file — `parents[4] / "data"` — which named the
application's `data/` while they lived in it, and after the move named the root
of this checkout, or site-packages in a wheel. The engine did that from Phase C
until the runtime moved and it was found. An application that wants its old
directory names it: `ai_spirit_agent`'s runtime passes its own `data/`.

## Python 3.13

The floor every distribution here shares. Both layers ran only on 3.14 before
they moved. The engine called `uuid.uuid7`, which 3.13 does not have — `ids.uuid7`
is the standard library's where there is one and RFC 9562's layout where there
is not. `mypy` runs with `python_version = "3.13"`, which is what found it,
and found `uuid.uuid7` twice more in the runtime. The runtime also wrote
`except OSError, TypeError:` — PEP 758's unparenthesised form, which 3.13 reads
as a syntax error.

## Checking it

```bash
just agentic-install
just agentic-check   # ruff format --check, ruff check, mypy --strict, pytest
```
