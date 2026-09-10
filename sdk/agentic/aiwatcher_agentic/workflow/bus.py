from __future__ import annotations

import logging
from collections.abc import Callable, Sequence
from typing import Protocol

from aiwatcher_agentic.workflow.messages import Event, UserCommand

logger = logging.getLogger(__name__)

type CommandHandler = Callable[[UserCommand], None]
type EventHandler = Callable[[Event], None]


# ---------------------------------------------------------------------------
# Protocols
# ---------------------------------------------------------------------------


class CommandBus(Protocol):
    """Exactly-one handler per command type. Raises if no handler registered."""

    def register(self, command_type: str, handler: CommandHandler) -> None: ...

    def send(self, command: UserCommand) -> None: ...


class EventBus(Protocol):
    """Fan-out to zero or more subscribers per event type."""

    def subscribe(self, event_type: str, handler: EventHandler) -> None: ...

    def publish(self, event: Event) -> None: ...

    def publish_many(self, events: Sequence[Event]) -> None: ...


# ---------------------------------------------------------------------------
# In-memory implementations
# ---------------------------------------------------------------------------


class InMemoryCommandBus:
    """Exactly-one handler per command type."""

    def __init__(self) -> None:
        self._handlers: dict[str, CommandHandler] = {}

    def register(self, command_type: str, handler: CommandHandler) -> None:
        if command_type in self._handlers:
            raise ValueError(f"Command type '{command_type}' already has a registered handler")
        self._handlers[command_type] = handler

    def send(self, command: UserCommand) -> None:
        handler = self._handlers.get(command.type)
        if handler is None:
            raise ValueError(f"No handler registered for command type '{command.type}'")
        handler(command)


class InMemoryEventBus:
    """Fan-out: each event type can have multiple subscribers."""

    def __init__(self) -> None:
        self._subscribers: dict[str, list[EventHandler]] = {}

    def subscribe(self, event_type: str, handler: EventHandler) -> None:
        if event_type not in self._subscribers:
            self._subscribers[event_type] = []
        self._subscribers[event_type].append(handler)

    def publish(self, event: Event) -> None:
        handlers = self._subscribers.get(event.type, [])
        for handler in handlers:
            try:
                handler(event)
            except Exception:
                logger.exception("Event handler failed for event type '%s'", event.type)

    def publish_many(self, events: Sequence[Event]) -> None:
        for event in events:
            self.publish(event)
