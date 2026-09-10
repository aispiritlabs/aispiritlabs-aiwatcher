from collections.abc import Generator
from contextlib import contextmanager
from typing import Any

import pytest

from aiwatcher_agentic.agent import AgentResult
from aiwatcher_agentic.agent import PromptSnapshot as AgentPromptSnapshot
from aiwatcher_agentic.tracer import NoopLLMTracer
from aiwatcher_agentic.usage import RequestUsage
from aiwatcher_agentic.workflow.execution import WorkflowExecution
from aiwatcher_agentic.workflow.message_bus import InMemoryMessageBus
from aiwatcher_agentic.workflow.messages import (
    AssistantMessage,
    ConversationData,
    Event,
    PromptSnapshot,
    RecordedMessageMetadata,
    TurnCompleted,
    TurnStarted,
    UserMessage,
)
from aiwatcher_agentic.workflow.trace import TraceSnapshot
from aiwatcher_agentic.workflow.turn_execution import TurnExecutor, TurnPlan


class _TraceHandle:
    def update(self, **kwargs: Any) -> None:
        del kwargs


class _TraceTracer(NoopLLMTracer):
    @contextmanager
    def workflow(self, **kwargs: Any) -> Generator[_TraceHandle, None, None]:
        del kwargs
        yield _TraceHandle()

    @property
    def current_trace_id(self) -> str | None:
        return "trace-turn-1"


def test_turn_executor_emits_routed_turn_messages_in_order() -> None:
    bus = InMemoryMessageBus()
    executor = TurnExecutor(bus=bus, tracer=NoopLLMTracer())

    reply = executor.execute(
        TurnPlan(
            incoming=UserMessage(
                data=ConversationData(role="user", text="Dodaj notatkę Projekt"),
                metadata=RecordedMessageMetadata(
                    runtime_id="rt-1",
                    turn_id="turn-1",
                    domain="manage_notes",
                    source="user",
                    target="manage_notes",
                ),
            ),
            handler=lambda message: "Notatka Projekt dodana.",
            trace_name="manage_notes",
            lifecycle_domain="manage_notes",
            lifecycle_target="manage_notes",
            lifecycle_workflow_name="manage_notes",
            output_agent_name="manage_notes",
            selected_workflow="manage_notes",
        )
    )

    assert reply == "Notatka Projekt dodana."
    assert isinstance(bus.messages[0], TurnStarted)
    assert isinstance(bus.messages[1], Event)
    assert bus.messages[1].type == "workflow_selected"
    assert isinstance(bus.messages[2], UserMessage)

    assistant_messages = [m for m in bus.messages if isinstance(m, AssistantMessage)]
    assert len(assistant_messages) == 1
    assert assistant_messages[0].metadata.scope == "canonical"
    assert assistant_messages[0].data.text == "Notatka Projekt dodana."
    assert isinstance(bus.messages[-1], TurnCompleted)


def test_turn_executor_preserves_trace_and_run_id_for_fallback_execution() -> None:
    bus = InMemoryMessageBus()
    executor = TurnExecutor(bus=bus, tracer=_TraceTracer())

    reply = executor.execute(
        TurnPlan(
            incoming=UserMessage(
                data=ConversationData(role="user", text="Hej fallback"),
                metadata=RecordedMessageMetadata(
                    runtime_id="rt-1",
                    turn_id="turn-1",
                    domain="general",
                    source="user",
                ),
            ),
            handler=lambda message: WorkflowExecution(
                text=f"fallback:{message.data.text}",
                agent_result=AgentResult(
                    content=f"fallback:{message.data.text}",
                    run_id="run-fallback-1",
                    trace=TraceSnapshot(trace_id="trace-turn-1", session_id="rt-1"),
                ),
            ),
            trace_name="general",
            lifecycle_domain="general",
            lifecycle_target=None,
            lifecycle_workflow_name="general",
            output_agent_name="assistant",
        )
    )

    assert reply == "fallback:Hej fallback"
    turn_started = bus.messages[0]
    assert isinstance(turn_started, TurnStarted)
    assert turn_started.metadata.trace_id == "trace-turn-1"

    user_message = bus.messages[1]
    assert isinstance(user_message, UserMessage)
    assert user_message.metadata.trace_id == "trace-turn-1"

    assistants = [m for m in bus.messages if isinstance(m, AssistantMessage)]
    assert assistants[-1].metadata.agent_run_id == "run-fallback-1"

    turn_completed = bus.messages[-1]
    assert isinstance(turn_completed, TurnCompleted)
    assert turn_completed.metadata.trace_id == "trace-turn-1"


def test_turn_executor_persists_llm_reproducibility_metadata() -> None:
    bus = InMemoryMessageBus()
    executor = TurnExecutor(bus=bus, tracer=NoopLLMTracer())
    result = AgentResult(
        content="answer",
        run_id="run-1",
        prompt_snapshot=AgentPromptSnapshot(
            text="system prompt",
            prompt_name="assistant-v2",
            prompt_hash="prompt-hash",
        ),
        request_usage=RequestUsage(
            prompt_tokens=12,
            completion_tokens=4,
            total_tokens=16,
            latency_ms=25.5,
            model="model-1",
            finish_reason="stop",
        ),
        model_provider="openai",
        generation_config_hash="config-hash",
    )

    executor.execute(
        TurnPlan(
            incoming=UserMessage(
                data=ConversationData(role="user", text="question"),
                metadata=RecordedMessageMetadata(
                    runtime_id="rt-1",
                    turn_id="turn-1",
                    domain="research",
                    source="user",
                ),
            ),
            handler=lambda _: WorkflowExecution(text="answer", agent_result=result),
            trace_name="research",
            lifecycle_domain="research",
            lifecycle_target="research",
            lifecycle_workflow_name="research",
            output_agent_name="research",
        )
    )

    snapshot = next(message for message in bus.messages if isinstance(message, PromptSnapshot))
    assert snapshot.metadata.model_name == "model-1"
    assert snapshot.metadata.model_provider == "openai"
    assert snapshot.metadata.input_tokens == 12
    assert snapshot.metadata.output_tokens == 4
    assert snapshot.metadata.total_tokens == 16
    assert snapshot.metadata.model_latency_ms == 25.5
    assert snapshot.metadata.finish_reason == "stop"
    assert snapshot.metadata.generation_config_hash == "config-hash"


def test_turn_executor_reuses_existing_targeted_turn_id() -> None:
    bus = InMemoryMessageBus()
    executor = TurnExecutor(bus=bus, tracer=NoopLLMTracer())

    executor.execute(
        TurnPlan(
            incoming=UserMessage(
                data=ConversationData(role="user", text="Pomóż mi podjąć decyzję"),
                metadata=RecordedMessageMetadata(
                    runtime_id="rt-1",
                    turn_id="turn-existing",
                    domain="sage",
                    source="user",
                    target="sage",
                ),
            ),
            handler=lambda message: "Decyzja",
            trace_name="sage",
            lifecycle_domain="sage",
            lifecycle_target="sage",
            lifecycle_workflow_name="sage",
            output_agent_name="sage",
        )
    )

    turn_started = bus.messages[0]
    targeted_message = bus.messages[1]
    assert isinstance(turn_started, TurnStarted)
    assert isinstance(targeted_message, UserMessage)
    assert turn_started.metadata.turn_id == "turn-existing"
    assert targeted_message.metadata.turn_id == "turn-existing"


def test_turn_executor_publishes_error_turn_completed() -> None:
    bus = InMemoryMessageBus()
    executor = TurnExecutor(bus=bus, tracer=NoopLLMTracer())

    def _explode(message: UserMessage) -> str:
        del message
        raise RuntimeError("boom")

    with pytest.raises(RuntimeError, match="boom"):
        executor.execute(
            TurnPlan(
                incoming=UserMessage(
                    data=ConversationData(role="user", text="Dodaj notatkę"),
                    metadata=RecordedMessageMetadata(
                        runtime_id="rt-1",
                        turn_id="turn-1",
                        domain="manage_notes",
                        source="user",
                        target="manage_notes",
                    ),
                ),
                handler=_explode,
                trace_name="manage_notes",
                lifecycle_domain="manage_notes",
                lifecycle_target="manage_notes",
                lifecycle_workflow_name="manage_notes",
                output_agent_name="manage_notes",
            )
        )

    turn_completed = bus.messages[-1]
    assert isinstance(turn_completed, TurnCompleted)
    assert turn_completed.metadata.status == "error"
    assert turn_completed.data == {
        "workflow": "manage_notes",
        "error_type": "RuntimeError",
        "error_message": "boom",
    }
