from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass
from typing import Any

from aiwatcher_agentic.workflow._workflow import AgenticWorkflow
from aiwatcher_agentic.workflow.description import Description
from aiwatcher_agentic.workflow.ids import uuid7
from aiwatcher_agentic.workflow.message_bus import InMemoryMessageBus
from aiwatcher_agentic.workflow.messages import (
    ConversationData,
    Message,
    RecordedMessageMetadata,
    UserCommand,
    UserMessage,
)
from aiwatcher_agentic.workflow.output_handler import WorkflowOutputHandler
from aiwatcher_agentic.workflow.tracer import NoopWorkflowTracer, WorkflowTracer
from aiwatcher_agentic.workflow.turn_execution import TurnExecutor, TurnPlan, coerce_reply_text

type WorkflowHandler = Callable[[Message], Any]


@dataclass(slots=True)
class FunctionWorkflow(AgenticWorkflow):
    description: Description
    _handler: WorkflowHandler
    inputs: tuple[str, ...] = ("UserMessage",)

    def handle(self, message: Message) -> Any:
        return self._handler(message)

    def close(self) -> None:
        return None


class WorkflowRuntime:
    def __init__(
        self,
        *,
        bus: InMemoryMessageBus | None = None,
        tracer: WorkflowTracer | None = None,
        max_inline_bytes: int = 4096,
        chunk_bytes: int = 4096,
    ) -> None:
        self.bus = bus or InMemoryMessageBus()
        self._tracer = tracer or NoopWorkflowTracer()
        self.turn_executor = TurnExecutor(
            bus=self.bus,
            tracer=self._tracer,
            max_inline_bytes=max_inline_bytes,
            chunk_bytes=chunk_bytes,
        )
        self.workflows: dict[str, AgenticWorkflow] = {}

    def register_workflow(
        self,
        name: str,
        workflow: AgenticWorkflow | WorkflowHandler,
    ) -> AgenticWorkflow:
        normalized = self._coerce_workflow(name, workflow)
        self.workflows[name] = normalized
        return normalized

    def register_output_handler(self, handler: WorkflowOutputHandler) -> None:
        self.bus.register_output_handler(handler)

    def publish(self, message: Message) -> list[str]:
        return self.bus.publish(message)

    def publish_many(self, messages: list[Message]) -> list[str]:
        return self.bus.publish_many(messages)

    def flush_output_handlers(self) -> list[str]:
        return self.bus.flush_output_handlers()

    @property
    def message_log(self) -> list[Message]:
        return list(self.bus.messages)

    def publish_workflow_execution(
        self,
        *,
        incoming: UserMessage,
        workflow_name: str,
        execution: Any,
    ) -> str | None:
        return self.turn_executor.publish_workflow_execution(
            incoming=incoming,
            workflow_name=workflow_name,
            execution=execution,
        )

    def execute_turn(self, plan: TurnPlan) -> str:
        return self.turn_executor.execute(plan)

    def execute_workflow(
        self,
        workflow_name: str,
        incoming: UserMessage,
        *,
        selected_workflow: str | None = None,
        pre_messages: tuple[Message, ...] = (),
        trace_name: str | None = None,
        lifecycle_domain: str | None = None,
        lifecycle_target: str | None = None,
        output_agent_name: str | None = None,
    ) -> str:
        workflow = self.workflows[workflow_name]
        message = incoming.with_metadata(
            domain=lifecycle_domain or workflow_name,
            target=(
                lifecycle_target
                if lifecycle_target is not None
                else (incoming.metadata.target or workflow_name)
            ),
            runtime_id=incoming.metadata.runtime_id or self._new_id(),
            turn_id=incoming.metadata.turn_id or self._new_id(),
        )
        return self.execute_turn(
            TurnPlan(
                incoming=message,
                handler=workflow.handle,
                trace_name=trace_name or workflow_name,
                lifecycle_domain=lifecycle_domain or workflow_name,
                lifecycle_target=(
                    lifecycle_target if lifecycle_target is not None else message.metadata.target
                ),
                lifecycle_workflow_name=workflow_name,
                output_agent_name=output_agent_name or workflow_name,
                selected_workflow=selected_workflow,
                pre_messages=pre_messages,
            )
        )

    def handle(self, message: Message) -> str:
        if isinstance(message, UserMessage):
            workflow_name = message.metadata.target or message.metadata.domain
            if workflow_name in self.workflows:
                return self.execute_workflow(workflow_name, message)

        if isinstance(message, UserCommand):
            self.bus.publish(message)
            workflow_name = message.metadata.target or message.metadata.domain
            if workflow_name in self.workflows:
                return coerce_reply_text(self.workflows[workflow_name].handle(message))

        replies = self.bus.publish(message)
        return coerce_reply_text(replies[-1] if replies else "")

    def run_text(
        self,
        text: str,
        workflow_name: str,
        *,
        runtime_id: str | None = None,
        turn_id: str | None = None,
        source: str = "user",
    ) -> str:
        return self.execute_workflow(
            workflow_name,
            UserMessage(
                data=ConversationData(role="user", text=text),
                metadata=RecordedMessageMetadata(
                    runtime_id=runtime_id or self._new_id(),
                    turn_id=turn_id or self._new_id(),
                    domain=workflow_name,
                    source=source,
                    target=workflow_name,
                ),
            ),
        )

    def close(self) -> None:
        self.bus.close()

    @staticmethod
    def _new_id() -> str:
        return str(uuid7())

    @staticmethod
    def _coerce_workflow(
        name: str,
        workflow: AgenticWorkflow | WorkflowHandler,
    ) -> AgenticWorkflow:
        if hasattr(workflow, "handle"):
            return workflow  # type: ignore[return-value]
        return FunctionWorkflow(
            description=Description(
                agent_name=name,
                description=f"{name} workflow",
                capabilities=(),
            ),
            _handler=workflow,
        )
