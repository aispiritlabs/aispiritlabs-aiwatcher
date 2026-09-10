from __future__ import annotations

import uuid
from collections.abc import Callable, Sequence
from dataclasses import dataclass, field
from enum import StrEnum
from typing import Protocol, runtime_checkable

from aiwatcher_agentic.workflow.errors import ConcurrencyConflictError, IllegalStateError
from aiwatcher_agentic.workflow.event_store import EventStore, StreamPosition
from aiwatcher_agentic.workflow.messages import Message


class StartFrom(StrEnum):
    BEGINNING = "BEGINNING"
    END = "END"
    CURRENT = "CURRENT"


class CheckpointStore(Protocol):
    def read(self, processor_id: str) -> StreamPosition | None: ...
    def store(self, processor_id: str, position: StreamPosition) -> None: ...


@runtime_checkable
class CompareAndSwapCheckpointStore(Protocol):
    def compare_and_store(
        self,
        processor_id: str,
        expected_position: StreamPosition | None,
        position: StreamPosition,
    ) -> bool: ...


class ProcessorLock(Protocol):
    def acquire(self, processor_key: str, instance_id: str, lease_seconds: float) -> bool: ...
    def refresh(self, processor_key: str, instance_id: str, lease_seconds: float) -> bool: ...
    def release(self, processor_key: str, instance_id: str) -> None: ...


class InMemoryCheckpointStore:
    def __init__(self) -> None:
        self._checkpoints: dict[str, StreamPosition] = {}

    def read(self, processor_id: str) -> StreamPosition | None:
        return self._checkpoints.get(processor_id)

    def store(self, processor_id: str, position: StreamPosition) -> None:
        self._checkpoints[processor_id] = position

    def compare_and_store(
        self,
        processor_id: str,
        expected_position: StreamPosition | None,
        position: StreamPosition,
    ) -> bool:
        if self._checkpoints.get(processor_id) != expected_position:
            return False
        self._checkpoints[processor_id] = position
        return True


class InMemoryProcessorLock:
    def __init__(self) -> None:
        self._owners: dict[str, str] = {}

    def acquire(self, processor_key: str, instance_id: str, lease_seconds: float) -> bool:
        del lease_seconds
        owner = self._owners.get(processor_key)
        if owner is not None and owner != instance_id:
            return False
        self._owners[processor_key] = instance_id
        return True

    def refresh(self, processor_key: str, instance_id: str, lease_seconds: float) -> bool:
        del lease_seconds
        return self._owners.get(processor_key) == instance_id

    def release(self, processor_key: str, instance_id: str) -> None:
        if self._owners.get(processor_key) == instance_id:
            self._owners.pop(processor_key, None)


type BatchHandler = Callable[[Sequence[Message]], None]


@dataclass(frozen=True, slots=True)
class ProcessorConfig:
    processor_id: str
    start_from: StartFrom = StartFrom.BEGINNING
    batch_size: int = 100
    version: int = 1
    partition: str = "default"
    instance_id: str = field(default_factory=lambda: uuid.uuid4().hex)
    lease_seconds: float = 30.0

    def __post_init__(self) -> None:
        if not self.processor_id.strip():
            raise ValueError("processor_id must not be empty")
        if self.batch_size < 1:
            raise ValueError("batch_size must be positive")
        if self.version < 1:
            raise ValueError("processor version must be positive")
        if not self.partition.strip():
            raise ValueError("processor partition must not be empty")
        if self.lease_seconds <= 0:
            raise ValueError("lease_seconds must be positive")


@dataclass(frozen=True, slots=True)
class ProcessorStats:
    processed_messages: int = 0
    processed_batches: int = 0
    lag: int = 0


@dataclass(slots=True)
class MessageProcessor:
    """At-least-once stream/global-feed processor with durable checkpoint fencing."""

    config: ProcessorConfig
    handler: BatchHandler
    checkpoint_store: CheckpointStore
    lock: ProcessorLock | None = None
    _active: bool = field(default=False, init=False)
    _source: str | None = field(default=None, init=False)
    _checkpoint_key: str = field(default="", init=False)
    _stats: ProcessorStats = field(default_factory=ProcessorStats, init=False)

    @property
    def processor_id(self) -> str:
        return self.config.processor_id

    @property
    def is_active(self) -> bool:
        return self._active

    @property
    def stats(self) -> ProcessorStats:
        return self._stats

    def start(self, event_store: EventStore, stream_name: str | None = None) -> StreamPosition:
        self._source = stream_name
        self._checkpoint_key = self._build_checkpoint_key(stream_name)
        if self.lock is not None and not self.lock.acquire(
            self._checkpoint_key,
            self.config.instance_id,
            self.config.lease_seconds,
        ):
            raise IllegalStateError(f"Processor lock is held: {self._checkpoint_key}")

        try:
            position = self._resolve_start_position(event_store, stream_name)
            if self.checkpoint_store.read(self._checkpoint_key) is None:
                self.checkpoint_store.store(self._checkpoint_key, position)
            self._mirror_legacy_checkpoint(position)
            self._active = True
            return position
        except Exception:
            if self.lock is not None:
                self.lock.release(self._checkpoint_key, self.config.instance_id)
            raise

    def process(self, event_store: EventStore, stream_name: str | None = None) -> int:
        if not self._active:
            return 0
        self._assert_source(stream_name)
        from_position = self._read_checkpoint()
        if from_position is None:
            from_position = self._resolve_start_position(event_store, stream_name)

        if stream_name is None:
            result = event_store.read_all(
                from_position=from_position,
                max_count=self.config.batch_size,
            )
            events = result.events
            new_position = result.next_position
            end_position = result.end_position
        else:
            page = event_store.read_stream(
                stream_name,
                from_position=from_position,
                max_count=self.config.batch_size,
            )
            events = page.events
            new_position = from_position + len(events)
            end_position = page.current_version

        if not events:
            self._stats = ProcessorStats(
                processed_messages=self._stats.processed_messages,
                processed_batches=self._stats.processed_batches,
                lag=max(end_position - new_position, 0),
            )
            return 0

        self.handler(events)
        self._commit_checkpoint(from_position, new_position)
        if self.lock is not None and not self.lock.refresh(
            self._checkpoint_key,
            self.config.instance_id,
            self.config.lease_seconds,
        ):
            self._active = False
            raise IllegalStateError(f"Processor lease was lost: {self._checkpoint_key}")

        self._stats = ProcessorStats(
            processed_messages=self._stats.processed_messages + len(events),
            processed_batches=self._stats.processed_batches + 1,
            lag=max(end_position - new_position, 0),
        )
        return len(events)

    def run_to_end(self, event_store: EventStore, stream_name: str | None = None) -> int:
        total = 0
        while True:
            processed = self.process(event_store, stream_name)
            if processed == 0:
                break
            total += processed
        return total

    def close(self) -> None:
        if self.lock is not None and self._checkpoint_key:
            self.lock.release(self._checkpoint_key, self.config.instance_id)
        self._active = False

    def _resolve_start_position(
        self,
        event_store: EventStore,
        stream_name: str | None,
    ) -> StreamPosition:
        existing = self._read_checkpoint()
        if existing is not None:
            return existing
        if self.config.start_from == StartFrom.BEGINNING:
            return 0
        if stream_name is None:
            return event_store.read_all(from_position=0, max_count=0).end_position
        return event_store.read_stream(stream_name).current_version

    def _read_checkpoint(self) -> StreamPosition | None:
        if not self._checkpoint_key:
            return None
        checkpoint = self.checkpoint_store.read(self._checkpoint_key)
        if checkpoint is not None:
            return checkpoint
        if self.config.version == 1 and self.config.partition == "default":
            return self.checkpoint_store.read(self.config.processor_id)
        return None

    def _commit_checkpoint(self, expected: StreamPosition, position: StreamPosition) -> None:
        if isinstance(self.checkpoint_store, CompareAndSwapCheckpointStore):
            if not self.checkpoint_store.compare_and_store(
                self._checkpoint_key,
                expected,
                position,
            ):
                raise ConcurrencyConflictError(self._checkpoint_key, expected, "changed")
        else:
            self.checkpoint_store.store(self._checkpoint_key, position)
        self._mirror_legacy_checkpoint(position)

    def _mirror_legacy_checkpoint(self, position: StreamPosition) -> None:
        if self.config.version == 1 and self.config.partition == "default":
            self.checkpoint_store.store(self.config.processor_id, position)

    def _build_checkpoint_key(self, stream_name: str | None) -> str:
        source = "$all" if stream_name is None else stream_name
        return (
            f"{self.config.processor_id}:v{self.config.version}:"
            f"p{self.config.partition}:source:{source}"
        )

    def _assert_source(self, stream_name: str | None) -> None:
        if stream_name != self._source:
            raise IllegalStateError(
                f"Processor started for {self._source!r}, cannot process {stream_name!r}"
            )
