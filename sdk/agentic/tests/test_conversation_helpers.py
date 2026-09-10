"""Conversation message constructors and accessors.

``Message`` splits payload from envelope. These helpers are what keep a reply
inside the same conversation turn as the message it answers — get the envelope
wrong and traces fan out into unrelated turns.
"""

from __future__ import annotations

import pytest

from aiwatcher_agentic.workflow import assistant_message, message_text, reply_metadata, user_message
from aiwatcher_agentic.workflow.messages import (
    AssistantMessage,
    ConversationData,
    Event,
    Message,
    RecordedMessageMetadata,
    UserMessage,
)
from aiwatcher_agentic.workflow.trace import TraceSnapshot


def _source() -> UserMessage:
    return UserMessage(
        data=ConversationData(role="user", text="pytanie"),
        metadata=RecordedMessageMetadata(
            runtime_id="rt-1",
            session_id="user:jan:ws:badania",
            turn_id="turn-7",
            message_id="msg-42",
            domain="notes",
            source="user",
            target="organizer",
        ),
    )


# ---------------------------------------------------------------------------
# message_text
# ---------------------------------------------------------------------------


def test_message_text_reads_conversation_data() -> None:
    assert message_text(_source()) == "pytanie"


def test_message_text_reads_a_dict_payload() -> None:
    assert message_text(Event(type="x", data={"text": "z eventu"})) == "z eventu"


@pytest.mark.parametrize(
    "message",
    [
        Message(data=ConversationData(role="user", text=None)),
        Message(data={}),
        Message(data={"text": None}),
        Message(data={"text": 42}),
        Message(data={"other": "field"}),
        Message(data=None),
        Message(data=["not", "a", "mapping"]),
        Message(data="bare string"),
    ],
)
def test_message_text_returns_an_empty_string_when_there_is_no_text(message: Message) -> None:
    # Callers concatenate the result straight into prompts; never None.
    assert message_text(message) == ""


# ---------------------------------------------------------------------------
# reply_metadata
# ---------------------------------------------------------------------------


def test_reply_metadata_keeps_the_conversation_turn() -> None:
    metadata = reply_metadata(_source(), source="organizer")

    assert metadata.runtime_id == "rt-1"
    assert metadata.session_id == "user:jan:ws:badania"
    assert metadata.turn_id == "turn-7"


def test_reply_metadata_links_back_to_the_message_it_answers() -> None:
    metadata = reply_metadata(_source(), source="organizer")

    assert metadata.reply_to_message_id == "msg-42"


def test_reply_metadata_sets_the_new_source_rather_than_copying_it() -> None:
    metadata = reply_metadata(_source(), source="organizer")

    assert metadata.source == "organizer"


def test_reply_metadata_inherits_the_domain_by_default() -> None:
    assert reply_metadata(_source(), source="organizer").domain == "notes"


def test_reply_metadata_overrides_the_domain_when_asked() -> None:
    assert reply_metadata(_source(), source="organizer", domain="calendar").domain == "calendar"


def test_reply_metadata_can_clear_the_domain_with_an_empty_string() -> None:
    # `None` means "inherit"; "" means "explicitly none".
    assert reply_metadata(_source(), source="organizer", domain="").domain == ""


def test_reply_metadata_drops_the_source_target_unless_given() -> None:
    # A reply is not addressed to whoever the request was addressed to.
    assert reply_metadata(_source(), source="organizer").target is None
    assert reply_metadata(_source(), source="organizer", target="sage").target == "sage"


def test_reply_metadata_carries_the_trace_forward() -> None:
    trace = TraceSnapshot(trace_id="tr-1", span_id="sp-1")
    source = _source().with_metadata(trace=trace)

    assert reply_metadata(source, source="organizer").trace is source.metadata.trace


def test_reply_metadata_handles_a_source_without_a_message_id() -> None:
    plain = UserMessage(
        data=ConversationData(role="user", text="x"),
        metadata=RecordedMessageMetadata(turn_id="turn-1"),
    )

    # An unrecorded message has message_id="" — that must become None, not "".
    assert reply_metadata(plain, source="organizer").reply_to_message_id is None


def test_reply_metadata_handles_metadata_without_a_message_id_field() -> None:
    from aiwatcher_agentic.workflow.messages import MessageMetadata

    plain = Message(data={}, metadata=MessageMetadata(turn_id="turn-1"))

    assert reply_metadata(plain, source="organizer").reply_to_message_id is None


# ---------------------------------------------------------------------------
# user_message / assistant_message
# ---------------------------------------------------------------------------


def test_user_message_builds_a_standalone_message() -> None:
    message = user_message("cześć")

    assert isinstance(message, UserMessage)
    assert message.data == ConversationData(role="user", text="cześć")
    assert message.metadata.source == "user"
    assert message.metadata.turn_id == ""


def test_assistant_message_builds_a_standalone_message() -> None:
    message = assistant_message("odpowiedź")

    assert isinstance(message, AssistantMessage)
    assert message.data == ConversationData(role="assistant", text="odpowiedź")
    assert message.metadata.source == "assistant"


@pytest.mark.parametrize("factory", [user_message, assistant_message])
def test_a_reply_continues_the_source_turn(factory) -> None:  # type: ignore[no-untyped-def]
    source = _source()

    message = factory("odpowiedź", source="organizer", reply_to=source)

    assert message.metadata.turn_id == "turn-7"
    assert message.metadata.session_id == "user:jan:ws:badania"
    assert message.metadata.reply_to_message_id == "msg-42"
    assert message.metadata.source == "organizer"


@pytest.mark.parametrize("factory", [user_message, assistant_message])
def test_a_reply_inherits_the_source_domain_when_none_is_given(factory) -> None:  # type: ignore[no-untyped-def]
    # `domain=""` at the call site means "unset", so the source domain wins.
    assert factory("x", reply_to=_source()).metadata.domain == "notes"


@pytest.mark.parametrize("factory", [user_message, assistant_message])
def test_an_explicit_domain_wins_over_the_source(factory) -> None:  # type: ignore[no-untyped-def]
    assert factory("x", domain="calendar", reply_to=_source()).metadata.domain == "calendar"


@pytest.mark.parametrize("factory", [user_message, assistant_message])
def test_a_standalone_message_keeps_the_domain_it_was_given(factory) -> None:  # type: ignore[no-untyped-def]
    assert factory("x", domain="calendar").metadata.domain == "calendar"
    assert factory("x").metadata.domain == ""


@pytest.mark.parametrize("factory", [user_message, assistant_message])
def test_the_built_message_round_trips_through_message_text(factory) -> None:  # type: ignore[no-untyped-def]
    assert message_text(factory("treść")) == "treść"


@pytest.mark.parametrize("factory", [user_message, assistant_message])
def test_an_empty_body_is_preserved_rather_than_dropped(factory) -> None:  # type: ignore[no-untyped-def]
    message = factory("")

    assert message.data.text == ""
    assert message_text(message) == ""
