from __future__ import annotations

from pathlib import Path

from aiwatcher_agentic.runtime.distributed.contracts import AgentHeartbeat, AgentRegistration
from aiwatcher_agentic.runtime.distributed.service import DistributedService
from aiwatcher_agentic.runtime.distributed.transport import MalformedRecord
from aiwatcher_agentic.workflow import SQLiteEventStore
from aiwatcher_agentic.workflow.messages import (
    AssistantMessage,
    ConversationData,
    Message,
    RecordedMessageMetadata,
    UserMessage,
)


class _FakeTransport:
    def __init__(self) -> None:
        self.published: list[Message] = []
        self.acks: list[tuple[str, str, str]] = []
        self.dead_letters: list[dict[str, object]] = []
        self.fail_publishes = 0

    def publish_message(self, message: Message) -> str:
        if self.fail_publishes > 0:
            self.fail_publishes -= 1
            raise RuntimeError("transport unavailable")
        self.published.append(message)
        return f"{len(self.published)}-0"

    def ack(self, stream: str, group: str, entry_id: str) -> int:
        self.acks.append((stream, group, entry_id))
        return 1

    def publish_dead_letter(self, **record: object) -> str:
        self.dead_letters.append(record)
        return f"dlq-{len(self.dead_letters)}"


class _FakeRegistry:
    def register(self, registration: AgentRegistration) -> None:
        return None

    def heartbeat(self, heartbeat: AgentHeartbeat) -> None:
        return None


class _FakeDiscovery:
    def __init__(self, transport: _FakeTransport) -> None:
        self.transport = transport
        self.registry = _FakeRegistry()


def test_distributed_service_replays_recorded_outputs_on_duplicate_input(tmp_path: Path) -> None:
    calls: list[str] = []

    def handler(message: Message, discovery: _FakeDiscovery) -> tuple[Message, ...]:
        del discovery
        calls.append(getattr(message.metadata, "message_id", ""))
        return (
            AssistantMessage(
                data=ConversationData(role="assistant", text="done"),
                metadata=RecordedMessageMetadata(
                    runtime_id=message.metadata.runtime_id,
                    turn_id=message.metadata.turn_id,
                    domain=message.metadata.domain,
                    source="planner",
                    target="chat",
                ),
            ),
        )

    transport = _FakeTransport()
    discovery = _FakeDiscovery(transport)
    service = DistributedService(
        agent_name="planner",
        capabilities=("plan",),
        discovery=discovery,  # type: ignore[arg-type]
        handler=handler,  # type: ignore[arg-type]
        event_store=SQLiteEventStore(tmp_path / "workflow.sqlite3"),
    )
    message = UserMessage(
        data=ConversationData(role="user", text="hello"),
        metadata=RecordedMessageMetadata(
            runtime_id="runtime-1",
            turn_id="turn-1",
            domain="lab6",
            source="chat",
            target="planner",
            message_id="msg-1",
        ),
    )

    service._handle_record("test:messages:planner", "1-0", message)
    first_published = list(transport.published)

    assert len(calls) == 1
    assert len(first_published) == 1
    assert first_published[0].metadata.reply_to_message_id

    transport.published.clear()
    service._handle_record("test:messages:planner", "2-0", message)

    assert len(calls) == 1
    assert len(transport.published) == 1
    assert transport.published[0].data.text == "done"
    republished, original = transport.published[0].metadata, first_published[0].metadata
    assert isinstance(republished, RecordedMessageMetadata)
    assert isinstance(original, RecordedMessageMetadata)
    assert republished.message_id == original.message_id
    assert len(transport.acks) == 2


def _input() -> UserMessage:
    return UserMessage(
        data=ConversationData(role="user", text="hello"),
        metadata=RecordedMessageMetadata(
            runtime_id="runtime-1",
            turn_id="turn-1",
            domain="lab6",
            source="chat",
            target="planner",
            message_id="msg-1",
            event_id="input-event-1",
        ),
    )


def _output(message: Message) -> AssistantMessage:
    return AssistantMessage(
        data=ConversationData(role="assistant", text="done"),
        metadata=RecordedMessageMetadata(
            runtime_id=message.metadata.runtime_id,
            turn_id=message.metadata.turn_id,
            domain=message.metadata.domain,
            source="planner",
            target="chat",
        ),
    )


def test_transient_failure_is_not_acked_and_can_resume(tmp_path: Path) -> None:
    attempts = 0

    def handler(message: Message, discovery: _FakeDiscovery) -> tuple[Message, ...]:
        nonlocal attempts
        del discovery
        attempts += 1
        if attempts == 1:
            raise TimeoutError("search timed out")
        return (_output(message),)

    transport = _FakeTransport()
    service = DistributedService(
        agent_name="planner",
        capabilities=("plan",),
        discovery=_FakeDiscovery(transport),  # type: ignore[arg-type]
        handler=handler,  # type: ignore[arg-type]
        event_store=SQLiteEventStore(tmp_path / "workflow.sqlite3"),
    )

    service._handle_record("messages", "1-0", _input())
    assert transport.acks == []
    assert service.metrics.retried == 1

    service._handle_record("messages", "1-0", _input())

    assert attempts == 2
    assert len(transport.acks) == 1
    assert [message.data.text for message in transport.published] == ["done"]


def test_terminal_failure_goes_to_dlq_before_ack(tmp_path: Path) -> None:
    transport = _FakeTransport()

    def handler(message: Message, discovery: _FakeDiscovery) -> tuple[Message, ...]:
        del message, discovery
        raise RuntimeError("broken")

    service = DistributedService(
        agent_name="planner",
        capabilities=("plan",),
        discovery=_FakeDiscovery(transport),  # type: ignore[arg-type]
        handler=handler,  # type: ignore[arg-type]
        event_store=SQLiteEventStore(tmp_path / "workflow.sqlite3"),
        max_delivery_attempts=2,
    )

    service._handle_record("messages", "1-0", _input())
    service._handle_record("messages", "1-0", _input())

    assert len(transport.dead_letters) == 1
    assert len(transport.acks) == 1
    assert len(transport.published) == 2
    assert service.metrics.dead_lettered == 1


def test_publish_failure_leaves_input_pending_and_replays_outbox(tmp_path: Path) -> None:
    calls = 0

    def handler(message: Message, discovery: _FakeDiscovery) -> tuple[Message, ...]:
        nonlocal calls
        del discovery
        calls += 1
        return (_output(message),)

    transport = _FakeTransport()
    transport.fail_publishes = 1
    service = DistributedService(
        agent_name="planner",
        capabilities=("plan",),
        discovery=_FakeDiscovery(transport),  # type: ignore[arg-type]
        handler=handler,  # type: ignore[arg-type]
        event_store=SQLiteEventStore(tmp_path / "workflow.sqlite3"),
    )

    service._handle_record("messages", "1-0", _input())
    assert transport.acks == []

    service._handle_record("messages", "1-0", _input())

    assert calls == 1
    assert len(transport.acks) == 1
    assert [message.data.text for message in transport.published] == ["done"]
    assert service.metrics.publish_failures == 1
    assert service.metrics.replayed == 1


def test_malformed_record_is_dead_lettered_then_acked(tmp_path: Path) -> None:
    transport = _FakeTransport()
    service = DistributedService(
        agent_name="planner",
        capabilities=("plan",),
        discovery=_FakeDiscovery(transport),  # type: ignore[arg-type]
        handler=lambda message, discovery: (),
        event_store=SQLiteEventStore(tmp_path / "workflow.sqlite3"),
    )

    service._handle_record(
        "messages",
        "1-0",
        MalformedRecord("not-json", "JSONDecodeError", "invalid"),
    )

    assert len(transport.dead_letters) == 1
    assert len(transport.acks) == 1
    assert service.metrics.dead_lettered == 1
