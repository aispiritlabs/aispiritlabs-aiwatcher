"""`agentic.workflow.EventStore`, backed by one aiwatcher hosted execution.

`agentic.workflow` already has the worker half of a durable
decider — `DurableWorkflowExecutor`, an inbox by causation, a cached decision
across OCC retries, `ProcessorLock`. What it does not have is a **shared**
history: in lab 6 every agent worker holds its own SQLite, so a fan-out of three
whose worker restarts between the second and third completion has no store its
join could live in. This is that store.

## No import from the agent's packages

The same rule the tracer in this package keeps, and here it needs help: the
protocol's argument and return types are `agentic`'s. Two seams settle it.

The **codec** turns the caller's message type into a record and back, so
`agentic.workflow.Message` is a thing this module is handed rather than one it
imports. :func:`dataclass_codec` builds one for any frozen dataclass shaped like
that message, which is what a caller inside `ai_spirit_agent` passes.

The **error** is the one place a structural match is not enough:
`DurableWorkflowExecutor` catches `ConcurrencyConflictError` by class to drive
its retry, so a conflict raised as some other type would break the loop it is
there to serve. This module raises `agentic`'s own when `agentic` is importable
and its own :class:`ConcurrencyConflictError` when it is not — which is safe, because
in that case nothing is catching the other one.

## What crosses the wire

A record's `type` and `metadata` go to aiwatcher plainly: metadata carries no
text, and it is what a review queue and an exclusion report read. The `data` is
conversation content and does **not** — it goes to a :class:`PayloadStore` and
aiwatcher is told a reference, a plaintext digest and a size. That is the
`external` payload policy, and it is the only one this client
implements: `sealed` needs the conversation archive's crypt behind routes that
do not exist yet, and quietly writing `external` when somebody asked for
`sealed` is the silent downgrade that policy forbids.

## When aiwatcher cannot be reached

Handed an :class:`~aiwatcher_sdk.outbox.Outbox`, the store splits appends in
two. A **hop** — an append that checks no version, which is how a fact is
written — is committed to the outbox first and sent after, and waits there
while aiwatcher is away rather than raising into the turn. A **decision** — an
append that names a version — is not a hop: its answer *is* the result, and a
queued claim would be telling the caller it won without asking. It goes
straight to aiwatcher, after this process's own hops, and is refused while any
of them are still waiting.

Reads follow from that. Each one drains first, so a process reads its own
writes; and when aiwatcher is unreachable, a read answers from the history this
store last saw plus the hops still waiting, rather than failing the turn. That
view can be behind — another worker's facts are missing from it — and it is
safe to be behind for exactly the reason above: every fact is monotone and
every decision is arbitrated by aiwatcher, so a stale read can make a process
wait and can never make it win something it should not have.
"""

from __future__ import annotations

import dataclasses
import datetime
import importlib
import json
import logging
import threading
import uuid
from collections.abc import Callable, Sequence
from dataclasses import dataclass
from typing import Any, Final, Literal, Protocol

from aiwatcher_sdk.api import ApiError, Transport
from aiwatcher_sdk.outbox import BATCH, Delivery, Outbox, RetryableError, Sender, drain, encode

from .payloads import PayloadStore, digest_of, encode_payload

__all__ = [
    "STREAM_APPEND",
    "AiwatcherEventStore",
    "AppendResult",
    "ConcurrencyConflictError",
    "JoinTimers",
    "MessageCodec",
    "MessageRecord",
    "ReadStreamResult",
    "SagaTimers",
    "TimerPolicy",
    "TimerRequest",
    "UndeliveredHopsError",
    "dataclass_codec",
    "deliver",
    "stream_sender",
]

logger = logging.getLogger(__name__)

#: The kind an outbox row carries when it is one batch for one execution's stream.
STREAM_APPEND: Final = "stream.append"

#: How many times a queued batch re-reads the version when another writer
#: appended between that read and its own post. Past this it waits for the next
#: drain, which is an attempt counted on a row rather than a spin.
CONFLICT_RETRIES: Final = 3

type Expectation = Literal["STREAM_EXISTS", "STREAM_DOES_NOT_EXIST", "NO_CONCURRENCY_CHECK"]
type ExpectedVersion = int | Expectation

#: What `agentic` spells the three non-numeric expectations. Matched by value
#: rather than imported, which is what makes this module free of the dependency
#: — and annotated, so that a default written with one of them still checks.
NO_CONCURRENCY_CHECK: Expectation = "NO_CONCURRENCY_CHECK"
STREAM_EXISTS: Expectation = "STREAM_EXISTS"
STREAM_DOES_NOT_EXIST: Expectation = "STREAM_DOES_NOT_EXIST"

#: The largest page `…/history` is asked for. The route caps at 500 itself; this
#: is the number of round trips a long graph's replay costs.
PAGE = 500


class ConcurrencyConflictError(RuntimeError):
    """Somebody else appended first.

    Raised only when `agentic` is not importable; when it is, this module raises
    `agentic.workflow.errors.ConcurrencyConflictError`, because that is the
    class `DurableWorkflowExecutor` catches to drive its retry.
    """

    def __init__(self, stream_name: str, expected: object, current: object) -> None:
        super().__init__(f"{stream_name}: expected version {expected}, and it is at {current}")
        self.stream_name = stream_name
        self.expected = expected
        self.current = current


class UndeliveredHopsError(ApiError):
    """A decision was asked for while this process's own hops were still waiting.

    Refused rather than reordered: a claim that reached the stream ahead of the
    completion it was taken over would be a decision made from facts nobody
    else can read yet. An :class:`ApiError` with no status, so it reads as
    "aiwatcher is not there", which is almost always why the hops are waiting —
    and like that, it can be tried again once they have drained.
    """


def _conflict(stream_name: str, expected: object, current: object) -> Exception:
    """The engine's conflict error when it is installed, and this module's when it is not.

    `aiwatcher_agentic` first, which is where the engine lives now;
    `agentic.workflow.errors` is that same module under the name it had in
    `ai_spirit_agent`, and is tried second for an agent still on the release
    before the move. Imported by name, so this module still depends on neither.
    """
    for module in ("aiwatcher_agentic.workflow.errors", "agentic.workflow.errors"):
        try:  # pragma: no cover - exercised by whichever half is installed
            errors = importlib.import_module(module)
        except ImportError:
            continue
        theirs: Exception = errors.ConcurrencyConflictError(stream_name, expected, current)
        return theirs
    return ConcurrencyConflictError(stream_name, expected, current)


@dataclass(frozen=True, slots=True)
class TimerRequest:
    """A deferred append the engine should hold until `due_at`.

    `cancel` withdraws one instead of setting it, which is what a saga does when
    the thing it was waiting for arrived first.
    """

    timer_id: str
    due_at: float | None = None
    cancel: bool = False


class TimerPolicy(Protocol):
    """Which of a worker's own messages are also timers.

    A seam rather than a rule, and the reason is where the vocabulary lives:
    `saga.timeout_scheduled` is `agentic.workflow.Saga`'s word, not this store's
    and certainly not the engine's. The worker is allowed to know what its
    messages mean — that is what makes it the decider — so the recognising
    happens here, in its own client, and aiwatcher is handed a row it was told
    about rather than a stream it worked something out from.
    """

    def timer_for(self, record: MessageRecord) -> TimerRequest | None:
        """The timer this message asks for, if it asks for one."""
        ...


class SagaTimers:
    """`agentic.workflow.Saga`'s timeouts, recognised by their own event types.

    This is the whole of what makes a saga's timers *fire* with no change to
    `agentic`. That package has had `schedule_timeout`, `due_timeouts` and
    `fire_timeout` since before any of this existed, and every one of them works
    — what has never existed is something that wakes up and looks, because a
    worker holding its own SQLite is not running when the timeout comes due.
    A `saga.timeout_scheduled` event still goes into the stream exactly as it
    did; alongside it, this asks aiwatcher to hold the fired message and hand it
    back at the time.
    """

    #: What a saga appends when it sets a timeout.
    SCHEDULED = "saga.timeout_scheduled"
    #: What it appends when one has been dealt with.
    FIRED = "saga.timeout_fired"

    def timer_for(self, record: MessageRecord) -> TimerRequest | None:
        if not isinstance(record.data, dict):
            return None
        timeout_id = record.data.get("timeout_id")
        if not isinstance(timeout_id, str):
            return None
        if record.type == self.SCHEDULED:
            due_at_ns = record.data.get("due_at_ns")
            if not isinstance(due_at_ns, int):
                return None
            # `agentic` counts nanoseconds and this API takes seconds; the
            # conversion is here rather than on the wire so that the number a
            # saga wrote is the number it reads back.
            return TimerRequest(timer_id=timeout_id, due_at=due_at_ns / 1_000_000_000)
        if record.type == self.FIRED:
            # Handled: withdraw the row, so a tick does not deliver it a second
            # time to a saga that has already moved on.
            return TimerRequest(timer_id=timeout_id, cancel=True)
        return None


class JoinTimers:
    """A graph fan-in's deadline, recognised by its own event types.

    The second policy, and it is here rather than in the graph package for the
    reason the first one is: the vocabulary is the *worker's*. A join writes
    `graph.join_deadline_scheduled` when its fan-in is reserved and
    `graph.summary_completed` when the summarizer returns, and both are
    `agentic_graph.join`'s words — restated here as strings, because this
    package imports nothing from the agent's.

    What it buys is the thing a join could not do for itself: a node that never
    completes leaves the fan-in waiting for ever, and noticing that needs
    something that wakes up and looks. The engine holds the scheduling message
    and hands it back at the time, which is the wake-up.

    The cancel is on *completion* rather than on the claim. A claim is somebody
    saying they are running the summarizer, and a worker that says that and then
    dies is precisely the case the deadline is for; only a summary that finished
    makes the deadline moot.
    """

    #: What a join appends when its fan-in is reserved.
    SCHEDULED = "graph.join_deadline_scheduled"
    #: What it appends when the summarizer returned.
    COMPLETED = "graph.summary_completed"

    def timer_for(self, record: MessageRecord) -> TimerRequest | None:
        if not isinstance(record.data, dict):
            return None
        timer_id = _join_timer_id(record.data)
        if timer_id is None:
            return None
        if record.type == self.SCHEDULED:
            due_at = record.data.get("due_at")
            if not isinstance(due_at, (int, float)) or isinstance(due_at, bool):
                return None
            return TimerRequest(timer_id=timer_id, due_at=float(due_at))
        if record.type == self.COMPLETED:
            return TimerRequest(timer_id=timer_id, cancel=True)
        return None


def _join_timer_id(data: dict[str, Any]) -> str | None:
    """The turn and the summarizer, which is what the deadline is about.

    Derived rather than generated, so the append that schedules it and the one
    that withdraws it name the same row without either having to remember an id
    the other minted.
    """
    turn_id = data.get("turn_id")
    summarizer_node_id = data.get("summarizer_node_id")
    if not isinstance(turn_id, str) or not isinstance(summarizer_node_id, str):
        return None
    return f"join:{turn_id}:{summarizer_node_id}"


@dataclass(frozen=True, slots=True)
class MessageRecord:
    """One message, split the way the wire splits it.

    `data` is the half that never reaches aiwatcher under the `external` policy.
    """

    type: str
    kind: str
    metadata: dict[str, Any]
    data: Any


@dataclass(frozen=True, slots=True)
class AppendResult:
    """Where the stream got to. `agentic.workflow.AppendResult`'s shape.

    ``delivered`` is this client's addition, defaulted so the shape still
    matches. ``False`` means the batch is in the outbox and not on the stream —
    aiwatcher could not be reached, or refused it and a person has to look —
    and ``next_version`` is then the last version this store saw, which does
    not count it.
    """

    next_version: int
    delivered: bool = True


@dataclass(frozen=True, slots=True)
class ReadStreamResult:
    """`agentic.workflow.ReadStreamResult`'s shape, over whatever the codec built."""

    events: tuple[Any, ...]
    current_version: int
    stream_exists: bool


@dataclass(frozen=True, slots=True)
class ReadAllResult:
    """`agentic.workflow.ReadAllResult`'s shape."""

    events: tuple[Any, ...]
    next_position: int
    end_position: int


@dataclass(frozen=True, slots=True)
class AggregateStreamResult:
    """`agentic.workflow.AggregateStreamResult`'s shape."""

    state: Any
    current_version: int
    stream_exists: bool


class MessageCodec(Protocol):
    """The caller's message type, in both directions.

    Two methods rather than a pair of loose functions, so a caller with a
    stateful codec — one that upcasts, or that remembers a schema — has
    somewhere to keep it.
    """

    def as_record(self, message: Any) -> MessageRecord:
        """Take one message apart into what the wire carries."""
        ...

    def build_message(self, record: MessageRecord) -> Any:
        """Put one back together, for a reader that expects the caller's type."""
        ...


def dataclass_codec(message_type: type[Any], metadata_type: type[Any]) -> MessageCodec:
    """A codec for a frozen dataclass shaped like `agentic.workflow.Message`.

    That is: `kind`, `type`, `data` and a `metadata` dataclass. Handed the two
    classes rather than importing them, which is the whole of how this module
    stays free of the agent's packages — a caller inside `ai_spirit_agent`
    writes `dataclass_codec(Message, RecordedMessageMetadata)`.

    Unknown metadata fields are dropped on the way back rather than raising: a
    stream written by a newer build of the agent must still replay under an
    older one, which is the same forwards-compatibility every reader here keeps.
    """
    return _DataclassCodec(message_type, metadata_type)


class _DataclassCodec:
    def __init__(self, message_type: type[Any], metadata_type: type[Any]) -> None:
        self._message = message_type
        self._metadata = metadata_type

    def as_record(self, message: Any) -> MessageRecord:
        return MessageRecord(
            type=getattr(message, "type", "message"),
            kind=getattr(message, "kind", "message"),
            metadata=dataclasses.asdict(message.metadata),
            data=message.data,
        )

    def build_message(self, record: MessageRecord) -> Any:
        known = {field.name for field in dataclasses.fields(self._metadata)}
        metadata = self._metadata(
            **{key: value for key, value in record.metadata.items() if key in known}
        )
        return self._message(
            kind=record.kind,
            type=record.type,
            data=record.data,
            metadata=metadata,
        )


class AiwatcherEventStore:
    """One hosted execution's history, behind `agentic.workflow.EventStore`.

    Bound to one execution, because that is aiwatcher's unit: a stream name is
    checked against the one this store was opened for rather than used to route,
    so a decider that wandered onto another graph's stream is told rather than
    silently writing there.
    """

    def __init__(
        self,
        transport: Transport,
        execution_id: str,
        *,
        payloads: PayloadStore,
        codec: MessageCodec,
        holder: str,
        stream_name: str | None = None,
        timers: TimerPolicy | None = None,
        policy: str = "external",
        outbox: Outbox | None = None,
    ) -> None:
        self._transport = transport
        self._execution_id = execution_id
        self._payloads = payloads
        self._codec = codec
        #: Which decider this is. Checked against the lease server-side, so a
        #: worker that was taken over is refused before it pays for a turn.
        self._holder = holder
        #: What `agentic` calls this stream. Defaults to the execution id, which
        #: is what a decider that has no name of its own should use.
        self._stream_name = stream_name or execution_id
        #: Which policy the payload store implements. `sealed` when `payloads`
        #: is a :class:`SealedPayloadStore`, and it has to agree with what the
        #: run was started under or every append is refused.
        self._policy = policy
        #: Which of the worker's messages are also deferred appends. `None`
        #: means none of them are — an honest default, because a store that
        #: guessed would be reading the caller's vocabulary without being told
        #: it.
        self._timers = timers
        #: Where a hop is written before it is sent. `None` keeps every append a
        #: request and every failure raised, which is right for a caller with
        #: nothing to fall back on; see the module note for what one changes.
        self._outbox = outbox
        #: What a read answers from when aiwatcher cannot be reached: every row
        #: this store has read, by stream version, the hops it delivered since
        #: the last read, and the highest version it has seen. Kept only with an
        #: outbox, because only then is there a fallback to serve.
        self._seen: dict[int, Any] = {}
        self._unread: list[Any] = []
        self._seen_version = 0
        self._offline = False
        #: One drain-and-read at a time in this process. Held across the
        #: request, which serialises this store's reads — the price of a
        #: fallback view that cannot miss a hop delivered between a read's
        #: request and its bookkeeping.
        self._sync = threading.RLock()

    # ── The protocol ─────────────────────────────────────────────────────

    def read_stream(
        self,
        stream_name: str,
        *,
        from_position: int = 0,
        max_count: int | None = None,
    ) -> ReadStreamResult:
        self._check_stream(stream_name)
        outbox = self._outbox
        if outbox is None:
            rows, version = self._remote(from_position, max_count)
            events = [message for _, message in rows]
        else:
            with self._sync:
                events, version = self._read_through(outbox, from_position, max_count)
        if max_count is not None:
            del events[max_count:]
        return ReadStreamResult(
            events=tuple(events),
            current_version=version,
            stream_exists=version > 0,
        )

    def append_to_stream(
        self,
        stream_name: str,
        events: Sequence[Any],
        *,
        expected_version: ExpectedVersion = NO_CONCURRENCY_CHECK,
        idempotency_key: str | None = None,
    ) -> AppendResult:
        """Append, and raise the caller's conflict error when somebody else won.

        `idempotency_key` is this client's own addition to the protocol, and it
        is keyword-only with a default so the protocol's signature still
        matches. Without one a retried request whose answer was lost would
        append the batch twice; the default derives one from the batch's own
        message ids, so two calls carrying the same messages are one append and
        two calls carrying different ones are two.
        """
        self._check_stream(stream_name)
        records = [self._codec.as_record(event) for event in events]
        # The payloads are written here, before the batch is queued or sent —
        # the data before the receipt that points at it.
        messages = [self._wire_message(record) for record in records]
        batch: dict[str, Any] = {"holder": self._holder, "messages": messages}
        # In the same append rather than beside it: a decision that schedules a
        # timeout and the record of having scheduled it are one decision, and
        # split in two a crash between them leaves either a timer nobody decided
        # on or a decision whose timer never happened.
        timers = [
            wire
            for record, message in zip(records, messages, strict=True)
            if (wire := self._wire_timer(record, message)) is not None
        ]
        if timers:
            batch["timers"] = timers
        key = idempotency_key or _key_for(records)
        outbox = self._outbox
        if outbox is None:
            return self._decide(stream_name, expected_version, batch, key)
        with self._sync:
            if expected_version == NO_CONCURRENCY_CHECK:
                return self._queue(outbox, key, batch)
            self._settle_own_hops(outbox)
            decided = self._decide(stream_name, expected_version, batch, key)
            self._unread.extend(self._messages_in(batch))
            return decided

    def _decide(
        self,
        stream_name: str,
        expected_version: ExpectedVersion,
        batch: dict[str, Any],
        key: str,
    ) -> AppendResult:
        """One append, straight to aiwatcher, under the version the caller named."""
        version = self._version_for(stream_name, expected_version)
        try:
            answer = self._transport.json(
                "POST",
                f"/api/v1/executions/{self._execution_id}/stream",
                {**batch, "expected_version": version},
                idempotency_key=key,
            )
        except ApiError as error:
            # A 409 means two different things and only one of them is worth
            # retrying. `version_conflict` is "somebody appended first": re-read
            # and decide again, which is the loop `DurableWorkflowExecutor`
            # runs. `lease_held` is "somebody else is deciding this run", and
            # turning that into a conflict would make the executor retry
            # exactly where it must stop — the distinction the server draws by
            # checking the lease before the version, thrown away one layer up.
            if error.status == 409 and error.code == "version_conflict":
                raise _conflict(stream_name, expected_version, error.code) from error
            raise
        reached = int(answer["version"])
        self._seen_version = max(self._seen_version, reached)
        return AppendResult(next_version=reached)

    def stream_exists(self, stream_name: str) -> bool:
        # One row, because the answer is on every page: `version` is the
        # stream's, not the page's. Reading the messages to find out whether
        # there are any would walk a day of them to answer a boolean.
        self._check_stream(stream_name)
        return self._version() > 0

    def aggregate_stream(
        self,
        stream_name: str,
        *,
        evolve: Callable[[Any, Any], Any],
        initial_state: Callable[[], Any],
        from_position: int = 0,
    ) -> AggregateStreamResult:
        """The fold `agentic`'s decider asks for, over the same read.

        Not a route: aiwatcher folds none of these, on purpose — an engine that
        read a hosted run's messages would be a second decider. The fold is the
        caller's, and it runs here.
        """
        stream = self.read_stream(stream_name, from_position=from_position)
        state = initial_state()
        for event in stream.events:
            state = evolve(state, event)
        return AggregateStreamResult(
            state=state,
            current_version=stream.current_version,
            stream_exists=stream.stream_exists,
        )

    def read_all(
        self,
        *,
        from_position: int = 0,
        max_count: int | None = None,
        stream_prefix: str | None = None,
    ) -> ReadAllResult:
        """Every message of *this* execution.

        aiwatcher's unit is an execution and there is no read across all of
        them: the thing that spans executions is the event log, which carries
        facts about work and deliberately carries no content. A `stream_prefix`
        naming anything but this store's own stream is refused rather than
        quietly answered with one execution's messages, which would read as an
        empty result for every other graph.
        """
        if stream_prefix is not None and not self._stream_name.startswith(stream_prefix):
            raise ValueError(
                f"this store holds one execution, `{self._stream_name}`, and "
                f"`{stream_prefix}` does not name it. There is no read across "
                "executions here: that is the event log, and it carries no content"
            )
        stream = self.read_stream(
            self._stream_name, from_position=from_position, max_count=max_count
        )
        return ReadAllResult(
            events=stream.events,
            next_position=from_position + len(stream.events),
            end_position=stream.current_version,
        )

    # ── Inside ───────────────────────────────────────────────────────────

    def _remote(
        self, from_position: int, max_count: int | None
    ) -> tuple[list[tuple[int, Any]], int]:
        """The caller's messages after `from_position`, with their versions."""
        rows: list[tuple[int, Any]] = []
        after = from_position
        version = 0
        while True:
            # Always a full page of *rows*. `max_count` counts the caller's
            # messages, and a hosted stream also holds this engine's own — the
            # append markers and the run's start — so sizing the request by
            # what is still wanted asks for one row at a time as soon as a
            # marker is skipped, which is a request per marker for the rest of
            # the stream.
            page = self._page(after, PAGE)
            version = int(page.get("version", 0))
            for row in page.get("messages", ()):
                message = self._message_from(row)
                if message is not None:
                    rows.append((int(row.get("stream_version", 0)), message))
            if max_count is not None and len(rows) >= max_count:
                break
            following = page.get("next_after")
            if following is None:
                break
            after = int(following)
        return rows, version

    def _read_through(
        self, outbox: Outbox, from_position: int, max_count: int | None
    ) -> tuple[list[Any], int]:
        """A read with an outbox: drain, read, and fall back to what was seen."""
        self._drain(outbox)
        try:
            rows, version = self._remote(from_position, max_count)
        except ApiError as error:
            if not error.is_retryable:
                raise
            if not self._offline:
                logger.warning(
                    "aiwatcher is unreachable (%s); execution %s reads from what this "
                    "process last saw and the hops it still has waiting",
                    error,
                    self._execution_id,
                )
                self._offline = True
            known = [message for at, message in sorted(self._seen.items()) if at > from_position]
            return [*known, *self._unread, *self._waiting(outbox)], self._seen_version
        if self._offline:
            logger.warning("aiwatcher is reachable again for execution %s", self._execution_id)
            self._offline = False
        self._seen.update(rows)
        # Whatever this store delivered is on the stream now, and in `rows`.
        self._unread.clear()
        self._seen_version = max(self._seen_version, version)
        return [*(message for _, message in rows), *self._waiting(outbox)], version

    def _queue(self, outbox: Outbox, key: str, batch: dict[str, Any]) -> AppendResult:
        """A hop: committed to the outbox, then sent if aiwatcher is there."""
        outbox.put(
            Delivery(
                message_id=key,
                execution_id=self._execution_id,
                kind=STREAM_APPEND,
                body=encode(batch),
            )
        )
        self._drain(outbox)
        if any(
            row.delivery.message_id == key
            for row in outbox.pending(None, execution_id=self._execution_id)
        ):
            return AppendResult(next_version=self._seen_version, delivered=False)
        for row in outbox.dead_letters(None):
            if row.delivery.message_id == key:
                # Not raised: the turn goes on and the hop waits for a person,
                # which is the policy — and logged, because a dead letter
                # nobody hears about is a hop lost with extra steps.
                logger.warning(
                    "aiwatcher refused hop %s on execution %s and it is dead-lettered: %s",
                    key,
                    self._execution_id,
                    row.last_error,
                )
                return AppendResult(next_version=self._seen_version, delivered=False)
        return AppendResult(next_version=self._seen_version)

    def _drain(self, outbox: Outbox) -> None:
        """Everything this execution has waiting, until done or aiwatcher is not there."""
        while True:
            report = drain(outbox, self._deliver, execution_id=self._execution_id)
            if report.deferred or report.moved < BATCH:
                return

    def _settle_own_hops(self, outbox: Outbox) -> None:
        self._drain(outbox)
        waiting = outbox.pending(None, execution_id=self._execution_id)
        if waiting:
            raise UndeliveredHopsError(
                f"{len(waiting)} hop(s) this process wrote to `{self._execution_id}` are "
                f"still in the outbox ({waiting[0].last_error or 'not sent yet'}); a "
                "decision taken before they reach the stream would be taken over a "
                "history nobody else can read. Try again once they have drained"
            )

    def _deliver(self, delivery: Delivery, /) -> None:
        """The drain's sender for this store: send, then remember what went."""
        reached = deliver(self._transport, delivery)
        self._seen_version = max(self._seen_version, reached)
        try:
            self._unread.extend(self._messages_in(json.loads(delivery.body)))
        except (OSError, KeyError, ValueError) as unreadable:
            # Sent is sent. Raising here would have the drain dead-letter a row
            # aiwatcher has already accepted; what is lost instead is only this
            # process's offline view of it, until the next read that reaches.
            logger.warning(
                "hop %s reached execution %s, and its payload could not be read back: %s",
                delivery.message_id,
                self._execution_id,
                unreadable,
            )

    def _waiting(self, outbox: Outbox) -> list[Any]:
        """This execution's hops still in the outbox, as the caller's messages."""
        return [
            message
            for row in outbox.pending(None, execution_id=self._execution_id)
            for message in self._messages_in(json.loads(row.delivery.body))
        ]

    def _messages_in(self, batch: dict[str, Any]) -> list[Any]:
        """A batch's wire messages read back, as `…/history` would hand them over."""
        return [
            message
            for wire in batch.get("messages", ())
            if (message := self._message_from({"message": {"kind": "hosted", **wire}})) is not None
        ]

    def _page(self, after: int, limit: int) -> dict[str, Any]:
        return self._transport.json(
            "GET",
            f"/api/v1/executions/{self._execution_id}/history",
            params={"after": after, "limit": limit},
        )

    def _version(self) -> int:
        """Where the stream is, in one request.

        `…/history` reports the stream's version on every page rather than the
        page's own end, which is what makes this a single row rather than a
        walk.
        """
        return int(self._page(0, 1).get("version", 0))

    def _check_stream(self, stream_name: str) -> None:
        if stream_name != self._stream_name:
            raise ValueError(
                f"this store is open on `{self._stream_name}` and was asked for "
                f"`{stream_name}`. One store, one execution — open another for "
                "another graph rather than routing by name"
            )

    def _version_for(self, stream_name: str, expected: ExpectedVersion) -> int:
        """`agentic`'s expectation as the number this API requires.

        The three literals are resolved by *reading*, which is honest about what
        they cost: aiwatcher's append always names a version, because a decider
        that did not read the stream has nothing to decide from. The executor
        that matters passes an integer it already read, so this path is for the
        callers that do not.
        """
        if isinstance(expected, int):
            return expected
        current = self._version()
        if expected == STREAM_DOES_NOT_EXIST and current != 0:
            raise _conflict(stream_name, expected, current)
        if expected == STREAM_EXISTS and current == 0:
            raise _conflict(stream_name, expected, current)
        return current

    def _wire_message(self, record: MessageRecord) -> dict[str, Any]:
        metadata = dict(record.metadata)
        # `kind` rides in the metadata rather than in a field of its own: the
        # wire's `kind` is aiwatcher's own three-way split — command, event,
        # hosted — and a second one under the same name would be read wrong by
        # whichever side looked first.
        metadata["agentic_kind"] = record.kind
        message: dict[str, Any] = {"message_type": record.type, "metadata": metadata}
        if record.data not in (None, {}, ()):
            digest = digest_of(record.data)
            message["payload"] = {
                "reference": self._payloads.store_payload(digest, record.data),
                "digest": digest,
                "size": len(encode_payload(record.data)),
                # What the *store* is, not what the caller hoped: aiwatcher
                # checks every message against the policy its run was started
                # under, and a client that named one it is not implementing
                # would be the silent downgrade the policy exists to prevent.
                "policy": self._policy,
            }
        return message

    def _wire_timer(self, record: MessageRecord, message: dict[str, Any]) -> dict[str, Any] | None:
        """The timer this message asks for, as the append route spells it."""
        if self._timers is None:
            return None
        asked = self._timers.timer_for(record)
        if asked is None:
            return None
        if asked.cancel:
            return {"cancel": {"timer_id": asked.timer_id}}
        if asked.due_at is None:  # pragma: no cover - a policy that asked for nothing
            return None
        return {
            "schedule": {
                "timer_id": asked.timer_id,
                "due_at": _rfc3339(asked.due_at),
                # The message the engine hands back when it comes due — the same
                # one being appended now, so a saga reading its own history and
                # a saga receiving the timeout see the same words.
                "message": message,
            }
        }

    def _message_from(self, row: Any) -> Any | None:
        """One `…/history` row as the caller's message, or `None` when it is not one.

        A hosted stream also holds the marker aiwatcher writes for each append,
        and the run's own `execution_requested` and `execution_started`. Those
        are this engine's, not the decider's, and handing them to a fold written
        against `agentic`'s vocabulary would be handing it somebody else's
        events.
        """
        wire = row.get("message", {})
        if wire.get("kind") != "hosted":
            return None
        message_type = wire.get("message_type", "")
        if message_type == "HostedAppend":
            return None
        metadata = dict(wire.get("metadata") or {})
        kind = metadata.pop("agentic_kind", "message")
        return self._codec.build_message(
            MessageRecord(
                type=message_type,
                kind=kind,
                metadata=metadata,
                data=self._payload_of(wire.get("payload")),
            )
        )

    def _payload_of(self, payload: Any) -> Any:
        if not payload:
            return {}
        data = self._payloads.get_payload(payload["reference"])
        # Checked rather than trusted, for the prompt registry's reason: a
        # payload store is the worker's own, and a graph replayed from somebody
        # else's words with nothing to say so is the one corruption no metric
        # catches.
        found = digest_of(data)
        if found != payload["digest"]:
            raise ValueError(
                f"`{payload['reference']}` holds {found} where the stream "
                f"recorded {payload['digest']}: this is not the payload that "
                "was appended"
            )
        return data


def deliver(transport: Transport, delivery: Delivery) -> int:
    """Send one outbox row to the stream it names, and return where the stream got to.

    The version is resolved here, when the row is sent, and not when it was
    written: a hop checks no version by definition, and the one it would have
    named is stale by the time aiwatcher is back. What makes a repeat safe is
    the key rather than the version — aiwatcher reads its inbox before anything
    else, so a batch whose acknowledgement was lost is recognised whatever
    version the retry names.

    Classified for :func:`~aiwatcher_sdk.outbox.drain`. Unreachable, a
    retryable status, or a lease somebody else holds is a
    :class:`~aiwatcher_sdk.outbox.RetryableError`: a lease runs out, and a hop is
    a fact that is still true when it does. Anything else aiwatcher refused is
    raised as it came, and the drain dead-letters the row. One request each, the
    drain's retry being the only one — two loops over one send is the same work
    retried in two places.
    """
    if delivery.kind != STREAM_APPEND:
        raise ValueError(f"`{delivery.kind}` is not a stream append, and this sends nothing else")
    batch = json.loads(delivery.body)
    route = f"/api/v1/executions/{delivery.execution_id}"
    for _ in range(CONFLICT_RETRIES):
        try:
            version = int(
                transport.json(
                    "GET", f"{route}/history", params={"after": 0, "limit": 1}, attempts=1
                ).get("version", 0)
            )
            answer = transport.json(
                "POST",
                f"{route}/stream",
                {**batch, "expected_version": version},
                idempotency_key=delivery.message_id,
                attempts=1,
            )
        except ApiError as error:
            if error.status == 409 and error.code == "version_conflict":
                continue
            if error.is_retryable or (error.status == 409 and error.code == "lease_held"):
                raise RetryableError(str(error)) from error
            raise
        return int(answer["version"])
    raise RetryableError(
        f"lost the race for `{delivery.execution_id}` {CONFLICT_RETRIES} times running; "
        "the next drain reads the version again"
    )


def stream_sender(transport: Transport) -> Sender:
    """A :class:`~aiwatcher_sdk.outbox.Sender` for rows a store is not around to send.

    What an operator's drain uses: the process that wrote the rows is gone, and
    everything a row needs — the execution, the holder, the messages and the key
    — is in the row.
    """

    def send(delivery: Delivery, /) -> None:
        deliver(transport, delivery)

    return send


def _rfc3339(seconds: float) -> str:
    """A Unix instant as the API reads times, in UTC with a `Z`."""
    return (
        datetime.datetime.fromtimestamp(seconds, tz=datetime.UTC).isoformat().replace("+00:00", "Z")
    )


def _key_for(records: Sequence[MessageRecord]) -> str:
    """One batch's idempotency key, derived from the messages in it.

    Named by everything it identifies: every message id in the batch, in
    order. Two calls carrying the same messages are one append; two carrying
    different ones are two. A key derived
    from the execution alone would make the second batch a redelivery of the
    first.
    """
    ids = [str(record.metadata.get("message_id") or "") for record in records]
    if all(ids):
        return "agentic/" + digest_of(ids)
    # Nothing stable to derive from, so nothing is claimed: a fresh key is
    # honest about the fact that this batch cannot be recognised on a retry.
    return f"agentic/{uuid.uuid4()}"
