import asyncio
from collections.abc import Generator
from contextlib import contextmanager
from typing import Any

import pytest

from aiwatcher_agentic.agent import Agent, AgentResult, Context
from aiwatcher_agentic.message import AssistantMessage
from aiwatcher_agentic.model import ModelResponse
from aiwatcher_agentic.prompts import GemmaPromptBuilder
from aiwatcher_agentic.tools import ToolContext, Toolset, Toolsets


class FakeModel:
    def __init__(self, responses: list[str]):
        self._responses = iter(responses)
        self.prompts: list[str] = []

    def response(self, prompt: str | list[dict[str, str]], **kwargs: Any) -> ModelResponse:
        assert isinstance(prompt, str), "these tests render with Gemma, which gives a string"
        self.prompts.append(prompt)
        return ModelResponse(text=next(self._responses))

    def close(self) -> None:
        pass


class FakeProvider:
    def __init__(self, model: FakeModel):
        self._model = model

    @contextmanager
    def session(self, name: str = "model") -> Generator[FakeModel, None, None]:
        yield self._model


def test_agent_runs_with_flat_tools_without_toolsets() -> None:
    def echo(text: str) -> str:
        return f"ok:{text}"

    model = FakeModel(['{"name":"echo","parameters":{"text":"ping"}}'])
    agent = Agent(
        model_provider=FakeProvider(model),
        prompt_builder=GemmaPromptBuilder(system_prompt="SYSTEM"),
        tools=[echo],
    )

    result = agent.run("hej")

    assert result.tool_call == ("echo", {"text": "ping"})
    assert result.tool_calls == [("echo", {"text": "ping"})]

    run_result = agent.toolsets.run_tool(result.tool_calls[0])
    assert run_result is not None
    assert run_result.output == "ok:ping"


def test_agent_merges_tools_and_toolsets() -> None:
    def first_tool(value: str) -> str:
        return f"first:{value}"

    def second_tool(value: str) -> str:
        return f"second:{value}"

    model = FakeModel(['{"name":"second_tool","parameters":{"value":"x"}}'])
    agent = Agent(
        model_provider=FakeProvider(model),
        prompt_builder=GemmaPromptBuilder(system_prompt="SYSTEM"),
        tools=[first_tool],
        toolsets=Toolsets([Toolset([second_tool])]),
    )

    result = agent.run("hej")

    assert result.tool_call == ("second_tool", {"value": "x"})
    assert result.tool_calls == [("second_tool", {"value": "x"})]

    run_result = agent.toolsets.run_tool(result.tool_calls[0])
    assert run_result is not None
    assert run_result.output == "second:x"


def test_agent_fails_fast_for_duplicate_tool_names() -> None:
    def ping() -> str:
        return "pong"

    with pytest.raises(ValueError, match="Duplicate tool names found: ping"):
        Agent(
            model_provider=FakeProvider(FakeModel([])),
            prompt_builder=GemmaPromptBuilder(system_prompt="SYSTEM"),
            tools=[ping],
            toolsets=Toolsets([Toolset([ping])]),
        )


def test_tool_context_is_forwarded_through_run_tool() -> None:
    captured_contexts: list[ToolContext] = []

    def capture(value: str, tool_context: ToolContext | None = None) -> str:
        assert tool_context is not None
        captured_contexts.append(tool_context)
        return f"{tool_context.agent_id}:{tool_context.track_id}:{value}"

    model = FakeModel(
        [
            '{"name":"capture","parameters":{"value":"first"}}',
            '{"name":"capture","parameters":{"value":"second"}}',
        ]
    )
    agent = Agent(
        model_provider=FakeProvider(model),
        prompt_builder=GemmaPromptBuilder(system_prompt="SYSTEM"),
        tools=[capture],
    )

    ctx1 = ToolContext(agent_id="agent-1", track_id="track-1")
    ctx2 = ToolContext(agent_id="agent-1", track_id="track-2")

    first = agent.run("hej")
    first_result = agent.toolsets.run_tool(first.tool_calls[0], tool_context=ctx1)
    second = agent.run("hej")
    second_result = agent.toolsets.run_tool(second.tool_calls[0], tool_context=ctx2)

    assert first_result is not None
    assert second_result is not None
    assert first_result.output.endswith(":first")
    assert second_result.output.endswith(":second")
    assert len(captured_contexts) == 2
    assert captured_contexts[0].agent_id == captured_contexts[1].agent_id
    assert captured_contexts[0].track_id != captured_contexts[1].track_id


def test_tool_without_tool_context_parameter_still_executes() -> None:
    def upper(value: str) -> str:
        return value.upper()

    model = FakeModel(['{"name":"upper","parameters":{"value":"abc"}}'])
    agent = Agent(
        model_provider=FakeProvider(model),
        prompt_builder=GemmaPromptBuilder(system_prompt="SYSTEM"),
        tools=[upper],
    )

    result = agent.run("hej")
    run_result = agent.toolsets.run_tool(result.tool_calls[0])

    assert run_result is not None
    assert run_result.output == "ABC"


def test_tool_context_is_hidden_from_prompt_tool_schema() -> None:
    def with_context(name: str, tool_context: ToolContext | None = None) -> str:
        return name

    prompt = GemmaPromptBuilder(system_prompt="{tools}")
    rendered = prompt.build_prompt("hej", toolsets=Toolsets.from_sources(tools=[with_context]))

    assert isinstance(rendered, str)
    assert "with_context" in rendered
    assert "tool_context" not in rendered
    assert '"name"' in rendered


def test_context_can_disable_history_in_prompt() -> None:
    model = FakeModel(["plain response"])
    agent = Agent(
        model_provider=FakeProvider(model),
        prompt_builder=GemmaPromptBuilder(system_prompt="SYSTEM"),
        context=Context(add_history_to_context=False),
    )
    agent.history.add(AssistantMessage("OLD HISTORY"))

    _ = agent.run("new message")
    prompt = model.prompts[0]

    assert "OLD HISTORY" not in prompt
    assert "<start_of_turn>user\nnew message\n<end_of_turn>" in prompt


def test_arun_returns_agent_result() -> None:
    def echo(text: str) -> str:
        return f"ok:{text}"

    model = FakeModel(['{"name":"echo","parameters":{"text":"ping"}}'])
    agent = Agent(
        model_provider=FakeProvider(model),
        prompt_builder=GemmaPromptBuilder(system_prompt="SYSTEM"),
        tools=[echo],
    )

    async def _run() -> None:
        result = await agent.arun("hej")
        assert isinstance(result, AgentResult)
        assert result.tool_calls == [("echo", {"text": "ping"})]

    asyncio.run(_run())


def test_astream_yields_result_event() -> None:
    model = FakeModel(["plain response"])
    agent = Agent(
        model_provider=FakeProvider(model),
        prompt_builder=GemmaPromptBuilder(system_prompt="SYSTEM"),
    )

    async def _run() -> None:
        events = [event async for event in agent.astream("hej")]
        assert len(events) == 1
        assert events[0]["type"] == "result"
        assert isinstance(events[0]["result"], AgentResult)
        assert events[0]["result"].content == "plain response"

    asyncio.run(_run())
