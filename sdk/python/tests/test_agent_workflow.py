"""One agent as a workflow of its own: registered, started, answered — no graph."""

from __future__ import annotations

import json
from dataclasses import asdict
from typing import Any, cast

import httpx
import pytest

from aiwatcher_sdk import AiwatcherClient, NullTransport
from aiwatcher_sdk.conversations import Consent, ConversationArchive, PatternRedactor
from aiwatcher_sdk.integrations.agentic import (
    TURN,
    MemoryPayloadStore,
    agent_workflow,
    digest_of,
)
from aiwatcher_sdk.integrations.agentic.tracer import current_attempt
from aiwatcher_sdk.runtime import ExecutionPool, Runtime
from aiwatcher_sdk.worker import get_task_context
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


def worker_api(workflow: Workflow, parameters: dict[str, Any], *, number: int = 1) -> Any:
    """The server's side of one attempt of the one step."""
    from test_worker import WorkerApi, assignment

    only = only_step(workflow)
    return WorkerApi(
        assignment(
            execution_id="digest-1",
            step_id=TURN,
            attempt=number,
            context_id=f"digest-1/{TURN}/{number}",
            task_ref=only["task_ref"],
            queue=QUEUE,
            params=only["params"],
            parameters=parameters,
            inputs=[],
        )
    )


def attempt(
    workflow: Workflow, parameters: dict[str, Any], *, number: int = 1, api: Any = None
) -> tuple[dict[str, Any], list[Any]]:
    """Run the one step as a worker would, and answer the report it sent."""
    api = api or worker_api(workflow, parameters, number=number)
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
        runtime.run_attempt(f"digest-1/{TURN}/{number}", pool="local")
    [report] = api.reports
    return report, api.requests


class ArchiveServer:
    """The archive's side of `POST /api/v1/conversation-turns`."""

    def __init__(self, status: int = 200, code: str = "") -> None:
        self.status, self.code = status, code
        self.bodies: list[dict[str, Any]] = []
        #: How many reports the worker had sent each time the archive was written.
        self.reports_before: list[int] = []
        self.worker: Any = None

    def handle(self, request: httpx.Request) -> httpx.Response:
        body = json.loads(request.content)
        self.bodies.append(body)
        if self.worker is not None:
            self.reports_before.append(len(self.worker.reports))
        if self.status != 200:
            return httpx.Response(self.status, json={"code": self.code, "message": "no"})
        return httpx.Response(200, json={"turns": body["turns"]})

    def archive(self) -> ConversationArchive:
        return ConversationArchive(
            "http://aiwatcher.invalid",
            redactor=PatternRedactor(),
            consent=Consent(subject="e2e", basis="synthetic", scope=["train"]),
            attempts=1,
            client=httpx.Client(transport=httpx.MockTransport(self.handle)),
        )


def archived(server: ArchiveServer) -> Workflow:
    return agent_workflow(
        "digest", Agent(), version="1", payloads=MemoryPayloadStore(), archive=server.archive()
    )


def test_an_agent_is_a_workflow_of_one_step_with_no_graph_around_it() -> None:
    workflow = agent_workflow("digest", Agent(), version="1", payloads=MemoryPayloadStore())

    step = only_step(workflow)

    assert workflow.to_definition(QUEUE)["name"] == "digest"
    assert step["id"] == TURN
    assert step["task_ref"] == "digest@1"
    assert step["queue"] == QUEUE
    assert step["after"] == [] and step["inputs"] == [] and step["outputs"] == []
    assert len(workflow.get_tasks()) == 1


def test_a_turn_is_delivered_at_least_once_and_an_agent_can_ask_for_fewer() -> None:
    # The server retries an attempt that may not have finished; an agent whose
    # tools cannot be keyed by the step asks for one attempt instead.
    default = agent_workflow("digest", Agent(), version="1", payloads=MemoryPayloadStore())
    once = agent_workflow(
        "digest",
        Agent(),
        version="1",
        payloads=MemoryPayloadStore(),
        retry=RetryPolicy(max_attempts=1),
    )

    assert only_step(default)["retry"] == asdict(RetryPolicy())
    assert RetryPolicy().max_attempts > 1
    assert only_step(once)["retry"]["max_attempts"] == 1


def test_a_step_key_is_the_same_on_every_attempt_where_the_context_id_is_not() -> None:
    seen: list[tuple[str, str, str | None]] = []

    def respond(message: str) -> str:
        context = get_task_context()
        found = current_attempt.get()
        run = found.run_id if found is not None else None
        seen.append((context.context_id, context.step_key, run))
        return "ok"

    workflow = agent_workflow("digest", respond, version="1", payloads=MemoryPayloadStore())
    attempt(workflow, {"message": "hi"}, number=1)
    attempt(workflow, {"message": "hi"}, number=2)

    (first, first_key, run), (second, second_key, _) = seen
    assert first != second
    assert first_key == second_key == f"digest-1/{TURN}"
    # The attempt is visible to a tracer built before it, and only while it runs.
    assert run == "digest-1"
    assert current_attempt.get() is None


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


def test_with_an_archive_a_turn_is_recorded_as_one_exchange_joined_to_its_run() -> None:
    server = ArchiveServer()

    report, _ = attempt(archived(server), {"message": "what changed?"})

    assert report["outcome"] == "completed"
    [body] = server.bodies
    user, assistant = body["turns"]
    assert (user["role"], assistant["role"]) == ("user", "assistant")
    assert user["conversation_id"] == assistant["conversation_id"] == "digest-1"
    assert (user["message_id"], assistant["message_id"]) == (
        f"digest-1.{TURN}.user",
        f"digest-1.{TURN}.assistant",
    )
    assert assistant["parent_message_id"] == user["message_id"]
    assert user["content"]["parts"] == [{"kind": "text", "text": "what changed?"}]
    assert assistant["content"]["parts"] == [{"kind": "text", "text": "you said: what changed?"}]
    assert assistant["provenance"] == {
        "run_id": "digest-1",
        "agent_id": "digest",
        "trace_id": "ab" * 16,
        "span_id": "cd" * 8,
    }
    assert assistant["policy"]["consent"]["scope"] == ["train"]


def test_the_exchange_is_in_the_archive_before_the_step_reports() -> None:
    server = ArchiveServer()
    workflow = archived(server)
    server.worker = worker_api(workflow, {"message": "hi"})

    attempt(workflow, {"message": "hi"}, api=server.worker)

    assert server.reports_before == [0]
    assert len(server.worker.reports) == 1


def test_a_retried_turn_files_its_exchange_under_the_ids_the_first_attempt_used() -> None:
    # A retry overwrites the exchange it wrote rather than adding a second one.
    server = ArchiveServer()
    workflow = archived(server)

    attempt(workflow, {"message": "hi"}, number=1)
    attempt(workflow, {"message": "hi"}, number=2)

    first, second = ([turn["message_id"] for turn in body["turns"]] for body in server.bodies)
    assert first == second == [f"digest-1.{TURN}.user", f"digest-1.{TURN}.assistant"]


@pytest.mark.parametrize(
    ("status", "code", "classification"),
    [
        (503, "unavailable", "infrastructure"),
        (501, "registry_disabled", "policy"),
        (422, "turn_rejected", "policy"),
    ],
)
def test_an_archive_failure_fails_the_turn_and_is_retried_only_when_it_could_pass(
    status: int, code: str, classification: str
) -> None:
    payloads = MemoryPayloadStore()
    workflow = agent_workflow(
        "digest",
        Agent(),
        version="1",
        payloads=payloads,
        archive=ArchiveServer(status, code).archive(),
    )

    report, _ = attempt(workflow, {"message": "hi"})

    assert report["outcome"] == "failed"
    assert report["class"] == classification
    assert "archive" in report["message"]
