"""The one entry point to distributed agents: find one, run one, ask one.

Workshop code and `personal_assistant` hold this and nothing below it. Below it
is aiwatcher (`aiwatcher.py`) in a deployment and a dict of lists
(`in_memory_transport.py`) in a test, and neither is named above this line.
"""

from __future__ import annotations

from collections.abc import Callable, Sequence
from typing import TYPE_CHECKING

from aiwatcher_agentic.runtime.config import DistributedSettings, RuntimeConfig
from aiwatcher_agentic.runtime.distributed.client import DistributedChatClient
from aiwatcher_agentic.runtime.distributed.registry import AgentSnapshot, ServiceRegistry
from aiwatcher_agentic.runtime.distributed.transport import MessageTransport
from aiwatcher_agentic.workflow import SQLiteEventStore, SQLiteProcessorLock
from aiwatcher_agentic.workflow.messages import Message

if TYPE_CHECKING:
    from aiwatcher_agentic.runtime.distributed.service import AgentService

type ServiceHandler = Callable[[Message, "AgenticServiceDiscovery"], Sequence[Message]]
type CloseHook = Callable[[], None]


class AgenticServiceDiscovery:
    """Facade for distributed agent infrastructure.

    Workshop participants use this as the single entry point — never touching
    a transport or a registry directly.
    """

    def __init__(
        self,
        transport: MessageTransport,
        registry: ServiceRegistry,
        *,
        liveness_ttl_seconds: float = 15.0,
        settings: DistributedSettings | None = None,
    ) -> None:
        self._transport = transport
        self._registry = registry
        self._liveness_ttl_seconds = liveness_ttl_seconds
        self._settings: DistributedSettings = settings if settings is not None else RuntimeConfig()

    @classmethod
    def from_settings(cls, settings: DistributedSettings) -> AgenticServiceDiscovery:
        """Distributed agents on the aiwatcher at `AIWATCHER_URL`, as *settings* say.

        *settings* is the application's own — it read them from wherever it
        reads configuration — and every service this discovery creates is held
        to the delivery budget they name.
        """
        from aiwatcher_agentic.runtime.distributed.aiwatcher import (
            AiwatcherServiceRegistry,
            AiwatcherTransport,
        )

        transport = AiwatcherTransport.from_env(prefix=settings.distributed_prefix)
        return cls(
            transport,
            AiwatcherServiceRegistry(transport),
            liveness_ttl_seconds=settings.agent_liveness_ttl_seconds,
            settings=settings,
        )

    # ------------------------------------------------------------------
    # Discovery — the workshop-facing API
    # ------------------------------------------------------------------

    def find(self, capability: str) -> AgentSnapshot:
        """Find a live agent by capability. Raises ``RuntimeError`` if none found."""
        agent = self._registry.find_by_capability(
            capability,
            max_age_seconds=self._liveness_ttl_seconds,
        )
        if agent is None:
            raise RuntimeError(f"No live agent with capability '{capability}' is registered.")
        return agent

    def find_optional(self, capability: str) -> AgentSnapshot | None:
        """Find a live agent by capability, returning ``None`` if none found."""
        return self._registry.find_by_capability(
            capability,
            max_age_seconds=self._liveness_ttl_seconds,
        )

    def live_agents(self) -> list[AgentSnapshot]:
        """Return all live agents sorted by name."""
        return self._registry.live_agents(max_age_seconds=self._liveness_ttl_seconds)

    # ------------------------------------------------------------------
    # Factories
    # ------------------------------------------------------------------

    def create_service(
        self,
        name: str,
        *,
        capabilities: tuple[str, ...],
        handler: ServiceHandler,
        role: str = "worker",
        heartbeat_seconds: float = 5.0,
        close_hook: CloseHook | None = None,
        min_idle_ms: int | None = None,
    ) -> AgentService:
        """One agent's service, on whichever transport this discovery holds.

        On aiwatcher it is a worker claiming the agent's own queue, and the
        server owns delivery, retries and the dead-letter sink — so
        ``heartbeat_seconds`` and ``min_idle_ms``, which were the broker's
        knobs, are read only by the in-memory ``DistributedService``.
        """
        from aiwatcher_agentic.runtime.distributed.aiwatcher import (
            AiwatcherService,
            AiwatcherTransport,
        )
        from aiwatcher_agentic.runtime.distributed.service import DistributedService

        settings = self._settings
        if isinstance(self._transport, AiwatcherTransport):
            return AiwatcherService(
                agent_name=name,
                capabilities=capabilities,
                discovery=self,
                handler=handler,
                transport=self._transport,
                role=role,
                close_hook=close_hook,
                max_delivery_attempts=settings.distributed_max_delivery_attempts,
            )

        if settings.distributed_require_event_store and not settings.event_store_path:
            raise RuntimeError(
                "EVENT_STORE_PATH is required when DISTRIBUTED_REQUIRE_EVENT_STORE=true"
            )
        event_store = (
            SQLiteEventStore(settings.event_store_path) if settings.event_store_path else None
        )
        return DistributedService(
            agent_name=name,
            capabilities=capabilities,
            discovery=self,
            handler=handler,
            role=role,
            heartbeat_seconds=heartbeat_seconds,
            close_hook=close_hook,
            min_idle_ms=(
                settings.distributed_retry_min_idle_ms if min_idle_ms is None else min_idle_ms
            ),
            event_store=event_store,
            workflow_lock=(
                SQLiteProcessorLock(event_store.path) if event_store is not None else None
            ),
            max_delivery_attempts=settings.distributed_max_delivery_attempts,
        )

    def create_client(
        self,
        *,
        entry_agent: str = "planner",
        source: str = "chat",
        timeout_seconds: float = 60.0,
        domain: str = "lab6",
    ) -> DistributedChatClient:
        """Create a ``DistributedChatClient`` using the owned transport."""
        return DistributedChatClient(
            self._transport,
            entry_agent=entry_agent,
            source=self._transport.reply_address(source),
            timeout_seconds=timeout_seconds,
            domain=domain,
        )

    # ------------------------------------------------------------------
    # Internal — used by DistributedService, not by workshop participants
    # ------------------------------------------------------------------

    @property
    def transport(self) -> MessageTransport:
        return self._transport

    @property
    def registry(self) -> ServiceRegistry:
        return self._registry

    @property
    def liveness_ttl_seconds(self) -> float:
        return self._liveness_ttl_seconds

    def close(self) -> None:
        """Close the underlying transport."""
        self._transport.close()
