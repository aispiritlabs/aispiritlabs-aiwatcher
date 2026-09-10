from __future__ import annotations

import pytest

from aiwatcher_agentic.workflow.errors import ConcurrencyConflictError, IllegalStateError
from aiwatcher_agentic.workflow.event_store import InMemoryEventStore
from aiwatcher_agentic.workflow.messages import Event, Message
from aiwatcher_agentic.workflow.processor import (
    InMemoryCheckpointStore,
    InMemoryProcessorLock,
    MessageProcessor,
    ProcessorConfig,
    StartFrom,
)


def _event(event_type: str, **data: object) -> Event:
    return Event(type=event_type, data=dict(data))


def _seed_stream(store: InMemoryEventStore, stream: str, count: int) -> None:
    events = [_event(f"evt_{i}") for i in range(count)]
    store.append_to_stream(stream, events)


class TestInMemoryCheckpointStore:
    def test_returns_none_for_unknown_processor(self) -> None:
        store = InMemoryCheckpointStore()
        assert store.read("unknown") is None

    def test_stores_and_reads_checkpoint(self) -> None:
        store = InMemoryCheckpointStore()
        store.store("p1", 42)
        assert store.read("p1") == 42

    def test_overwrites_checkpoint(self) -> None:
        store = InMemoryCheckpointStore()
        store.store("p1", 10)
        store.store("p1", 20)
        assert store.read("p1") == 20

    def test_isolates_processors(self) -> None:
        store = InMemoryCheckpointStore()
        store.store("p1", 10)
        store.store("p2", 20)
        assert store.read("p1") == 10
        assert store.read("p2") == 20


class TestMessageProcessorBasicFlow:
    def test_processes_all_events(self) -> None:
        event_store = InMemoryEventStore()
        _seed_stream(event_store, "orders", 5)

        collected: list[Message] = []
        processor = MessageProcessor(
            config=ProcessorConfig(processor_id="proj-1"),
            handler=lambda events: collected.extend(events),
            checkpoint_store=InMemoryCheckpointStore(),
        )

        processor.start(event_store, "orders")
        total = processor.run_to_end(event_store, "orders")

        assert total == 5
        assert len(collected) == 5

    def test_processes_in_batches(self) -> None:
        event_store = InMemoryEventStore()
        _seed_stream(event_store, "orders", 7)

        batch_sizes: list[int] = []
        processor = MessageProcessor(
            config=ProcessorConfig(processor_id="proj-1", batch_size=3),
            handler=lambda events: batch_sizes.append(len(events)),
            checkpoint_store=InMemoryCheckpointStore(),
        )

        processor.start(event_store, "orders")
        total = processor.run_to_end(event_store, "orders")

        assert total == 7
        assert batch_sizes == [3, 3, 1]

    def test_empty_stream_processes_nothing(self) -> None:
        event_store = InMemoryEventStore()

        called = False

        def handler(events: object) -> None:
            nonlocal called
            called = True

        processor = MessageProcessor(
            config=ProcessorConfig(processor_id="proj-1"),
            handler=handler,
            checkpoint_store=InMemoryCheckpointStore(),
        )

        processor.start(event_store, "orders")
        total = processor.run_to_end(event_store, "orders")

        assert total == 0
        assert not called


class TestMessageProcessorCheckpointing:
    def test_resumes_from_checkpoint_after_restart(self) -> None:
        event_store = InMemoryEventStore()
        _seed_stream(event_store, "orders", 5)

        checkpoint_store = InMemoryCheckpointStore()
        collected: list[Message] = []

        # First run: process first 3
        processor = MessageProcessor(
            config=ProcessorConfig(processor_id="proj-1", batch_size=3),
            handler=lambda events: collected.extend(events),
            checkpoint_store=checkpoint_store,
        )
        processor.start(event_store, "orders")
        processor.process(event_store, "orders")
        processor.close()

        assert len(collected) == 3
        assert checkpoint_store.read("proj-1") == 3

        # Second run: new processor, same checkpoint store — resumes from 3
        collected.clear()
        processor2 = MessageProcessor(
            config=ProcessorConfig(processor_id="proj-1", batch_size=10),
            handler=lambda events: collected.extend(events),
            checkpoint_store=checkpoint_store,
        )
        processor2.start(event_store, "orders")
        total = processor2.run_to_end(event_store, "orders")

        assert total == 2
        assert len(collected) == 2
        assert collected[0].type == "evt_3"
        assert collected[1].type == "evt_4"

    def test_new_events_after_catchup(self) -> None:
        event_store = InMemoryEventStore()
        _seed_stream(event_store, "orders", 3)

        checkpoint_store = InMemoryCheckpointStore()
        collected: list[Message] = []

        processor = MessageProcessor(
            config=ProcessorConfig(processor_id="proj-1"),
            handler=lambda events: collected.extend(events),
            checkpoint_store=checkpoint_store,
        )
        processor.start(event_store, "orders")
        processor.run_to_end(event_store, "orders")
        assert len(collected) == 3

        # Append more events
        event_store.append_to_stream("orders", [_event("evt_3"), _event("evt_4")])

        # Process again — only new events
        collected.clear()
        total = processor.run_to_end(event_store, "orders")
        assert total == 2
        assert collected[0].type == "evt_3"


class TestMessageProcessorStartFrom:
    def test_start_from_beginning(self) -> None:
        event_store = InMemoryEventStore()
        _seed_stream(event_store, "orders", 5)

        collected: list[Message] = []
        processor = MessageProcessor(
            config=ProcessorConfig(processor_id="proj-1", start_from=StartFrom.BEGINNING),
            handler=lambda events: collected.extend(events),
            checkpoint_store=InMemoryCheckpointStore(),
        )

        processor.start(event_store, "orders")
        processor.run_to_end(event_store, "orders")
        assert len(collected) == 5

    def test_start_from_end(self) -> None:
        event_store = InMemoryEventStore()
        _seed_stream(event_store, "orders", 5)

        collected: list[Message] = []
        processor = MessageProcessor(
            config=ProcessorConfig(processor_id="proj-1", start_from=StartFrom.END),
            handler=lambda events: collected.extend(events),
            checkpoint_store=InMemoryCheckpointStore(),
        )

        processor.start(event_store, "orders")
        # Existing events are skipped
        total = processor.run_to_end(event_store, "orders")
        assert total == 0

        # New events after start are processed
        event_store.append_to_stream("orders", [_event("new_1")])
        total = processor.run_to_end(event_store, "orders")
        assert total == 1
        assert collected[0].type == "new_1"

    def test_start_from_current_without_checkpoint(self) -> None:
        event_store = InMemoryEventStore()
        _seed_stream(event_store, "orders", 5)

        collected: list[Message] = []
        processor = MessageProcessor(
            config=ProcessorConfig(processor_id="proj-1", start_from=StartFrom.CURRENT),
            handler=lambda events: collected.extend(events),
            checkpoint_store=InMemoryCheckpointStore(),
        )

        processor.start(event_store, "orders")
        total = processor.run_to_end(event_store, "orders")
        # CURRENT without checkpoint = END
        assert total == 0

    def test_start_from_current_with_checkpoint(self) -> None:
        event_store = InMemoryEventStore()
        _seed_stream(event_store, "orders", 5)

        checkpoint_store = InMemoryCheckpointStore()
        checkpoint_store.store("proj-1", 2)

        collected: list[Message] = []
        processor = MessageProcessor(
            config=ProcessorConfig(processor_id="proj-1", start_from=StartFrom.CURRENT),
            handler=lambda events: collected.extend(events),
            checkpoint_store=checkpoint_store,
        )

        processor.start(event_store, "orders")
        total = processor.run_to_end(event_store, "orders")
        assert total == 3
        assert collected[0].type == "evt_2"


class TestMessageProcessorLifecycle:
    def test_inactive_processor_does_not_process(self) -> None:
        event_store = InMemoryEventStore()
        _seed_stream(event_store, "orders", 5)

        processor = MessageProcessor(
            config=ProcessorConfig(processor_id="proj-1"),
            handler=lambda events: None,
            checkpoint_store=InMemoryCheckpointStore(),
        )

        # Not started — should not process
        total = processor.process(event_store, "orders")
        assert total == 0
        assert not processor.is_active

    def test_close_deactivates_processor(self) -> None:
        event_store = InMemoryEventStore()
        processor = MessageProcessor(
            config=ProcessorConfig(processor_id="proj-1"),
            handler=lambda events: None,
            checkpoint_store=InMemoryCheckpointStore(),
        )

        processor.start(event_store, "orders")
        assert processor.is_active

        processor.close()
        assert not processor.is_active

        total = processor.process(event_store, "orders")
        assert total == 0


class TestGlobalFeedAndFencing:
    def test_global_feed_preserves_commit_order_across_streams(self) -> None:
        event_store = InMemoryEventStore()
        event_store.append_to_stream("orders:1", (_event("order.created"),))
        event_store.append_to_stream("research:1", (_event("research.started"),))
        event_store.append_to_stream("orders:2", (_event("order.created"),))
        collected: list[Message] = []
        processor = MessageProcessor(
            config=ProcessorConfig(processor_id="all-events", batch_size=2),
            handler=lambda events: collected.extend(events),
            checkpoint_store=InMemoryCheckpointStore(),
        )

        processor.start(event_store)
        processed = processor.run_to_end(event_store)

        assert processed == 3
        assert [event.metadata.global_position for event in collected] == [0, 1, 2]
        assert processor.stats.processed_batches == 2
        assert processor.stats.lag == 0

    def test_processor_versions_have_independent_rebuild_checkpoints(self) -> None:
        event_store = InMemoryEventStore()
        _seed_stream(event_store, "orders", 2)
        checkpoints = InMemoryCheckpointStore()
        first: list[Message] = []
        rebuilt: list[Message] = []
        v1 = MessageProcessor(
            config=ProcessorConfig(processor_id="orders-view", version=1),
            handler=lambda events: first.extend(events),
            checkpoint_store=checkpoints,
        )
        v2 = MessageProcessor(
            config=ProcessorConfig(processor_id="orders-view", version=2),
            handler=lambda events: rebuilt.extend(events),
            checkpoint_store=checkpoints,
        )

        v1.start(event_store, "orders")
        v1.run_to_end(event_store, "orders")
        v2.start(event_store, "orders")
        v2.run_to_end(event_store, "orders")

        assert len(first) == 2
        assert len(rebuilt) == 2

    def test_compare_and_swap_never_overwrites_a_newer_checkpoint(self) -> None:
        event_store = InMemoryEventStore()
        _seed_stream(event_store, "orders", 2)
        checkpoints = InMemoryCheckpointStore()
        checkpoint_key = "orders-view:v1:pdefault:source:orders"

        def racing_handler(events: object) -> None:
            del events
            checkpoints.store(checkpoint_key, 99)

        processor = MessageProcessor(
            config=ProcessorConfig(processor_id="orders-view"),
            handler=racing_handler,
            checkpoint_store=checkpoints,
        )
        processor.start(event_store, "orders")

        with pytest.raises(ConcurrencyConflictError):
            processor.process(event_store, "orders")
        assert checkpoints.read(checkpoint_key) == 99

    def test_lease_prevents_two_processors_owning_same_partition(self) -> None:
        event_store = InMemoryEventStore()
        checkpoints = InMemoryCheckpointStore()
        lock = InMemoryProcessorLock()
        first = MessageProcessor(
            config=ProcessorConfig(processor_id="orders-view", instance_id="one"),
            handler=lambda events: None,
            checkpoint_store=checkpoints,
            lock=lock,
        )
        second = MessageProcessor(
            config=ProcessorConfig(processor_id="orders-view", instance_id="two"),
            handler=lambda events: None,
            checkpoint_store=checkpoints,
            lock=lock,
        )

        first.start(event_store, "orders")
        with pytest.raises(IllegalStateError):
            second.start(event_store, "orders")
        first.close()
        second.start(event_store, "orders")
        assert second.is_active
