from __future__ import annotations

from collections.abc import Sequence

import pytest

from aiwatcher_agentic.workflow.errors import ConcurrencyConflictError, DuplicateMessageError
from aiwatcher_agentic.workflow.event_store import (
    NO_CONCURRENCY_CHECK,
    STREAM_DOES_NOT_EXIST,
    STREAM_EXISTS,
    InMemoryEventStore,
)
from aiwatcher_agentic.workflow.messages import Event, Message, RecordedMessageMetadata


def _event(event_type: str, **data: object) -> Event:
    return Event(type=event_type, data=dict(data))


class TestReadStream:
    def test_returns_empty_for_nonexistent_stream(self) -> None:
        store = InMemoryEventStore()
        result = store.read_stream("cart-1")
        assert result.events == ()
        assert result.current_version == 0
        assert result.stream_exists is False

    def test_returns_appended_events(self) -> None:
        store = InMemoryEventStore()
        store.append_to_stream("cart-1", [_event("item_added", item="shoes")])
        store.append_to_stream("cart-1", [_event("item_added", item="hat")])

        result = store.read_stream("cart-1")
        assert len(result.events) == 2
        assert result.events[0].data == {"item": "shoes"}
        assert result.events[1].data == {"item": "hat"}
        assert result.current_version == 2
        assert result.stream_exists is True

    def test_reads_from_position(self) -> None:
        store = InMemoryEventStore()
        store.append_to_stream("cart-1", [_event("a"), _event("b"), _event("c")])

        result = store.read_stream("cart-1", from_position=1)
        assert len(result.events) == 2
        assert result.events[0].type == "b"
        assert result.events[1].type == "c"


class TestReadAll:
    def test_reads_global_feed_and_filters_by_stream_prefix(self) -> None:
        store = InMemoryEventStore()
        store.append_to_stream("research:one", (_event("one"),))
        store.append_to_stream("orders:one", (_event("two"),))
        store.append_to_stream("research:two", (_event("three"),))

        all_events = store.read_all(from_position=0, max_count=2)
        research = store.read_all(stream_prefix="research:")

        assert [event.type for event in all_events.events] == ["one", "two"]
        assert all_events.next_position == 2
        assert all_events.end_position == 3
        assert [event.type for event in research.events] == ["one", "three"]
        assert research.next_position == 3

    def test_limits_by_max_count(self) -> None:
        store = InMemoryEventStore()
        store.append_to_stream("cart-1", [_event("a"), _event("b"), _event("c")])

        result = store.read_stream("cart-1", max_count=2)
        assert len(result.events) == 2
        assert result.events[0].type == "a"
        assert result.events[1].type == "b"

    def test_combines_from_position_and_max_count(self) -> None:
        store = InMemoryEventStore()
        store.append_to_stream("cart-1", [_event("a"), _event("b"), _event("c"), _event("d")])

        result = store.read_stream("cart-1", from_position=1, max_count=2)
        assert len(result.events) == 2
        assert result.events[0].type == "b"
        assert result.events[1].type == "c"


class TestAggregateStream:
    def test_folds_events_into_state(self) -> None:
        store = InMemoryEventStore()
        store.append_to_stream(
            "cart-1",
            [
                _event("item_added", item="shoes", qty=2),
                _event("item_added", item="hat", qty=1),
            ],
        )

        def evolve(state: dict[str, int], event: Event) -> dict[str, int]:
            if event.type == "item_added":
                return {**state, event.data["item"]: event.data["qty"]}
            return state

        result = store.aggregate_stream(
            "cart-1",
            evolve=evolve,
            initial_state=dict,
        )

        assert result.state == {"shoes": 2, "hat": 1}
        assert result.current_version == 2
        assert result.stream_exists is True

    def test_returns_initial_state_for_nonexistent_stream(self) -> None:
        store = InMemoryEventStore()

        result = store.aggregate_stream(
            "cart-999",
            evolve=lambda s, e: s,
            initial_state=list,
        )

        assert result.state == []
        assert result.current_version == 0
        assert result.stream_exists is False

    def test_aggregates_from_position(self) -> None:
        store = InMemoryEventStore()
        store.append_to_stream(
            "counter-1",
            [
                _event("incremented"),
                _event("incremented"),
                _event("incremented"),
            ],
        )

        result = store.aggregate_stream(
            "counter-1",
            evolve=lambda count, _: count + 1,
            initial_state=lambda: 0,
            from_position=1,
        )

        assert result.state == 2


class TestAppendToStream:
    def test_creates_new_stream(self) -> None:
        store = InMemoryEventStore()
        result = store.append_to_stream("cart-1", [_event("created")])

        assert result.next_version == 1
        assert store.stream_exists("cart-1")

    def test_appends_to_existing_stream(self) -> None:
        store = InMemoryEventStore()
        store.append_to_stream("cart-1", [_event("a")])
        result = store.append_to_stream("cart-1", [_event("b"), _event("c")])

        assert result.next_version == 3
        assert len(store.read_stream("cart-1").events) == 3

    def test_appends_multiple_events_at_once(self) -> None:
        store = InMemoryEventStore()
        result = store.append_to_stream("cart-1", [_event("a"), _event("b")])

        assert result.next_version == 2

    def test_streams_are_isolated(self) -> None:
        store = InMemoryEventStore()
        store.append_to_stream("cart-1", [_event("a")])
        store.append_to_stream("cart-2", [_event("b")])

        assert store.read_stream("cart-1").events[0].type == "a"
        assert store.read_stream("cart-2").events[0].type == "b"

    def test_same_logical_message_can_have_distinct_transport_records(self) -> None:
        store = InMemoryEventStore()
        records = [
            Event(
                type="message_started",
                metadata=RecordedMessageMetadata(
                    message_id="logical-1",
                    event_id="event-1",
                    idempotency_key="logical-1:started",
                ),
            ),
            Event(
                type="message_chunk",
                metadata=RecordedMessageMetadata(
                    message_id="logical-1",
                    event_id="event-2",
                    idempotency_key="logical-1:chunk:0",
                ),
            ),
        ]

        store.append_to_stream("messages", records)

        assert len(store.read_stream("messages").events) == 2

    def test_rejects_duplicate_idempotency_key(self) -> None:
        store = InMemoryEventStore()
        first = Event(
            type="requested",
            metadata=RecordedMessageMetadata(
                message_id="logical-1",
                event_id="event-1",
                idempotency_key="request-1",
            ),
        )
        duplicate = Event(
            type="requested",
            metadata=RecordedMessageMetadata(
                message_id="logical-2",
                event_id="event-2",
                idempotency_key="request-1",
            ),
        )
        store.append_to_stream("messages", (first,))

        with pytest.raises(DuplicateMessageError):
            store.append_to_stream("messages", (duplicate,))


class TestConcurrencyControl:
    def test_specific_version_succeeds_when_matching(self) -> None:
        store = InMemoryEventStore()
        store.append_to_stream("cart-1", [_event("a")])
        result = store.append_to_stream("cart-1", [_event("b")], expected_version=1)
        assert result.next_version == 2

    def test_specific_version_fails_when_mismatched(self) -> None:
        store = InMemoryEventStore()
        store.append_to_stream("cart-1", [_event("a"), _event("b")])

        with pytest.raises(ConcurrencyConflictError) as exc_info:
            store.append_to_stream("cart-1", [_event("c")], expected_version=1)

        assert exc_info.value.stream_name == "cart-1"
        assert exc_info.value.expected == 1
        assert exc_info.value.current == 2

    def test_stream_exists_succeeds_when_stream_has_events(self) -> None:
        store = InMemoryEventStore()
        store.append_to_stream("cart-1", [_event("a")])
        result = store.append_to_stream("cart-1", [_event("b")], expected_version=STREAM_EXISTS)
        assert result.next_version == 2

    def test_stream_exists_fails_on_empty_store(self) -> None:
        store = InMemoryEventStore()
        with pytest.raises(ConcurrencyConflictError):
            store.append_to_stream("cart-1", [_event("a")], expected_version=STREAM_EXISTS)

    def test_stream_does_not_exist_succeeds_on_new_stream(self) -> None:
        store = InMemoryEventStore()
        result = store.append_to_stream(
            "cart-1", [_event("a")], expected_version=STREAM_DOES_NOT_EXIST
        )
        assert result.next_version == 1

    def test_stream_does_not_exist_fails_when_stream_has_events(self) -> None:
        store = InMemoryEventStore()
        store.append_to_stream("cart-1", [_event("a")])
        with pytest.raises(ConcurrencyConflictError):
            store.append_to_stream("cart-1", [_event("b")], expected_version=STREAM_DOES_NOT_EXIST)

    def test_no_concurrency_check_always_succeeds(self) -> None:
        store = InMemoryEventStore()
        store.append_to_stream("cart-1", [_event("a")])
        result = store.append_to_stream(
            "cart-1", [_event("b")], expected_version=NO_CONCURRENCY_CHECK
        )
        assert result.next_version == 2

    def test_no_concurrency_check_is_default(self) -> None:
        store = InMemoryEventStore()
        store.append_to_stream("cart-1", [_event("a")])
        result = store.append_to_stream("cart-1", [_event("b")])
        assert result.next_version == 2

    def test_version_zero_for_first_append(self) -> None:
        store = InMemoryEventStore()
        result = store.append_to_stream("cart-1", [_event("a")], expected_version=0)
        assert result.next_version == 1


class TestStreamExists:
    def test_false_for_nonexistent_stream(self) -> None:
        store = InMemoryEventStore()
        assert store.stream_exists("cart-1") is False

    def test_true_after_append(self) -> None:
        store = InMemoryEventStore()
        store.append_to_stream("cart-1", [_event("a")])
        assert store.stream_exists("cart-1") is True


class TestAggregateWorkflow:
    """End-to-end: load state, decide, append — the core event sourcing loop."""

    def test_load_decide_append(self) -> None:
        store = InMemoryEventStore()

        # Initial command: create cart
        store.append_to_stream(
            "cart-1",
            [_event("cart_opened", customer="alice")],
            expected_version=STREAM_DOES_NOT_EXIST,
        )

        # Load current state
        result = store.aggregate_stream(
            "cart-1",
            evolve=_cart_evolve,
            initial_state=lambda: {"status": "empty", "items": []},
        )

        assert result.state["status"] == "opened"
        assert result.state["items"] == []

        # Decide: add item (business logic checks state)
        assert result.state["status"] == "opened"  # invariant check
        new_events = [_event("item_added", item="shoes")]

        # Append with version check
        append_result = store.append_to_stream(
            "cart-1",
            new_events,
            expected_version=result.current_version,
        )
        assert append_result.next_version == 2

        # Verify final state
        final = store.aggregate_stream(
            "cart-1",
            evolve=_cart_evolve,
            initial_state=lambda: {"status": "empty", "items": []},
        )
        assert final.state == {"status": "opened", "items": ["shoes"]}
        assert final.current_version == 2

    def test_concurrent_append_is_rejected(self) -> None:
        store = InMemoryEventStore()
        store.append_to_stream("cart-1", [_event("cart_opened", customer="alice")])

        # Two concurrent readers load version 1
        reader_a = store.aggregate_stream(
            "cart-1", evolve=_cart_evolve, initial_state=lambda: {"status": "empty", "items": []}
        )
        reader_b = store.aggregate_stream(
            "cart-1", evolve=_cart_evolve, initial_state=lambda: {"status": "empty", "items": []}
        )

        # Reader A appends successfully
        store.append_to_stream(
            "cart-1",
            [_event("item_added", item="shoes")],
            expected_version=reader_a.current_version,
        )

        # Reader B's append is rejected — version moved from 1 to 2
        with pytest.raises(ConcurrencyConflictError):
            store.append_to_stream(
                "cart-1",
                [_event("item_added", item="hat")],
                expected_version=reader_b.current_version,
            )


class TestAfterCommitHooks:
    def test_hook_receives_stream_name_and_events(self) -> None:
        calls: list[tuple[str, list[str]]] = []

        def hook(stream_name: str, events: Sequence[Message]) -> None:
            calls.append((stream_name, [e.type for e in events]))

        store = InMemoryEventStore(after_commit_hooks=[hook])
        store.append_to_stream("cart-1", [_event("a"), _event("b")])

        assert calls == [("cart-1", ["a", "b"])]

    def test_hook_runs_after_events_are_stored(self) -> None:
        stored_version: list[int] = []

        def hook(stream_name: str, events: object) -> None:
            # At this point, events should already be in the store
            result = store.read_stream(stream_name)
            stored_version.append(result.current_version)

        store = InMemoryEventStore(after_commit_hooks=[hook])
        store.append_to_stream("cart-1", [_event("a")])

        assert stored_version == [1]

    def test_hook_failure_does_not_prevent_append(self) -> None:
        def failing_hook(stream_name: str, events: object) -> None:
            raise RuntimeError("hook failed")

        store = InMemoryEventStore(after_commit_hooks=[failing_hook])
        result = store.append_to_stream("cart-1", [_event("a")])

        assert result.next_version == 1
        assert store.read_stream("cart-1").events[0].type == "a"

    def test_multiple_hooks_all_called(self) -> None:
        calls: list[str] = []
        store = InMemoryEventStore(
            after_commit_hooks=[
                lambda s, e: calls.append("hook1"),
                lambda s, e: calls.append("hook2"),
            ]
        )
        store.append_to_stream("cart-1", [_event("a")])
        assert calls == ["hook1", "hook2"]

    def test_add_hook_dynamically(self) -> None:
        calls: list[str] = []
        store = InMemoryEventStore()
        store.add_after_commit_hook(lambda s, e: calls.append("dynamic"))
        store.append_to_stream("cart-1", [_event("a")])
        assert calls == ["dynamic"]


class TestInlineProjections:
    def test_projection_receives_events_on_append(self) -> None:
        projected: list[tuple[str, list[str]]] = []

        class TrackingProjection:
            def project(self, stream_name: str, events: Sequence[Message]) -> None:
                projected.append((stream_name, [e.type for e in events]))

        store = InMemoryEventStore(inline_projections=[TrackingProjection()])
        store.append_to_stream("orders-1", [_event("placed"), _event("paid")])

        assert projected == [("orders-1", ["placed", "paid"])]

    def test_projection_runs_before_hooks(self) -> None:
        order: list[str] = []

        class OrderProjection:
            def project(self, stream_name: str, events: object) -> None:
                order.append("projection")

        store = InMemoryEventStore(
            inline_projections=[OrderProjection()],
            after_commit_hooks=[lambda s, e: order.append("hook")],
        )
        store.append_to_stream("s-1", [_event("a")])
        assert order == ["projection", "hook"]

    def test_add_projection_dynamically(self) -> None:
        projected: list[str] = []

        class DynProjection:
            def project(self, stream_name: str, events: object) -> None:
                projected.append(stream_name)

        store = InMemoryEventStore()
        store.add_inline_projection(DynProjection())
        store.append_to_stream("s-1", [_event("a")])
        assert projected == ["s-1"]

    def test_projection_failure_rolls_back_new_stream(self) -> None:
        class FailingProjection:
            def project(self, stream_name: str, events: object) -> None:
                raise RuntimeError("boom")

        store = InMemoryEventStore(inline_projections=[FailingProjection()])

        with pytest.raises(RuntimeError, match="boom"):
            store.append_to_stream("s-1", [_event("a")])

        assert store.stream_exists("s-1") is False
        assert store.read_stream("s-1").events == ()

    def test_projection_failure_rolls_back_existing_stream(self) -> None:
        class FailingProjection:
            def project(self, stream_name: str, events: object) -> None:
                raise RuntimeError("boom")

        store = InMemoryEventStore()
        store.append_to_stream("s-1", [_event("existing")], expected_version=0)
        store.add_inline_projection(FailingProjection())

        with pytest.raises(RuntimeError, match="boom"):
            store.append_to_stream("s-1", [_event("new")], expected_version=1)

        result = store.read_stream("s-1")
        assert tuple(event.type for event in result.events) == ("existing",)
        assert result.current_version == 1

    def test_projection_failure_prevents_after_commit_hooks(self) -> None:
        calls: list[str] = []

        class FailingProjection:
            def project(self, stream_name: str, events: object) -> None:
                raise RuntimeError("boom")

        store = InMemoryEventStore(
            inline_projections=[FailingProjection()],
            after_commit_hooks=[lambda s, e: calls.append("hook")],
        )

        with pytest.raises(RuntimeError, match="boom"):
            store.append_to_stream("s-1", [_event("a")])

        assert calls == []


class TestUpcasting:
    def test_upcaster_transforms_events_on_read(self) -> None:
        def rename_v1_to_v2(event: Event) -> Event:
            if event.type == "item_added_v1":
                return Event(type="item_added", data={**event.data, "version": 2})
            return event

        store = InMemoryEventStore(upcasters=[rename_v1_to_v2])
        # Store old-format events
        store.append_to_stream("cart-1", [Event(type="item_added_v1", data={"item": "shoes"})])

        # Read returns upcasted events
        result = store.read_stream("cart-1")
        assert result.events[0].type == "item_added"
        assert result.events[0].data == {"item": "shoes", "version": 2}

    def test_upcaster_applies_during_aggregate(self) -> None:
        def add_quantity(event: Event) -> Event:
            if event.type == "item_added" and "qty" not in event.data:
                return Event(type="item_added", data={**event.data, "qty": 1})
            return event

        store = InMemoryEventStore(upcasters=[add_quantity])
        store.append_to_stream("cart-1", [Event(type="item_added", data={"item": "shoes"})])

        def evolve(state: dict[str, int], event: Event) -> dict[str, int]:
            if event.type == "item_added":
                return {**state, event.data["item"]: event.data["qty"]}
            return state

        result = store.aggregate_stream("cart-1", evolve=evolve, initial_state=dict)
        assert result.state == {"shoes": 1}

    def test_multiple_upcasters_chain(self) -> None:
        def step1(event: Event) -> Event:
            if event.type == "old":
                return Event(type="mid", data=event.data)
            return event

        def step2(event: Event) -> Event:
            if event.type == "mid":
                return Event(type="new", data=event.data)
            return event

        store = InMemoryEventStore(upcasters=[step1, step2])
        store.append_to_stream("s-1", [Event(type="old", data={})])

        result = store.read_stream("s-1")
        assert result.events[0].type == "new"

    def test_no_upcaster_returns_original(self) -> None:
        store = InMemoryEventStore()
        store.append_to_stream("s-1", [_event("original")])
        result = store.read_stream("s-1")
        assert result.events[0].type == "original"

    def test_add_upcaster_dynamically(self) -> None:
        store = InMemoryEventStore()
        store.append_to_stream("s-1", [Event(type="v1", data={})])

        store.add_upcaster(lambda e: Event(type="v2", data=e.data) if e.type == "v1" else e)

        result = store.read_stream("s-1")
        assert result.events[0].type == "v2"


def _cart_evolve(state: dict[str, object], event: Event) -> dict[str, object]:
    match event.type:
        case "cart_opened":
            return {"status": "opened", "items": []}
        case "item_added":
            raw = state.get("items", [])
            items = list(raw) if isinstance(raw, list) else []
            items.append(event.data["item"])
            return {**state, "items": items}
        case "cart_confirmed":
            return {**state, "status": "confirmed"}
        case _:
            return state
