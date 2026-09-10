"""An application's agents, hosted on aiwatcher as workflows of their own."""

from __future__ import annotations

from collections.abc import Sequence
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import pytest

pytest.importorskip("aiwatcher_sdk")

from aiwatcher_sdk import AiwatcherClient, NullTransport
from aiwatcher_sdk.integrations.agentic import TURN, MemoryPayloadStore
from aiwatcher_sdk.runtime import Runtime
from aiwatcher_sdk.worker.context import current_context
from aiwatcher_sdk.workflow import RetryPolicy, Workflow

from aiwatcher_agentic.runtime.config import RuntimeConfig
from aiwatcher_agentic.runtime.hosted import DEFAULT_PAYLOAD_ROOT, hosted_runtime, payload_root
from aiwatcher_agentic.runtime.runtime import AgenticRuntime
from aiwatcher_agentic.tracer import NoopLLMTracer
from aiwatcher_agentic.workflow.description import Description


class Router:
    """A router that fails the test if it is asked: a hosted turn is targeted."""

    def route(self, message: str, available_workflows_summary: str) -> str:
        raise AssertionError("a hosted turn names its agent and never asks the router")

    def start(self) -> str:
        return "hello"

    def close(self) -> None:
        pass


class Echo:
    """An agent that says what it was asked, in its own name."""

    def __init__(self, name: str) -> None:
        self.description = Description(name, "says it back", ())
        self.inputs: Sequence[str] = ["UserMessage"]
        self.seen: list[Any] = []
        self.closed = False

    def handle(self, message: Any) -> str:
        self.seen.append(message)
        return f"{self.description.agent_name}: {message.data.text}"

    def close(self) -> None:
        self.closed = True


@pytest.fixture
def agents() -> dict[str, Echo]:
    return {"digest": Echo("digest"), "triage": Echo("triage")}


@pytest.fixture
def app(tmp_path: Path, agents: dict[str, Echo]) -> AgenticRuntime:
    return AgenticRuntime(
        workflows=list(agents.values()),
        router=Router(),
        settings=RuntimeConfig(
            event_store_path=str(tmp_path / "events.db"),
            message_store_path=str(tmp_path / "messages.db"),
        ),
        tracer=NoopLLMTracer(),
    )


def hosting(app: AgenticRuntime, **changes: Any) -> Runtime:
    options: dict[str, Any] = {
        "version": "1",
        "url": "http://aiwatcher.invalid",
        "payloads": MemoryPayloadStore(),
        "telemetry": AiwatcherClient(service="test", transport=NullTransport()),
    }
    return hosted_runtime(app, **(options | changes))


def only_step(workflow: Workflow) -> dict[str, Any]:
    """The one step an agent's workflow has, as its definition names it."""
    steps = workflow.to_definition("agents")["steps"]
    assert isinstance(steps, list)
    [step] = steps
    assert isinstance(step, dict)
    return step


def turn(hosted: Runtime, agent: str, parameters: dict[str, Any]) -> dict[str, Any]:
    """Invoke the one step as a worker does: the run's parameters under the step's."""
    workflow = hosted.workflows[f"{agent}@1"]
    step = only_step(workflow)
    [task] = workflow.get_tasks()
    result = task.invoke(parameters | step["params"])
    assert isinstance(result, dict)
    return result


def test_every_agent_in_the_runtime_is_registered_as_a_workflow_of_its_own(
    app: AgenticRuntime,
) -> None:
    hosted = hosting(app)

    assert sorted(hosted.workflows) == ["digest@1", "triage@1"]
    for workflow in hosted.workflows.values():
        step = only_step(workflow)
        assert step["id"] == TURN
        assert step["after"] == []
    hosted.close()


def test_a_turn_reaches_its_agent_past_the_router_and_its_reply_goes_to_the_payload_store(
    app: AgenticRuntime, agents: dict[str, Echo]
) -> None:
    payloads = MemoryPayloadStore()
    hosted = hosting(app, payloads=payloads)

    result = turn(hosted, "digest", {"message": "what changed?"})

    assert payloads.get_payload(result["reply"]["reference"]) == "digest: what changed?"
    [message] = agents["digest"].seen
    assert message.metadata.target == "digest"
    assert message.metadata.source == "aiwatcher"
    assert agents["triage"].seen == []
    hosted.close()


def test_a_turn_inside_an_attempt_is_named_by_that_attempt(
    app: AgenticRuntime, agents: dict[str, Echo]
) -> None:
    # What joins the agent's own records to the execution that asked for them.
    hosted = hosting(app)
    attempt = SimpleNamespace(
        context_id="digest-7/turn/2", run_id="digest-7", step_key="digest-7/turn"
    )
    token = current_context.set(attempt)  # type: ignore[arg-type]
    try:
        turn(hosted, "digest", {"message": "what changed?"})
    finally:
        current_context.reset(token)

    [message] = agents["digest"].seen
    assert message.metadata.turn_id == "digest-7/turn/2"
    assert message.metadata.correlation_id == "digest-7"
    # The same on the first attempt as on this one: what a tool keys its write
    # by so that the retry does not do it again.
    assert message.metadata.idempotency_key == "digest-7/turn"
    hosted.close()


def test_a_default_message_is_what_a_scheduled_run_of_that_agent_answers(
    app: AgenticRuntime,
) -> None:
    payloads = MemoryPayloadStore()
    hosted = hosting(app, payloads=payloads, default_messages={"digest": "summarise yesterday"})

    result = turn(hosted, "digest", {})

    assert payloads.get_payload(result["reply"]["reference"]) == "digest: summarise yesterday"
    hosted.close()


def test_agents_narrows_which_are_registered(app: AgenticRuntime) -> None:
    hosted = hosting(app, agents=["triage"])

    assert list(hosted.workflows) == ["triage@1"]
    hosted.close()


def test_a_turn_is_delivered_at_least_once_unless_an_agent_s_budget_says_otherwise(
    app: AgenticRuntime,
) -> None:
    hosted = hosting(app, retry={"digest": RetryPolicy(max_attempts=1)})

    def attempts(agent: str) -> int:
        step = only_step(hosted.workflows[f"{agent}@1"])
        return int(step["retry"]["max_attempts"])

    assert attempts("digest") == 1
    assert attempts("triage") == RetryPolicy().max_attempts > 1
    hosted.close()


def test_closing_the_hosted_runtime_closes_the_archive_it_was_handed(
    app: AgenticRuntime,
) -> None:
    archive = SimpleNamespace(closed=False)
    archive.close = lambda: setattr(archive, "closed", True)
    hosted = hosting(app, archive=archive)

    hosted.close()

    assert archive.closed


def test_closing_the_hosted_runtime_closes_the_agent_runtime(
    app: AgenticRuntime, agents: dict[str, Echo]
) -> None:
    hosted = hosting(app)

    hosted.close()

    assert all(agent.closed for agent in agents.values())


@pytest.mark.parametrize(
    ("changes", "refusal"),
    [
        ({"agents": ["nobody"]}, "nobody"),
        ({"default_messages": {"nobody": "hi"}}, "nobody"),
        ({"retry": {"nobody": RetryPolicy()}}, "nobody"),
        ({"url": ""}, "AIWATCHER_URL"),
    ],
)
def test_a_refused_hosting_still_closes_the_runtime_it_was_handed(
    app: AgenticRuntime,
    agents: dict[str, Echo],
    monkeypatch: pytest.MonkeyPatch,
    changes: dict[str, Any],
    refusal: str,
) -> None:
    monkeypatch.delenv("AIWATCHER_URL", raising=False)

    with pytest.raises(ValueError, match=refusal):
        hosting(app, **changes)

    assert all(agent.closed for agent in agents.values())


def test_a_reply_lands_beside_the_agents_data_unless_a_directory_is_named(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    monkeypatch.delenv("AIWATCHER_PAYLOAD_ROOT", raising=False)
    assert payload_root() == DEFAULT_PAYLOAD_ROOT

    monkeypatch.setenv("AIWATCHER_PAYLOAD_ROOT", str(tmp_path))
    assert payload_root() == tmp_path
