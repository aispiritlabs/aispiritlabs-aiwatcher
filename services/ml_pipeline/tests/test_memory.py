"""The three answers, and why each one is what it is.

Section 15.4 at the second runtime. A reactor asks after a timeout it did not
expect, and what it does next is decided entirely by which of these it gets:
`running` means wait, `done` means go and read the receipt, `absent` means the
work is safe to do again.
"""

from __future__ import annotations

from ml_pipeline.memory import ABSENT, DONE, RUNNING, ExecutionMemory

KEY = "01a0/detect/1"


def test_a_key_nobody_ran_here_is_absent_rather_than_an_error() -> None:
    # The ordinary answer. Another replica ran it, this process restarted, or
    # the note expired — none of those is a failure, and all of them mean the
    # same thing to a reactor.
    assert ExecutionMemory().seen(KEY) == {"state": ABSENT}


def test_a_notebook_that_is_still_going_says_so_rather_than_looking_absent() -> None:
    # The whole reason this exists: without it, a reactor whose timeout fired
    # runs the step again beside the one that is still executing.
    memory = ExecutionMemory()
    memory.started(KEY)

    assert memory.seen(KEY)["state"] == RUNNING


def test_a_finished_run_names_the_revision_it_ran_and_not_the_rows() -> None:
    # The revision, because a receipt records it too and the two disagreeing
    # means the stored rows came from a different notebook. The rows go to the
    # artifact the reactor uploads; what stays here is a note.
    memory = ExecutionMemory()
    memory.started(KEY)
    memory.finished(KEY, "ab" * 32, 50)

    assert memory.seen(KEY) == {"state": DONE, "revision": "ab" * 32, "rows": 50}


def test_a_run_that_ended_badly_is_forgotten_rather_than_left_running() -> None:
    # A `running` note nothing will ever complete is a reactor waiting until its
    # lease expires. `absent` is honest: nothing here ran to completion.
    memory = ExecutionMemory()
    memory.started(KEY)
    memory.failed(KEY)

    assert memory.seen(KEY) == {"state": ABSENT}


def test_a_note_that_outlived_its_window_stops_answering_for_the_next_attempt() -> None:
    # A note read long after the attempt it describes would answer for whatever
    # asks next. Nothing asks next at *this* key — the attempt number is in it —
    # but the window is what makes that true rather than assumed.
    memory = ExecutionMemory(ttl_seconds=0)
    memory.started(KEY)

    assert memory.seen(KEY) == {"state": ABSENT}


def test_notes_nobody_reads_do_not_accumulate_for_the_life_of_the_process() -> None:
    # The one way an in-process store is worse than the file the query service
    # writes: nothing sweeps a dict but this.
    memory = ExecutionMemory(ttl_seconds=0)
    for index in range(5):
        memory.started(f"01a0/detect/{index}")
    memory.started("fresh")

    assert list(memory._notes) == ["fresh"]
