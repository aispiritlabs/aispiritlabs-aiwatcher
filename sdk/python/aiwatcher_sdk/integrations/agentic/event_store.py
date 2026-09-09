"""`agentic.workflow.EventStore`, backed by one aiwatcher hosted execution.

Section 40.3. `agentic.workflow` already has the worker half of a durable
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
`external` policy of section 40.4, and it is the only one this client
implements: `sealed` needs the conversation archive's crypt behind routes that
do not exist yet, and quietly writing `external` when somebody asked for
`sealed` is the silent downgrade that policy forbids.
"""

from __future__ import annotations

import dataclasses
import uuid
from collections.abc import Callable, Sequence
from dataclasses import dataclass
from typing import Any, Literal, Protocol, TypeAlias

from aiwatcher_sdk.api import ApiError, Transport

from .payloads import PayloadStore, digest_of, encode_payload

__all__ = [
    "AiwatcherEventStore",
    "AppendResult",
    "ConcurrencyConflictError",
    "MessageCodec",
    "MessageRecord",
    "ReadStreamResult",
    "dataclass_codec",
]

#: An assignment rather than PEP 695's `type` statement: `requires-python` is
#: 3.11 and CI runs the floor, the same reason a `@contextmanager` here is
#: annotated `Generator[T, None, None]`.
Expectation: TypeAlias = Literal["STREAM_EXISTS", "STREAM_DOES_NOT_EXIST", "NO_CONCURRENCY_CHECK"]
# A string, so the alias needs no runtime union of a `Literal` and stays
# readable on the 3.11 floor.
ExpectedVersion: TypeAlias = "int | Expectation"

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


def _conflict(stream_name: str, expected: object, current: object) -> Exception:
    """`agentic`'s conflict error when it is there, and this module's when it is not."""
    try:  # pragma: no cover - exercised by whichever half is installed
        from agentic.workflow.errors import (  # type: ignore[import-not-found]
            ConcurrencyConflictError as Agentic,
        )
    except ImportError:
        return ConcurrencyConflictError(stream_name, expected, current)
    theirs: Exception = Agentic(stream_name, expected, current)
    return theirs


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
    """Where the stream got to. `agentic.workflow.AppendResult`'s shape."""

    next_version: int


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

    # ── The protocol ─────────────────────────────────────────────────────

    def read_stream(
        self,
        stream_name: str,
        *,
        from_position: int = 0,
        max_count: int | None = None,
    ) -> ReadStreamResult:
        self._check_stream(stream_name)
        events: list[Any] = []
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
                    events.append(message)
            if max_count is not None and len(events) >= max_count:
                del events[max_count:]
                break
            following = page.get("next_after")
            if following is None:
                break
            after = int(following)
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
        version = self._version_for(stream_name, expected_version)
        body = {
            "expected_version": version,
            "holder": self._holder,
            "messages": [self._wire_message(record) for record in records],
        }
        try:
            answer = self._transport.json(
                "POST",
                f"/api/v1/executions/{self._execution_id}/stream",
                body,
                idempotency_key=idempotency_key or _key_for(records),
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
        return AppendResult(next_version=int(answer["version"]))

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
                "policy": "external",
            }
        return message

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


def _key_for(records: Sequence[MessageRecord]) -> str:
    """One batch's idempotency key, derived from the messages in it.

    Named by what it identifies (section 43.10, and the kickoff's first trap):
    every message id in the batch, in order. Two calls carrying the same
    messages are one append; two carrying different ones are two. A key derived
    from the execution alone would make the second batch a redelivery of the
    first.
    """
    ids = [str(record.metadata.get("message_id") or "") for record in records]
    if all(ids):
        return "agentic/" + digest_of(ids)
    # Nothing stable to derive from, so nothing is claimed: a fresh key is
    # honest about the fact that this batch cannot be recognised on a retry.
    return f"agentic/{uuid.uuid4()}"
