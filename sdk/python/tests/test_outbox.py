"""The outbox, and the four things it is allowed to do with a row.

Both stores are put through the same contract, because a memory adapter that
disagrees with the durable one about when a row disappears is an adapter that
passes its tests and loses a hop in production. `SqliteOutbox` then has three of
its own, for the thing memory cannot be asked about: surviving the process.
"""

from __future__ import annotations

import json
import sqlite3
from collections.abc import Iterator
from pathlib import Path

import pytest

from aiwatcher_sdk.outbox import (
    Delivery,
    MemoryOutbox,
    Outbox,
    RetryableError,
    SqliteOutbox,
    drain,
    encode,
)


def hop(index: int, execution: str = "exec-1") -> Delivery:
    return Delivery(
        message_id=f"{execution}/handoff/{index}",
        execution_id=execution,
        kind="stream.append",
        body=encode({"to": "summarizer", "index": index}),
    )


@pytest.fixture(params=["memory", "sqlite"])
def outbox(request: pytest.FixtureRequest, tmp_path: Path) -> Iterator[Outbox]:
    if request.param == "memory":
        yield MemoryOutbox()
        return
    store = SqliteOutbox(tmp_path / "outbox.db")
    try:
        yield store
    finally:
        store.close()


# ── The contract both stores keep ────────────────────────────────────────────


def test_a_settled_row_is_gone_rather_than_marked(outbox: Outbox) -> None:
    outbox.put(hop(1))
    outbox.settle(hop(1).message_id)

    assert outbox.pending() == ()
    assert outbox.depth() == (0, 0)


def test_writing_one_hop_twice_queues_it_once(outbox: Outbox) -> None:
    assert outbox.put(hop(1)) is True
    assert outbox.put(hop(1)) is False

    assert len(outbox.pending()) == 1


def test_a_deferred_row_keeps_its_place_and_counts_the_attempt(outbox: Outbox) -> None:
    outbox.put(hop(1))
    outbox.defer(hop(1).message_id, "connection refused")

    waiting = outbox.pending()
    assert len(waiting) == 1
    assert waiting[0].attempts == 1
    assert waiting[0].last_error == "connection refused"


def test_a_rejected_row_leaves_the_retry_path_without_being_deleted(outbox: Outbox) -> None:
    outbox.put(hop(1))
    outbox.reject(hop(1).message_id, "422: unknown execution")

    assert outbox.pending() == ()
    assert outbox.depth() == (0, 1)


def test_rows_are_offered_in_the_order_they_were_written(outbox: Outbox) -> None:
    for index in range(5):
        outbox.put(hop(index))

    assert [row.delivery.message_id for row in outbox.pending()] == [
        hop(index).message_id for index in range(5)
    ]


def test_a_drain_settles_what_the_sender_accepted(outbox: Outbox) -> None:
    for index in range(3):
        outbox.put(hop(index))
    sent: list[Delivery] = []

    report = drain(outbox, sent.append)

    assert report.settled == 3
    assert len(sent) == 3
    assert outbox.depth() == (0, 0)


def test_a_drain_stops_at_the_first_retryable_failure(outbox: Outbox) -> None:
    for index in range(5):
        outbox.put(hop(index))

    def unreachable(delivery: Delivery) -> None:
        raise RetryableError("connection refused")

    report = drain(outbox, unreachable)

    # One attempt proves aiwatcher is down; the other four would prove it again.
    assert report.deferred == 1
    assert outbox.depth() == (5, 0)
    assert outbox.pending()[0].attempts == 1


def test_a_drain_rejects_a_permanent_failure_and_keeps_going(outbox: Outbox) -> None:
    for index in range(3):
        outbox.put(hop(index))
    refused = hop(1).message_id

    def picky(delivery: Delivery) -> None:
        if delivery.message_id == refused:
            raise ValueError("no such execution")

    report = drain(outbox, picky)

    assert (report.settled, report.rejected, report.deferred) == (2, 1, 0)
    assert outbox.depth() == (0, 1)


def test_no_attempt_count_ever_discards_a_hop(outbox: Outbox) -> None:
    outbox.put(hop(1))
    for _ in range(200):
        outbox.defer(hop(1).message_id, "503")

    waiting = outbox.pending()
    assert len(waiting) == 1
    assert waiting[0].attempts == 200


def test_settling_something_that_is_not_there_is_not_an_error(outbox: Outbox) -> None:
    outbox.settle("never-written")
    outbox.defer("never-written", "503")
    outbox.reject("never-written", "422")

    assert outbox.depth() == (0, 0)


def test_a_delivery_without_the_three_things_that_identify_it_is_refused() -> None:
    for missing in ("message_id", "execution_id", "kind"):
        fields = {
            "message_id": "m",
            "execution_id": "e",
            "kind": "k",
            "body": "{}",
            missing: "",
        }
        with pytest.raises(ValueError, match=missing):
            Delivery(**fields)


# ── What only the durable store can be asked ─────────────────────────────────


def test_a_hop_written_before_the_process_died_is_there_afterwards(tmp_path: Path) -> None:
    path = tmp_path / "outbox.db"
    with SqliteOutbox(path) as before:
        before.put(hop(1))

    with SqliteOutbox(path) as after:
        assert [row.delivery.message_id for row in after.pending()] == [hop(1).message_id]


def test_a_send_whose_acknowledgement_was_lost_is_re_sent_under_the_same_id(
    tmp_path: Path,
) -> None:
    path = tmp_path / "outbox.db"
    with SqliteOutbox(path) as before:
        before.put(hop(1))
        # Sent, and the process dies before `settle`. Nothing distinguishes this
        # from a send that never arrived, which is why the id has to carry it.
        drain(before, lambda delivery: (_ for _ in ()).throw(RetryableError("timeout")))

    resent: list[str] = []
    with SqliteOutbox(path) as after:
        drain(after, lambda delivery: resent.append(delivery.message_id))

    assert resent == [hop(1).message_id]


def test_the_row_is_committed_before_a_send_could_have_happened(tmp_path: Path) -> None:
    # The whole ordering: a second connection sees the row with no cooperation
    # from the one that wrote it, so the write is durable before the request
    # that follows it goes out.
    path = tmp_path / "outbox.db"
    with SqliteOutbox(path) as store:
        store.put(hop(1))
        observer = sqlite3.connect(path)
        try:
            held = observer.execute("SELECT COUNT(*) FROM outbox").fetchone()[0]
        finally:
            observer.close()

    assert held == 1


def test_the_body_is_stored_as_the_bytes_that_will_be_sent(tmp_path: Path) -> None:
    # Not a mapping re-encoded at send time: a library upgrade that orders keys
    # differently would then make the digest aiwatcher recorded a digest of
    # bytes nobody has.
    with SqliteOutbox(tmp_path / "outbox.db") as store:
        store.put(hop(7))
        stored = store.pending()[0].delivery.body

    assert stored == hop(7).body
    assert json.loads(stored) == {"to": "summarizer", "index": 7}


def test_two_processes_encoding_one_message_produce_one_string() -> None:
    assert encode({"b": 1, "a": 2}) == encode({"a": 2, "b": 1})
