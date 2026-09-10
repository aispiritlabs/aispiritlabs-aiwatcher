"""One agent as a workflow of its own: registered, started, answered — no graph."""

from __future__ import annotations

import json
from typing import Any, cast

import httpx
import pytest

from aiwatcher_sdk import AiwatcherClient, NullTransport
from aiwatcher_sdk.integrations.agentic import (
    TURN,
    MemoryPayloadStore,
    agent_workflow,
    digest_of,
)
from aiwatcher_sdk.runtime import ExecutionPool, Runtime
from aiwatcher_sdk.workflow import RetryPolicy, Workflow

QUEUE = "agents"


class Agent:
    """An agent that says what it was asked, so a test can see which message won."""

    def __init__(self, reply: object = None) -> None:
        self.asked: list[str] = []
        self._reply = reply

    def __call__(self, message: str) -> Any:
        self.asked.append(message)
        return f"you said: {message}" if self._reply is None else self._reply


def only_step(workflow: Workflow) -> dict[str, Any]:
    steps = workflow.to_definition(QUEUE)["steps"]
    assert isinstance(steps, list) and len(steps) == 1
    assert isinstance(steps[0], dict)
    return cast(dict[str, Any], steps[0])


def attempt(workflow: Workflow, parameters: dict[str, Any]) -> tuple[dict[str, Any], list[Any]]:
    """Run the one step as a worker would, and answer the report it sent."""
    from test_worker import WorkerApi, assignment

    only = only_step(workflow)
    api = WorkerApi(
        assignment(
            execution_id="digest-1",
            step_id=TURN,
            context_id=f"digest-1/{TURN}/1",
            task_ref=only["task_ref"],
            queue=QUEUE,
            params=only["params"],
            parameters=parameters,
            inputs=[],
        )
    )
    with httpx.Client(transport=httpx.MockTransport(api.handle)) as client:
        runtime = Runtime(
            name="agents",
            url="http://aiwatcher.invalid",
            workflows=[workflow],
            pools=[ExecutionPool("local", QUEUE)],
            placement={workflow.ref: "local"},
            client=client,
            telemetry=AiwatcherClient(service="test", transport=NullTransport()),
        )
        runtime.run_attempt(f"digest-1/{TURN}/1", pool="local")
    [report] = api.reports
    return report, api.requests


def test_an_agent_is_a_workflow_of_one_step_with_no_graph_around_it() -> None:
    workflow = agent_workflow("digest", Agent(), version="1", payloads=MemoryPayloadStore())

    step = only_step(workflow)

    assert workflow.to_definition(QUEUE)["name"] == "digest"
    assert step["id"] == TURN
    assert step["task_ref"] == "digest@1"
    assert step["queue"] == QUEUE
    assert step["after"] == [] and step["inputs"] == [] and step["outputs"] == []
    assert len(workflow.get_tasks()) == 1


def test_a_turn_is_tried_once_because_a_retry_would_repeat_what_its_tools_wrote() -> None:
    once = agent_workflow("digest", Agent(), version="1", payloads=MemoryPayloadStore())
    reads_only = agent_workflow(
        "digest",
        Agent(),
        version="1",
        payloads=MemoryPayloadStore(),
        retry=RetryPolicy(max_attempts=3),
    )

    assert only_step(once)["retry"]["max_attempts"] == 1
    assert only_step(reads_only)["retry"]["max_attempts"] == 3


def test_a_turn_answers_and_the_result_carries_a_reference_rather_than_the_words() -> None:
    payloads = MemoryPayloadStore()
    agent = Agent()
    workflow = agent_workflow("digest", agent, version="1", payloads=payloads)

    report, requests = attempt(workflow, {"message": "what changed overnight?"})

    assert agent.asked == ["what changed overnight?"]
    assert report["outcome"] == "completed"
    reply = report["result"]["reply"]
    assert report["result"]["agent"] == "digest"
    assert payloads.get_payload(reply["reference"]) == "you said: what changed overnight?"
    assert reply["digest"] == digest_of("you said: what changed overnight?")
    assert reply["size"] == len(json.dumps("you said: what changed overnight?"))
    # The result is what lands on the execution's stream and its projection.
    # The reply is a completion, so nothing sent to aiwatcher may hold it.
    assert not any(b"you said" in request.content for request in requests)


def test_a_run_started_with_no_message_is_refused_as_a_validation_failure() -> None:
    agent = Agent()
    workflow = agent_workflow("digest", agent, version="1", payloads=MemoryPayloadStore())

    report, _ = attempt(workflow, {})

    assert report["outcome"] == "failed"
    assert report["class"] == "validation"
    assert "default_message" in report["message"]
    assert agent.asked == []


def test_a_scheduled_run_answers_the_message_the_agent_was_registered_with() -> None:
    # A schedule starts a run with no parameters, so the instruction it runs
    # on is part of the definition.
    agent = Agent()
    workflow = agent_workflow(
        "digest",
        agent,
        version="1",
        payloads=MemoryPayloadStore(),
        default_message="summarise yesterday",
    )

    report, _ = attempt(workflow, {})

    assert report["outcome"] == "completed"
    assert agent.asked == ["summarise yesterday"]


def test_a_run_s_own_message_wins_over_the_registered_default() -> None:
    agent = Agent()
    workflow = agent_workflow(
        "digest",
        agent,
        version="1",
        payloads=MemoryPayloadStore(),
        default_message="summarise yesterday",
    )

    attempt(workflow, {"message": "summarise last week"})

    assert agent.asked == ["summarise last week"]


def test_a_changed_default_is_a_new_revision_not_the_same_definition() -> None:
    nine = agent_workflow(
        "digest", Agent(), version="1", payloads=MemoryPayloadStore(), default_message="a"
    )
    ten = agent_workflow(
        "digest", Agent(), version="1", payloads=MemoryPayloadStore(), default_message="b"
    )

    assert nine.to_definition(QUEUE) != ten.to_definition(QUEUE)


def test_an_agent_that_answers_with_something_other_than_text_fails_its_turn() -> None:
    workflow = agent_workflow("digest", Agent(reply=42), version="1", payloads=MemoryPayloadStore())

    report, _ = attempt(workflow, {"message": "hello"})

    assert report["outcome"] == "failed"
    assert report["class"] == "user_code"
    assert "int" in report["message"]


@pytest.mark.parametrize("parameters", [{"message": "hi", "user": "ana"}])
def test_a_parameter_the_turn_does_not_take_is_refused_rather_than_dropped(
    parameters: dict[str, Any],
) -> None:
    agent = Agent()
    workflow = agent_workflow("digest", agent, version="1", payloads=MemoryPayloadStore())

    report, _ = attempt(workflow, parameters)

    assert report["outcome"] == "failed"
    assert report["class"] == "validation"
    assert agent.asked == []
