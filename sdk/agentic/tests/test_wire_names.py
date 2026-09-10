"""A record names its type on the wire, and that name did not move with the code.

`fixtures/records_before_the_move.jsonl` is what `serialize_record` wrote for
these three records in `ai_spirit_agent`, at the last commit with the engine in
it. Rows like them are already stored, so the engine that moved has to write
the same bytes and read those back.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path

import pytest

from aiwatcher_agentic.workflow.messages import (
    ConversationData,
    Event,
    Message,
    RecordedMessageMetadata,
    UserMessage,
)
from aiwatcher_agentic.workflow.reactor import LLMResponse
from aiwatcher_agentic.workflow.serialization import (
    WIRE_PREFIX,
    deserialize_record,
    register_record_types,
    serialize_record,
)

BEFORE_THE_MOVE = (
    (Path(__file__).parent / "fixtures" / "records_before_the_move.jsonl").read_text().splitlines()
)

METADATA = RecordedMessageMetadata(
    runtime_id="run-1", message_id="m-1", event_id="e-1", occurred_at_ns=1
)
RECORDS: tuple[Message, ...] = (
    UserMessage(data=ConversationData(role="user", text="hello"), metadata=METADATA),
    Event(type="order.created", data={"order": "o-1"}, metadata=METADATA),
    LLMResponse(data=ConversationData(role="assistant", text="hi"), metadata=METADATA),
)


@dataclass(frozen=True, slots=True, kw_only=True)
class OrderShipped(Event):
    """A type an application defines, whose module path is its own business."""

    type: str = "order.shipped"


@pytest.mark.parametrize(("record", "before"), zip(RECORDS, BEFORE_THE_MOVE, strict=True))
def test_a_record_is_written_as_the_same_bytes_as_before_the_move(
    record: Message, before: str
) -> None:
    assert serialize_record(record) == before


#: The two the engine registers. `LLMResponse` is written but was never
#: registered, so no version of the engine reads it back by name.
READABLE = tuple(zip(RECORDS[:2], BEFORE_THE_MOVE[:2], strict=True))


@pytest.mark.parametrize(("record", "before"), READABLE)
def test_a_record_written_before_the_move_reads_back_and_writes_the_same_bytes(
    record: Message, before: str
) -> None:
    read = deserialize_record(before)

    assert type(read) is type(record)
    assert serialize_record(read) == before


def test_only_the_engine_types_keep_the_old_prefix() -> None:
    register_record_types(OrderShipped)
    written = serialize_record(OrderShipped(metadata=METADATA))
    name = json.loads(written)["__type__"]

    assert name == f"{OrderShipped.__module__}:OrderShipped"
    assert not name.startswith(f"{WIRE_PREFIX}.")
    assert type(deserialize_record(written)) is OrderShipped
