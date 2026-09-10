"""Distributed agents on aiwatcher, against a stand-in for the routes they use."""

from __future__ import annotations

import json
import threading
import time
import uuid
from collections.abc import Iterator, Sequence
from typing import Any

import httpx
import pytest

pytest.importorskip("aiwatcher_sdk")

from aiwatcher_sdk import AiwatcherClient, NullTransport
from aiwatcher_sdk.api import ApiError
from aiwatcher_sdk.integrations.agentic import MemoryPayloadStore
from aiwatcher_sdk.task_errors import TaskError
from aiwatcher_sdk.workflow import WorkflowStep

from aiwatcher_agentic.runtime.distributed.aiwatcher import (
    HOP,
    AiwatcherService,
    AiwatcherServiceRegistry,
    AiwatcherTransport,
    UnknownAgentError,
)
from aiwatcher_agentic.runtime.distributed.client import DistributedChatClient
from aiwatcher_agentic.runtime.distributed.discovery import AgenticServiceDiscovery
from aiwatcher_agentic.runtime.distributed.in_memory_transport import (
    InMemoryServiceRegistry,
    InMemoryTransport,
)
from aiwatcher_agentic.runtime.distributed.service import DistributedService, PermanentMessageError
from aiwatcher_agentic.workflow.messages import (
    AssistantMessage,
    ConversationData,
    Message,
    RecordedMessageMetadata,
    TurnCompleted,
    UserMessage,
)

BASE = "http://aiwatcher.test"
#: Somebody's words: the thing that must never reach aiwatcher.
WORDS = "what is the capital of Burkina Faso, and who told you?"


class FakeAiwatcher:
    """The five routes the transport uses, keeping the server's rules for each.

    A start is idempotent by key and definition, a stream append by key, and an
    append naming a version the stream has moved past is a `version_conflict`.
    """

    def __init__(self) -> None:
        self.definitions: dict[str, dict[str, Any]] = {}
        self.runs: dict[str, dict[str, Any]] = {}
        self.keys: dict[tuple[str, str], str] = {}
        self.bodies: list[bytes] = []
        self.conflicts = 0

    def __call__(self, request: httpx.Request) -> httpx.Response:
        self.bodies.append(request.content)
        path = request.url.path
        body = json.loads(request.content) if request.content else {}
        if path == "/api/v1/workflow-definitions":
            if request.method == "GET":
                return httpx.Response(200, json=list(self.definitions.values()))
            self.definitions[body["name"]] = {
                "definition": body,
                "revision": f"rev-{body['name']}",
                "registered_by": "test",
                "registered_at": "2026-09-10T12:00:00Z",
            }
            return httpx.Response(200, json=self.definitions[body["name"]])
        if path == "/api/v1/executions":
            return self._start(request, body)
        _, _, _, _, execution, route = path.split("/")
        run = self.runs[execution]
        if route == "history":
            after = int(request.url.params.get("after", 0))
            limit = int(request.url.params.get("limit", 100))
            rows = run["rows"]
            page = [row for row in rows if row["stream_version"] > after][:limit]
            more = bool(page) and page[-1]["stream_version"] < len(rows)
            return httpx.Response(
                200,
                json={
                    "version": len(rows),
                    "messages": page,
                    "next_after": page[-1]["stream_version"] if more else None,
                },
            )
        return self._append(request, run, body)

    def _start(self, request: httpx.Request, body: dict[str, Any]) -> httpx.Response:
        name = body["target"]["name"]
        if name not in self.definitions:
            return httpx.Response(
                404, json={"code": "not_found", "message": f"workflow definition {name}"}
            )
        key = request.headers["idempotency-key"]
        created = (name, key) not in self.keys
        execution = self.keys.setdefault((name, key), uuid.uuid4().hex)
        if created:
            self.runs[execution] = {
                "name": name,
                "parameters": body.get("parameters", {}),
                "hosted": body.get("decided_by") == "worker",
                "rows": [{"stream_version": 1, "message": {"kind": "event"}}],
                "appended": {},
            }
        return httpx.Response(
            202, json={"execution": {"execution_id": execution}, "created": created}
        )

    def _append(
        self, request: httpx.Request, run: dict[str, Any], body: dict[str, Any]
    ) -> httpx.Response:
        key = request.headers["idempotency-key"]
        rows = run["rows"]
        if key in run["appended"]:
            return httpx.Response(200, json={"version": run["appended"][key]})
        if body["expected_version"] != len(rows):
            return httpx.Response(409, json={"code": "version_conflict", "message": "moved"})
        rows.append(
            {
                "stream_version": len(rows) + 1,
                "message": {"kind": "hosted", "message_type": "HostedAppend"},
            }
        )
        for message in body["messages"]:
            rows.append({"stream_version": len(rows) + 1, "message": {"kind": "hosted", **message}})
        run["appended"][key] = len(rows)
        return httpx.Response(200, json={"version": len(rows)})

    def runs_of(self, name: str) -> list[dict[str, Any]]:
        return [run for run in self.runs.values() if run["name"] == name]


@pytest.fixture
def server() -> FakeAiwatcher:
    return FakeAiwatcher()


@pytest.fixture
def payloads() -> MemoryPayloadStore:
    return MemoryPayloadStore()


def wire_to(server: FakeAiwatcher) -> httpx.Client:
    return httpx.Client(transport=httpx.MockTransport(server))


@pytest.fixture
def transport(server: FakeAiwatcher, payloads: MemoryPayloadStore) -> Iterator[AiwatcherTransport]:
    opened = AiwatcherTransport(
        BASE, prefix="lab", payloads=payloads, client=wire_to(server), poll_seconds=0.01
    )
    yield opened
    opened.close()


type Handler = Any


def serving(
    transport: AiwatcherTransport,
    server: FakeAiwatcher,
    agent: str,
    handler: Handler,
    *,
    capabilities: tuple[str, ...] = (),
    attempts: int = 3,
) -> AiwatcherService:
    service = AiwatcherService(
        agent_name=agent,
        capabilities=capabilities,
        discovery=AgenticServiceDiscovery(transport, AiwatcherServiceRegistry(transport)),
        handler=handler,
        transport=transport,
        max_delivery_attempts=attempts,
        client=wire_to(server),
        telemetry=AiwatcherClient(service="test", transport=NullTransport()),
    )
    service.register()
    return service


def deliver(service: AiwatcherService, run: dict[str, Any]) -> Any:
    """What a worker does with a claimed hop: the run's parameters over the step's."""
    (step,) = service.workflow.steps
    assert isinstance(step, WorkflowStep)
    return step.task.invoke(dict(run["parameters"]) | dict(step.params))


def asked(text: str = WORDS, *, message_id: str = "", target: str = "planner") -> UserMessage:
    return UserMessage(
        data=ConversationData(role="user", text=text),
        metadata=RecordedMessageMetadata(
            runtime_id="runtime-1",
            turn_id="turn-1",
            domain="lab",
            source="mailbox:chat",
            target=target,
            message_id=message_id,
        ),
    )


def answer_to(message: Message, text: str, *, target: str) -> AssistantMessage:
    return AssistantMessage(
        data=ConversationData(role="assistant", text=text),
        metadata=RecordedMessageMetadata(
            turn_id=message.metadata.turn_id, source="planner", target=target
        ),
    )


def ignore(message: Message, discovery: Any) -> Sequence[Message]:
    return ()


# ── Sending ──────────────────────────────────────────────────────────────


def test_a_hop_is_a_run_of_the_target_agents_workflow_started_under_the_message_id(
    transport: AiwatcherTransport, server: FakeAiwatcher
) -> None:
    serving(transport, server, "planner", ignore, capabilities=("plan",))

    execution = transport.publish_message(asked(message_id="m-1"))

    run = server.runs[execution]
    assert run["name"] == "lab.planner"
    assert server.keys[("lab.planner", "m-1")] == execution
    hop = run["parameters"]["hop"]
    assert (hop["message_id"], hop["to"], hop["from"]) == ("m-1", "planner", "mailbox:chat")
    assert {"reference", "digest", "size"} <= set(hop)


def test_a_message_sent_twice_is_one_run(
    transport: AiwatcherTransport, server: FakeAiwatcher
) -> None:
    serving(transport, server, "planner", ignore)

    first = transport.publish_message(asked(message_id="m-1"))
    again = transport.publish_message(asked(message_id="m-1"))

    assert first == again
    assert len(server.runs_of("lab.planner")) == 1


def test_the_words_go_to_the_payload_store_and_never_to_aiwatcher(
    transport: AiwatcherTransport, server: FakeAiwatcher, payloads: MemoryPayloadStore
) -> None:
    serving(transport, server, "planner", ignore)

    transport.publish_message(asked(message_id="m-1"))
    transport.publish_message(answer_to(asked(), WORDS, target="mailbox:chat"))

    assert not [body for body in server.bodies if WORDS.encode() in body]
    assert any(WORDS in json.dumps(stored) for stored in payloads._payloads.values())


def test_a_hop_to_an_agent_nobody_registered_is_refused_by_name(
    transport: AiwatcherTransport,
) -> None:
    with pytest.raises(UnknownAgentError, match=r"`planner`.*`lab\.planner`"):
        transport.publish_message(asked())


def test_a_mailbox_address_says_what_it_is_and_no_agent_may_take_its_name(
    transport: AiwatcherTransport,
) -> None:
    assert transport.reply_address("chat") == "mailbox:chat"
    assert transport.reply_address("mailbox:chat") == "mailbox:chat"
    with pytest.raises(ValueError, match="reserved"):
        transport.workflow_name("mailbox")
    with pytest.raises(ValueError, match="letters, digits"):
        transport.workflow_name("../planner")


# ── A reply ──────────────────────────────────────────────────────────────


def test_a_reply_lands_in_the_clients_mailbox_and_is_read_from_a_cursor(
    transport: AiwatcherTransport,
) -> None:
    cursor = transport.last_message_id("chat")

    transport.publish_message(answer_to(asked(), "Ouagadougou", target="mailbox:chat"))
    (received,) = transport.read_messages("chat", after_id=cursor, block_ms=0)

    assert isinstance(received.record, AssistantMessage)
    assert received.record.data.text == "Ouagadougou"
    assert transport.read_messages("chat", after_id=received.entry_id, block_ms=0) == []


def test_every_process_finds_the_same_mailbox_without_being_told_its_id(
    transport: AiwatcherTransport, server: FakeAiwatcher, payloads: MemoryPayloadStore
) -> None:
    other = AiwatcherTransport(BASE, prefix="lab", payloads=payloads, client=wire_to(server))
    transport.publish_message(answer_to(asked(), "Ouagadougou", target="mailbox:chat"))

    (received,) = other.read_messages("chat", block_ms=0)

    assert isinstance(received.record, AssistantMessage)
    assert received.record.data.text == "Ouagadougou"
    assert len([run for run in server.runs.values() if run["hosted"]]) == 1


def test_a_reply_that_lost_a_race_for_the_version_is_appended_once(
    transport: AiwatcherTransport, server: FakeAiwatcher
) -> None:
    transport.last_message_id("chat")
    (mailbox,) = [run for run in server.runs.values() if run["hosted"]]
    reads = 0
    original = transport._version

    def moved_once(execution: str) -> int:
        nonlocal reads
        reads += 1
        # Another worker appends between this read and the post, once.
        return original(execution) - (1 if reads == 1 else 0)

    transport._version = moved_once  # type: ignore[method-assign]
    transport.publish_message(answer_to(asked(), "Ouagadougou", target="mailbox:chat"))

    assert reads == 2
    assert [
        row for row in mailbox["rows"] if row["message"].get("message_type") == "assistant_message"
    ]


def test_the_chat_client_hears_its_answer_through_the_mailbox(
    transport: AiwatcherTransport, server: FakeAiwatcher
) -> None:
    def planner(message: Message, discovery: Any) -> Sequence[Message]:
        return (answer_to(message, "Ouagadougou", target=message.metadata.source),)

    service = serving(transport, server, "planner", planner)

    def worker() -> None:
        while not server.runs_of("lab.planner"):
            time.sleep(0.01)
        deliver(service, server.runs_of("lab.planner")[0])

    threading.Thread(target=worker, daemon=True).start()
    client = DistributedChatClient(
        transport, source=transport.reply_address("chat"), timeout_seconds=5.0
    )

    assert client.ask(WORDS) == "Ouagadougou"


# ── The registry ─────────────────────────────────────────────────────────


def test_the_registry_is_the_definitions_aiwatcher_holds(
    transport: AiwatcherTransport, server: FakeAiwatcher
) -> None:
    serving(transport, server, "planner", ignore, capabilities=("plan",))
    serving(transport, server, "search", ignore, capabilities=("web-search",))
    server.definitions["lab.notes"] = {"definition": {"name": "lab.notes", "steps": [{"id": "x"}]}}
    server.definitions["other.search"] = {
        "definition": {
            "name": "other.search",
            "steps": [{"id": HOP, "params": {"capabilities": ["web-search"]}}],
        }
    }
    registry = AiwatcherServiceRegistry(transport)

    assert [agent.agent_name for agent in registry.live_agents(max_age_seconds=1)] == [
        "planner",
        "search",
    ]
    found = registry.find_by_capability("web-search", max_age_seconds=1)
    assert found is not None
    assert (found.agent_name, found.consumer_group) == ("search", "lab.search")


def test_an_agent_is_claimed_from_a_queue_of_its_own(
    transport: AiwatcherTransport, server: FakeAiwatcher
) -> None:
    serving(transport, server, "planner", ignore)

    (step,) = server.definitions["lab.planner"]["definition"]["steps"]

    assert (step["id"], step["queue"]) == (HOP, "lab.planner")


# ── One hop ──────────────────────────────────────────────────────────────


def test_a_retried_hop_hands_off_under_the_same_ids_and_starts_one_run(
    transport: AiwatcherTransport, server: FakeAiwatcher
) -> None:
    def planner(message: Message, discovery: Any) -> Sequence[Message]:
        return (
            answer_to(message, "look it up", target="search"),
            TurnCompleted(data={"workflow": "planner"}, metadata=RecordedMessageMetadata()),
        )

    service = serving(transport, server, "planner", planner)
    serving(transport, server, "search", ignore)
    run = server.runs[transport.publish_message(asked(message_id="m-1"))]

    first = deliver(service, run)
    again = deliver(service, run)

    assert first["sent"] == again["sent"]
    assert [sent["to"] for sent in first["sent"]] == ["search"]
    assert len(server.runs_of("lab.search")) == 1
    assert service.metrics.handled == 2


def test_a_handler_that_says_not_to_retry_fails_the_step_and_tells_the_client_now(
    transport: AiwatcherTransport, server: FakeAiwatcher
) -> None:
    def planner(message: Message, discovery: Any) -> Sequence[Message]:
        raise PermanentMessageError("the question is not a question")

    service = serving(transport, server, "planner", planner)
    run = server.runs[transport.publish_message(asked())]

    with pytest.raises(TaskError) as refused:
        deliver(service, run)

    assert refused.value.classification == "validation"
    replies = [record.record for record in transport.read_messages("chat", block_ms=0)]
    assert [type(reply).__name__ for reply in replies] == ["AssistantMessage", "TurnCompleted"]
    assert isinstance(replies[0], AssistantMessage)
    assert replies[0].metadata.status == "error"
    assert replies[0].data.text is not None
    assert "planner failed" in replies[0].data.text
    assert service.metrics.dead_lettered == 1


def test_a_retryable_failure_is_left_to_the_servers_budget_until_the_last_attempt(
    transport: AiwatcherTransport, server: FakeAiwatcher
) -> None:
    def planner(message: Message, discovery: Any) -> Sequence[Message]:
        raise ConnectionError("the model is not answering")

    patient = serving(transport, server, "planner", planner, attempts=3)
    run = server.runs[transport.publish_message(asked())]
    with pytest.raises(TaskError) as retried:
        deliver(patient, run)
    assert retried.value.classification == "infrastructure"
    assert transport.read_messages("chat", block_ms=0) == []
    assert patient.metrics.retried == 1

    last = serving(transport, server, "planner", planner, attempts=1)
    with pytest.raises(TaskError):
        deliver(last, run)
    assert len(transport.read_messages("chat", block_ms=0)) == 2


def test_a_hand_off_aiwatcher_did_not_answer_is_the_servers_to_retry(
    transport: AiwatcherTransport, server: FakeAiwatcher, monkeypatch: pytest.MonkeyPatch
) -> None:
    def planner(message: Message, discovery: Any) -> Sequence[Message]:
        return (answer_to(message, "look it up", target="search"),)

    service = serving(transport, server, "planner", planner)
    run = server.runs[transport.publish_message(asked())]

    def unreachable(message: Message) -> str:
        raise ApiError("aiwatcher at http://aiwatcher.test is unreachable")

    monkeypatch.setattr(transport, "publish_message", unreachable)
    with pytest.raises(TaskError) as failed:
        deliver(service, run)

    assert failed.value.classification == "infrastructure"
    assert service.metrics.publish_failures == 1


def test_a_hop_whose_words_this_process_cannot_read_is_refused_and_says_why(
    transport: AiwatcherTransport, server: FakeAiwatcher
) -> None:
    service = serving(transport, server, "planner", ignore)
    run = server.runs[transport.publish_message(asked())]
    hop = run["parameters"]["hop"]

    with pytest.raises(TaskError, match="AIWATCHER_PAYLOAD_ROOT") as missing:
        deliver(service, {"parameters": {"hop": {**hop, "reference": "memory://nothing"}}})
    with pytest.raises(TaskError, match="not the message that was sent") as altered:
        deliver(service, {"parameters": {"hop": {**hop, "digest": "0" * 64}}})

    assert missing.value.classification == altered.value.classification == "validation"
    assert WORDS not in str(missing.value) + str(altered.value)


# ── Wiring ───────────────────────────────────────────────────────────────


def test_discovery_runs_an_agent_on_whichever_transport_it_holds(
    transport: AiwatcherTransport, server: FakeAiwatcher
) -> None:
    on_aiwatcher = AgenticServiceDiscovery(transport, AiwatcherServiceRegistry(transport))
    in_memory = AgenticServiceDiscovery(InMemoryTransport(), InMemoryServiceRegistry())

    hosted = on_aiwatcher.create_service("planner", capabilities=("plan",), handler=ignore)
    local = in_memory.create_service("planner", capabilities=("plan",), handler=ignore)
    try:
        assert isinstance(hosted, AiwatcherService)
        assert isinstance(local, DistributedService)
    finally:
        hosted.close()
        local.close()


def test_distributed_mode_without_aiwatchers_address_is_refused_by_name(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.delenv("AIWATCHER_URL", raising=False)

    with pytest.raises(RuntimeError, match="AIWATCHER_URL"):
        AiwatcherTransport.from_env()
