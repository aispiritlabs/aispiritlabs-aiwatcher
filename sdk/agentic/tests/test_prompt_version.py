"""A turn names the registry version of the prompt it ran on: the template it read."""

from __future__ import annotations

import hashlib
from collections.abc import Callable, Generator
from contextlib import contextmanager
from typing import Any

from aiwatcher_agentic.agent import Agent
from aiwatcher_agentic.capabilities import AbstractCapability, HookContext
from aiwatcher_agentic.model import ModelResponse, TextModel
from aiwatcher_agentic.prompts import ChatPromptBuilder, PromptBuilder
from aiwatcher_agentic.tracer import NoopLLMTracer

#: What the registry holds. Rendering strips it and fills `{tools}`, so what the
#: model is sent is not this text, and its hash names no version.
TEMPLATE = "  Answer as the sage.\n{tools}\n"
VERSION = hashlib.sha256(TEMPLATE.encode()).hexdigest()


class Pong:
    def response(self, prompt: str | list[dict[str, str]], **kwargs: Any) -> ModelResponse:
        return ModelResponse(text="pong", model="pong")

    def close(self) -> None:
        pass


class Lends:
    @contextmanager
    def session(self, name: str = "model") -> Generator[TextModel | None, None, None]:
        yield Pong()


class Recording(NoopLLMTracer):
    def __init__(self) -> None:
        self.attributes: list[dict[str, Any]] = []

    def llm(
        self,
        *,
        name: str,
        model: str,
        messages: list[dict[str, Any]],
        invoke: Callable[..., ModelResponse],
        **kwargs: Any,
    ) -> ModelResponse:
        self.attributes.append(dict(kwargs.get("extra_attributes") or {}))
        return invoke()


class Instructs(AbstractCapability):
    def get_instructions(self, context: HookContext) -> str | None:
        return "Cite what you read."


def read(name: str) -> str:
    return TEMPLATE


def turn(builder: PromptBuilder, *capabilities: AbstractCapability) -> dict[str, Any]:
    tracer = Recording()
    agent = Agent(Lends(), prompt_builder=builder, tracer=tracer, capabilities=capabilities or None)
    agent.run("ping")
    return tracer.attributes[-1]


def test_a_turn_names_the_version_of_the_template_it_read_not_the_prompt_it_rendered() -> None:
    attributes = turn(ChatPromptBuilder(external_prompt_name="sage", prompt_source=read))

    assert attributes["agentic.prompt_name"] == "sage"
    assert attributes["agentic.prompt_version"] == VERSION
    assert attributes["agentic.prompt_hash"] != VERSION


def test_a_capability_that_touches_the_prompt_keeps_its_version() -> None:
    builder = ChatPromptBuilder(external_prompt_name="sage", prompt_source=read)

    attributes = turn(builder, Instructs())

    assert attributes["agentic.prompt_version"] == VERSION


def test_a_prompt_written_inline_names_no_version() -> None:
    attributes = turn(ChatPromptBuilder(system_prompt=TEMPLATE))

    assert "agentic.prompt_name" not in attributes
    assert "agentic.prompt_version" not in attributes
