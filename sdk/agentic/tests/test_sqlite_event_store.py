from __future__ import annotations

import sqlite3
from pathlib import Path

import pytest

from aiwatcher_agentic.workflow import (
    STREAM_DOES_NOT_EXIST,
    STREAM_EXISTS,
    ConversationData,
    Event,
    RecordedMessageMetadata,
    SQLiteCheckpointStore,
    SQLiteEventStore,
    UserMessage,
)
from aiwatcher_agentic.workflow.errors import DuplicateMessageError


class _XorCodec:
    """Tiny reversible test codec; production supplies authenticated encryption."""

    def encode(self, payload: bytes) -> bytes:
        return bytes(value ^ 0xA5 for value in payload)

    def decode(self, payload: bytes) -> bytes:
        return self.encode(payload)


def test_sqlite_event_store_round_trips_messages_with_positions(tmp_path: Path) -> None:
    store = SQLiteEventStore(tmp_path / "workflow.sqlite3")

    store.append_to_stream(
        "workflow:lab6:turn-1",
        (
            UserMessage(
                data=ConversationData(role="user", text="hello"),
                metadata=RecordedMessageMetadata(
                    runtime_id="runtime-1",
                    turn_id="turn-1",
                    source="chat",
                    target="planner",
                    message_id="msg-1",
                ),
            ),
            Event(
                type="planned",
                data={"query": "redis streams"},
                metadata=RecordedMessageMetadata(
                    runtime_id="runtime-1",
                    turn_id="turn-1",
                    source="planner",
                    target="search",
                    message_id="msg-2",
                    reply_to_message_id="msg-1",
                ),
            ),
        ),
        expected_version=STREAM_DOES_NOT_EXIST,
    )

    result = store.read_stream("workflow:lab6:turn-1")

    assert result.stream_exists is True
    assert result.current_version == 2
    assert isinstance(result.events[0], UserMessage)
    assert result.events[0].metadata.stream_name == "workflow:lab6:turn-1"
    assert result.events[0].metadata.stream_position == 0
    assert result.events[0].metadata.global_position == 0
    assert result.events[1].metadata.stream_position == 1
    assert result.events[1].metadata.global_position == 1
    assert result.events[1].metadata.reply_to_message_id == "msg-1"


def test_sqlite_event_store_enforces_expected_version(tmp_path: Path) -> None:
    store = SQLiteEventStore(tmp_path / "workflow.sqlite3")
    store.append_to_stream(
        "orders-1",
        (Event(type="created", data={"id": "orders-1"}),),
        expected_version=STREAM_DOES_NOT_EXIST,
    )

    result = store.append_to_stream(
        "orders-1",
        (Event(type="confirmed", data={"id": "orders-1"}),),
        expected_version=STREAM_EXISTS,
    )

    assert result.next_version == 2


def test_sqlite_checkpoint_store_persists_positions(tmp_path: Path) -> None:
    checkpoint_store = SQLiteCheckpointStore(tmp_path / "workflow.sqlite3")

    assert checkpoint_store.read("projection-1") is None

    checkpoint_store.store("projection-1", 3)
    checkpoint_store.store("projection-1", 7)

    assert checkpoint_store.read("projection-1") == 7


def test_sqlite_store_deduplicates_by_idempotency_key_not_logical_message_id(
    tmp_path: Path,
) -> None:
    store = SQLiteEventStore(tmp_path / "workflow.sqlite3")
    records = (
        Event(
            type="message_started",
            metadata=RecordedMessageMetadata(
                message_id="logical-1",
                event_id="event-1",
                idempotency_key="logical-1:started",
            ),
        ),
        Event(
            type="message_chunk",
            metadata=RecordedMessageMetadata(
                message_id="logical-1",
                event_id="event-2",
                idempotency_key="logical-1:chunk:0",
            ),
        ),
    )
    store.append_to_stream("messages", records)

    assert len(store.read_stream("messages").events) == 2
    with pytest.raises(DuplicateMessageError):
        store.append_to_stream(
            "messages",
            (
                Event(
                    type="message_chunk",
                    metadata=RecordedMessageMetadata(
                        message_id="other-logical-id",
                        event_id="event-3",
                        idempotency_key="logical-1:chunk:0",
                    ),
                ),
            ),
        )


def test_sqlite_event_store_supports_application_payload_codec(tmp_path: Path) -> None:
    path = tmp_path / "workflow.sqlite3"
    store = SQLiteEventStore(path, payload_codec=_XorCodec())
    store.append_to_stream(
        "sensitive",
        (Event(type="secret.recorded", data={"secret": "classified"}),),
    )

    with sqlite3.connect(path) as connection:
        raw = connection.execute(
            "SELECT payload_json FROM event_messages WHERE stream_name = 'sensitive'"
        ).fetchone()[0]
    restored = store.read_stream("sensitive").events[0]

    assert b"classified" not in raw
    assert restored.data == {"secret": "classified"}


def test_sqlite_event_store_reads_global_feed_with_prefix(tmp_path: Path) -> None:
    store = SQLiteEventStore(tmp_path / "workflow.sqlite3")
    store.append_to_stream("research:one", (Event(type="one"),))
    store.append_to_stream("orders:one", (Event(type="two"),))
    store.append_to_stream("research:two", (Event(type="three"),))

    result = store.read_all(stream_prefix="research:")

    assert [event.type for event in result.events] == ["one", "three"]
    assert result.next_position == 3
    assert result.end_position == 3
