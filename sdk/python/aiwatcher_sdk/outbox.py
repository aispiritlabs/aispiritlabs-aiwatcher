"""What an agent writes down before it tries to send, and deletes after.

This package already has two failure policies and says so. Telemetry swallows
and counts — a full queue drops events, because a telemetry library must never
take an agent down. The registry clients raise — reading the prompt a service is
about to run on *is* the work, so a failure there is the caller's to see.

A hop between two agents is neither. Dropped, it is work lost with nothing to
say so; raised, it takes down the agent whose message it was. It has to be
**written down, then sent, then forgotten** — which is the outbox the Rust side
already runs on the other end of the same wire, and this is that rule in a
second language rather than a new one.

## Four operations, and what each one is for

``put`` writes locally and commits. It is idempotent in ``message_id``: a caller
that retries its own write does not queue the hop twice.

``settle`` **deletes**. `CLAUDE.md`: *never keep an outbox row the log has
accepted*. The fact is on aiwatcher's stream, which is the durable copy and the
one every fold reads; a second copy answers no question and grows with every hop
of every conversation.

``defer`` keeps the row and counts the attempt. It is for the answer that will
be different next time — a refused connection, a 503, a timeout.

``reject`` keeps the row too, and moves it out of the retry path. It is for the
answer that will be the same next time — a 4xx about the message itself.
`CLAUDE.md`'s adapter rule, from the other side: *`Unavailable` is retried,
`Rejected` is dead-lettered. Getting this backwards either spins forever or
discards good data.*

**Nothing here deletes on failure**, at any attempt count. A retry budget that
ends in a discard is a silent loss of exactly the kind this module exists to
prevent; what a budget may do is stop retrying, which ``reject`` does while
leaving the row where a person can read it.

## What a person can do

A queue nobody can inspect is a queue nobody trusts, so the durable store is
readable and drainable from outside the process that wrote it::

    python -m aiwatcher_sdk.outbox list  .data/aiwatcher-outbox.duckdb
    python -m aiwatcher_sdk.outbox drain .data/aiwatcher-outbox.duckdb --url http://…
    python -m aiwatcher_sdk.outbox requeue .data/aiwatcher-outbox.duckdb MESSAGE_ID

``dead_letters`` is what a drain gave up on, and ``requeue`` puts one back —
which is a person's decision and never a drain's, because the drain already
decided the answer would not change. Nothing here deletes a row by hand either:
a hop somebody wants gone is a hop somebody should have to open the file for.

## Which store

The two of the Rust workflow store's four tiers that make sense on an agent's
machine. ``MemoryOutbox`` survives a failed request and not a failed process,
and says so. ``DuckdbOutbox`` survives both, and its file is one an operator can
also open with any DuckDB client and ask in SQL. It needs the ``duckdb`` extra
and imports it only when constructed: this distribution's telemetry half
depends on nothing, and a process that only publishes spans must not pay for a
database it never opens.
"""

from __future__ import annotations

import argparse
import json
import os
import random
import sys
import threading
import time
from collections.abc import Generator, Sequence
from contextlib import contextmanager
from dataclasses import asdict, dataclass, field, replace
from pathlib import Path
from types import TracebackType
from typing import TYPE_CHECKING, Any, Final, Protocol, Self

if TYPE_CHECKING:
    import duckdb

__all__ = [
    "BATCH",
    "Delivery",
    "DrainReport",
    "DuckdbOutbox",
    "MemoryOutbox",
    "Outbox",
    "PendingDelivery",
    "RetryableError",
    "Sender",
    "drain",
    "encode",
]

#: How many rows a drain takes in one pass when the caller names no bound.
BATCH: Final = 128


@dataclass(frozen=True, slots=True)
class Delivery:
    """One thing that has to reach aiwatcher, and the id that makes it once.

    ``message_id`` is aiwatcher's inbox key, so it must name everything it
    identifies — `CLAUDE.md`'s rule, and the failure it names is precise: an id
    derived from the execution and the event name alone is unique for a one-step
    plan and collides for a two-step one, after which the second step reads as a
    redelivery of the first and the run sits behind a lease nothing releases.

    ``body`` is already-encoded JSON rather than a mapping, because what is
    stored has to be the bytes that will be sent: a mapping re-encoded at send
    time can encode differently after a library upgrade, and then the digest
    aiwatcher recorded is not of the bytes anybody has.
    """

    message_id: str
    execution_id: str
    kind: str
    body: str

    def __post_init__(self) -> None:
        for name in ("message_id", "execution_id", "kind"):
            if not getattr(self, name):
                raise ValueError(f"a delivery needs a {name}")


@dataclass(frozen=True, slots=True)
class PendingDelivery:
    """A row that has not been accepted yet, and its history of trying.

    What an operator reads. ``attempts`` and ``last_error`` are here because a
    queue nobody can inspect is a queue nobody trusts, and "how long has this
    been stuck and what did it say" is the only question anybody asks of one.
    """

    delivery: Delivery
    attempts: int = 0
    first_seen: float = field(default_factory=time.time)
    last_error: str | None = None
    rejected: bool = False


@dataclass(frozen=True, slots=True)
class DrainReport:
    """What one pass did. Counts, never an opinion about what to do next."""

    settled: int = 0
    deferred: int = 0
    rejected: int = 0

    @property
    def moved(self) -> int:
        """Rows that left the retry path, either accepted or dead-lettered."""
        return self.settled + self.rejected


class Outbox(Protocol):
    """The four operations, and a way to look at what is stuck.

    A protocol rather than a base class: a caller that already has a durable
    store — `agentic.workflow`'s SQLite, a Postgres a service owns — implements
    these methods against it rather than running a second database beside the
    one it has.
    """

    def put(self, delivery: Delivery) -> bool:
        """Write and commit. ``False`` when this ``message_id`` is already held."""
        ...

    def pending(
        self, limit: int | None = BATCH, *, execution_id: str | None = None
    ) -> Sequence[PendingDelivery]:
        """The oldest rows still worth sending, rejected ones excluded.

        ``execution_id`` narrows to one execution's rows, which is what a store
        draining its own stream asks for: a row for another execution waiting
        out somebody's lease must not hold this one's hops behind it.
        ``limit=None`` is every row.
        """
        ...

    def dead_letters(self, limit: int | None = BATCH) -> Sequence[PendingDelivery]:
        """The rows taken out of the retry path, oldest first."""
        ...

    def requeue(self, message_id: str) -> bool:
        """Put a dead-lettered row back in the retry path. ``False`` if none is.

        The attempt count and the last error stay, so the record of why it was
        taken out survives the decision to try it again.
        """
        ...

    def settle(self, message_id: str) -> None:
        """aiwatcher accepted it. Delete the row."""
        ...

    def defer(self, message_id: str, reason: str) -> None:
        """The answer may differ next time. Keep the row, count the attempt."""
        ...

    def reject(self, message_id: str, reason: str) -> None:
        """The answer will not differ. Keep the row, out of the retry path."""
        ...

    def depth(self) -> tuple[int, int]:
        """How many rows are waiting, and how many are dead-lettered."""
        ...


class MemoryOutbox:
    """An outbox that survives a failed request and not a failed process.

    The default, and the honest one for a single-process run: a preview that
    runs once in one process needs nothing else, and paying for durability there
    would be paying for a restart that cannot happen.
    """

    def __init__(self) -> None:
        self._rows: dict[str, PendingDelivery] = {}
        self._lock = threading.Lock()

    def put(self, delivery: Delivery) -> bool:
        with self._lock:
            if delivery.message_id in self._rows:
                return False
            self._rows[delivery.message_id] = PendingDelivery(delivery=delivery)
            return True

    def pending(
        self, limit: int | None = BATCH, *, execution_id: str | None = None
    ) -> Sequence[PendingDelivery]:
        with self._lock:
            waiting = [
                row
                for row in self._rows.values()
                if not row.rejected
                and (execution_id is None or row.delivery.execution_id == execution_id)
            ]
        # Stable, so two rows written in one clock tick keep insertion order.
        waiting.sort(key=lambda row: row.first_seen)
        return tuple(waiting if limit is None else waiting[:limit])

    def dead_letters(self, limit: int | None = BATCH) -> Sequence[PendingDelivery]:
        with self._lock:
            held = [row for row in self._rows.values() if row.rejected]
        held.sort(key=lambda row: row.first_seen)
        return tuple(held if limit is None else held[:limit])

    def requeue(self, message_id: str) -> bool:
        with self._lock:
            row = self._rows.get(message_id)
            if row is None or not row.rejected:
                return False
            self._rows[message_id] = replace(row, rejected=False)
            return True

    def settle(self, message_id: str) -> None:
        with self._lock:
            self._rows.pop(message_id, None)

    def defer(self, message_id: str, reason: str) -> None:
        self._mark(message_id, reason, rejected=False)

    def reject(self, message_id: str, reason: str) -> None:
        self._mark(message_id, reason, rejected=True)

    def depth(self) -> tuple[int, int]:
        with self._lock:
            rejected = sum(1 for row in self._rows.values() if row.rejected)
            return len(self._rows) - rejected, rejected

    def _mark(self, message_id: str, reason: str, *, rejected: bool) -> None:
        with self._lock:
            row = self._rows.get(message_id)
            if row is None:
                return
            self._rows[message_id] = PendingDelivery(
                delivery=row.delivery,
                attempts=row.attempts + 1,
                first_seen=row.first_seen,
                last_error=reason,
                rejected=rejected,
            )


class DuckdbOutbox:
    """An outbox that survives the process, in one DuckDB file.

    **A connection per operation**, which is the design rather than a
    shortcut. DuckDB locks its file exclusively for as long as a connection is
    open — another process is refused even read-only — so a connection held for
    this object's life would make the file one process's. Two things need it to
    be everybody's: an operator listing what is stuck *while the agent that
    wrote it is still running*, and several workers on one machine sharing the
    rows. The Rust `DuckdbWorkflowStore` holds one connection and says
    `multi_process: false`, which is right for a server that owns its store; this
    file has more than one reader on purpose. Measured, an operation costs about
    7 ms opened and closed against 0.4 ms on a held connection — a hop is a
    handful of them, beside a model call measured in seconds.

    Every operation is one statement, so autocommit is the transaction, and a
    ``put`` is on disk before the request it precedes goes out. Another
    process's operation is waited out for up to :attr:`LOCK_WAIT` seconds
    rather than failed on, because it holds the file for milliseconds.
    """

    SCHEMA: Final = (
        "CREATE SEQUENCE IF NOT EXISTS outbox_written",
        # `written` is the order a drain takes rows in. Not `first_seen`: two
        # hops in one tick of the clock would tie, and a tie broken arbitrarily
        # reorders one stream's facts on their way out.
        """
        CREATE TABLE IF NOT EXISTS outbox (
            message_id   VARCHAR PRIMARY KEY,
            written      BIGINT NOT NULL DEFAULT nextval('outbox_written'),
            execution_id VARCHAR NOT NULL,
            kind         VARCHAR NOT NULL,
            body         VARCHAR NOT NULL,
            attempts     INTEGER NOT NULL DEFAULT 0,
            first_seen   DOUBLE NOT NULL,
            last_error   VARCHAR,
            rejected     BOOLEAN NOT NULL DEFAULT false
        )
        """,
    )

    #: How long an operation waits for another process's to let go of the file.
    LOCK_WAIT: Final = 10.0

    def __init__(self, path: str | Path) -> None:
        try:
            import duckdb
        except ImportError as missing:
            raise ImportError(
                "the durable outbox needs DuckDB: install `aiwatcher-sdk[duckdb]`, or use "
                "MemoryOutbox for a run that does not have to survive its process"
            ) from missing
        self._duckdb = duckdb
        self._path = Path(path)
        self._path.parent.mkdir(parents=True, exist_ok=True)
        self._lock = threading.Lock()
        with self._lock, self._connection() as db:
            for statement in self.SCHEMA:
                db.execute(statement)

    def __enter__(self) -> Self:
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        self.close()

    def close(self) -> None:
        """Nothing to release: no connection outlives the operation that opened it."""

    def put(self, delivery: Delivery) -> bool:
        with self._lock, self._connection() as db:
            inserted = db.execute(
                "INSERT INTO outbox (message_id, execution_id, kind, body, first_seen) "
                "VALUES (?, ?, ?, ?, ?) ON CONFLICT (message_id) DO NOTHING",
                (
                    delivery.message_id,
                    delivery.execution_id,
                    delivery.kind,
                    delivery.body,
                    time.time(),
                ),
            ).fetchone()
        return bool(inserted and inserted[0])

    def pending(
        self, limit: int | None = BATCH, *, execution_id: str | None = None
    ) -> Sequence[PendingDelivery]:
        return self._select(rejected=False, limit=limit, execution_id=execution_id)

    def dead_letters(self, limit: int | None = BATCH) -> Sequence[PendingDelivery]:
        return self._select(rejected=True, limit=limit, execution_id=None)

    def requeue(self, message_id: str) -> bool:
        with self._lock, self._connection() as db:
            changed = db.execute(
                "UPDATE outbox SET rejected = false WHERE message_id = ? AND rejected",
                (message_id,),
            ).fetchone()
        return bool(changed and changed[0] == 1)

    def settle(self, message_id: str) -> None:
        with self._lock, self._connection() as db:
            db.execute("DELETE FROM outbox WHERE message_id = ?", (message_id,))

    def defer(self, message_id: str, reason: str) -> None:
        self._mark(message_id, reason, rejected=False)

    def reject(self, message_id: str, reason: str) -> None:
        self._mark(message_id, reason, rejected=True)

    def depth(self) -> tuple[int, int]:
        with self._lock, self._connection() as db:
            counted = db.execute(
                "SELECT count(*) FILTER (WHERE NOT rejected), count(*) FILTER (WHERE rejected) "
                "FROM outbox"
            ).fetchone()
        waiting, rejected = counted if counted is not None else (0, 0)
        return int(waiting), int(rejected)

    @contextmanager
    def _connection(self) -> Generator[duckdb.DuckDBPyConnection, None, None]:
        deadline = time.monotonic() + self.LOCK_WAIT
        pause = 0.005
        while True:
            try:
                db = self._duckdb.connect(str(self._path))
                break
            except self._duckdb.IOException as failure:
                # Only the lock is waited for. A file that is not a database, or
                # a directory that is not writable, says the same thing in ten
                # seconds as it does now.
                if "lock" not in str(failure).lower() or time.monotonic() >= deadline:
                    raise
                # Jittered, so two workers that collided do not collide again.
                time.sleep(pause * (0.5 + random.random()))  # noqa: S311 - a backoff, not a secret
                pause = min(pause * 2, 0.1)
        try:
            yield db
        finally:
            db.close()

    def _mark(self, message_id: str, reason: str, *, rejected: bool) -> None:
        with self._lock, self._connection() as db:
            db.execute(
                "UPDATE outbox SET attempts = attempts + 1, last_error = ?, rejected = ? "
                "WHERE message_id = ?",
                (reason, rejected, message_id),
            )

    def _select(
        self, *, rejected: bool, limit: int | None, execution_id: str | None
    ) -> Sequence[PendingDelivery]:
        # One fixed statement: a NULL execution matches every row, and the limit
        # is applied by fetching rather than by writing it into the SQL — an
        # outbox is small, and a query assembled from strings is the thing a
        # fixed one is here to avoid.
        with self._lock, self._connection() as db:
            cursor = db.execute(
                "SELECT message_id, execution_id, kind, body, attempts, first_seen, last_error "
                "FROM outbox WHERE rejected = ? "
                "AND (CAST(? AS VARCHAR) IS NULL OR execution_id = ?) ORDER BY written",
                (rejected, execution_id, execution_id),
            )
            rows = cursor.fetchall() if limit is None else cursor.fetchmany(limit)
        return tuple(
            PendingDelivery(
                delivery=Delivery(message_id=row[0], execution_id=row[1], kind=row[2], body=row[3]),
                attempts=row[4],
                first_seen=row[5],
                last_error=row[6],
                rejected=rejected,
            )
            for row in rows
        )


class Sender(Protocol):
    """What a drain calls, and the three things it may answer.

    ``None`` is acceptance. A :class:`RetryableError` is the answer that may differ
    next time; anything else raised is the answer that will not. That split is
    the caller's to make, because only the thing holding the socket knows
    whether a 409 was a redelivery it should treat as accepted or a conflict it
    should not.
    """

    #: Positional-only, so any one-argument callable satisfies this whatever it
    #: called its parameter. A named parameter in a callback protocol makes
    #: every plain function a type error at the call site and teaches people to
    #: silence it, which is the opposite of what the protocol is for.
    def __call__(self, delivery: Delivery, /) -> None: ...


class RetryableError(Exception):
    """The send failed in a way that may not fail next time.

    Raised by a sender for a refused connection, a 503, a timeout — anything
    where nothing about the message itself was wrong.
    """


def drain(
    outbox: Outbox,
    send: Sender,
    *,
    limit: int = BATCH,
    execution_id: str | None = None,
) -> DrainReport:
    """Try the waiting rows once, and report what moved.

    One pass, never a loop: how often to drain and whether to back off are the
    caller's, and a function that decided them here would be a scheduler
    pretending to be a queue. It stops at the first :class:`RetryableError` — if
    aiwatcher is unreachable it is unreachable for the next row too, and the
    rows are ordered, so pressing on would spend the batch proving one fact.

    ``execution_id`` drains one execution's rows only. The order that matters is
    within one stream, and one stream's row waiting out a lease must not stop
    another's from going.
    """
    settled = deferred = rejected = 0
    for row in outbox.pending(limit, execution_id=execution_id):
        try:
            send(row.delivery)
        except RetryableError as failure:
            outbox.defer(row.delivery.message_id, str(failure) or failure.__class__.__name__)
            deferred += 1
            break
        except Exception as failure:  # noqa: BLE001 — the sender classifies; this records
            outbox.reject(row.delivery.message_id, f"{failure.__class__.__name__}: {failure}")
            rejected += 1
        else:
            outbox.settle(row.delivery.message_id)
            settled += 1
    return DrainReport(settled=settled, deferred=deferred, rejected=rejected)


def encode(payload: Any) -> str:
    """The one encoding a delivery's body is written with.

    ``sort_keys`` so that two processes encoding one message produce one string,
    which is what lets a digest of the body mean anything.
    """
    return json.dumps(payload, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


# ── The operator's door ──────────────────────────────────────────────────────


def main(argv: Sequence[str] | None = None) -> int:
    """``python -m aiwatcher_sdk.outbox``: list an outbox, drain it, requeue a row.

    Only the durable store can be opened from outside the process that wrote
    it, which is the case this exists for: the agent is gone, or stuck, and a
    person wants to know what it did not manage to say.
    """
    parser = argparse.ArgumentParser(
        prog="python -m aiwatcher_sdk.outbox",
        description="Look at, drain or requeue an agent's local outbox.",
    )
    commands = parser.add_subparsers(dest="command", required=True)
    listing = commands.add_parser("list", help="every row, waiting and dead-lettered")
    listing.add_argument("path", type=Path, help="the outbox's DuckDB file")
    listing.add_argument("--json", action="store_true", help="one JSON document, for a script")
    draining = commands.add_parser("drain", help="one pass over the waiting rows")
    draining.add_argument("path", type=Path)
    draining.add_argument("--url", default=os.environ.get("AIWATCHER_URL"))
    draining.add_argument("--execution", default=None, help="only this execution's rows")
    requeueing = commands.add_parser("requeue", help="put a dead-lettered row back")
    requeueing.add_argument("path", type=Path)
    requeueing.add_argument("message_id")
    args = parser.parse_args(argv)

    if not args.path.exists():
        # `DuckdbOutbox` creates what it is pointed at, which is right for an
        # agent and wrong for somebody who mistyped the path to one.
        print(f"no outbox at {args.path}", file=sys.stderr)
        return 2
    try:
        opened = DuckdbOutbox(args.path)
    except ImportError as missing:
        print(missing, file=sys.stderr)
        return 2
    with opened as outbox:
        if args.command == "list":
            return _list(outbox, as_json=args.json)
        if args.command == "requeue":
            if outbox.requeue(args.message_id):
                print(f"requeued {args.message_id}")
                return 0
            print(f"{args.message_id} is not dead-lettered here", file=sys.stderr)
            return 1
        if not args.url:
            print("drain needs --url or AIWATCHER_URL", file=sys.stderr)
            return 2
        # Imported here rather than at the top: this module is the telemetry
        # half's and depends on nothing, and only this one command needs a
        # client — the same reason `as_torch_dataloader` imports torch inside.
        from aiwatcher_sdk.api import Transport
        from aiwatcher_sdk.integrations.agentic.event_store import stream_sender

        with Transport(args.url, subject="aiwatcher") as transport:
            report = drain(outbox, stream_sender(transport), execution_id=args.execution)
        waiting, rejected = outbox.depth()
        print(
            f"settled={report.settled} deferred={report.deferred} rejected={report.rejected} "
            f"waiting={waiting} dead_lettered={rejected}"
        )
        return 0


def _list(outbox: DuckdbOutbox, *, as_json: bool) -> int:
    rows = [*outbox.pending(None), *outbox.dead_letters(None)]
    if as_json:
        print(
            json.dumps(
                [
                    {
                        **asdict(row.delivery),
                        "attempts": row.attempts,
                        "first_seen": row.first_seen,
                        "last_error": row.last_error,
                        "rejected": row.rejected,
                    }
                    for row in rows
                ],
                indent=2,
            )
        )
        return 0
    now = time.time()
    for row in rows:
        print(
            "\t".join(
                (
                    "dead" if row.rejected else "waiting",
                    row.delivery.execution_id,
                    row.delivery.message_id,
                    row.delivery.kind,
                    f"attempts={row.attempts}",
                    f"age={now - row.first_seen:.0f}s",
                    row.last_error or "",
                )
            )
        )
    waiting, rejected = outbox.depth()
    print(f"{waiting} waiting, {rejected} dead-lettered")
    return 0


if __name__ == "__main__":
    # Run as `python -m`, this file is `__main__` and `aiwatcher_sdk.outbox` is a
    # second copy of it. The sender raises that copy's `RetryableError`, which
    # this copy's `drain` would not recognise — and would dead-letter a hop that
    # only had to wait. So the importable module runs, not this one.
    from aiwatcher_sdk.outbox import main as entry

    sys.exit(entry())
