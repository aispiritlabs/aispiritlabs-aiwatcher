from __future__ import annotations

from collections import deque
from collections.abc import Callable, Sequence
from typing import Protocol, TypeVar

from aiwatcher_agentic.workflow.messages import Message

T = TypeVar("T")


class MessageStream(Protocol):
    """Stream wiadomosci sesji. Kazde wejscie/wyjscie, kazda decyzja."""

    def append(self, message: Message) -> None: ...

    def read_next(self) -> Message | None: ...

    def is_empty(self) -> bool: ...

    def all_messages(self) -> Sequence[Message]: ...


class InMemoryMessageStream:
    """Lokalna implementacja — deque. Dla dev/test i lokalnych agentow."""

    def __init__(self) -> None:
        self._queue: deque[Message] = deque()
        self._history: list[Message] = []

    def append(self, message: Message) -> None:
        self._queue.append(message)
        self._history.append(message)

    def read_next(self) -> Message | None:
        if self._queue:
            return self._queue.popleft()
        return None

    def is_empty(self) -> bool:
        return len(self._queue) == 0

    def all_messages(self) -> Sequence[Message]:
        return list(self._history)


def project[T](stream: MessageStream, projection: Callable[[Sequence[Message]], T]) -> T:
    """Kontekst = projekcja streama. Buduje read model z historii wiadomosci."""
    return projection(stream.all_messages())
