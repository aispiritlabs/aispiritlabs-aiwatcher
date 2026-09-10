"""Distributed agents on aiwatcher: a hop is a run of the target agent's workflow.

`DistributedAgenticRuntime`, the chat front end `ai_spirit_agent` puts on this,
stayed with that application: it answers in Polish and names its chat and image
modes, and neither is something every application built on this has.
"""

from aiwatcher_agentic.runtime.distributed.aiwatcher import (
    AiwatcherService,
    AiwatcherServiceRegistry,
    AiwatcherTransport,
    UnknownAgentError,
)
from aiwatcher_agentic.runtime.distributed.client import DistributedChatClient
from aiwatcher_agentic.runtime.distributed.contracts import AgentHeartbeat, AgentRegistration
from aiwatcher_agentic.runtime.distributed.discovery import AgenticServiceDiscovery
from aiwatcher_agentic.runtime.distributed.registry import AgentSnapshot, ServiceRegistry
from aiwatcher_agentic.runtime.distributed.serialization import register_record_types
from aiwatcher_agentic.runtime.distributed.service import (
    AgentService,
    DeliveryMetrics,
    PermanentMessageError,
)
from aiwatcher_agentic.runtime.distributed.transport import (
    ConsumedRecord,
    MalformedRecord,
    MessageTransport,
)

__all__ = [
    "AgentHeartbeat",
    "AgentRegistration",
    "AgentService",
    "AgentSnapshot",
    "AgenticServiceDiscovery",
    "AiwatcherService",
    "AiwatcherServiceRegistry",
    "AiwatcherTransport",
    "ConsumedRecord",
    "DeliveryMetrics",
    "DistributedChatClient",
    "MalformedRecord",
    "MessageTransport",
    "PermanentMessageError",
    "ServiceRegistry",
    "UnknownAgentError",
    "register_record_types",
]
