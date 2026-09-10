"""What the engine reads off an agent's turn, and nothing else.

A turn's result becomes recorded messages — the prompt it was asked, each tool
call and its output, the reply — and those are built from a handful of fields.
They are declared here as read-only protocols rather than imported from the
agent that produced them, so the engine runs any agent that answers them:
`agentic`'s ``AgentResult`` and ``ToolRunResult`` do, structurally, and a test
double needs no model stack to.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from typing import Any, Protocol

from aiwatcher_agentic.workflow.trace import TraceSnapshot

#: A tool name and the arguments it was called with.
type ToolCall = tuple[str, Mapping[str, Any]]


class ModelUsage(Protocol):
    """One model request's cost, as a prompt snapshot records it."""

    @property
    def model(self) -> str: ...
    @property
    def prompt_tokens(self) -> int: ...
    @property
    def completion_tokens(self) -> int: ...
    @property
    def total_tokens(self) -> int: ...
    @property
    def latency_ms(self) -> float: ...
    @property
    def finish_reason(self) -> str: ...


class AgentPrompt(Protocol):
    """The system prompt a turn ran on, named and hashed."""

    @property
    def text(self) -> str: ...
    @property
    def prompt_name(self) -> str | None: ...
    @property
    def prompt_hash(self) -> str: ...
    @property
    def tool_schema(self) -> Sequence[Mapping[str, Any]]: ...


class ToolRun(Protocol):
    """One tool call and what it answered."""

    @property
    def tool_call(self) -> ToolCall: ...
    @property
    def output(self) -> str: ...
    @property
    def trace(self) -> TraceSnapshot | None: ...


class AgentRun(Protocol):
    """One agent turn's outcome."""

    @property
    def run_id(self) -> str | None: ...
    @property
    def trace(self) -> TraceSnapshot | None: ...
    @property
    def attempt_no(self) -> int | None: ...
    @property
    def loop_iteration(self) -> int | None: ...
    @property
    def tool_calls(self) -> Sequence[ToolCall]: ...
    @property
    def request_usage(self) -> ModelUsage: ...
    @property
    def prompt_snapshot(self) -> AgentPrompt | None: ...
    @property
    def model_provider(self) -> str: ...
    @property
    def generation_config_hash(self) -> str: ...
