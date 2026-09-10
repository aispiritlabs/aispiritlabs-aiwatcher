from __future__ import annotations

import pytest

from aiwatcher_agentic.runtime.distributed.discovery import AgenticServiceDiscovery
from aiwatcher_agentic.runtime.distributed.registry import AgentSnapshot
from aiwatcher_agentic.runtime.distributed.transport import BEGINNING, ConsumedRecord
from aiwatcher_agentic.workflow.messages import Message


class _FakeRegistry:
    """Registry that returns canned snapshots without needing a broker."""

    def __init__(self, agents: list[AgentSnapshot] | None = None) -> None:
        self._agents = agents or []

    def find_by_capability(
        self,
        capability: str,
        *,
        max_age_seconds: float,
    ) -> AgentSnapshot | None:
        del max_age_seconds
        for agent in self._agents:
            if capability in agent.capabilities:
                return agent
        return None

    def live_agents(self, *, max_age_seconds: float) -> list[AgentSnapshot]:
        del max_age_seconds
        return list(self._agents)


class _FakeTransport:
    """A transport discovery holds and never sends on: finding reads the registry."""

    def __init__(self) -> None:
        self.closed = False

    def reply_address(self, name: str) -> str:
        return name

    def publish_message(self, message: Message) -> str:
        raise AssertionError("discovery never sends")

    def last_message_id(self, target: str) -> str:
        return BEGINNING

    def read_messages(
        self,
        target: str,
        *,
        after_id: str = BEGINNING,
        block_ms: int = 1_000,
        count: int = 10,
    ) -> list[ConsumedRecord]:
        return []

    def close(self) -> None:
        self.closed = True


_SEARCH_AGENT = AgentSnapshot(
    agent_name="search",
    capabilities=("web-search",),
    role="worker",
    consumer_group="search",
    status="alive",
    last_seen_ns=0,
)


def _make_discovery(
    agents: list[AgentSnapshot] | None = None,
) -> AgenticServiceDiscovery:
    transport = _FakeTransport()
    registry = _FakeRegistry(agents)
    return AgenticServiceDiscovery(transport, registry, liveness_ttl_seconds=20.0)


def test_find_returns_agent_snapshot_when_found() -> None:
    discovery = _make_discovery([_SEARCH_AGENT])

    result = discovery.find("web-search")

    assert result.agent_name == "search"
    assert "web-search" in result.capabilities


def test_find_raises_when_no_live_agent() -> None:
    discovery = _make_discovery([])

    with pytest.raises(RuntimeError, match="No live agent with capability 'web-search'"):
        discovery.find("web-search")


def test_find_optional_returns_none_when_not_found() -> None:
    discovery = _make_discovery([])

    assert discovery.find_optional("web-search") is None


def test_find_optional_returns_snapshot_when_found() -> None:
    discovery = _make_discovery([_SEARCH_AGENT])

    result = discovery.find_optional("web-search")
    assert result is not None
    assert result.agent_name == "search"


def test_live_agents_delegates_to_registry() -> None:
    discovery = _make_discovery([_SEARCH_AGENT])

    agents = discovery.live_agents()

    assert len(agents) == 1
    assert agents[0].agent_name == "search"


def test_close_closes_transport() -> None:
    transport = _FakeTransport()
    discovery = AgenticServiceDiscovery(transport, _FakeRegistry())

    discovery.close()

    assert transport.closed is True
