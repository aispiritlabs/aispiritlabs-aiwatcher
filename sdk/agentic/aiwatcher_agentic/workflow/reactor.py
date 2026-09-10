from __future__ import annotations

from collections.abc import Callable, Sequence
from dataclasses import dataclass
from typing import Any, Protocol

from aiwatcher_agentic.workflow.messages import (
    AssistantMessage,
    Message,
)


class Reactor(Protocol):
    """Side effect po fakcie biznesowym: input message -> invocation -> output message."""

    def can_handle(self, command: Message) -> bool: ...

    def invoke(self, command: Message) -> Message: ...


MessageRouter = Callable[[Message], Sequence[Message]]
"""Stateless message router: takes a message, returns commands/events for the stream."""

TechnicalRoutingFn = Callable[[Message], Reactor | None]
"""Mapowanie komend na Reactors: przyjmuje komende, zwraca ktory Reactor obsluguje."""


@dataclass(frozen=True, kw_only=True)
class LLMResponse(AssistantMessage):
    """Response from LLM with tool execution details for MessageRouter inspection."""

    kind: str = "llm_response"
    tool_calls: tuple[tuple[str, dict[str, Any]], ...] = ()
    _agent_result: Any = None  # AgentResult for observability (prompt_snapshot, usage)
    _tool_results: tuple[Any, ...] = ()  # ToolRunResult objects

    @property
    def has_tool_calls(self) -> bool:
        return len(self.tool_calls) > 0
