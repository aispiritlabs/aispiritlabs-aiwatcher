from __future__ import annotations

from dataclasses import dataclass

import pytest

from aiwatcher_agentic.workflow.messages import (
    ConversationData,
    Event,
    RecordedMessageMetadata,
    UserMessage,
)
from aiwatcher_agentic.workflow.output_handler import (
    EachBatchHandler,
    EachMessageHandler,
    OutputHandlerDispatcher,
    dispatch_output_handlers,
    workflow_output_handler,
)


@dataclass(frozen=True, slots=True, kw_only=True)
class CreatedNote(Event):
    kind: str = "created_note"
    type: str = "created_note"
    note_name: str = ""
    note_content: str = ""


@dataclass(frozen=True, slots=True, kw_only=True)
class NoteUpdated(Event):
    kind: str = "note_updated"
    type: str = "note_updated"
    note_name: str = ""
    note_path: str = ""


def test_factory_raises_when_both_strategies_given() -> None:
    with pytest.raises(ValueError, match="not both"):
        workflow_output_handler(
            can_handle=(UserMessage,),
            each_message=lambda m: None,
            each_batch=lambda ms: [None],
        )


def test_factory_raises_when_neither_strategy_given() -> None:
    with pytest.raises(ValueError, match="either each_message or each_batch"):
        workflow_output_handler(can_handle=(UserMessage,))


def test_factory_creates_each_message_handler() -> None:
    handler = workflow_output_handler(
        can_handle=(UserMessage,),
        each_message=lambda m: "ok",
        name="test",
    )

    assert handler.name == "test"
    assert handler.can_handle == (UserMessage,)
    assert isinstance(handler.strategy, EachMessageHandler)


def test_factory_creates_each_batch_handler() -> None:
    handler = workflow_output_handler(
        can_handle=(UserMessage,),
        each_batch=lambda ms: ["ok"],
        batch_size=5,
        name="batch-test",
    )

    assert handler.name == "batch-test"
    assert isinstance(handler.strategy, EachBatchHandler)
    assert handler.strategy.batch_size == 5


def _note_updated(note_name: str) -> NoteUpdated:
    return NoteUpdated(
        note_name=note_name,
        note_path=f"/notes/{note_name}.md",
        metadata=RecordedMessageMetadata(runtime_id="r1", source="test"),
    )


def test_dispatch_invokes_matching_each_message_handler() -> None:
    handler = workflow_output_handler(
        can_handle=(CreatedNote,),
        each_message=lambda m: f"handled:{m.note_name}",
    )
    message = CreatedNote(
        note_name="Foo",
        note_content="bar",
        metadata=RecordedMessageMetadata(runtime_id="r1", source="test"),
    )

    results = dispatch_output_handlers([handler], message)

    assert results == ["handled:Foo"]


def test_dispatch_ignores_non_matching_message_type() -> None:
    handler = workflow_output_handler(
        can_handle=(CreatedNote,),
        each_message=lambda m: "should not run",
    )
    message = UserMessage(
        data=ConversationData(role="user", text="hello"),
        metadata=RecordedMessageMetadata(runtime_id="r1", source="user"),
    )

    results = dispatch_output_handlers([handler], message)

    assert results == []


def test_dispatch_runs_all_matching_handlers() -> None:
    handler_a = workflow_output_handler(
        can_handle=(CreatedNote,),
        each_message=lambda m: "a",
    )
    handler_b = workflow_output_handler(
        can_handle=(CreatedNote,),
        each_message=lambda m: "b",
    )
    message = CreatedNote(
        note_name="X",
        note_content="Y",
        metadata=RecordedMessageMetadata(runtime_id="r1", source="test"),
    )

    results = dispatch_output_handlers([handler_a, handler_b], message)

    assert results == ["a", "b"]


def test_dispatch_filters_none_results() -> None:
    handler = workflow_output_handler(
        can_handle=(CreatedNote,),
        each_message=lambda m: None,
    )
    message = CreatedNote(
        note_name="X",
        note_content="Y",
        metadata=RecordedMessageMetadata(runtime_id="r1", source="test"),
    )

    results = dispatch_output_handlers([handler], message)

    assert results == []


def test_dispatch_with_each_batch_handler() -> None:
    handler = workflow_output_handler(
        can_handle=(NoteUpdated,),
        each_batch=lambda ms: [f"batch:{len(ms)}"],
        batch_size=2,
    )
    messages = [_note_updated("N1"), _note_updated("N2")]

    results = dispatch_output_handlers([handler], messages)

    assert results == ["batch:2"]


def test_dispatch_each_batch_filters_none() -> None:
    handler = workflow_output_handler(
        can_handle=(NoteUpdated,),
        each_batch=lambda ms: [None, "ok", None],
        batch_size=2,
    )
    messages = [_note_updated("N1"), _note_updated("N2")]

    results = dispatch_output_handlers([handler], messages)

    assert results == ["ok"]


def test_dispatcher_buffers_until_batch_size_then_emits() -> None:
    handler = workflow_output_handler(
        can_handle=(NoteUpdated,),
        each_batch=lambda ms: [",".join(message.note_name for message in ms)],
        batch_size=2,
    )
    dispatcher = OutputHandlerDispatcher([handler])

    first_results = dispatcher.dispatch(_note_updated("A"))
    second_results = dispatcher.dispatch(_note_updated("B"))

    assert first_results == []
    assert second_results == ["A,B"]


def test_dispatcher_flushes_trailing_partial_batch() -> None:
    handler = workflow_output_handler(
        can_handle=(NoteUpdated,),
        each_batch=lambda ms: [f"batch:{len(ms)}:{','.join(message.note_name for message in ms)}"],
        batch_size=2,
    )
    dispatcher = OutputHandlerDispatcher([handler])

    assert dispatcher.dispatch(_note_updated("A")) == []
    assert dispatcher.dispatch(_note_updated("B")) == ["batch:2:A,B"]
    assert dispatcher.dispatch(_note_updated("C")) == []
    assert dispatcher.dispatch(_note_updated("D")) == ["batch:2:C,D"]
    assert dispatcher.dispatch(_note_updated("E")) == []

    flushed_results = dispatcher.flush()

    assert flushed_results == ["batch:1:E"]


def test_dispatcher_clear_discards_pending_partial_batch() -> None:
    handled_batches: list[list[str]] = []

    def each_batch(ms: list[NoteUpdated]) -> list[str | None]:
        handled_batches.append([message.note_name for message in ms])
        return ["ok"]

    handler = workflow_output_handler(
        can_handle=(NoteUpdated,),
        each_batch=each_batch,
        batch_size=2,
    )
    dispatcher = OutputHandlerDispatcher([handler])

    assert dispatcher.dispatch(_note_updated("A")) == []
    dispatcher.clear()

    assert dispatcher.flush() == []
    assert handled_batches == []
