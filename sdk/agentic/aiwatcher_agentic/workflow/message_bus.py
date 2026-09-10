from __future__ import annotations

import contextlib
from collections.abc import Callable
from typing import Any, Protocol

from aiwatcher_agentic.workflow.errors import DuplicateMessageError
from aiwatcher_agentic.workflow.event_store import EventStore
from aiwatcher_agentic.workflow.messages import Message, normalize_recorded_message
from aiwatcher_agentic.workflow.output_handler import OutputHandlerDispatcher, WorkflowOutputHandler
from aiwatcher_agentic.workflow.processor import CheckpointStore
from aiwatcher_agentic.workflow.streaming import hash_text

type MessageHandler = Callable[[Message], str | None]


class MessageStore(Protocol):
    def enqueue(self, message: Message) -> None: ...

    def close(self) -> None: ...


class InMemoryMessageBus:
    def __init__(self, store: MessageStore | None = None) -> None:
        self.messages: list[Message] = []
        self._subscribers: list[MessageHandler] = []
        self._output_dispatcher = OutputHandlerDispatcher()
        self._store = store
        self._sequence_no = 0

    def _normalize_message(self, message: Message) -> Message:
        normalized = normalize_recorded_message(message)
        updates: dict[str, Any] = {}
        sequence_no = getattr(message.metadata, "sequence_no", None)
        content_sha256 = getattr(message.metadata, "content_sha256", None)
        text = message.data.text if hasattr(message.data, "text") else None
        if sequence_no is None:
            self._sequence_no += 1
            updates["sequence_no"] = self._sequence_no
        if text and not content_sha256:
            updates["content_sha256"] = hash_text(text)
        if updates:
            return normalized.with_metadata(**updates)
        return normalized

    def subscribe(self, handler: MessageHandler) -> None:
        self._subscribers.append(handler)

    def register_output_handler(self, handler: WorkflowOutputHandler) -> None:
        self._output_dispatcher.register(handler)

    def publish(self, message: Message) -> list[str]:
        normalized = self._normalize_message(message)
        return self._dispatch_normalized(normalized)

    def _dispatch_normalized(self, normalized: Message) -> list[str]:
        self.messages.append(normalized)
        if self._store is not None:
            self._store.enqueue(normalized)
        results: list[str] = []
        for handler in self._subscribers:
            result = handler(normalized)
            if result is not None:
                results.append(result)
        results.extend(self._output_dispatcher.dispatch(normalized))
        return results

    def publish_many(self, messages: list[Message]) -> list[str]:
        results: list[str] = []
        for message in messages:
            results.extend(self.publish(message))
        return results

    def flush_output_handlers(self) -> list[str]:
        return self._output_dispatcher.flush()

    def clear(self) -> None:
        self.messages.clear()
        self._output_dispatcher.clear()
        self._sequence_no = 0

    def close(self) -> None:
        self.flush_output_handlers()
        if self._store is not None:
            self._store.close()


class DurableMessageBus(InMemoryMessageBus):
    """Local at-least-once bus backed by an event stream and a dispatch checkpoint.

    The Event Store is the write-ahead source of truth. ``MessageStore`` remains
    an asynchronous query/audit projection and can be rebuilt from the stream.
    """

    def __init__(
        self,
        *,
        event_store: EventStore,
        checkpoint_store: CheckpointStore,
        store: MessageStore | None = None,
        stream_name: str = "local-message-bus",
        consumer_id: str = "local-dispatch-v1",
        replay_batch_size: int = 100,
    ) -> None:
        super().__init__(store=store)
        if not stream_name.strip():
            raise ValueError("stream_name must not be empty")
        if not consumer_id.strip():
            raise ValueError("consumer_id must not be empty")
        if replay_batch_size <= 0:
            raise ValueError("replay_batch_size must be positive")
        self._event_store = event_store
        self._checkpoint_store = checkpoint_store
        self._stream_name = stream_name
        self._checkpoint_key = f"message-bus:{consumer_id}:{stream_name}"
        self._replay_batch_size = replay_batch_size

    @property
    def checkpoint(self) -> int:
        return self._checkpoint_store.read(self._checkpoint_key) or 0

    def publish(self, message: Message) -> list[str]:
        normalized = self._normalize_message(message)
        # A crash may happen after append but before dispatch/checkpoint.
        # Replaying from the checkpoint completes that interrupted delivery.
        with contextlib.suppress(DuplicateMessageError):
            self._event_store.append_to_stream(self._stream_name, (normalized,))
        return self.replay_pending()

    def replay_pending(self) -> list[str]:
        """Dispatch committed, uncheckpointed messages in stream order."""
        results: list[str] = []
        while True:
            position = self.checkpoint
            batch = self._event_store.read_stream(
                self._stream_name,
                from_position=position,
                max_count=self._replay_batch_size,
            )
            if not batch.events:
                return results
            for message in batch.events:
                results.extend(self._dispatch_normalized(message))
                next_position = (message.metadata.stream_position or 0) + 1
                self._checkpoint_store.store(self._checkpoint_key, next_position)

    def clear(self) -> None:
        """Clear volatile views only; durable history/checkpoint are intentionally retained."""
        self.messages.clear()
        self._output_dispatcher.clear()
