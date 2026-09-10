"""What every distributed transport hands its callers, whichever one it is.

Two implement it. `AiwatcherTransport` runs every hop through an aiwatcher
server — a hop is a run of the target agent's workflow, and a reply is a row in
the client's mailbox (see `aiwatcher.py`). `InMemoryTransport` keeps them in
this process, for tests. Apache Iggy, through `laser-sdk`, was a third, and was
removed in AW-2's Phase F: everything it did for a message is what aiwatcher's
claims already do for a step.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Protocol

from aiwatcher_agentic.workflow.messages import Message, normalize_recorded_message

__all__ = [
    "BEGINNING",
    "ConsumedRecord",
    "MalformedRecord",
    "MessageTransport",
    "normalize_distributed_message",
]

#: Before every entry, in the spelling every caller already passes.
BEGINNING = "0-0"


@dataclass(frozen=True, slots=True)
class ConsumedRecord:
    stream: str
    entry_id: str
    record: object


@dataclass(frozen=True, slots=True)
class MalformedRecord:
    raw_payload: str
    error_type: str
    error_message: str


def normalize_distributed_message(message: Message) -> Message:
    return normalize_recorded_message(message)


class MessageTransport(Protocol):
    """The half of a transport a client uses: send, and tail a reply address."""

    def reply_address(self, name: str) -> str:
        """Where replies to a client called *name* are sent, and read from."""
        ...

    def publish_message(self, message: Message) -> str:
        """Hand *message* to its target, and say where it landed."""
        ...

    def last_message_id(self, target: str) -> str:
        """The cursor at the end of *target*'s replies, so a reader skips the past."""
        ...

    def read_messages(
        self,
        target: str,
        *,
        after_id: str = BEGINNING,
        block_ms: int = 1_000,
        count: int = 10,
    ) -> list[ConsumedRecord]:
        """What arrived at *target* after *after_id*, waiting up to *block_ms* for some."""
        ...

    def close(self) -> None:
        """Release whatever the transport holds."""
        ...
