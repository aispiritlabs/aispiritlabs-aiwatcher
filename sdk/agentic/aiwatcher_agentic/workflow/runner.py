"""Workflow runner — bridges Consumer + MessageRouter + Reactor to WorkflowExecution.

Runs a workflow through the MessageConsumer and builds a WorkflowExecution
from the resulting stream, preserving backward compatibility with the existing
runtime bus publishing.
"""

from __future__ import annotations

from collections.abc import Sequence

from aiwatcher_agentic.workflow.consumer import ConsumerConfig, MessageConsumer
from aiwatcher_agentic.workflow.execution import ExecutionTurnRecord, WorkflowExecution
from aiwatcher_agentic.workflow.message_stream import InMemoryMessageStream
from aiwatcher_agentic.workflow.messages import ConversationData, Message, UserMessage
from aiwatcher_agentic.workflow.reactor import LLMResponse, MessageRouter, TechnicalRoutingFn


def run_workflow(
    message: UserMessage,
    decider: MessageRouter,
    routing_fn: TechnicalRoutingFn,
    *,
    config: ConsumerConfig | None = None,
) -> WorkflowExecution:
    """Run a workflow through the consumer and build WorkflowExecution from the stream.

    1. Creates an InMemoryMessageStream
    2. Appends the initial message
    3. Runs the consumer (decider → routing → reactor cycle)
    4. Builds WorkflowExecution from stream messages
    """
    stream = InMemoryMessageStream()
    consumer = MessageConsumer(config=config)

    stream.append(message)
    consumer.consume(stream, decider, routing_fn)

    return _build_execution(stream.all_messages())


def _build_execution(messages: Sequence[Message]) -> WorkflowExecution:
    """Build WorkflowExecution from stream messages."""
    llm_responses: list[LLMResponse] = []
    domain_events: list[Message] = []

    for msg in messages:
        if isinstance(msg, LLMResponse):
            llm_responses.append(msg)
        elif isinstance(msg, UserMessage):
            continue  # skip input messages
        else:
            domain_events.append(msg)

    if not llm_responses:
        return WorkflowExecution(text="")

    last_response = llm_responses[-1]

    for response in llm_responses:
        if response._agent_result is None:
            continue
        response._agent_result.attempt_no = response.metadata.attempt_no
        response._agent_result.loop_iteration = response.metadata.loop_iteration

    recorded_turns = tuple(
        ExecutionTurnRecord(
            agent_result=resp._agent_result,
            tool_results=resp._tool_results,
        )
        for resp in llm_responses
        if resp._agent_result is not None
    )

    return WorkflowExecution(
        text=last_response.data.text
        if isinstance(last_response.data, ConversationData) and last_response.data.text
        else "",
        agent_result=last_response._agent_result,
        tool_results=last_response._tool_results,
        emitted_events=tuple(domain_events),
        recorded_turns=recorded_turns,
    )
