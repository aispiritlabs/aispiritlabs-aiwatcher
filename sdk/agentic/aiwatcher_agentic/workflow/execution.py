from __future__ import annotations

from dataclasses import dataclass

from aiwatcher_agentic.workflow.agent_run import AgentRun, ToolRun
from aiwatcher_agentic.workflow.messages import Message


@dataclass(frozen=True, slots=True)
class ExecutionTurnRecord:
    agent_result: AgentRun
    tool_results: tuple[ToolRun, ...] = ()


@dataclass(frozen=True, slots=True)
class WorkflowExecution:
    text: str
    agent_result: AgentRun | None = None
    tool_results: tuple[ToolRun, ...] = ()
    emitted_events: tuple[Message, ...] = ()
    recorded_turns: tuple[ExecutionTurnRecord, ...] = ()
