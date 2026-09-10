from __future__ import annotations

import time

from aiwatcher_agentic.workflow.ids import uuid7
from aiwatcher_agentic.workflow.messages import (
    AssistantMessage,
    ConversationData,
    Message,
    RecordedMessageMetadata,
    TurnCompleted,
    UserMessage,
)

from .transport import MessageTransport


class DistributedChatClient:
    def __init__(
        self,
        transport: MessageTransport,
        *,
        entry_agent: str = "planner",
        source: str = "chat",
        timeout_seconds: float = 60.0,
        domain: str = "lab6",
    ) -> None:
        self._transport = transport
        self._entry_agent = entry_agent
        self._source = source
        self._timeout_seconds = timeout_seconds
        self._domain = domain

    def ask(self, text: str) -> str:
        prompt = text.strip()
        if not prompt:
            raise ValueError("Distributed chat prompt cannot be empty.")

        runtime_id = str(uuid7())
        turn_id = str(uuid7())
        last_seen_id = self._transport.last_message_id(self._source)
        self._transport.publish_message(
            UserMessage(
                data=ConversationData(role="user", text=prompt),
                metadata=RecordedMessageMetadata(
                    runtime_id=runtime_id,
                    turn_id=turn_id,
                    domain=self._domain,
                    source=self._source,
                    target=self._entry_agent,
                ),
            )
        )

        deadline = time.monotonic() + self._timeout_seconds
        next_id = last_seen_id
        while time.monotonic() < deadline:
            records = self._transport.read_messages(
                self._source,
                after_id=next_id,
                block_ms=1_000,
                count=10,
            )
            if not records:
                continue

            for record in records:
                next_id = record.entry_id
                message = record.record
                if not isinstance(message, Message) or message.metadata.turn_id != turn_id:
                    continue
                if isinstance(message, AssistantMessage) and message.metadata.scope == "canonical":
                    return message.data.text or ""
                if isinstance(message, TurnCompleted) and message.metadata.status == "error":
                    payload = message.data if isinstance(message.data, dict) else {}
                    error_message = payload.get("error_message")
                    if isinstance(error_message, str) and error_message:
                        raise RuntimeError(error_message)
                    raise RuntimeError("Distributed turn failed.")

        raise TimeoutError(f"Timed out waiting for distributed response from {self._entry_agent}.")

    def close(self) -> None:
        self._transport.close()
