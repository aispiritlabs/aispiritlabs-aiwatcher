from __future__ import annotations

import hashlib
from collections.abc import Iterable
from dataclasses import dataclass
from uuid import uuid4

from aiwatcher_agentic.workflow.agent_run import AgentPrompt, AgentRun, ToolRun
from aiwatcher_agentic.workflow.messages import (
    AssistantMessage,
    ConversationData,
    Message,
    MessageChunk,
    MessageCompleted,
    MessageStarted,
    PromptSnapshot,
    RecordedMessageMetadata,
    ToolCallEvent,
    ToolResultMessage,
    UserMessage,
)
from aiwatcher_agentic.workflow.trace import TraceSnapshot, build_trace_snapshot


def new_message_id() -> str:
    return str(uuid4())


def hash_text(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


@dataclass(frozen=True, slots=True)
class TraceContext:
    trace: TraceSnapshot | None
    session_id: str
    trace_id: str | None
    span_id: str | None
    parent_span_id: str | None
    span_name: str | None
    span_type: str | None
    attempt_no: int | None
    loop_iteration: int | None

    def build_snapshot(self) -> TraceSnapshot | None:
        return build_trace_snapshot(
            self.trace,
            session_id=self.session_id,
            trace_id=self.trace_id,
            span_id=self.span_id,
            parent_span_id=self.parent_span_id,
            span_name=self.span_name,
            span_type=self.span_type,
        )


def resolve_trace_context(
    *,
    incoming: UserMessage,
    trace: TraceSnapshot | None = None,
    attempt_no: int | None = None,
    loop_iteration: int | None = None,
) -> TraceContext:
    return TraceContext(
        trace=trace,
        session_id=(
            (trace.session_id if trace else "")
            or incoming.metadata.session_id
            or incoming.metadata.runtime_id
        ),
        trace_id=(trace.trace_id if trace else "") or incoming.metadata.trace_id,
        span_id=(trace.span_id if trace else "") or incoming.metadata.span_id,
        parent_span_id=(
            (trace.parent_span_id if trace else "") or incoming.metadata.parent_span_id
        ),
        span_name=(trace.span_name if trace else "") or incoming.metadata.span_name,
        span_type=(trace.span_type if trace else "") or incoming.metadata.span_type,
        attempt_no=attempt_no,
        loop_iteration=loop_iteration,
    )


def utf8_chunks(text: str, chunk_bytes: int) -> list[str]:
    if chunk_bytes <= 0:
        raise ValueError("chunk_bytes must be positive")
    if not text:
        return [""]

    chunks: list[str] = []
    current: list[str] = []
    current_size = 0
    for character in text:
        encoded = character.encode("utf-8")
        size = len(encoded)
        if current and current_size + size > chunk_bytes:
            chunks.append("".join(current))
            current = []
            current_size = 0
        current.append(character)
        current_size += size
    if current:
        chunks.append("".join(current))
    return chunks


def build_prompt_snapshot_message(
    *,
    incoming: UserMessage,
    agent_name: str,
    snapshot: AgentPrompt | None,
    agent_run_id: str | None,
    agent_result: AgentRun,
    ctx: TraceContext,
) -> PromptSnapshot | None:
    if snapshot is None:
        return None
    return PromptSnapshot(
        data=ConversationData(
            role="system",
            text=snapshot.text,
            payload={"tool_schema": list(snapshot.tool_schema)},
        ),
        metadata=RecordedMessageMetadata(
            runtime_id=incoming.metadata.runtime_id,
            turn_id=incoming.metadata.turn_id,
            domain=incoming.metadata.domain,
            source=agent_name,
            target=incoming.metadata.source,
            prompt_name=snapshot.prompt_name,
            prompt_hash=snapshot.prompt_hash,
            agent_run_id=agent_run_id,
            trace=ctx.build_snapshot(),
            attempt_no=ctx.attempt_no,
            loop_iteration=ctx.loop_iteration,
            model_name=agent_result.request_usage.model,
            model_provider=agent_result.model_provider,
            input_tokens=agent_result.request_usage.prompt_tokens,
            output_tokens=agent_result.request_usage.completion_tokens,
            total_tokens=agent_result.request_usage.total_tokens,
            model_latency_ms=agent_result.request_usage.latency_ms,
            finish_reason=agent_result.request_usage.finish_reason,
            generation_config_hash=agent_result.generation_config_hash,
        ),
    )


def build_tool_messages(
    *,
    incoming: UserMessage,
    agent_name: str,
    agent_result: AgentRun | None,
    tool_results: Iterable[ToolRun],
    reply_to_message_id: str | None,
) -> tuple[list[Message], str | None]:
    if agent_result is None:
        return [], reply_to_message_id

    published: list[Message] = []
    reply_to = reply_to_message_id
    tool_results_list = list(tool_results)
    for index, tool_call in enumerate(agent_result.tool_calls):
        tool_name, parameters = tool_call
        tool_call_id = str(uuid4())
        tool_message_id = new_message_id()
        tool_trace = (
            tool_results_list[index].trace if index < len(tool_results_list) else agent_result.trace
        )
        ctx = resolve_trace_context(
            incoming=incoming,
            trace=tool_trace,
            attempt_no=agent_result.attempt_no,
            loop_iteration=agent_result.loop_iteration,
        )
        published.append(
            ToolCallEvent(
                data={"name": tool_name, "parameters": parameters},
                metadata=RecordedMessageMetadata(
                    runtime_id=incoming.metadata.runtime_id,
                    turn_id=incoming.metadata.turn_id,
                    message_id=tool_message_id,
                    reply_to_message_id=reply_to,
                    domain=incoming.metadata.domain,
                    source=agent_name,
                    target=incoming.metadata.source,
                    role="assistant",
                    tool_call_id=tool_call_id,
                    agent_run_id=agent_result.run_id,
                    trace=ctx.build_snapshot(),
                    attempt_no=ctx.attempt_no,
                    loop_iteration=ctx.loop_iteration,
                ),
            )
        )
        reply_to = tool_message_id

        if index >= len(tool_results_list):
            continue
        tool_result = tool_results_list[index]
        tool_result_message_id = new_message_id()
        published.append(
            ToolResultMessage(
                data=ConversationData(
                    role="tool",
                    name=tool_name,
                    text=tool_result.output,
                    payload={"name": tool_name, "parameters": dict(parameters)},
                ),
                metadata=RecordedMessageMetadata(
                    runtime_id=incoming.metadata.runtime_id,
                    turn_id=incoming.metadata.turn_id,
                    message_id=tool_result_message_id,
                    reply_to_message_id=reply_to,
                    domain=incoming.metadata.domain,
                    source=tool_name,
                    target=agent_name,
                    tool_call_id=tool_call_id,
                    agent_run_id=agent_result.run_id,
                    content_sha256=hash_text(tool_result.output),
                    trace=ctx.build_snapshot(),
                    attempt_no=ctx.attempt_no,
                    loop_iteration=ctx.loop_iteration,
                ),
            )
        )
        reply_to = tool_result_message_id

    return published, reply_to


def build_assistant_messages(
    *,
    incoming: UserMessage,
    agent_name: str,
    text: str,
    reply_to_message_id: str | None,
    agent_run_id: str | None,
    max_inline_bytes: int,
    chunk_bytes: int,
    ctx: TraceContext,
) -> tuple[list[Message], str | None]:
    if not text:
        return [], None

    message_id = new_message_id()
    encoded_length = len(text.encode("utf-8"))
    content_sha = hash_text(text)
    snapshot = ctx.build_snapshot()
    published: list[Message] = []

    if encoded_length > max_inline_bytes:
        chunks = utf8_chunks(text, chunk_bytes)
        published.append(
            MessageStarted(
                data={
                    "logical_kind": "assistant_message",
                    "chunk_count": len(chunks),
                    "total_bytes": encoded_length,
                },
                metadata=RecordedMessageMetadata(
                    runtime_id=incoming.metadata.runtime_id,
                    turn_id=incoming.metadata.turn_id,
                    message_id=message_id,
                    idempotency_key=f"{message_id}:started",
                    reply_to_message_id=reply_to_message_id,
                    domain=incoming.metadata.domain,
                    source=agent_name,
                    target=incoming.metadata.source,
                    scope="transport",
                    role="assistant",
                    agent_run_id=agent_run_id,
                    content_sha256=content_sha,
                    trace=snapshot,
                    attempt_no=ctx.attempt_no,
                    loop_iteration=ctx.loop_iteration,
                ),
            )
        )
        for index, chunk in enumerate(chunks):
            published.append(
                MessageChunk(
                    data=ConversationData(role="assistant", text=chunk),
                    metadata=RecordedMessageMetadata(
                        runtime_id=incoming.metadata.runtime_id,
                        turn_id=incoming.metadata.turn_id,
                        message_id=message_id,
                        idempotency_key=f"{message_id}:chunk:{index}",
                        reply_to_message_id=reply_to_message_id,
                        domain=incoming.metadata.domain,
                        source=agent_name,
                        target=incoming.metadata.source,
                        scope="transport",
                        role="assistant",
                        chunk_index=index,
                        chunk_count=len(chunks),
                        agent_run_id=agent_run_id,
                        trace=snapshot,
                        attempt_no=ctx.attempt_no,
                        loop_iteration=ctx.loop_iteration,
                    ),
                )
            )
        published.append(
            MessageCompleted(
                data={"chunk_count": len(chunks), "total_bytes": encoded_length},
                metadata=RecordedMessageMetadata(
                    runtime_id=incoming.metadata.runtime_id,
                    turn_id=incoming.metadata.turn_id,
                    message_id=message_id,
                    idempotency_key=f"{message_id}:completed",
                    reply_to_message_id=reply_to_message_id,
                    domain=incoming.metadata.domain,
                    source=agent_name,
                    target=incoming.metadata.source,
                    scope="transport",
                    role="assistant",
                    agent_run_id=agent_run_id,
                    content_sha256=content_sha,
                    trace=snapshot,
                    attempt_no=ctx.attempt_no,
                    loop_iteration=ctx.loop_iteration,
                ),
            )
        )

    published.append(
        AssistantMessage(
            data=ConversationData(role="assistant", text=text),
            metadata=RecordedMessageMetadata(
                runtime_id=incoming.metadata.runtime_id,
                turn_id=incoming.metadata.turn_id,
                message_id=message_id,
                idempotency_key=f"{message_id}:canonical",
                reply_to_message_id=reply_to_message_id,
                domain=incoming.metadata.domain,
                source=agent_name,
                target=incoming.metadata.source,
                agent_run_id=agent_run_id,
                content_sha256=content_sha,
                trace=snapshot,
                attempt_no=ctx.attempt_no,
                loop_iteration=ctx.loop_iteration,
            ),
        )
    )
    return published, message_id
