from __future__ import annotations

import time
from dataclasses import dataclass, field


def _now_ns() -> int:
    return time.time_ns()


@dataclass(frozen=True, slots=True)
class AgentRegistration:
    agent_name: str
    capabilities: tuple[str, ...] = ()
    role: str = "worker"
    consumer_group: str = ""
    created_at_ns: int = field(default_factory=_now_ns)


@dataclass(frozen=True, slots=True)
class AgentHeartbeat:
    agent_name: str
    status: str = "alive"
    emitted_at_ns: int = field(default_factory=_now_ns)
