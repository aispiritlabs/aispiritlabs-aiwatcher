"""The agent runtime, moved from `ai_spirit_agent`'s `agentic_runtime` (AW-2).

Where an application composes its agents (`runtime.AgenticRuntime`), where
their conversation is kept (`storage`), how it becomes fine-tuning rows
(`fine_tuning`), how each agent is hosted on aiwatcher as a workflow of its own
(`hosted`), and how agents in separate processes hand each other work through
aiwatcher (`distributed`). Every module keeps the name it had there.

It reaches no model and no tracing backend by itself: a tracer is handed to it
through `LLMTracer` and its settings through `RuntimeSettings`. `aiwatcher-sdk`
is imported only by `hosted` and `distributed.aiwatcher`, when they are used,
which is the `[aiwatcher]` extra — so this package does not import either.
"""

from __future__ import annotations

from aiwatcher_agentic.runtime.config import DistributedSettings, RuntimeConfig, RuntimeSettings
from aiwatcher_agentic.runtime.fine_tuning import (
    export_agent_fine_tuning_rows,
    export_router_fine_tuning_rows,
    write_jsonl,
)
from aiwatcher_agentic.runtime.protocols import RuntimeProtocol
from aiwatcher_agentic.runtime.runtime import (
    AgenticRuntime,
    RouterProtocol,
    RuntimeServices,
    ShutdownStep,
)
from aiwatcher_agentic.runtime.storage.sqlite_store import SQLiteMessageStore

__all__ = [
    "AgenticRuntime",
    "DistributedSettings",
    "RouterProtocol",
    "RuntimeConfig",
    "RuntimeProtocol",
    "RuntimeServices",
    "RuntimeSettings",
    "SQLiteMessageStore",
    "ShutdownStep",
    "export_agent_fine_tuning_rows",
    "export_router_fine_tuning_rows",
    "write_jsonl",
]
