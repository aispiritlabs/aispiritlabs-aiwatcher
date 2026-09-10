from aiwatcher_agentic.workflow.messages import Event, RecordedMessageMetadata, UserCommand


def test_user_command_uses_emmett_style_type_data_and_metadata_without_target() -> None:
    command = UserCommand(
        type="reset",
        data={"hard": True},
        metadata=RecordedMessageMetadata(
            runtime_id="runtime-1",
            turn_id="turn-1",
            message_id="message-1",
            domain="manage_notes",
            source="runtime",
            target="manage_notes",
            sequence_no=7,
        ),
    )

    assert command.type == "reset"
    assert command.data == {"hard": True}
    assert command.metadata.runtime_id == "runtime-1"
    assert command.metadata.turn_id == "turn-1"
    assert command.metadata.message_id == "message-1"
    assert command.metadata.domain == "manage_notes"
    assert command.metadata.source == "runtime"
    assert command.metadata.target == "manage_notes"
    assert command.metadata.scope == "canonical"
    assert command.metadata.sequence_no == 7


def test_event_uses_emmett_style_type_data_and_metadata() -> None:
    event = Event(
        type="workflow_selected",
        data={"workflow": "manage_notes"},
        metadata=RecordedMessageMetadata(
            runtime_id="runtime-1",
            turn_id="turn-1",
            message_id="message-1",
            reply_to_message_id="message-0",
            domain="routing",
            source="router",
            target="manage_notes",
            role="assistant",
            scope="transport",
            sequence_no=3,
            tool_call_id="call-1",
            agent_run_id="run-1",
            prompt_name="router",
            prompt_hash="hash",
            status="success",
        ),
    )

    assert event.type == "workflow_selected"
    assert event.data == {"workflow": "manage_notes"}
    assert event.metadata.runtime_id == "runtime-1"
    assert event.metadata.turn_id == "turn-1"
    assert event.metadata.message_id == "message-1"
    assert event.metadata.reply_to_message_id == "message-0"
    assert event.metadata.domain == "routing"
    assert event.metadata.source == "router"
    assert event.metadata.target == "manage_notes"
    assert event.metadata.role == "assistant"
    assert event.metadata.scope == "transport"
    assert event.metadata.sequence_no == 3
    assert event.metadata.tool_call_id == "call-1"
    assert event.metadata.agent_run_id == "run-1"
    assert event.metadata.prompt_name == "router"
    assert event.metadata.prompt_hash == "hash"
    assert event.metadata.status == "success"
