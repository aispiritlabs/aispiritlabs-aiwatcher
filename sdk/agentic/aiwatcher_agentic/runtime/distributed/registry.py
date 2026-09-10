"""Who can be handed a message, in the shape every registry answers with.

The broker registry that used to live here folded two Apache Iggy topics into a
picture of who was running. Its replacement is `AiwatcherServiceRegistry`, which
reads the definitions aiwatcher holds; the in-memory one is for tests. Both
answer with :class:`AgentSnapshot`, which is what a handler is given when it
asks `discovery.find(capability)`.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Protocol

__all__ = ["AgentSnapshot", "ServiceRegistry"]


@dataclass(frozen=True, slots=True)
class AgentSnapshot:
    agent_name: str
    capabilities: tuple[str, ...]
    role: str
    consumer_group: str
    status: str
    last_seen_ns: int


class ServiceRegistry(Protocol):
    """What discovery asks a registry. Registering is the service's own business."""

    def live_agents(self, *, max_age_seconds: float) -> list[AgentSnapshot]:
        """Every agent that may be handed a message, sorted by name."""
        ...

    def find_by_capability(
        self,
        capability: str,
        *,
        max_age_seconds: float,
    ) -> AgentSnapshot | None:
        """The first of those that says it can do *capability*."""
        ...
