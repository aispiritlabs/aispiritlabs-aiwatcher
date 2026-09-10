"""Workflow / Saga pattern for long-running processes spanning multiple aggregates.

A Saga coordinates a multi-step process by reacting to events and emitting
commands. It follows the same decide/evolve/initial_state shape as a Decider,
but operates on cross-aggregate coordination rather than single-aggregate logic.

Each step in the saga can:
- Emit commands to other aggregates
- Emit events to record saga progress
- Fail and trigger compensating actions
"""

from __future__ import annotations

from collections.abc import Callable, Sequence
from dataclasses import dataclass
from enum import StrEnum
from functools import reduce
from typing import Any

from aiwatcher_agentic.workflow.errors import ConcurrencyConflictError
from aiwatcher_agentic.workflow.event_store import EventStore
from aiwatcher_agentic.workflow.messages import (
    Event,
    Message,
    RecordedMessageMetadata,
    normalize_recorded_message,
)


class SagaAction(StrEnum):
    INITIATED_BY = "initiated_by"
    RECEIVED = "received"
    SENT = "sent"
    PUBLISHED = "published"
    COMPENSATED = "compensated"


@dataclass(frozen=True, slots=True)
class SagaStep[O]:
    """A single output from a saga decide function."""

    action: SagaAction
    message: O


@dataclass(frozen=True, slots=True)
class Saga[I, S, O]:
    """Long-running process coordinator: decide + evolve + initial_state.

    - decide(input, state) -> steps: what commands/events to emit
    - evolve(state, input) -> state: track saga progress
    - initial_state() -> state: factory for new saga
    """

    decide: Callable[[I, S], Sequence[SagaStep[O]]]
    evolve: Callable[[S, I], S]
    initial_state: Callable[[], S]


@dataclass(frozen=True, slots=True)
class SagaResult[S, O]:
    steps: tuple[SagaStep[O], ...]
    new_state: S


def run_saga_step[I, S, O](
    saga: Saga[I, S, O],
    state: S,
    input_event: I,
) -> SagaResult[S, O]:
    """Execute a single saga step: evolve state, then decide on outputs."""
    new_state = saga.evolve(state, input_event)
    steps = tuple(saga.decide(input_event, new_state))
    return SagaResult(steps=steps, new_state=new_state)


def replay_saga[I, S, O](
    saga: Saga[I, S, O],
    events: Sequence[I],
) -> S:
    """Reconstruct saga state from event history."""
    return reduce(saga.evolve, events, saga.initial_state())


@dataclass(frozen=True, slots=True)
class DurableSagaResult[S]:
    state: S
    outputs: tuple[Message, ...]
    duplicate: bool
    next_version: int


class DurableSagaCoordinator[S]:
    """Event-sourced process manager with an input inbox and output outbox."""

    def __init__(
        self,
        *,
        name: str,
        saga: Saga[Message, S, Message],
        event_store: EventStore,
        max_conflict_retries: int = 16,
    ) -> None:
        if not name.strip():
            raise ValueError("saga name must not be empty")
        if max_conflict_retries <= 0:
            raise ValueError("max_conflict_retries must be positive")
        self._name = name
        self._saga = saga
        self._event_store = event_store
        self._max_conflict_retries = max_conflict_retries

    def handle(self, saga_id: str, input_message: Message) -> DurableSagaResult[S]:
        normalized_input = normalize_recorded_message(input_message).with_metadata(
            workflow_action=SagaAction.RECEIVED.value,
        )
        input_id = getattr(normalized_input.metadata, "message_id", "")

        for _ in range(self._max_conflict_retries):
            stream_name = self.stream_name(saga_id)
            stream = self._event_store.read_stream(stream_name)
            state, processed_inputs = self._rebuild(stream.events)
            if input_id in processed_inputs:
                outputs = self._outputs_for(stream.events, input_id)
                return DurableSagaResult(
                    state=state,
                    outputs=outputs,
                    duplicate=True,
                    next_version=stream.current_version,
                )

            result = run_saga_step(self._saga, state, normalized_input)
            outputs = tuple(
                self._prepare_output(normalized_input, step)
                for step in result.steps
                if isinstance(step.message, Message)
            )
            try:
                appended = self._event_store.append_to_stream(
                    stream_name,
                    (normalized_input, *outputs),
                    expected_version=stream.current_version,
                )
                return DurableSagaResult(
                    state=result.new_state,
                    outputs=outputs,
                    duplicate=False,
                    next_version=appended.next_version,
                )
            except ConcurrencyConflictError:
                continue
        raise ConcurrencyConflictError(self.stream_name(saga_id), "stable version", "busy")

    def state(self, saga_id: str) -> S:
        stream = self._event_store.read_stream(self.stream_name(saga_id))
        state, _ = self._rebuild(stream.events)
        return state

    def schedule_timeout(
        self,
        saga_id: str,
        *,
        timeout_id: str,
        due_at_ns: int,
        data: dict[str, Any] | None = None,
    ) -> int:
        if due_at_ns <= 0:
            raise ValueError("due_at_ns must be positive")
        stream_name = self.stream_name(saga_id)
        for _ in range(self._max_conflict_retries):
            stream = self._event_store.read_stream(stream_name)
            if any(
                event.type == "saga.timeout_scheduled"
                and isinstance(event.data, dict)
                and event.data.get("timeout_id") == timeout_id
                for event in stream.events
            ):
                return stream.current_version
            timeout = normalize_recorded_message(
                Event(
                    type="saga.timeout_scheduled",
                    data={"timeout_id": timeout_id, "due_at_ns": due_at_ns, **(data or {})},
                )
            ).with_metadata(workflow_action=SagaAction.PUBLISHED.value)
            try:
                return self._event_store.append_to_stream(
                    stream_name,
                    (timeout,),
                    expected_version=stream.current_version,
                ).next_version
            except ConcurrencyConflictError:
                continue
        raise ConcurrencyConflictError(stream_name, "stable version", "busy")

    def due_timeouts(self, saga_id: str, *, now_ns: int) -> tuple[Event, ...]:
        stream = self._event_store.read_stream(self.stream_name(saga_id))
        fired = {
            str(event.data.get("timeout_id"))
            for event in stream.events
            if event.type == "saga.timeout_fired" and isinstance(event.data, dict)
        }
        return tuple(
            event
            for event in stream.events
            if isinstance(event, Event)
            and event.type == "saga.timeout_scheduled"
            and isinstance(event.data, dict)
            and int(event.data.get("due_at_ns", 0)) <= now_ns
            and str(event.data.get("timeout_id")) not in fired
        )

    def fire_timeout(
        self,
        saga_id: str,
        *,
        timeout_id: str,
        now_ns: int,
    ) -> DurableSagaResult[S] | None:
        """Turn one due timer into an idempotent saga input.

        The stable message id makes two schedulers racing on the same timer
        converge on the same inbox entry.
        """
        scheduled = next(
            (
                timeout
                for timeout in self.due_timeouts(saga_id, now_ns=now_ns)
                if str(timeout.data.get("timeout_id")) == timeout_id
            ),
            None,
        )
        if scheduled is None:
            return None
        timeout_input = Event(
            type="saga.timeout_fired",
            data={**scheduled.data, "fired_at_ns": now_ns},
            metadata=RecordedMessageMetadata(
                message_id=f"saga-timeout:{self._name}:{saga_id}:{timeout_id}",
                idempotency_key=f"saga-timeout:{self._name}:{saga_id}:{timeout_id}",
                correlation_id=scheduled.metadata.correlation_id,
                causation_id=scheduled.metadata.message_id,
            ),
        )
        return self.handle(saga_id, timeout_input)

    def stream_name(self, saga_id: str) -> str:
        return f"saga:{self._name}:{saga_id}"

    def _rebuild(self, events: Sequence[Message]) -> tuple[S, frozenset[str]]:
        state = self._saga.initial_state()
        processed: set[str] = set()
        for event in events:
            if event.metadata.workflow_action not in {
                SagaAction.INITIATED_BY.value,
                SagaAction.RECEIVED.value,
            }:
                continue
            state = self._saga.evolve(state, event)
            message_id = getattr(event.metadata, "message_id", "")
            if message_id:
                processed.add(message_id)
        return state, frozenset(processed)

    @staticmethod
    def _outputs_for(events: Sequence[Message], input_id: str) -> tuple[Message, ...]:
        return tuple(
            event
            for event in events
            if event.metadata.causation_id == input_id
            or event.metadata.reply_to_message_id == input_id
        )

    @staticmethod
    def _prepare_output(input_message: Message, step: SagaStep[Message]) -> Message:
        input_id = getattr(input_message.metadata, "message_id", "")
        return normalize_recorded_message(step.message).with_metadata(
            runtime_id=step.message.metadata.runtime_id or input_message.metadata.runtime_id,
            session_id=step.message.metadata.session_id or input_message.metadata.session_id,
            turn_id=step.message.metadata.turn_id or input_message.metadata.turn_id,
            correlation_id=(
                step.message.metadata.correlation_id
                or input_message.metadata.correlation_id
                or input_id
            ),
            causation_id=step.message.metadata.causation_id or input_id or None,
            reply_to_message_id=step.message.metadata.reply_to_message_id or input_id or None,
            tenant_id=step.message.metadata.tenant_id or input_message.metadata.tenant_id,
            workspace_id=step.message.metadata.workspace_id or input_message.metadata.workspace_id,
            workflow_action=step.action.value,
        )
