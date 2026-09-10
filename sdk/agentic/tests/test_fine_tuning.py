import sqlite3
from pathlib import Path

from aiwatcher_agentic.runtime.fine_tuning import (
    export_agent_fine_tuning_rows,
    export_router_fine_tuning_rows,
)
from aiwatcher_agentic.runtime.storage.sqlite_store import SQLiteMessageStore
from aiwatcher_agentic.workflow.message_bus import InMemoryMessageBus
from aiwatcher_agentic.workflow.messages import (
    AssistantMessage,
    ConversationData,
    Event,
    MessageChunk,
    MessageStarted,
    PromptSnapshot,
    RecordedMessageMetadata,
    ToolCallEvent,
    ToolResultMessage,
    TurnCompleted,
    TurnStarted,
    UserMessage,
)
from aiwatcher_agentic.workflow.streaming import build_assistant_messages, resolve_trace_context


def _user_message(
    *,
    runtime_id: str,
    turn_id: str,
    message_id: str = "",
    domain: str,
    source: str,
    target: str | None = None,
    text: str | None = None,
) -> UserMessage:
    return UserMessage(
        data=ConversationData(role="user", text=text),
        metadata=RecordedMessageMetadata(
            runtime_id=runtime_id,
            turn_id=turn_id,
            message_id=message_id,
            domain=domain,
            source=source,
            target=target,
        ),
    )


def test_sqlite_store_assembles_chunked_assistant_message_into_conversation_record(
    tmp_path: Path,
) -> None:
    store_path = tmp_path / "message_stream.sqlite3"
    store = SQLiteMessageStore(
        path=store_path,
        batch_size=2,
        flush_interval_seconds=0.01,
    )
    bus = InMemoryMessageBus(store=store)
    incoming = _user_message(
        runtime_id="runtime-1",
        turn_id="turn-1",
        message_id="user-1",
        domain="manage_notes",
        source="user",
        target="manage_notes",
        text="Dodaj notatkę",
    )
    large_text = "Projekt sprintu.\n" * 512
    messages, message_id = build_assistant_messages(
        incoming=incoming,
        agent_name="manage_notes",
        text=large_text,
        reply_to_message_id=incoming.metadata.message_id,
        agent_run_id="run-1",
        max_inline_bytes=64,
        chunk_bytes=64,
        ctx=resolve_trace_context(incoming=incoming),
    )

    bus.publish_many(messages)
    bus.close()

    with sqlite3.connect(store_path) as connection:
        stream_rows = connection.execute(
            """
            SELECT kind, text
            FROM message_stream
            WHERE message_id = ?
            ORDER BY id
            """,
            [message_id],
        ).fetchall()
        conversation_rows = connection.execute(
            """
            SELECT kind, event_type, role, text
            FROM conversation_records
            WHERE message_id = ?
            """,
            [message_id],
        ).fetchall()

    assert stream_rows[0][0] == "message_started"
    assert stream_rows[-2][0] == "message_completed"
    assert stream_rows[-2][1] is None
    assert stream_rows[-1][0] == "assistant_message"
    assert len(conversation_rows) == 1
    assert conversation_rows[0][0:3] == (
        "assistant_message",
        "AssembledTransportMessage",
        "assistant",
    )
    assert conversation_rows[0][3] == large_text


def test_fine_tuning_exports_keep_router_and_agent_rows_separate(tmp_path: Path) -> None:
    store_path = tmp_path / "message_stream.sqlite3"
    store = SQLiteMessageStore(
        path=store_path,
        batch_size=2,
        flush_interval_seconds=0.01,
    )
    bus = InMemoryMessageBus(store=store)
    runtime_id = "runtime-1"
    turn_id = "turn-1"

    bus.publish(
        PromptSnapshot(
            data=ConversationData(role="system", text="Select the workflow."),
            metadata=RecordedMessageMetadata(
                runtime_id=runtime_id,
                turn_id=turn_id,
                message_id="routing-prompt",
                domain="routing",
                source="router",
                target="user",
                prompt_name="router",
                prompt_hash="router-hash",
            ),
        )
    )
    bus.publish(
        Event(
            type="workflow_selected",
            data={"workflow": "manage_notes"},
            metadata=RecordedMessageMetadata(
                runtime_id=runtime_id,
                turn_id=turn_id,
                domain="routing",
                source="router",
                target="manage_notes",
                scope="transport",
            ),
        )
    )
    bus.publish(
        TurnStarted(
            data={"workflow": "manage_notes"},
            metadata=RecordedMessageMetadata(
                runtime_id=runtime_id,
                turn_id=turn_id,
                domain="manage_notes",
                source="runtime",
                target="manage_notes",
            ),
        )
    )
    bus.publish(
        _user_message(
            runtime_id=runtime_id,
            turn_id=turn_id,
            message_id="user-1",
            domain="manage_notes",
            source="user",
            target="manage_notes",
            text="Dodaj notatkę Projekt",
        )
    )
    bus.publish(
        PromptSnapshot(
            data=ConversationData(
                role="system",
                text="You manage notes.",
                payload={"tool_schema": [{"name": "add_note"}]},
            ),
            metadata=RecordedMessageMetadata(
                runtime_id=runtime_id,
                turn_id=turn_id,
                message_id="workflow-prompt",
                domain="manage_notes",
                source="manage_notes",
                target="user",
                prompt_name="manage-notes",
                prompt_hash="workflow-hash",
            ),
        )
    )
    bus.publish(
        ToolCallEvent(
            data={"name": "add_note", "parameters": {"note_name": "Projekt"}},
            metadata=RecordedMessageMetadata(
                runtime_id=runtime_id,
                turn_id=turn_id,
                message_id="tool-call-message",
                reply_to_message_id="user-1",
                domain="manage_notes",
                source="manage_notes",
                target="user",
                role="assistant",
                tool_call_id="call-1",
            ),
        )
    )
    bus.publish(
        ToolResultMessage(
            data=ConversationData(
                role="tool",
                name="add_note",
                text="Zapisano notatkę Projekt.",
                payload={"note_name": "Projekt"},
            ),
            metadata=RecordedMessageMetadata(
                runtime_id=runtime_id,
                turn_id=turn_id,
                message_id="tool-result-message",
                reply_to_message_id="tool-call-message",
                domain="manage_notes",
                source="add_note",
                target="manage_notes",
                tool_call_id="call-1",
            ),
        )
    )
    bus.publish(
        AssistantMessage(
            data=ConversationData(role="assistant", text="Notatka gotowa."),
            metadata=RecordedMessageMetadata(
                runtime_id=runtime_id,
                turn_id=turn_id,
                message_id="assistant-1",
                reply_to_message_id="tool-result-message",
                domain="manage_notes",
                source="manage_notes",
                target="user",
            ),
        )
    )
    bus.publish(
        TurnCompleted(
            data={"workflow": "manage_notes", "final_message_id": "assistant-1"},
            metadata=RecordedMessageMetadata(
                runtime_id=runtime_id,
                turn_id=turn_id,
                domain="manage_notes",
                source="runtime",
                target="manage_notes",
                status="success",
            ),
        )
    )
    bus.close()

    agent_rows = export_agent_fine_tuning_rows(store_path)
    router_rows = export_router_fine_tuning_rows(store_path)

    assert agent_rows == [
        {
            "messages": [
                {"role": "system", "content": "You manage notes."},
                {"role": "user", "content": "Dodaj notatkę Projekt"},
                {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [
                        {
                            "id": "call-1",
                            "type": "function",
                            "function": {
                                "name": "add_note",
                                "arguments": '{"note_name":"Projekt"}',
                            },
                        }
                    ],
                },
                {
                    "role": "tool",
                    "content": "Zapisano notatkę Projekt.",
                    "tool_call_id": "call-1",
                    "name": "add_note",
                },
                {"role": "assistant", "content": "Notatka gotowa."},
            ],
            "tools": [{"name": "add_note"}],
            "metadata": {
                "runtime_id": runtime_id,
                "session_id": runtime_id,
                "turn_id": turn_id,
                "domain": "manage_notes",
                "prompt_name": "manage-notes",
                "prompt_hash": "workflow-hash",
                "trace_id": None,
                "agent_run_id": None,
                "final_message_id": "assistant-1",
                "retry_count": 0,
                "loop_iteration_count": 0,
            },
        }
    ]
    assert router_rows == [
        {
            "messages": [
                {"role": "system", "content": "Select the workflow."},
                {"role": "user", "content": "Dodaj notatkę Projekt"},
                {"role": "assistant", "content": "manage_notes"},
            ],
            "metadata": {
                "runtime_id": runtime_id,
                "session_id": runtime_id,
                "turn_id": turn_id,
                "domain": "routing",
                "expected_workflow": "manage_notes",
                "prompt_name": "router",
                "prompt_hash": "router-hash",
                "trace_id": None,
            },
        }
    ]


def test_fine_tuning_export_skips_incomplete_chunked_turn(tmp_path: Path) -> None:
    store_path = tmp_path / "message_stream.sqlite3"
    store = SQLiteMessageStore(
        path=store_path,
        batch_size=2,
        flush_interval_seconds=0.01,
    )
    bus = InMemoryMessageBus(store=store)
    runtime_id = "runtime-1"
    turn_id = "turn-1"

    bus.publish(
        TurnStarted(
            data={"workflow": "manage_notes"},
            metadata=RecordedMessageMetadata(
                runtime_id=runtime_id,
                turn_id=turn_id,
                domain="manage_notes",
                source="runtime",
                target="manage_notes",
            ),
        )
    )
    bus.publish(
        _user_message(
            runtime_id=runtime_id,
            turn_id=turn_id,
            message_id="user-1",
            domain="manage_notes",
            source="user",
            target="manage_notes",
            text="Dodaj notatkę Projekt",
        )
    )
    bus.publish(
        PromptSnapshot(
            data=ConversationData(role="system", text="You manage notes."),
            metadata=RecordedMessageMetadata(
                runtime_id=runtime_id,
                turn_id=turn_id,
                message_id="workflow-prompt",
                domain="manage_notes",
                source="manage_notes",
                target="user",
                prompt_name="manage-notes",
                prompt_hash="workflow-hash",
            ),
        )
    )
    bus.publish(
        MessageStarted(
            data={"logical_kind": "assistant_message", "chunk_count": 2},
            metadata=RecordedMessageMetadata(
                runtime_id=runtime_id,
                turn_id=turn_id,
                message_id="assistant-1",
                reply_to_message_id="user-1",
                domain="manage_notes",
                source="manage_notes",
                target="user",
                scope="transport",
                role="assistant",
            ),
        )
    )
    bus.publish(
        MessageChunk(
            data=ConversationData(role="assistant", text="Notatka "),
            metadata=RecordedMessageMetadata(
                runtime_id=runtime_id,
                turn_id=turn_id,
                message_id="assistant-1",
                reply_to_message_id="user-1",
                domain="manage_notes",
                source="manage_notes",
                target="user",
                scope="transport",
                role="assistant",
                chunk_index=0,
                chunk_count=2,
            ),
        )
    )
    bus.publish(
        TurnCompleted(
            data={"workflow": "manage_notes", "final_message_id": "assistant-1"},
            metadata=RecordedMessageMetadata(
                runtime_id=runtime_id,
                turn_id=turn_id,
                domain="manage_notes",
                source="runtime",
                target="manage_notes",
                status="success",
            ),
        )
    )
    bus.close()

    assert export_agent_fine_tuning_rows(store_path) == []
