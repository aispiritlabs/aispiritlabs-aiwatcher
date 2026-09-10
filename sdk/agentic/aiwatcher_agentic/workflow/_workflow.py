from __future__ import annotations

from collections.abc import Sequence
from typing import Protocol, runtime_checkable

from aiwatcher_agentic.workflow.description import Description
from aiwatcher_agentic.workflow.execution import WorkflowExecution
from aiwatcher_agentic.workflow.messages import Message


@runtime_checkable
class AgenticWorkflow(Protocol):
    description: Description
    inputs: Sequence[str]

    def handle(self, message: Message) -> WorkflowExecution | str: ...

    def close(self) -> None: ...
