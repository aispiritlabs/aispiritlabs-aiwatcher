"""The outbox, and the four things it is allowed to do with a row.

Both stores are put through the same contract, because a memory adapter that
disagrees with the durable one about when a row disappears is an adapter that
passes its tests and loses a hop in production. `DuckdbOutbox` then has tests of
its own, for what memory cannot be asked about: surviving the process, and
sharing its file with another one.
"""

from __future__ import annotations

import builtins
import json
import subprocess
import sys
import time
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest

from aiwatcher_sdk.outbox import (
    BATCH,
    Delivery,
    DuckdbOutbox,
    MemoryOutbox,
    Outbox,
    RetryableError,
    drain,
    encode,
    main,
)


def hop(index: int, execution: str = "exec-1") -> Delivery:
    return Delivery(
        message_id=f"{execution}/handoff/{index}",
        execution_id=execution,
        kind="stream.append",
        body=encode({"to": "summarizer", "index": index}),
    )


@pytest.fixture(params=["memory", "duckdb"])
def outbox(request: pytest.FixtureRequest, tmp_path: Path) -> Iterator[Outbox]:
    if request.param == "memory":
        yield MemoryOutbox()
        return
    store = DuckdbOutbox(tmp_path / "outbox.duckdb")
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


def test_one_execution_s_rows_can_be_asked_for_without_the_others(outbox: Outbox) -> None:
    outbox.put(hop(1, "exec-1"))
    outbox.put(hop(1, "exec-2"))
    outbox.put(hop(2, "exec-1"))

    assert [row.delivery.message_id for row in outbox.pending(execution_id="exec-1")] == [
        hop(1, "exec-1").message_id,
        hop(2, "exec-1").message_id,
    ]


def test_a_drain_for_one_execution_is_not_held_behind_another_s(outbox: Outbox) -> None:
    # exec-2's row is older and waiting out somebody's lease. Drained together,
    # it stops the pass before exec-1's hop is tried; drained by execution, the
    # stream that can move does.
    outbox.put(hop(1, "exec-2"))
    outbox.put(hop(1, "exec-1"))

    def lease_held_on_exec_2(delivery: Delivery) -> None:
        if delivery.execution_id == "exec-2":
            raise RetryableError("409 lease_held")

    assert drain(outbox, lease_held_on_exec_2).settled == 0
    assert drain(outbox, lease_held_on_exec_2, execution_id="exec-1").settled == 1
    assert [row.delivery.execution_id for row in outbox.pending()] == ["exec-2"]


def test_no_limit_is_every_row_rather_than_a_batch(outbox: Outbox) -> None:
    for index in range(BATCH + 5):
        outbox.put(hop(index))

    assert len(outbox.pending()) == BATCH
    assert len(outbox.pending(None)) == BATCH + 5


def test_rows_written_in_one_clock_tick_still_drain_in_the_order_written(
    outbox: Outbox, monkeypatch: pytest.MonkeyPatch
) -> None:
    # A burst of hops inside one tick of the clock is the ordinary case, and a
    # tie broken arbitrarily would reorder one stream's facts on the way out.
    monkeypatch.setattr(time, "time", lambda: 1_000.0)
    for index in range(20):
        outbox.put(hop(index))

    assert [row.delivery.message_id for row in outbox.pending()] == [
        hop(index).message_id for index in range(20)
    ]


def test_what_a_drain_gave_up_on_is_listed_for_a_person(outbox: Outbox) -> None:
    outbox.put(hop(1))
    outbox.put(hop(2))
    outbox.reject(hop(2).message_id, "422: unknown execution")

    [dead] = outbox.dead_letters()

    assert dead.delivery.message_id == hop(2).message_id
    assert (dead.rejected, dead.attempts, dead.last_error) == (True, 1, "422: unknown execution")


def test_a_requeued_row_is_back_in_the_retry_path_with_its_history(outbox: Outbox) -> None:
    outbox.put(hop(1))
    outbox.reject(hop(1).message_id, "422: unknown execution")

    assert outbox.requeue(hop(1).message_id) is True

    [row] = outbox.pending()
    # Why it was taken out survives the decision to try it again.
    assert (row.attempts, row.last_error, row.rejected) == (1, "422: unknown execution", False)
    assert outbox.dead_letters() == ()


def test_only_a_dead_letter_can_be_requeued(outbox: Outbox) -> None:
    outbox.put(hop(1))

    assert outbox.requeue(hop(1).message_id) is False
    assert outbox.requeue("never-written") is False
    assert outbox.depth() == (1, 0)


# ── What only the durable store can be asked ─────────────────────────────────


def test_a_hop_written_before_the_process_died_is_there_afterwards(tmp_path: Path) -> None:
    path = tmp_path / "outbox.duckdb"
    with DuckdbOutbox(path) as before:
        before.put(hop(1))

    with DuckdbOutbox(path) as after:
        assert [row.delivery.message_id for row in after.pending()] == [hop(1).message_id]


def test_a_send_whose_acknowledgement_was_lost_is_re_sent_under_the_same_id(
    tmp_path: Path,
) -> None:
    path = tmp_path / "outbox.duckdb"
    with DuckdbOutbox(path) as before:
        before.put(hop(1))
        # Sent, and the process dies before `settle`. Nothing distinguishes this
        # from a send that never arrived, which is why the id has to carry it.
        drain(before, lambda delivery: (_ for _ in ()).throw(RetryableError("timeout")))

    resent: list[str] = []
    with DuckdbOutbox(path) as after:
        drain(after, lambda delivery: resent.append(delivery.message_id))

    assert resent == [hop(1).message_id]


#: A second process — an operator, another worker — counting the rows.
COUNT_ROWS = (
    "import duckdb, sys\n"
    "db = duckdb.connect(sys.argv[1])\n"
    "print(db.execute('SELECT count(*) FROM outbox').fetchone()[0])\n"
)

#: A second process holding the file for half a second.
HOLD_THE_FILE = (
    "import duckdb, sys, time\n"
    "db = duckdb.connect(sys.argv[1])\n"
    "print('held', flush=True)\n"
    "time.sleep(0.5)\n"
    "db.close()\n"
)


def count_from_another_process(path: Path) -> int:
    # This interpreter, and a program this file wrote.
    read = subprocess.run(  # noqa: S603
        [sys.executable, "-c", COUNT_ROWS, str(path)],
        capture_output=True,
        text=True,
        check=True,
    )
    return int(read.stdout)


def test_another_process_reads_a_row_while_the_outbox_that_wrote_it_is_open(
    tmp_path: Path,
) -> None:
    # The whole ordering, and the reason for a connection per operation: the
    # row is on disk before the request that follows it goes out, and the file
    # is not held — somebody can list it while the agent that wrote it runs.
    path = tmp_path / "outbox.duckdb"
    with DuckdbOutbox(path) as store:
        store.put(hop(1))

        assert count_from_another_process(path) == 1


def test_an_operation_waits_out_another_process_s_rather_than_failing(tmp_path: Path) -> None:
    path = tmp_path / "outbox.duckdb"
    store = DuckdbOutbox(path)
    # This interpreter, and a program this file wrote.
    holder = subprocess.Popen(  # noqa: S603
        [sys.executable, "-c", HOLD_THE_FILE, str(path)],
        stdout=subprocess.PIPE,
        text=True,
    )
    try:
        assert holder.stdout is not None
        assert holder.stdout.readline().strip() == "held"

        assert store.put(hop(1)) is True
    finally:
        holder.wait(timeout=30)

    assert store.depth() == (1, 0)


def test_without_the_extra_the_durable_outbox_says_which_one(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    real_import = builtins.__import__

    def refuse(name: str, *args: Any, **kwargs: Any) -> Any:
        if name == "duckdb":
            raise ImportError(name)
        return real_import(name, *args, **kwargs)

    monkeypatch.setattr(builtins, "__import__", refuse)

    with pytest.raises(ImportError, match=r"aiwatcher-sdk\[duckdb\]"):
        DuckdbOutbox(tmp_path / "outbox.duckdb")


def test_the_body_is_stored_as_the_bytes_that_will_be_sent(tmp_path: Path) -> None:
    # Not a mapping re-encoded at send time: a library upgrade that orders keys
    # differently would then make the digest aiwatcher recorded a digest of
    # bytes nobody has.
    with DuckdbOutbox(tmp_path / "outbox.duckdb") as store:
        store.put(hop(7))
        stored = store.pending()[0].delivery.body

    assert stored == hop(7).body
    assert json.loads(stored) == {"to": "summarizer", "index": 7}


def test_two_processes_encoding_one_message_produce_one_string() -> None:
    assert encode({"b": 1, "a": 2}) == encode({"a": 2, "b": 1})


# ── The operator's door ──────────────────────────────────────────────────────


def stuck(path: Path) -> None:
    """One hop waiting after a refused connection, one dead-lettered."""
    with DuckdbOutbox(path) as store:
        store.put(hop(1))
        store.defer(hop(1).message_id, "connection refused")
        store.put(hop(2))
        store.reject(hop(2).message_id, "422: unknown execution")


def test_an_operator_sees_every_row_with_its_execution_message_and_attempts(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    path = tmp_path / "outbox.duckdb"
    stuck(path)

    assert main(["list", str(path), "--json"]) == 0

    rows = json.loads(capsys.readouterr().out)
    assert [
        (row["execution_id"], row["message_id"], row["attempts"], row["rejected"]) for row in rows
    ] == [
        ("exec-1", hop(1).message_id, 1, False),
        ("exec-1", hop(2).message_id, 1, True),
    ]


def test_the_plain_listing_names_the_same_things_and_what_they_last_said(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    path = tmp_path / "outbox.duckdb"
    stuck(path)

    assert main(["list", str(path)]) == 0

    out = capsys.readouterr().out
    assert f"waiting\texec-1\t{hop(1).message_id}\tstream.append\tattempts=1" in out
    assert "connection refused" in out
    assert f"dead\texec-1\t{hop(2).message_id}" in out
    assert out.rstrip().endswith("1 waiting, 1 dead-lettered")


def test_an_operator_requeues_a_dead_letter_by_its_id(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    path = tmp_path / "outbox.duckdb"
    stuck(path)

    assert main(["requeue", str(path), hop(2).message_id]) == 0
    assert main(["requeue", str(path), hop(2).message_id]) == 1, "it is no longer dead"

    with DuckdbOutbox(path) as store:
        assert store.depth() == (2, 0)


def test_a_mistyped_path_is_not_quietly_made_into_an_empty_outbox(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    missing = tmp_path / "outbox-typo.db"

    assert main(["list", str(missing)]) == 2

    assert not missing.exists()
    assert "no outbox at" in capsys.readouterr().err
