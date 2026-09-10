from __future__ import annotations

import dataclasses
import time
import uuid
from dataclasses import dataclass, field, replace
from typing import Any, Self

from aiwatcher_agentic.workflow.trace import TraceSnapshot, build_trace_snapshot

_RECORD_CONTRACT_NAMES: dict[type[object], str] = {}


def _bind_record_contract(record_type: type[object], contract_name: str) -> None:
    _RECORD_CONTRACT_NAMES[record_type] = contract_name


@dataclass(frozen=True, slots=True, kw_only=True)
class MessageMetadata:
    runtime_id: str = ""
    session_id: str = ""
    turn_id: str = ""
    reply_to_message_id: str | None = None
    correlation_id: str = ""
    causation_id: str | None = None
    idempotency_key: str = ""
    domain: str = ""
    source: str = ""
    target: str | None = None
    tenant_id: str = ""
    workspace_id: str = ""
    role: str = ""
    scope: str = "canonical"
    contract_name: str = ""
    schema_version: int = 1
    model_name: str = ""
    model_provider: str = ""
    input_tokens: int | None = None
    output_tokens: int | None = None
    total_tokens: int | None = None
    model_latency_ms: float | None = None
    finish_reason: str = ""
    generation_config_hash: str = ""
    occurred_at_ns: int = 0
    recorded_at_ns: int | None = None
    chunk_index: int | None = None
    chunk_count: int | None = None
    tool_call_id: str | None = None
    agent_run_id: str | None = None
    prompt_name: str | None = None
    prompt_hash: str | None = None
    status: str | None = None
    workflow_action: str | None = None
    stream_name: str = ""
    stream_position: int | None = None
    global_position: int | None = None
    trace: TraceSnapshot | None = None
    attempt_no: int | None = None
    loop_iteration: int | None = None

    @property
    def trace_id(self) -> str | None:
        return None if self.trace is None or not self.trace.trace_id else self.trace.trace_id

    @property
    def span_id(self) -> str | None:
        return None if self.trace is None or not self.trace.span_id else self.trace.span_id

    @property
    def parent_span_id(self) -> str | None:
        return (
            None
            if self.trace is None or not self.trace.parent_span_id
            else self.trace.parent_span_id
        )

    @property
    def span_name(self) -> str | None:
        return None if self.trace is None or not self.trace.span_name else self.trace.span_name

    @property
    def span_type(self) -> str | None:
        return None if self.trace is None or not self.trace.span_type else self.trace.span_type


@dataclass(frozen=True, slots=True, kw_only=True)
class RecordedMessageMetadata(MessageMetadata):
    event_id: str = ""
    message_id: str = ""
    sequence_no: int | None = None
    content_sha256: str | None = None


#: Metadata keys a tracer sets that are not fields of the metadata.
#:
#: `trace_id`, `span_id` and the rest are *properties* over `trace`, so a
#: tracer's `span_id=…` reaches the dataclass as an unknown keyword rather than
#: as a value it can store.
_TRACE_UPDATE_KEYS = (
    "session_id",
    "trace_id",
    "span_id",
    "parent_span_id",
    "span_name",
    "span_type",
)


def metadata_with_updates[MetadataT: MessageMetadata](
    metadata: MetadataT, **updates: Any
) -> MetadataT:
    """Apply `updates` to `metadata`, folding the trace keys into the snapshot.

    Every `with_metadata` reaches this, including the ones an event with a
    custom `__init__` has to override — those cannot `replace` themselves, and
    each was carrying its own copy of the fold. A copy that omits it raises
    `TypeError` the first time a tracer sets a span on that event and never
    before, which is how four of them stayed wrong while the graph ran untraced.
    """
    trace = updates.pop("trace", metadata.trace)
    if any(key in updates for key in _TRACE_UPDATE_KEYS):
        trace = build_trace_snapshot(
            trace,
            session_id=str(updates.pop("session_id", metadata.session_id) or ""),
            trace_id=updates.pop("trace_id", metadata.trace_id),
            span_id=updates.pop("span_id", metadata.span_id),
            parent_span_id=updates.pop("parent_span_id", metadata.parent_span_id),
            span_name=updates.pop("span_name", metadata.span_name),
            span_type=updates.pop("span_type", metadata.span_type),
        )
    return replace(metadata, trace=trace, **updates)


@dataclass(frozen=True, slots=True, kw_only=True)
class ConversationData:
    role: str = ""
    text: str | None = None
    name: str | None = None
    payload: dict[str, Any] | None = None


@dataclass(frozen=True, slots=True, kw_only=True)
class Message:
    kind: str = "message"
    type: str = "message"
    data: Any = field(default_factory=dict)
    metadata: MessageMetadata = field(default_factory=RecordedMessageMetadata)

    def with_metadata(self, **updates: Any) -> Self:
        return replace(self, metadata=metadata_with_updates(self.metadata, **updates))

    def with_data(self, **updates: Any) -> Self:
        if isinstance(self.data, dict):
            data = dict(self.data)
            data.update(updates)
            return replace(self, data=data)
        if not dataclasses.is_dataclass(self.data) or isinstance(self.data, type):
            kind = type(self.data).__name__
            raise TypeError(f"with_data() requires data to be a dict or dataclass, got {kind}")
        return replace(self, data=replace(self.data, **updates))


def normalize_recorded_message(
    message: Message,
    *,
    stream_name: str | None = None,
    stream_position: int | None = None,
    global_position: int | None = None,
    recorded_at_ns: int | None = None,
) -> Message:
    """Complete the durable envelope while preserving explicitly supplied values."""
    now_ns = time.time_ns()
    updates: dict[str, Any] = {}
    metadata = message.metadata
    message_id = getattr(metadata, "message_id", "")
    if isinstance(metadata, RecordedMessageMetadata):
        if not message_id:
            message_id = str(uuid.uuid4())
            updates["message_id"] = message_id
        if not metadata.event_id:
            updates["event_id"] = str(uuid.uuid4())
    if not metadata.turn_id and metadata.runtime_id:
        updates["turn_id"] = metadata.runtime_id
    if not metadata.correlation_id:
        updates["correlation_id"] = metadata.turn_id or metadata.runtime_id or message_id
    if metadata.causation_id is None and metadata.reply_to_message_id:
        updates["causation_id"] = metadata.reply_to_message_id
    if not metadata.idempotency_key and message_id:
        updates["idempotency_key"] = message_id
    if not metadata.contract_name:
        updates["contract_name"] = _RECORD_CONTRACT_NAMES.get(type(message), "") or message.type
    if metadata.occurred_at_ns <= 0:
        updates["occurred_at_ns"] = now_ns
    if stream_name is not None:
        updates["stream_name"] = stream_name
    if stream_position is not None:
        updates["stream_position"] = stream_position
    if global_position is not None:
        updates["global_position"] = global_position
    if recorded_at_ns is not None:
        updates["recorded_at_ns"] = recorded_at_ns
    return message.with_metadata(**updates) if updates else message


@dataclass(frozen=True, slots=True, kw_only=True)
class UserMessage(Message):
    kind: str = "conversation"
    type: str = "user_message"
    data: ConversationData = field(default_factory=lambda: ConversationData(role="user", text=""))
    metadata: RecordedMessageMetadata = field(default_factory=RecordedMessageMetadata)


Conversation = UserMessage


@dataclass(frozen=True, slots=True, kw_only=True)
class AssistantMessage(Message):
    kind: str = "assistant_message"
    type: str = "assistant_message"
    data: ConversationData = field(
        default_factory=lambda: ConversationData(role="assistant", text="")
    )
    metadata: RecordedMessageMetadata = field(default_factory=RecordedMessageMetadata)


@dataclass(frozen=True, slots=True, kw_only=True)
class PromptSnapshot(Message):
    kind: str = "prompt_snapshot"
    type: str = "prompt_snapshot"
    data: ConversationData = field(
        default_factory=lambda: ConversationData(role="system", text="", payload={})
    )
    metadata: RecordedMessageMetadata = field(default_factory=RecordedMessageMetadata)


@dataclass(frozen=True, slots=True, kw_only=True)
class ToolResultMessage(Message):
    kind: str = "tool_result"
    type: str = "tool_result"
    data: ConversationData = field(
        default_factory=lambda: ConversationData(role="tool", text="", payload={})
    )
    metadata: RecordedMessageMetadata = field(default_factory=RecordedMessageMetadata)


@dataclass(frozen=True, slots=True, kw_only=True)
class UserCommand(Message):
    kind: str = "command"
    type: str = ""
    data: dict[str, Any] = field(default_factory=dict)
    metadata: RecordedMessageMetadata = field(default_factory=RecordedMessageMetadata)


Command = UserCommand


@dataclass(frozen=True, slots=True, kw_only=True)
class Event(Message):
    kind: str = "event"
    type: str = ""
    data: dict[str, Any] = field(default_factory=dict)
    metadata: RecordedMessageMetadata = field(default_factory=RecordedMessageMetadata)


@dataclass(frozen=True, slots=True, kw_only=True)
class ToolCallEvent(Event):
    kind: str = "tool_call"
    type: str = "tool_call"


@dataclass(frozen=True, slots=True, kw_only=True)
class TurnStarted(Event):
    kind: str = "turn_started"
    type: str = "turn_started"
    metadata: RecordedMessageMetadata = field(
        default_factory=lambda: RecordedMessageMetadata(scope="transport")
    )


@dataclass(frozen=True, slots=True, kw_only=True)
class TurnCompleted(Event):
    kind: str = "turn_completed"
    type: str = "turn_completed"
    metadata: RecordedMessageMetadata = field(
        default_factory=lambda: RecordedMessageMetadata(scope="transport")
    )


@dataclass(frozen=True, slots=True, kw_only=True)
class MessageStarted(Event):
    kind: str = "message_started"
    type: str = "message_started"
    metadata: RecordedMessageMetadata = field(
        default_factory=lambda: RecordedMessageMetadata(scope="transport")
    )


@dataclass(frozen=True, slots=True, kw_only=True)
class MessageChunk(Message):
    kind: str = "message_chunk"
    type: str = "message_chunk"
    data: ConversationData = field(
        default_factory=lambda: ConversationData(role="assistant", text="")
    )
    metadata: RecordedMessageMetadata = field(
        default_factory=lambda: RecordedMessageMetadata(scope="transport")
    )


@dataclass(frozen=True, slots=True, kw_only=True)
class MessageCompleted(Event):
    kind: str = "message_completed"
    type: str = "message_completed"
    metadata: RecordedMessageMetadata = field(
        default_factory=lambda: RecordedMessageMetadata(scope="transport", status="completed")
    )
