from __future__ import annotations

import threading
import uuid
from collections.abc import Callable, Sequence
from dataclasses import dataclass

from aiwatcher_agentic.workflow.errors import ConcurrencyConflictError, IllegalStateError
from aiwatcher_agentic.workflow.event_store import EventStore
from aiwatcher_agentic.workflow.messages import Message, normalize_recorded_message
from aiwatcher_agentic.workflow.processor import ProcessorLock

type WorkflowDecision = Callable[[Message], Sequence[Message]]


@dataclass(frozen=True, slots=True)
class DurableWorkflowResult:
    outputs: tuple[Message, ...]
    duplicate: bool
    next_version: int


class DurableWorkflowExecutor:
    """Atomically records one workflow input and its output outbox.

    Decisions may be retried only when their call failed before returning. A
    successful decision is cached across optimistic-concurrency retries, which
    avoids repeating LLM and read-only search calls when another writer wins.
    """

    def __init__(
        self,
        event_store: EventStore,
        *,
        max_conflict_retries: int = 16,
        lock: ProcessorLock | None = None,
        lease_seconds: float = 300.0,
    ) -> None:
        if max_conflict_retries < 1:
            raise ValueError("max_conflict_retries must be positive")
        if lease_seconds <= 0:
            raise ValueError("lease_seconds must be positive")
        self._event_store = event_store
        self._max_conflict_retries = max_conflict_retries
        self._lock = lock
        self._lease_seconds = lease_seconds
        self._instance_id = uuid.uuid4().hex
        self._locks_guard = threading.Lock()
        self._stream_locks: dict[str, threading.Lock] = {}

    def execute(
        self,
        stream_name: str,
        input_message: Message,
        decision: WorkflowDecision,
    ) -> DurableWorkflowResult:
        with self._local_lock(stream_name):
            if self._lock is not None and not self._lock.acquire(
                stream_name,
                self._instance_id,
                self._lease_seconds,
            ):
                raise IllegalStateError(f"Workflow lock is held: {stream_name}")
            try:
                return self._execute_locked(stream_name, input_message, decision)
            finally:
                if self._lock is not None:
                    self._lock.release(stream_name, self._instance_id)

    def _execute_locked(
        self,
        stream_name: str,
        input_message: Message,
        decision: WorkflowDecision,
    ) -> DurableWorkflowResult:
        normalized_input = normalize_recorded_message(input_message)
        prepared_outputs: tuple[Message, ...] | None = None

        for _ in range(self._max_conflict_retries):
            stream = self._event_store.read_stream(stream_name)
            replayed = self._find_outputs(stream.events, normalized_input)
            if replayed is not None:
                return DurableWorkflowResult(
                    outputs=replayed,
                    duplicate=True,
                    next_version=stream.current_version,
                )

            if prepared_outputs is None:
                prepared_outputs = tuple(
                    self._prepare_output(normalized_input, output)
                    for output in decision(normalized_input)
                )
            try:
                appended = self._event_store.append_to_stream(
                    stream_name,
                    (normalized_input, *prepared_outputs),
                    expected_version=stream.current_version,
                )
                return DurableWorkflowResult(
                    outputs=prepared_outputs,
                    duplicate=False,
                    next_version=appended.next_version,
                )
            except ConcurrencyConflictError:
                continue

        raise ConcurrencyConflictError(stream_name, "stable version", "busy")

    def _local_lock(self, stream_name: str) -> threading.Lock:
        with self._locks_guard:
            lock = self._stream_locks.get(stream_name)
            if lock is None:
                lock = threading.Lock()
                self._stream_locks[stream_name] = lock
            return lock

    def persist_outputs(
        self,
        stream_name: str,
        input_message: Message,
        outputs: Sequence[Message],
    ) -> DurableWorkflowResult:
        prepared = tuple(outputs)
        return self.execute(stream_name, input_message, lambda _: prepared)

    @staticmethod
    def _find_outputs(
        events: Sequence[Message],
        input_message: Message,
    ) -> tuple[Message, ...] | None:
        input_id = getattr(input_message.metadata, "message_id", "")
        if not input_id:
            return None
        input_seen = False
        outputs: list[Message] = []
        for event in events:
            if getattr(event.metadata, "message_id", "") == input_id:
                input_seen = True
                continue
            if (
                event.metadata.reply_to_message_id == input_id
                or event.metadata.causation_id == input_id
            ):
                outputs.append(event)
        return tuple(outputs) if input_seen else None

    @staticmethod
    def _prepare_output(input_message: Message, output: Message) -> Message:
        input_id = getattr(input_message.metadata, "message_id", "")
        normalized = normalize_recorded_message(output)
        return normalized.with_metadata(
            runtime_id=normalized.metadata.runtime_id or input_message.metadata.runtime_id,
            session_id=normalized.metadata.session_id or input_message.metadata.session_id,
            turn_id=normalized.metadata.turn_id or input_message.metadata.turn_id,
            correlation_id=(
                normalized.metadata.correlation_id
                or input_message.metadata.correlation_id
                or input_message.metadata.turn_id
                or input_id
            ),
            causation_id=normalized.metadata.causation_id or input_id or None,
            reply_to_message_id=normalized.metadata.reply_to_message_id or input_id or None,
            domain=normalized.metadata.domain or input_message.metadata.domain,
            tenant_id=normalized.metadata.tenant_id or input_message.metadata.tenant_id,
            workspace_id=normalized.metadata.workspace_id or input_message.metadata.workspace_id,
            trace=normalized.metadata.trace or input_message.metadata.trace,
        )
