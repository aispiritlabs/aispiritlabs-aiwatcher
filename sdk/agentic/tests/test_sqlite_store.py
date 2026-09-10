from __future__ import annotations

import sqlite3
from dataclasses import dataclass
from pathlib import Path

import orjson

from aiwatcher_agentic.runtime.storage.sqlite_store import SQLiteMessageStore
from aiwatcher_agentic.workflow.message_bus import InMemoryMessageBus
from aiwatcher_agentic.workflow.messages import (
    ConversationData,
    Event,
    RecordedMessageMetadata,
    UserCommand,
    UserMessage,
)


@dataclass(frozen=True, slots=True, kw_only=True)
class CreatedNote(Event):
    kind: str = "created_note"
    type: str = "created_note"
    note_name: str = ""
    note_content: str = ""

    def __post_init__(self) -> None:
        if not self.data:
            object.__setattr__(
                self, "data", {"note_name": self.note_name, "note_content": self.note_content}
            )


@dataclass(frozen=True, slots=True, kw_only=True)
class NoteUpdated(Event):
    kind: str = "note_updated"
    type: str = "note_updated"
    note_name: str = ""
    note_path: str = ""

    def __post_init__(self) -> None:
        if not self.data:
            object.__setattr__(
                self, "data", {"note_name": self.note_name, "note_path": self.note_path}
            )


def test_sqlite_message_store_persists_runtime_stream(tmp_path: Path) -> None:
    store = SQLiteMessageStore(
        path=tmp_path / "message_stream.sqlite3",
        batch_size=2,
        flush_interval_seconds=0.01,
    )
    bus = InMemoryMessageBus(store=store)

    bus.publish(
        UserMessage(
            data=ConversationData(role="user", text="hej"),
            metadata=RecordedMessageMetadata(
                runtime_id="runtime-1",
                domain="general",
                source="user",
            ),
        )
    )
    bus.publish(
        Event(
            type="workflow_selected",
            data={"workflow": "manage_notes"},
            metadata=RecordedMessageMetadata(
                runtime_id="runtime-1",
                domain="routing",
                source="router",
                target="manage_notes",
            ),
        )
    )
    bus.publish(
        UserCommand(
            type="reset",
            metadata=RecordedMessageMetadata(
                runtime_id="runtime-1",
                domain="manage_notes",
                source="runtime",
            ),
        )
    )
    bus.publish(
        CreatedNote(
            note_name="Projekt",
            note_content="Plan sprintu",
            metadata=RecordedMessageMetadata(
                runtime_id="runtime-1",
                source="manage_notes",
                domain="manage_notes",
                target="organizer",
            ),
        )
    )
    bus.publish(
        NoteUpdated(
            note_name="Projekt",
            note_path="/vault/Projekt.md",
            metadata=RecordedMessageMetadata(
                runtime_id="runtime-1",
                source="manage_notes",
                domain="manage_notes",
                target="rag",
            ),
        )
    )
    bus.close()

    with sqlite3.connect(tmp_path / "message_stream.sqlite3") as connection:
        rows = connection.execute(
            """
            SELECT kind, domain, source, target, name, text, payload_json
            FROM message_stream
            ORDER BY id
            """
        ).fetchall()

    assert rows[0] == ("conversation", "general", "user", None, None, "hej", None)
    assert rows[1][0:6] == (
        "event",
        "routing",
        "router",
        "manage_notes",
        "workflow_selected",
        None,
    )
    assert orjson.loads(rows[1][6]) == {"workflow": "manage_notes"}
    assert rows[2] == ("command", "manage_notes", "runtime", None, "reset", None, None)
    assert rows[3][0:6] == (
        "created_note",
        "manage_notes",
        "manage_notes",
        "organizer",
        "created_note",
        None,
    )
    assert orjson.loads(rows[3][6]) == {
        "note_name": "Projekt",
        "note_content": "Plan sprintu",
    }
    assert rows[4][0:6] == (
        "note_updated",
        "manage_notes",
        "manage_notes",
        "rag",
        "note_updated",
        None,
    )
    assert orjson.loads(rows[4][6]) == {
        "note_name": "Projekt",
        "note_path": "/vault/Projekt.md",
    }


def test_sqlite_message_projection_is_idempotent_by_event_id(tmp_path: Path) -> None:
    path = tmp_path / "message_stream.sqlite3"
    store = SQLiteMessageStore(path=path, flush_interval_seconds=0.001)
    bus = InMemoryMessageBus(store=store)
    message = Event(
        type="research.requested",
        metadata=RecordedMessageMetadata(
            runtime_id="runtime-1",
            turn_id="turn-1",
            message_id="logical-1",
            event_id="event-1",
        ),
    )

    bus.publish(message)
    bus.publish(message)
    bus.close()

    with sqlite3.connect(path) as connection:
        count = connection.execute("SELECT COUNT(*) FROM message_stream").fetchone()
    assert count == (1,)
