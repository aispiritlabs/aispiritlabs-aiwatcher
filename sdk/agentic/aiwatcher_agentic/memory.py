"""Key-value memory an agent can carry between turns."""

from __future__ import annotations

import threading
from typing import Protocol, runtime_checkable

__all__ = ["InMemory", "Memory"]


@runtime_checkable
class Memory(Protocol):
    def retrieve(self, key: str, length: int = 0) -> str:
        """Return the stored value, truncated to ``length`` characters when positive."""
        ...

    def store(self, key: str, value: str) -> None: ...

    def summary(self) -> str:
        """Render the memory as prompt context. Empty when there is nothing to add."""
        ...


class InMemory:
    """Process-local memory. Values live only as long as the agent."""

    def __init__(self, max_summary_chars: int = 1000) -> None:
        self._memory: dict[str, str] = {}
        self._max_summary_chars = max_summary_chars
        self._lock = threading.Lock()

    def retrieve(self, key: str, length: int = 0) -> str:
        with self._lock:
            value = self._memory.get(key, "")
        return value[:length] if length > 0 else value

    def store(self, key: str, value: str) -> None:
        with self._lock:
            self._memory[key] = value

    def summary(self) -> str:
        with self._lock:
            items = list(self._memory.items())
        if not items:
            return ""

        lines = [f"{key}: {value}" for key, value in items]
        rendered = "\n".join(lines)
        if len(rendered) <= self._max_summary_chars:
            return rendered
        return rendered[: self._max_summary_chars - 1] + "…"

    def clear(self) -> None:
        with self._lock:
            self._memory.clear()
