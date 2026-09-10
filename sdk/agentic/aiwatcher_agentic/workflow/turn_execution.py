from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass
from typing import Any

from aiwatcher_agentic.workflow.execution import ExecutionTurnRecord, WorkflowExecution
from aiwatcher_agentic.workflow.message_bus import InMemoryMessageBus
from aiwatcher_agentic.workflow.messages import (
    ConversationData,
    Event,
    Message,
    RecordedMessageMetadata,
    TurnCompleted,
    TurnStarted,
    UserMessage,
)
from aiwatcher_agentic.workflow.streaming import (
    build_assistant_messages,
    build_prompt_snapshot_message,
    build_tool_messages,
    resolve_trace_context,
)
from aiwatcher_agentic.workflow.trace import TraceSnapshot, TracingContext
from aiwatcher_agentic.workflow.tracer import WorkflowTracer

type TurnHandler = Callable[[UserMessage], Any]


@dataclass(frozen=True, slots=True)
class TurnPlan:
    incoming: UserMessage
    handler: TurnHandler
    trace_name: str
    lifecycle_domain: str
    lifecycle_target: str | None
    lifecycle_workflow_name: str
    output_agent_name: str
    selected_workflow: str | None = None
    pre_messages: tuple[Message, ...] = ()


def coerce_execution(reply: Any) -> WorkflowExecution:
    if isinstance(reply, WorkflowExecution):
        return reply
    if isinstance(reply, str):
        return WorkflowExecution(text=reply)
    if reply is None:
        return WorkflowExecution(text="")
    if isinstance(reply, tuple) and len(reply) > 0:
        return WorkflowExecution(text=coerce_execution(reply[0]).text)
    return WorkflowExecution(text=str(reply))


def coerce_reply_text(reply: Any) -> str:
    return coerce_execution(reply).text


class TurnExecutor:
    def __init__(
        self,
        *,
        bus: InMemoryMessageBus,
        tracer: WorkflowTracer,
        max_inline_bytes: int = 4096,
        chunk_bytes: int = 4096,
    ) -> None:
        self.bus = bus
        self._tracer = tracer
        self._max_inline_bytes = max_inline_bytes
        self._chunk_bytes = chunk_bytes

    @staticmethod
    def _trace_from_message(message: Message) -> TraceSnapshot | None:
        if not message.metadata.trace_id and not message.metadata.span_id:
            return None
        return TraceSnapshot(
            session_id=message.metadata.session_id or message.metadata.runtime_id,
            trace_id=message.metadata.trace_id or "",
            span_id=message.metadata.span_id or "",
            parent_span_id=message.metadata.parent_span_id or "",
            span_name=message.metadata.span_name or "",
            span_type=message.metadata.span_type or "",
        )

    def _current_trace_snapshot(self, incoming: UserMessage) -> TraceSnapshot | None:
        trace = self._tracer.current_trace
        if trace is not None:
            return trace
        trace_id = self._tracer.current_trace_id
        if trace_id is None:
            return None
        # Partial snapshot: only trace_id is known; span fields default to "".
        return TraceSnapshot(
            session_id=incoming.metadata.session_id or incoming.metadata.runtime_id,
            trace_id=trace_id,
        )

    @staticmethod
    def _bind_to_turn(
        message: Message,
        turn_id: str,
        runtime_id: str,
        session_id: str,
        trace: TraceSnapshot | None = None,
    ) -> Message:
        updates: dict[str, Any] = {}
        if not message.metadata.runtime_id:
            updates["runtime_id"] = runtime_id
        if not message.metadata.session_id:
            updates["session_id"] = session_id
        if not message.metadata.turn_id:
            updates["turn_id"] = turn_id
        if trace is not None:
            if not message.metadata.trace_id:
                updates["trace_id"] = trace.trace_id or None
            if not message.metadata.span_id:
                updates["span_id"] = trace.span_id or None
            if not message.metadata.parent_span_id:
                updates["parent_span_id"] = trace.parent_span_id or None
            if not message.metadata.span_name:
                updates["span_name"] = trace.span_name or None
            if not message.metadata.span_type:
                updates["span_type"] = trace.span_type or None
        if updates:
            return message.with_metadata(**updates)
        return message

    def _publish_turn_started(
        self,
        *,
        runtime_id: str,
        session_id: str,
        turn_id: str,
        domain: str,
        target: str | None,
        workflow_name: str,
        trace: TraceSnapshot | None = None,
    ) -> None:
        self.bus.publish(
            TurnStarted(
                data={"workflow": workflow_name},
                metadata=RecordedMessageMetadata(
                    runtime_id=runtime_id,
                    session_id=session_id,
                    turn_id=turn_id,
                    domain=domain,
                    source="runtime",
                    target=target,
                    scope="transport",
                    trace=trace,
                ),
            )
        )

    def _publish_turn_completed(
        self,
        *,
        runtime_id: str,
        session_id: str,
        turn_id: str,
        domain: str,
        target: str | None,
        workflow_name: str,
        status: str,
        final_message_id: str | None = None,
        error: Exception | None = None,
        trace: TraceSnapshot | None = None,
    ) -> None:
        payload: dict[str, Any] = {"workflow": workflow_name}
        if final_message_id is not None:
            payload["final_message_id"] = final_message_id
        if error is not None:
            payload["error_type"] = type(error).__name__
            payload["error_message"] = str(error)
        self.bus.publish(
            TurnCompleted(
                data=payload,
                metadata=RecordedMessageMetadata(
                    runtime_id=runtime_id,
                    session_id=session_id,
                    turn_id=turn_id,
                    domain=domain,
                    source="runtime",
                    target=target,
                    scope="transport",
                    status=status,
                    trace=trace,
                ),
            )
        )

    def publish_workflow_execution(
        self,
        *,
        incoming: UserMessage,
        workflow_name: str,
        execution: WorkflowExecution,
    ) -> str | None:
        reply_to = getattr(incoming.metadata, "message_id", "") or None

        turns = execution.recorded_turns
        if not turns and execution.agent_result is not None:
            turns = (
                ExecutionTurnRecord(
                    agent_result=execution.agent_result,
                    tool_results=execution.tool_results,
                ),
            )

        for turn in turns:
            turn_ctx = resolve_trace_context(
                incoming=incoming,
                trace=turn.agent_result.trace,
                attempt_no=turn.agent_result.attempt_no,
                loop_iteration=turn.agent_result.loop_iteration,
            )
            snapshot_message = build_prompt_snapshot_message(
                incoming=incoming,
                agent_name=workflow_name,
                snapshot=turn.agent_result.prompt_snapshot,
                agent_run_id=turn.agent_result.run_id,
                agent_result=turn.agent_result,
                ctx=turn_ctx,
            )
            if snapshot_message is not None:
                self.bus.publish(snapshot_message)

            tool_messages, reply_to = build_tool_messages(
                incoming=incoming,
                agent_name=workflow_name,
                agent_result=turn.agent_result,
                tool_results=turn.tool_results,
                reply_to_message_id=reply_to,
            )
            self.bus.publish_many(tool_messages)

        if execution.emitted_events:
            bound_events = [
                self._bind_to_turn(
                    event,
                    incoming.metadata.turn_id,
                    incoming.metadata.runtime_id,
                    incoming.metadata.session_id or incoming.metadata.runtime_id,
                    self._trace_from_message(incoming),
                )
                for event in execution.emitted_events
            ]
            self.bus.publish_many(bound_events)

        final_agent_result = turns[-1].agent_result if turns else execution.agent_result
        final_ctx = resolve_trace_context(
            incoming=incoming,
            trace=final_agent_result.trace if final_agent_result else None,
            attempt_no=final_agent_result.attempt_no if final_agent_result else None,
            loop_iteration=final_agent_result.loop_iteration if final_agent_result else None,
        )
        assistant_messages, final_message_id = build_assistant_messages(
            incoming=incoming,
            agent_name=workflow_name,
            text=execution.text,
            reply_to_message_id=reply_to,
            agent_run_id=final_agent_result.run_id if final_agent_result else None,
            max_inline_bytes=self._max_inline_bytes,
            chunk_bytes=self._chunk_bytes,
            ctx=final_ctx,
        )
        self.bus.publish_many(assistant_messages)
        return final_message_id

    def execute(self, plan: TurnPlan) -> str:
        if plan.pre_messages:
            self.bus.publish_many(list(plan.pre_messages))

        with self._tracer.workflow(
            name=plan.trace_name,
            session_id=plan.incoming.metadata.session_id or plan.incoming.metadata.runtime_id,
            input=plan.incoming.data.text
            if isinstance(plan.incoming.data, ConversationData)
            else None,
            metadata={"turn_id": plan.incoming.metadata.turn_id},
            tracing_context=TracingContext(
                session_id=plan.incoming.metadata.session_id or plan.incoming.metadata.runtime_id,
                runtime_id=plan.incoming.metadata.runtime_id,
                turn_id=plan.incoming.metadata.turn_id,
                workflow=plan.lifecycle_workflow_name,
                domain=plan.lifecycle_domain,
            ),
        ) as span:
            trace = self._current_trace_snapshot(plan.incoming)
            incoming = plan.incoming.with_metadata(
                session_id=(
                    trace.session_id
                    if trace is not None and trace.session_id
                    else plan.incoming.metadata.session_id or plan.incoming.metadata.runtime_id
                ),
                trace_id=trace.trace_id if trace is not None else None,
                span_id=trace.span_id if trace is not None else None,
                parent_span_id=trace.parent_span_id if trace is not None else None,
                span_name=trace.span_name if trace is not None else None,
                span_type=trace.span_type if trace is not None else None,
            )

            self._publish_turn_started(
                runtime_id=incoming.metadata.runtime_id,
                session_id=incoming.metadata.session_id,
                turn_id=incoming.metadata.turn_id,
                domain=plan.lifecycle_domain,
                target=plan.lifecycle_target,
                workflow_name=plan.lifecycle_workflow_name,
                trace=trace,
            )
            if plan.selected_workflow is not None:
                self.bus.publish(
                    Event(
                        type="workflow_selected",
                        data={"workflow": plan.selected_workflow},
                        metadata=RecordedMessageMetadata(
                            runtime_id=incoming.metadata.runtime_id,
                            session_id=incoming.metadata.session_id,
                            turn_id=incoming.metadata.turn_id,
                            domain="routing",
                            source="router",
                            target=plan.selected_workflow,
                            scope="transport",
                            trace=trace,
                        ),
                    )
                )
            self.bus.publish(incoming)

            try:
                reply = plan.handler(incoming)
                execution = coerce_execution(reply)
                final_message_id = self.publish_workflow_execution(
                    incoming=incoming,
                    workflow_name=plan.output_agent_name,
                    execution=execution,
                )
            except Exception as error:
                span.update(level="ERROR", output={"error": str(error)})
                self._publish_turn_completed(
                    runtime_id=incoming.metadata.runtime_id,
                    session_id=incoming.metadata.session_id,
                    turn_id=incoming.metadata.turn_id,
                    domain=plan.lifecycle_domain,
                    target=plan.lifecycle_target,
                    workflow_name=plan.lifecycle_workflow_name,
                    status="error",
                    error=error,
                    trace=trace,
                )
                raise

            span.update(output={"status": "success", "final_message_id": final_message_id})
            self._publish_turn_completed(
                runtime_id=incoming.metadata.runtime_id,
                session_id=incoming.metadata.session_id,
                turn_id=incoming.metadata.turn_id,
                domain=plan.lifecycle_domain,
                target=plan.lifecycle_target,
                workflow_name=plan.lifecycle_workflow_name,
                status="success",
                final_message_id=final_message_id,
                trace=trace,
            )
            return execution.text
