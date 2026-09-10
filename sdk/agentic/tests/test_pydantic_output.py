"""A pydantic model as the shape of an answer, and the repair when it is wrong."""

from __future__ import annotations

from collections.abc import Generator
from contextlib import contextmanager
from typing import Any

import pytest
from pydantic import BaseModel

from aiwatcher_agentic.agent import Agent
from aiwatcher_agentic.exceptions import ModelRetry, RetryPolicy
from aiwatcher_agentic.model import ModelResponse
from aiwatcher_agentic.prompts import GemmaPromptBuilder
from aiwatcher_agentic.structured_output import (
    PydanticOutput,
    StructuredOutput,
    StructuredOutputProto,
)


class Point(BaseModel):
    x: float
    y: float


class Wall(BaseModel):
    label: str
    points: list[Point]


class Draft(BaseModel):
    """The shape planner needs: a list of nested models, which the dataclass
    parser cannot express at all."""

    walls: list[Wall]
    scale: float = 1.0


class FakeModel:
    def __init__(self, responses: list[str]) -> None:
        self._responses = iter(responses)
        self.calls: list[tuple[str, dict[str, Any]]] = []

    def response(self, prompt: Any, **kwargs: Any) -> ModelResponse:
        self.calls.append((str(prompt), kwargs))
        return ModelResponse(text=next(self._responses), model="test-model")

    def close(self) -> None:
        pass


class FakeProvider:
    def __init__(self, model: FakeModel) -> None:
        self._model = model

    @contextmanager
    def session(self, name: str = "model") -> Generator[FakeModel, None, None]:
        del name
        yield self._model


def _agent(model: FakeModel, **kwargs: Any) -> Agent:
    return Agent(
        FakeProvider(model),
        prompt_builder=GemmaPromptBuilder(system_prompt="draft it"),
        **kwargs,
    )


GOOD = '{"walls": [{"label": "north", "points": [{"x": 0, "y": 0}, {"x": 1, "y": 0}]}]}'


def test_both_output_kinds_satisfy_what_the_agent_asks_of_them() -> None:
    assert isinstance(StructuredOutput(), StructuredOutputProto)
    assert isinstance(PydanticOutput(Draft), StructuredOutputProto)


def test_a_nested_model_parses_into_the_model_itself() -> None:
    parsed = PydanticOutput(Draft).parse(GOOD)

    assert isinstance(parsed, Draft)
    assert parsed.walls[0].points[1].x == 1.0
    assert parsed.scale == 1.0


def test_a_fenced_answer_is_unwrapped_before_validation() -> None:
    assert PydanticOutput(Draft).parse(f"```json\n{GOOD}\n```").walls[0].label == "north"


def test_a_wrong_field_comes_back_as_a_retry_naming_the_path() -> None:
    with pytest.raises(ModelRetry) as caught:
        PydanticOutput(Draft).parse('{"walls": [{"label": "north", "points": "two"}]}')

    assert "walls.0.points" in str(caught.value)


def test_a_domain_rule_lives_in_the_validator_and_can_ask_for_a_retry() -> None:
    def no_open_walls(draft: Draft) -> Draft:
        if len(draft.walls[0].points) < 3:
            raise ModelRetry("wall 'north' is not closed: it has 2 points")
        return draft

    with pytest.raises(ModelRetry, match="not closed"):
        PydanticOutput(Draft, validator=no_open_walls).parse(GOOD)


def test_a_validator_that_accepts_may_also_rewrite_the_answer() -> None:
    parsed = PydanticOutput(Draft, validator=lambda d: d.model_copy(update={"scale": 50.0})).parse(
        GOOD
    )

    assert parsed.scale == 50.0


class TestSchema:
    def test_the_schema_is_sent_as_a_strict_json_schema_named_for_the_model(self) -> None:
        response_format = PydanticOutput(Draft).response_format()

        assert response_format["type"] == "json_schema"
        assert response_format["json_schema"]["name"] == "Draft"
        assert response_format["json_schema"]["strict"] is True

    def test_every_object_in_a_strict_schema_forbids_extra_properties(self) -> None:
        schema = PydanticOutput(Draft).json_schema()

        assert schema["additionalProperties"] is False
        assert schema["$defs"]["Wall"]["additionalProperties"] is False
        assert schema["$defs"]["Point"]["additionalProperties"] is False

    def test_a_model_that_rejects_strict_schemas_gets_pydantics_own(self) -> None:
        schema = PydanticOutput(Draft, strict=False).json_schema()

        assert "additionalProperties" not in schema
        assert (
            PydanticOutput(Draft, strict=False).response_format()["json_schema"]["strict"] is False
        )

    def test_the_dataclass_parser_asks_for_no_response_format(self) -> None:
        assert StructuredOutput().response_format() is None


class TestThroughTheAgent:
    def test_the_schema_travels_with_every_request(self) -> None:
        model = FakeModel([GOOD])
        result = _agent(model, structured_output=PydanticOutput(Draft)).run("draft it")

        assert isinstance(result.content, Draft)
        assert model.calls[0][1]["response_format"]["json_schema"]["name"] == "Draft"

    def test_a_rejected_answer_is_repaired_in_a_second_request(self) -> None:
        model = FakeModel(['{"walls": "none"}', GOOD])
        agent = _agent(
            model,
            structured_output=PydanticOutput(Draft),
            retry_policy=RetryPolicy(max_retries=2),
        )

        result = agent.run("draft it")

        assert isinstance(result.content, Draft)
        assert len(model.calls) == 2

    def test_keeping_history_shows_the_model_its_own_rejected_answer(self) -> None:
        model = FakeModel(['{"walls": "none"}', GOOD])
        agent = _agent(
            model,
            structured_output=PydanticOutput(Draft),
            retry_policy=RetryPolicy(max_retries=2, keep_history=True),
        )

        agent.run("draft it")

        second_prompt = model.calls[1][0]
        assert '{"walls": "none"}' in second_prompt
        assert "draft it" in second_prompt
        assert "Correct it and answer the original request again." in second_prompt

    def test_the_default_replaces_the_turn_instead_of_keeping_it(self) -> None:
        model = FakeModel(['{"walls": "none"}', GOOD])
        agent = _agent(
            model,
            structured_output=PydanticOutput(Draft),
            retry_policy=RetryPolicy(max_retries=2),
        )

        agent.run("draft it")

        second_prompt = model.calls[1][0]
        assert "Previous response:" in second_prompt
        assert "Correct it and answer the original request again." not in second_prompt

    def test_a_run_that_is_repaired_leaves_only_the_accepted_answer_in_history(self) -> None:
        model = FakeModel(['{"walls": "none"}', GOOD])
        agent = _agent(
            model,
            structured_output=PydanticOutput(Draft),
            retry_policy=RetryPolicy(max_retries=2, keep_history=True),
        )

        agent.run("draft it")

        stored = agent.history.conversation_text(10)
        assert '{"walls": "none"}' not in stored

    def test_a_caller_may_replay_turns_it_owns_without_storing_them(self) -> None:
        from aiwatcher_agentic.message import AssistantMessage, UserMessage

        model = FakeModel([GOOD])
        agent = _agent(model, structured_output=PydanticOutput(Draft))

        agent.run(
            "try again",
            history=[UserMessage("first attempt"), AssistantMessage("a broken draft")],
        )

        assert "a broken draft" in model.calls[0][0]
        assert "a broken draft" not in agent.history.conversation_text(10)
