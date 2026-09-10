"""Conversation history for a single agent."""

from __future__ import annotations

import threading
from collections import deque
from collections.abc import Sequence

from aiwatcher_agentic.message import AssistantMessage, Message, UserMessage

#: Turns kept in memory. Prompts only ever read a window of the most recent ones,
#: so an unbounded list would grow for the whole life of a session.
DEFAULT_MAX_TURNS = 200


class History:
    """Conversation history stored as role-tagged turns.

    Bounded: the oldest turns are dropped once ``max_turns`` is reached.
    """

    def __init__(self, max_turns: int = DEFAULT_MAX_TURNS) -> None:
        if max_turns <= 0:
            raise ValueError("max_turns must be positive.")
        self._max_turns = max_turns
        self._history: deque[Message] = deque(maxlen=max_turns)
        self._lock = threading.Lock()

    @property
    def max_turns(self) -> int:
        return self._max_turns

    def add(self, message: Message) -> None:
        with self._lock:
            self._history.append(message)

    def get(self, length: int) -> Sequence[Message]:
        """Return the last ``length`` turns; an empty sequence for ``length <= 0``."""
        if length <= 0:
            return ()
        with self._lock:
            return tuple(self._history)[-length:]

    def store(self, user_message: str, assistant_message: str) -> str:
        with self._lock:
            self._history.append(UserMessage(user_message))
            self._history.append(AssistantMessage(assistant_message))
        return assistant_message

    def conversation_text(self, last_messages: int = 20) -> str:
        return "\n".join(msg.as_turn() for msg in self.get(last_messages))

    def clear(self) -> None:
        with self._lock:
            self._history.clear()

    def __len__(self) -> int:
        return len(self._history)
