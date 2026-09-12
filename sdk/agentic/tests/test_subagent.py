"""What a subagent does with its two steps, and what it refuses to be declared as.

The model is scripted and records every request it was sent, because the whole
point of the type is *what reaches the endpoint*: the tool schema nobody wrote
twice, the tool result that arrives as a turn rather than as pasted text, and
the one step that withholds the tools so the run ends in an answer.
"""

from __future__ import annotations

from collections.abc import Generator
from contextlib import contextmanager
from typing import Any

import pytest
from pydantic import BaseModel

from aiwatcher_agentic.model import ModelResponse
from aiwatcher_agentic.prompts import GemmaPromptBuilder
from aiwatcher_agentic.structured_output import PydanticOutput
from aiwatcher_agentic.subagent import Subagent, ToolRun
from aiwatcher_agentic.usage import UsageLimits


class Price(BaseModel):
    price: int


def lookup(query: str) -> str:
    """Look a product up in the catalogue."""
    return f"found:{query}"


def explode(query: str) -> str:
    """Fail the way a tool fails."""
    raise RuntimeError(f"no catalogue for {query}")


class ScriptedModel:
    _model_name = "scripted"

    def __init__(self, *replies: str) -> None:
        self._replies = list(replies)
        self.requests: list[tuple[Any, dict[str, Any]]] = []

    def response(self, prompt: str | list[dict[str, str]], **kwargs: Any) -> ModelResponse:
        self.requests.append((prompt, kwargs))
        if not self._replies:
            raise AssertionError("the subagent asked for more turns than the script has")
        return ModelResponse(text=self._replies.pop(0), prompt_tokens=7, completion_tokens=3)

    def close(self) -> None:
        pass


class ScriptedProvider:
    _model_name = "scripted"

    def __init__(self, model: ScriptedModel) -> None:
        self._model = model

    @contextmanager
    def session(self, name: str = "model") -> Generator[ScriptedModel, None, None]:
        yield self._model


def catalog(model: ScriptedModel, **kwargs: Any) -> Subagent[Price]:
    return Subagent(
        "catalog",
        ScriptedProvider(model),
        prompt_builder=GemmaPromptBuilder(system_prompt="Answer from the catalogue."),
        tools=[lookup],
        structured_output=PydanticOutput(Price),
        **kwargs,
    )


def test_the_tool_result_reaches_the_answering_step_as_a_turn_not_as_pasted_text() -> None:
    model = ScriptedModel('{"name":"lookup","parameters":{"query":"beton"}}', '{"price":42}')

    result = catalog(model).run("ile kosztuje beton")

    assert result.content == Price(price=42)
    assert result.tool_runs == (ToolRun(name="lookup", output="found:beton", succeeded=True),)
    assert result.steps == 2
    answering_prompt = str(model.requests[1][0])
    assert "<tool_call_response>found:beton" in answering_prompt


def test_the_agents_own_tools_are_sent_to_the_model_and_withheld_on_the_last_step() -> None:
    model = ScriptedModel('{"name":"lookup","parameters":{"query":"beton"}}', '{"price":42}')

    catalog(model).run("ile kosztuje beton")

    asking, answering = (kwargs for _, kwargs in model.requests)
    # Nobody wrote this schema down: it is the signature and docstring of
    # `lookup`, which is also the callable the subagent runs.
    assert asking["tools"] == [
        {
            "type": "function",
            "function": {
                "name": "lookup",
                "description": "Look a product up in the catalogue.",
                "parameters": {
                    "type": "object",
                    "properties": {"query": {"type": "string"}},
                    "required": ["query"],
                },
            },
        }
    ]
    # A grammar on the asking step would have made the tool call unreachable.
    assert "response_format" not in asking
    assert "tools" not in answering
    assert answering["response_format"]["type"] == "json_schema"


def test_a_step_that_answers_instead_of_reaching_for_a_tool_is_asked_again_for_the_schema() -> None:
    model = ScriptedModel("I already know this one.", '{"price":7}')

    result = catalog(model).run("ile kosztuje beton")

    assert result.content == Price(price=7)
    assert result.tool_runs == ()
    assert result.steps == 2
    assert "tools" not in model.requests[1][1]


def test_a_tool_that_failed_is_reported_back_and_left_for_the_caller_to_judge() -> None:
    model = ScriptedModel('{"name":"explode","parameters":{"query":"beton"}}', '{"price":0}')
    subagent: Subagent[Price] = Subagent(
        "catalog",
        ScriptedProvider(model),
        prompt_builder=GemmaPromptBuilder(system_prompt="Answer from the catalogue."),
        tools=[explode],
        structured_output=PydanticOutput(Price),
    )

    result = subagent.run("ile kosztuje beton")

    assert [run.succeeded for run in result.tool_runs] == [False]
    assert "no catalogue for beton" in result.tool_runs[0].output
    # The model saw the failure and answered around it; whether that answer is
    # acceptable is not this type's call.
    assert result.content == Price(price=0)


def test_usage_covers_every_step_and_not_only_the_last() -> None:
    model = ScriptedModel('{"name":"lookup","parameters":{"query":"beton"}}', '{"price":42}')

    result = catalog(model).run("ile kosztuje beton")

    assert result.usage.requests == 2
    assert result.usage.input_tokens == 14
    assert result.usage.output_tokens == 6
    assert result.usage.tool_calls == 1


def test_a_subagent_holding_tools_it_could_never_call_is_refused() -> None:
    model = ScriptedModel('{"price":42}')

    with pytest.raises(ValueError, match="no step left that could call a tool"):
        catalog(model, max_steps=1)


def test_a_subagent_without_tools_is_a_single_step() -> None:
    model = ScriptedModel('{"price":42}')
    subagent: Subagent[Price] = Subagent(
        "extract",
        ScriptedProvider(model),
        prompt_builder=GemmaPromptBuilder(system_prompt="Extract."),
        structured_output=PydanticOutput(Price),
        max_steps=1,
    )

    result = subagent.run("ile kosztuje beton")

    assert result.content == Price(price=42)
    assert result.steps == 1
    assert len(model.requests) == 1


def test_the_asking_step_stops_at_the_limit_it_was_given() -> None:
    model = ScriptedModel(
        '{"name":"lookup","parameters":{"query":"a"}}',
        '{"name":"lookup","parameters":{"query":"b"}}',
        '{"price":42}',
    )

    result = catalog(model, max_steps=3, usage_limits=UsageLimits(request_limit=1)).run("beton")

    assert result.steps == 3
    assert [run.output for run in result.tool_runs] == ["found:a", "found:b"]
