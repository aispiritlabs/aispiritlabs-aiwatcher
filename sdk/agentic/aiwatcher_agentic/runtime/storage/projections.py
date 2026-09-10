from __future__ import annotations

import time
from dataclasses import dataclass
from typing import Protocol

import orjson

from aiwatcher_agentic.workflow.messages import ConversationData, Message


@dataclass(frozen=True, slots=True)
class MessageRow:
    event_id: str
    message_id: str
    runtime_id: str
    session_id: str
    turn_id: str
    reply_to_message_id: str | None
    kind: str
    event_type: str
    role: str | None
    scope: str
    domain: str
    source: str
    target: str | None
    name: str | None
    text: str | None
    payload_json: bytes | None
    sequence_no: int | None
    chunk_index: int | None
    chunk_count: int | None
    tool_call_id: str | None
    agent_run_id: str | None
    prompt_name: str | None
    prompt_hash: str | None
    status: str | None
    content_sha256: str | None
    trace_id: str | None
    span_id: str | None
    parent_span_id: str | None
    span_name: str | None
    span_type: str | None
    attempt_no: int | None
    loop_iteration: int | None
    created_at_ns: int


@dataclass(frozen=True, slots=True)
class ConversationRecordRow:
    message_id: str
    runtime_id: str
    session_id: str
    turn_id: str
    reply_to_message_id: str | None
    kind: str
    event_type: str
    role: str
    domain: str
    source: str
    target: str | None
    name: str | None
    text: str | None
    payload_json: bytes | None
    sequence_no: int | None
    tool_call_id: str | None
    agent_run_id: str | None
    prompt_name: str | None
    prompt_hash: str | None
    status: str | None
    content_sha256: str | None
    trace_id: str | None
    span_id: str | None
    parent_span_id: str | None
    span_name: str | None
    span_type: str | None
    attempt_no: int | None
    loop_iteration: int | None
    created_at_ns: int


class Projection(Protocol):
    @property
    def can_handle(self) -> tuple[type[Message], ...]: ...

    def handle(self, event: Message) -> MessageRow: ...


def handle_projections(
    projections: list[Projection],
    message: Message,
) -> MessageRow | None:
    for projection in projections:
        if isinstance(message, projection.can_handle):
            return projection.handle(message)
    return None


def _to_payload_json(payload: object) -> bytes | None:
    if payload in (None, {}):
        return None
    return orjson.dumps(payload)


def _message_role(message: Message) -> str | None:
    if isinstance(message.data, ConversationData):
        return message.data.role or None
    return message.metadata.role or None


def _message_name(message: Message) -> str | None:
    if isinstance(message.data, ConversationData):
        return message.data.name
    return message.type or None


def _message_text(message: Message) -> str | None:
    if isinstance(message.data, ConversationData):
        return message.data.text
    if isinstance(message.data, dict):
        text = message.data.get("text")
        return str(text) if text is not None else None
    return None


def _message_payload(message: Message) -> object:
    if isinstance(message.data, ConversationData):
        return message.data.payload
    return message.data


def row_to_conversation_record(row: MessageRow) -> ConversationRecordRow | None:
    if row.scope != "canonical":
        return None
    if row.role not in {"system", "user", "assistant", "tool"}:
        return None
    return ConversationRecordRow(
        message_id=row.message_id,
        runtime_id=row.runtime_id,
        session_id=row.session_id,
        turn_id=row.turn_id,
        reply_to_message_id=row.reply_to_message_id,
        kind=row.kind,
        event_type=row.event_type,
        role=row.role,
        domain=row.domain,
        source=row.source,
        target=row.target,
        name=row.name,
        text=row.text,
        payload_json=row.payload_json,
        sequence_no=row.sequence_no,
        tool_call_id=row.tool_call_id,
        agent_run_id=row.agent_run_id,
        prompt_name=row.prompt_name,
        prompt_hash=row.prompt_hash,
        status=row.status,
        content_sha256=row.content_sha256,
        trace_id=row.trace_id,
        span_id=row.span_id,
        parent_span_id=row.parent_span_id,
        span_name=row.span_name,
        span_type=row.span_type,
        attempt_no=row.attempt_no,
        loop_iteration=row.loop_iteration,
        created_at_ns=row.created_at_ns,
    )


@dataclass(frozen=True)
class GenericProjection:
    can_handle: tuple[type[Message], ...] = (Message,)

    def handle(self, event: Message) -> MessageRow:
        return MessageRow(
            event_id=getattr(event.metadata, "event_id", ""),
            message_id=getattr(event.metadata, "message_id", ""),
            runtime_id=event.metadata.runtime_id,
            session_id=event.metadata.session_id,
            turn_id=event.metadata.turn_id,
            reply_to_message_id=event.metadata.reply_to_message_id,
            kind=event.kind,
            event_type=type(event).__name__,
            role=_message_role(event),
            scope=event.metadata.scope,
            domain=event.metadata.domain,
            source=event.metadata.source,
            target=event.metadata.target,
            name=_message_name(event),
            text=_message_text(event),
            payload_json=_to_payload_json(_message_payload(event)),
            sequence_no=getattr(event.metadata, "sequence_no", None),
            chunk_index=event.metadata.chunk_index,
            chunk_count=event.metadata.chunk_count,
            tool_call_id=event.metadata.tool_call_id,
            agent_run_id=event.metadata.agent_run_id,
            prompt_name=event.metadata.prompt_name,
            prompt_hash=event.metadata.prompt_hash,
            status=event.metadata.status,
            content_sha256=getattr(event.metadata, "content_sha256", None),
            trace_id=event.metadata.trace_id,
            span_id=event.metadata.span_id,
            parent_span_id=event.metadata.parent_span_id,
            span_name=event.metadata.span_name,
            span_type=event.metadata.span_type,
            attempt_no=event.metadata.attempt_no,
            loop_iteration=event.metadata.loop_iteration,
            created_at_ns=time.time_ns(),
        )


DEFAULT_PROJECTIONS: list[Projection] = [GenericProjection()]
