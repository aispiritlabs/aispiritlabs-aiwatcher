from __future__ import annotations

from collections.abc import Sequence
from typing import Any

from aiwatcher_agentic.workflow.durable import DurableWorkflowExecutor
from aiwatcher_agentic.workflow.errors import ConcurrencyConflictError
from aiwatcher_agentic.workflow.event_store import AppendResult, InMemoryEventStore
from aiwatcher_agentic.workflow.messages import Event, Message, RecordedMessageMetadata


def _input() -> Event:
    return Event(
        type="research.requested",
        metadata=RecordedMessageMetadata(
            message_id="input-1",
            event_id="input-event-1",
        ),
    )


def _message_id(message: Message) -> str:
    metadata = message.metadata
    assert isinstance(metadata, RecordedMessageMetadata)
    return metadata.message_id


def test_duplicate_input_replays_same_outbox_without_repeating_decision() -> None:
    store = InMemoryEventStore()
    executor = DurableWorkflowExecutor(store)
    calls = 0

    def decide(message: Message) -> tuple[Message, ...]:
        nonlocal calls
        calls += 1
        return (Event(type="research.started", data={"input": message.type}),)

    first = executor.execute("workflow:research:1", _input(), decide)
    duplicate = executor.execute("workflow:research:1", _input(), decide)

    assert calls == 1
    assert first.duplicate is False
    assert duplicate.duplicate is True
    assert _message_id(duplicate.outputs[0]) == _message_id(first.outputs[0])
    assert duplicate.outputs[0].metadata.causation_id == "input-1"


class _ConflictOnceStore:
    def __init__(self) -> None:
        self.inner = InMemoryEventStore()
        self.conflict = True

    def __getattr__(self, name: str) -> Any:
        return getattr(self.inner, name)

    def append_to_stream(
        self,
        stream_name: str,
        events: Sequence[Message],
        *,
        expected_version: Any = "NO_CONCURRENCY_CHECK",
    ) -> AppendResult:
        if self.conflict:
            self.conflict = False
            self.inner.append_to_stream(stream_name, (Event(type="concurrent.write"),))
            raise ConcurrencyConflictError(stream_name, expected_version, 1)
        return self.inner.append_to_stream(
            stream_name,
            events,
            expected_version=expected_version,
        )


def test_occ_retry_reuses_decision_instead_of_repeating_external_call() -> None:
    store = _ConflictOnceStore()
    executor = DurableWorkflowExecutor(store)
    calls = 0

    def decide(message: Message) -> tuple[Message, ...]:
        nonlocal calls
        del message
        calls += 1
        return (Event(type="search.completed"),)

    result = executor.execute("workflow:research:1", _input(), decide)

    assert calls == 1
    assert result.duplicate is False
    assert [event.type for event in store.inner.read_stream("workflow:research:1").events] == [
        "concurrent.write",
        "research.requested",
        "search.completed",
    ]
