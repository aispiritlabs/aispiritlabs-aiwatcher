"""What the runtime and distributed agents read from their configuration.

Two protocols, each naming only what its reader reads, and one value that
answers both with defaults. Protocols rather than a settings class, because an
application already has one — `ai_spirit_agent`'s reads `.env` through
pydantic-settings and names the models it runs — and it satisfies these by
having the fields, without this distribution importing pydantic or learning
which models exist.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Protocol

__all__ = ["DistributedSettings", "RuntimeConfig", "RuntimeSettings"]


class RuntimeSettings(Protocol):
    """What `AgenticRuntime` reads: where its two stores are, and how it streams."""

    @property
    def event_store_path(self) -> str | None: ...

    @property
    def message_store_path(self) -> str | None: ...

    @property
    def message_store_batch_size(self) -> int: ...

    @property
    def message_store_flush_interval_seconds(self) -> float: ...

    @property
    def message_stream_inline_bytes(self) -> int: ...

    @property
    def message_stream_chunk_bytes(self) -> int: ...


class DistributedSettings(Protocol):
    """What `AgenticServiceDiscovery` reads: the prefix, and what a delivery may cost."""

    @property
    def distributed_prefix(self) -> str: ...

    @property
    def agent_liveness_ttl_seconds(self) -> float: ...

    @property
    def distributed_max_delivery_attempts(self) -> int: ...

    @property
    def distributed_retry_min_idle_ms(self) -> int: ...

    @property
    def distributed_require_event_store(self) -> bool: ...

    @property
    def event_store_path(self) -> str | None: ...


@dataclass(frozen=True, slots=True, kw_only=True)
class RuntimeConfig:
    """Both, with the defaults `ai_spirit_agent`'s settings had when this moved."""

    event_store_path: str | None = None
    message_store_path: str | None = None
    message_store_batch_size: int = 64
    message_store_flush_interval_seconds: float = 0.05
    message_stream_inline_bytes: int = 4096
    message_stream_chunk_bytes: int = 4096
    distributed_prefix: str = "agentic"
    agent_liveness_ttl_seconds: float = 20.0
    distributed_max_delivery_attempts: int = 3
    distributed_retry_min_idle_ms: int = 5_000
    distributed_require_event_store: bool = False
