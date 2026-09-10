from __future__ import annotations

import pytest

from aiwatcher_agentic.workflow.event_store import InMemoryEventStore
from aiwatcher_agentic.workflow.message_bus import DurableMessageBus
from aiwatcher_agentic.workflow.messages import Event, RecordedMessageMetadata
from aiwatcher_agentic.workflow.processor import InMemoryCheckpointStore


def _message() -> Event:
    return Event(
        type="research.requested",
        data={"query": "event sourcing"},
        metadata=RecordedMessageMetadata(
            message_id="message-1",
            event_id="event-1",
        ),
    )


def test_replays_message_after_crash_between_append_and_dispatch_checkpoint() -> None:
    event_store = InMemoryEventStore()
    checkpoints = InMemoryCheckpointStore()
    crashing_bus = DurableMessageBus(
        event_store=event_store,
        checkpoint_store=checkpoints,
    )
    crashing_bus.subscribe(lambda _: (_ for _ in ()).throw(RuntimeError("crash")))

    with pytest.raises(RuntimeError, match="crash"):
        crashing_bus.publish(_message())

    assert len(event_store.read_stream("local-message-bus").events) == 1
    assert crashing_bus.checkpoint == 0

    delivered: list[str] = []
    resumed_bus = DurableMessageBus(
        event_store=event_store,
        checkpoint_store=checkpoints,
    )
    resumed_bus.subscribe(lambda message: delivered.append(message.type))

    resumed_bus.replay_pending()
    resumed_bus.publish(_message())

    assert delivered == ["research.requested"]
    assert resumed_bus.checkpoint == 1


def test_records_before_calling_subscribers() -> None:
    event_store = InMemoryEventStore()
    bus = DurableMessageBus(
        event_store=event_store,
        checkpoint_store=InMemoryCheckpointStore(),
    )
    observed_versions: list[int] = []
    bus.subscribe(
        lambda _: observed_versions.append(
            event_store.read_stream("local-message-bus").current_version
        )
    )

    bus.publish(_message())

    assert observed_versions == [1]
