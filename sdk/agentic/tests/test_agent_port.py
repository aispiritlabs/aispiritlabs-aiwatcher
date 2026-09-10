"""The agent core against the ports it is written to.

Half of this is checked by `mypy` rather than by pytest: an assignment to a
protocol-typed name is the claim that a concrete type satisfies it, and the
engine's `AgentRun` was written to be satisfied by an `AgentResult` it could
not import.
"""

from __future__ import annotations

import subprocess
import sys
from collections.abc import Generator
from contextlib import contextmanager
from typing import Any

import pytest

from aiwatcher_agentic.agent import Agent, AgentResult, PromptSnapshot
from aiwatcher_agentic.model import ModelResponse, ModelSource, TextModel
from aiwatcher_agentic.prompts import ChatPromptBuilder
from aiwatcher_agentic.tools import ToolRunResult
from aiwatcher_agentic.tracer import LLMTracer, NoopLLMTracer
from aiwatcher_agentic.usage import RequestUsage
from aiwatcher_agentic.workflow.agent_run import AgentPrompt, AgentRun, ModelUsage, ToolRun
from aiwatcher_agentic.workflow.tracer import WorkflowTracer


class Pong:
    def __init__(self) -> None:
        self.prompts: list[str | list[dict[str, str]]] = []

    def response(self, prompt: str | list[dict[str, str]], **kwargs: Any) -> ModelResponse:
        self.prompts.append(prompt)
        return ModelResponse(text="pong", model="pong", total_tokens=5)

    def close(self) -> None:
        pass


class Lends:
    def __init__(self, model: TextModel | None) -> None:
        self._model = model

    @contextmanager
    def session(self, name: str = "model") -> Generator[TextModel | None, None, None]:
        yield self._model


class LendsNothingAndSaysWhy(Lends):
    def get_load_error(self, name: str = "model") -> str | None:
        return "no weights at the path configured"


def test_an_agent_runs_on_anything_that_lends_it_a_model() -> None:
    model = Pong()
    source: ModelSource = Lends(model)

    result = Agent(source, prompt_builder=ChatPromptBuilder(system_prompt="SYSTEM")).run("ping")

    assert result.content_text == "pong"
    assert result.request_usage.total_tokens == 5
    assert "ping" in str(model.prompts[-1])


def test_a_model_that_did_not_load_is_refused_with_the_source_s_reason() -> None:
    agent = Agent(LendsNothingAndSaysWhy(None), prompt_builder=ChatPromptBuilder(system_prompt="S"))

    with pytest.raises(RuntimeError, match="no weights at the path configured"):
        agent.run("ping")


def test_a_source_that_cannot_say_why_still_refuses() -> None:
    agent = Agent(Lends(None), prompt_builder=ChatPromptBuilder(system_prompt="S"))

    with pytest.raises(RuntimeError, match="not available for inference"):
        agent.run("ping")


def test_what_an_agent_returns_is_what_the_engine_reads() -> None:
    usage = RequestUsage(total_tokens=5, model="pong")
    snapshot = PromptSnapshot(text="SYSTEM", prompt_hash="h")
    result = AgentResult(
        "pong",
        tool_calls=[("search", {"q": "x"})],
        request_usage=usage,
        prompt_snapshot=snapshot,
        run_id="r-1",
    )

    run: AgentRun = result
    prompt: AgentPrompt = snapshot
    cost: ModelUsage = usage
    tool: ToolRun = ToolRunResult(tool_call=("search", {"q": "x"}), output="found")

    assert run.run_id == "r-1"
    assert list(run.tool_calls) == [("search", {"q": "x"})]
    assert prompt.prompt_hash == "h"
    assert cost.total_tokens == 5
    assert tool.output == "found"


def test_an_agent_s_tracer_is_one_the_engine_can_use() -> None:
    tracer: LLMTracer = NoopLLMTracer()
    engine: WorkflowTracer = tracer

    with engine.step(name="step") as span:
        span.update(output={"ok": True})

    assert tracer.llm(
        name="call", model="m", messages=[], invoke=lambda: ModelResponse(text="x")
    ).text == ("x")


MODEL_STACK = {
    "agentic",
    "aiwatcher_sdk",
    "httpx",
    "mlflow",
    "mlx",
    "openai",
    "providers",
    "pydantic",
    "registry",
    "torch",
    "transformers",
}


def test_importing_the_agent_core_imports_no_model_stack() -> None:
    probe = (
        "import sys, aiwatcher_agentic.agent, aiwatcher_agentic.tools, aiwatcher_agentic.prompts\n"
        f"print(' '.join(sorted({{m.split('.')[0] for m in sys.modules}} & {MODEL_STACK!r})))"
    )

    # A fresh interpreter, because this one has imported whatever the other tests needed.
    imported = subprocess.run(  # noqa: S603 - our own interpreter, running our own probe
        [sys.executable, "-c", probe], capture_output=True, text=True, check=True
    )

    assert imported.stdout.split() == []
