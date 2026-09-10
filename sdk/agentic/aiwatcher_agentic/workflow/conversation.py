"""Small constructors and accessors for conversation messages.

``Message`` splits payload (``data``) from envelope (``metadata``), which is the
right shape for the runtime but verbose at a call site that only wants "a user
message saying X, in the same turn as Y". These helpers cover that case.
"""

from __future__ import annotations

from aiwatcher_agentic.workflow.messages import (
    AssistantMessage,
    ConversationData,
    Message,
    MessageMetadata,
    RecordedMessageMetadata,
    UserMessage,
)

__all__ = [
    "assistant_message",
    "message_text",
    "reply_metadata",
    "user_message",
]


def message_text(message: Message) -> str:
    """Return a message's text, or an empty string when it carries none."""
    data = message.data
    if isinstance(data, ConversationData):
        return data.text or ""
    if isinstance(data, dict):
        text = data.get("text")
        return text if isinstance(text, str) else ""
    return ""


def reply_metadata(
    source_message: Message,
    *,
    source: str,
    domain: str | None = None,
    target: str | None = None,
) -> RecordedMessageMetadata:
    """Build metadata for a message emitted in response to ``source_message``.

    Keeps the runtime, session and turn identifiers so the reply stays part of
    the same conversation turn.
    """
    origin: MessageMetadata = source_message.metadata
    return RecordedMessageMetadata(
        runtime_id=origin.runtime_id,
        session_id=origin.session_id,
        turn_id=origin.turn_id,
        reply_to_message_id=getattr(origin, "message_id", "") or None,
        domain=domain if domain is not None else origin.domain,
        source=source,
        target=target,
        trace=origin.trace,
    )


def user_message(
    text: str,
    *,
    source: str = "user",
    domain: str = "",
    target: str | None = None,
    reply_to: Message | None = None,
) -> UserMessage:
    """Build a ``UserMessage``, optionally continuing ``reply_to``'s turn."""
    if reply_to is not None:
        metadata = reply_metadata(reply_to, source=source, domain=domain or None, target=target)
    else:
        metadata = RecordedMessageMetadata(domain=domain, source=source, target=target)
    return UserMessage(data=ConversationData(role="user", text=text), metadata=metadata)


def assistant_message(
    text: str,
    *,
    source: str = "assistant",
    domain: str = "",
    target: str | None = None,
    reply_to: Message | None = None,
) -> AssistantMessage:
    """Build an ``AssistantMessage``, optionally continuing ``reply_to``'s turn."""
    if reply_to is not None:
        metadata = reply_metadata(reply_to, source=source, domain=domain or None, target=target)
    else:
        metadata = RecordedMessageMetadata(domain=domain, source=source, target=target)
    return AssistantMessage(
        data=ConversationData(role="assistant", text=text),
        metadata=metadata,
    )
