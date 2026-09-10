from __future__ import annotations

from collections.abc import Callable, Sequence
from dataclasses import dataclass
from functools import reduce
from typing import TypeVar

from aiwatcher_agentic.workflow.errors import ConcurrencyConflictError, IllegalStateError
from aiwatcher_agentic.workflow.event_store import EventStore
from aiwatcher_agentic.workflow.messages import Event

C = TypeVar("C")
S = TypeVar("S")
E = TypeVar("E", bound=Event)


@dataclass(frozen=True, slots=True)
class Decider[C, S, E: Event]:
    """Core event sourcing triad: decide + evolve + initial_state.

    - decide(command, state) -> events: pure business logic
    - evolve(state, event) -> state: pure state transition
    - initial_state() -> state: factory for empty aggregate
    """

    decide: Callable[[C, S], Sequence[E]]
    evolve: Callable[[S, E], S]
    initial_state: Callable[[], S]


@dataclass(frozen=True, slots=True)
class CommandHandlerResult[S, E: Event]:
    new_events: tuple[E, ...]
    new_state: S
    next_version: int


def handle_command[C, S, E: Event](
    store: EventStore,
    stream_name: str,
    decider: Decider[C, S, E],
    command: C,
    *,
    max_retries: int = 3,
) -> CommandHandlerResult[S, E]:
    """Event sourcing command handler: load -> fold -> decide -> append.

    Retries on ConcurrencyConflictError up to max_retries times,
    reloading the stream each attempt.
    """
    for attempt in range(max_retries + 1):
        aggregate = store.aggregate_stream(
            stream_name,
            evolve=decider.evolve,
            initial_state=decider.initial_state,
        )

        new_events = decider.decide(command, aggregate.state)
        events_tuple = tuple(new_events)

        if not events_tuple:
            return CommandHandlerResult(
                new_events=(),
                new_state=aggregate.state,
                next_version=aggregate.current_version,
            )

        try:
            result = store.append_to_stream(
                stream_name,
                events_tuple,
                expected_version=aggregate.current_version,
            )
        except ConcurrencyConflictError:
            if attempt >= max_retries:
                raise
            continue

        new_state = reduce(decider.evolve, events_tuple, aggregate.state)

        return CommandHandlerResult(
            new_events=events_tuple,
            new_state=new_state,
            next_version=result.next_version,
        )

    raise AssertionError("unreachable: retry loop must return or raise")


# ---------------------------------------------------------------------------
# DeciderSpecification: BDD-style testing for event-sourced deciders
# ---------------------------------------------------------------------------


class DeciderSpecification[C, S, E: Event]:
    """BDD-style test helper: given(events).when(command).then(expected_events).

    Usage::

        spec = DeciderSpecification.for_decider(cart_decider)
        spec.given(item_added).when(ConfirmCart("c1")).then(cart_confirmed)
    """

    def __init__(self, decider: Decider[C, S, E]) -> None:
        self._decider = decider
        self._given_events: tuple[E, ...] = ()

    @classmethod
    def for_decider(cls, decider: Decider[C, S, E]) -> DeciderSpecification[C, S, E]:
        return cls(decider)

    def given(self, *events: E) -> DeciderSpecification[C, S, E]:
        self._given_events = events
        return self

    def when(self, command: C) -> WhenResult[C, S, E]:
        state = reduce(self._decider.evolve, self._given_events, self._decider.initial_state())
        return WhenResult(self._decider, state, command)


class WhenResult[C, S, E: Event]:
    """What a specification checks once its command is known.

    Its checks raise ``AssertionError`` rather than ``assert``: this ships in
    the library, and ``python -O`` compiles an ``assert`` out, which would leave
    a specification that passes because nothing was checked.
    """

    def __init__(self, decider: Decider[C, S, E], state: S, command: C) -> None:
        self._decider = decider
        self._state = state
        self._command = command

    def then(self, *expected_events: E) -> None:
        actual = tuple(self._decider.decide(self._command, self._state))
        expected = tuple(expected_events)
        if len(actual) != len(expected):
            raise AssertionError(f"Expected {len(expected)} event(s), got {len(actual)}: {actual}")
        for i, (a, e) in enumerate(zip(actual, expected, strict=True)):
            if a.type != e.type:
                raise AssertionError(f"Event[{i}].type: {a.type!r} != {e.type!r}")
            if a.data != e.data:
                raise AssertionError(f"Event[{i}].data: {a.data!r} != {e.data!r}")

    def then_nothing_happened(self) -> None:
        actual = tuple(self._decider.decide(self._command, self._state))
        if actual:
            raise AssertionError(f"Expected no events, got {len(actual)}: {actual}")

    def then_throws(
        self, error_type: type[Exception] = IllegalStateError, *, match: str = ""
    ) -> None:
        import pytest

        with pytest.raises(error_type, match=match):
            self._decider.decide(self._command, self._state)
