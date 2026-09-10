from __future__ import annotations

import json
from collections.abc import Callable, Mapping
from dataclasses import asdict, is_dataclass
from typing import Any, cast

from aiwatcher_agentic.workflow.messages import (
    AssistantMessage,
    Command,
    ConversationData,
    Event,
    Message,
    MessageChunk,
    MessageCompleted,
    MessageMetadata,
    MessageStarted,
    PromptSnapshot,
    RecordedMessageMetadata,
    ToolCallEvent,
    ToolResultMessage,
    TurnCompleted,
    TurnStarted,
    UserCommand,
    UserMessage,
    _bind_record_contract,
)
from aiwatcher_agentic.workflow.trace import TraceSnapshot

_BASE_SERIALIZABLE_TYPES = (
    AssistantMessage,
    Command,
    Event,
    Message,
    MessageChunk,
    MessageCompleted,
    MessageStarted,
    PromptSnapshot,
    ToolCallEvent,
    ToolResultMessage,
    TurnCompleted,
    TurnStarted,
    UserCommand,
    UserMessage,
)


#: Where this engine's own record types were named from before it moved.
#:
#: A record is written with its type's module path in ``__type__``, in
#: ``__record_contract__`` and in the metadata's ``contract_name``, and rows that
#: are already stored carry ``agentic.workflow.…``. So that prefix is kept as the
#: wire name of every type defined here: a record written after the move is the
#: same bytes as one written before it, and an older reader still recognises it.
#: An import path is where code lives; a persisted name is a promise.
WIRE_PREFIX = "agentic.workflow"
_PACKAGE = __name__.rpartition(".")[0]


def _type_key(record_type: type[object]) -> str:
    module = record_type.__module__
    if module == _PACKAGE or module.startswith(f"{_PACKAGE}."):
        module = WIRE_PREFIX + module.removeprefix(_PACKAGE)
    return f"{module}:{record_type.__qualname__}"


_TYPE_MAP: dict[str, type[object]] = {}
_TYPE_CONTRACTS: dict[type[object], str] = {}
for _record_type in _BASE_SERIALIZABLE_TYPES:
    _TYPE_MAP[_record_type.__name__] = _record_type
    _TYPE_MAP[_type_key(_record_type)] = _record_type
    _TYPE_CONTRACTS[_record_type] = _type_key(_record_type)


PayloadUpcaster = Callable[[dict[str, Any]], dict[str, Any]]
_PAYLOAD_UPCASTERS: dict[tuple[str, int], tuple[int, PayloadUpcaster]] = {}


_external_hooks: list[Callable[..., None]] = []


def add_registration_hook(hook: Callable[..., None]) -> None:
    """Register a callback invoked whenever new record types are added."""
    _external_hooks.append(hook)


def register_record_types(*record_types: type[object]) -> None:
    for record_type in record_types:
        _TYPE_MAP[record_type.__name__] = record_type
        _TYPE_MAP[_type_key(record_type)] = record_type
        _TYPE_CONTRACTS.setdefault(record_type, _type_key(record_type))
    for hook in _external_hooks:
        hook(*record_types)


def register_record_contract(
    record_type: type[object],
    contract_name: str,
    *,
    aliases: tuple[str, ...] = (),
) -> None:
    """Bind a dataclass to a stable wire name independent of its Python module path."""
    resolved = contract_name.strip()
    if not resolved:
        raise ValueError("contract_name must not be empty")
    register_record_types(record_type)
    for name in (resolved, *aliases):
        existing = _TYPE_MAP.get(name)
        if existing is not None and existing is not record_type:
            raise ValueError(f"Record contract {name!r} is already registered")
        _TYPE_MAP[name] = record_type
    _TYPE_CONTRACTS[record_type] = resolved
    _bind_record_contract(record_type, resolved)


def register_payload_upcaster(
    contract_name: str,
    from_version: int,
    to_version: int,
    upcaster: PayloadUpcaster,
) -> None:
    """Register one deterministic raw-payload migration for a message contract."""
    if from_version < 1 or to_version != from_version + 1:
        raise ValueError("Payload upcasters must advance exactly one positive schema version")
    key = (contract_name, from_version)
    if key in _PAYLOAD_UPCASTERS:
        raise ValueError(
            f"Payload upcaster already registered for {contract_name!r} v{from_version}"
        )
    _PAYLOAD_UPCASTERS[key] = (to_version, upcaster)


def _message_contract(payload: Mapping[str, Any], record_contract: str) -> str:
    return record_contract


def _upcast_payload(
    payload: dict[str, Any],
    *,
    contract_name: str,
    schema_version: int,
) -> tuple[dict[str, Any], int]:
    current = schema_version
    migrated = payload
    visited: set[int] = set()
    while (contract_name, current) in _PAYLOAD_UPCASTERS:
        if current in visited:
            raise ValueError(f"Payload upcaster cycle for {contract_name!r} v{current}")
        visited.add(current)
        next_version, upcaster = _PAYLOAD_UPCASTERS[(contract_name, current)]
        migrated = upcaster(dict(migrated))
        if not isinstance(migrated, dict):
            raise TypeError("Payload upcaster must return a dict")
        current = next_version
    metadata = migrated.get("metadata")
    if isinstance(metadata, dict):
        migrated["metadata"] = {**metadata, "schema_version": current}
    return migrated, current


def serialize_record(record: object) -> str:
    if not is_dataclass(record):
        raise TypeError(f"Cannot serialize non-dataclass record: {type(record)!r}")

    payload = asdict(cast(Any, record))
    record_type = type(record)
    record_contract = _TYPE_CONTRACTS.get(record_type, _type_key(record_type))
    metadata = payload.get("metadata")
    schema_version = 1
    if isinstance(metadata, Mapping):
        schema_version = int(metadata.get("schema_version", 1))
        if not metadata.get("contract_name"):
            payload["metadata"] = {**metadata, "contract_name": record_contract}
    payload["__type__"] = _type_key(record_type)
    payload["__record_contract__"] = record_contract
    payload["__schema_version__"] = schema_version
    return json.dumps(payload, ensure_ascii=True, separators=(",", ":"))


def deserialize_record(value: str | bytes) -> Any:
    raw = value.decode("utf-8") if isinstance(value, bytes) else value
    payload = json.loads(raw)
    if not isinstance(payload, dict):
        raise TypeError("Serialized record must decode to a JSON object")

    record_type_name = payload.pop("__type__", None)
    record_contract = payload.pop("__record_contract__", record_type_name)
    schema_version = payload.pop("__schema_version__", 1)
    if not isinstance(record_type_name, str):
        raise KeyError("Serialized record is missing __type__")
    if not isinstance(record_contract, str):
        raise TypeError("Serialized record contract must be a string")
    if not isinstance(schema_version, int) or schema_version < 1:
        raise ValueError("Serialized schema version must be a positive integer")

    record_type = _TYPE_MAP.get(record_contract) or _TYPE_MAP.get(record_type_name)
    if record_type is None and ":" in record_type_name:
        record_type = _TYPE_MAP.get(record_type_name.split(":", 1)[1].split(".")[-1])
    if record_type is None:
        raise KeyError(f"Unknown serialized record type: {record_type_name}")
    contract_name = _message_contract(payload, record_contract)
    payload, _ = _upcast_payload(
        payload,
        contract_name=contract_name,
        schema_version=schema_version,
    )
    if isinstance(payload.get("metadata"), dict):
        metadata_payload = dict(payload["metadata"])
        trace_payload = metadata_payload.get("trace")
        if isinstance(trace_payload, dict):
            metadata_payload["trace"] = TraceSnapshot(**trace_payload)
        metadata_type = MessageMetadata
        if any(
            key in metadata_payload
            for key in ("event_id", "message_id", "sequence_no", "content_sha256")
        ):
            metadata_type = RecordedMessageMetadata
        payload["metadata"] = metadata_type(**metadata_payload)
    if record_type in {
        AssistantMessage,
        MessageChunk,
        PromptSnapshot,
        ToolResultMessage,
        UserMessage,
    } and isinstance(payload.get("data"), dict):
        payload["data"] = ConversationData(**payload["data"])
    return record_type(**payload)
