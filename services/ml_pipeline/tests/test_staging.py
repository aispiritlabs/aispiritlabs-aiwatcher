from __future__ import annotations

from pathlib import Path

import pytest

from ml_pipeline.staging import Staging, StagingError, columns_of


def test_nothing_staged_is_a_normal_state_rather_than_a_failure(tmp_path: Path) -> None:
    staged = Staging(root=tmp_path).get_input("pii_detection")

    assert staged.rows == []
    assert staged.is_staged is False


def test_staging_replaces_what_was_there_and_keeps_column_order(tmp_path: Path) -> None:
    staging = Staging(root=tmp_path)

    staging.stage("demo", [{"b": 1, "a": 2}], params={"text_column": "a"})
    staging.stage("demo", [{"z": 1}, {"z": 2, "y": 3}])
    staged = staging.get_input("demo")

    assert [row["z"] for row in staged.rows] == [1, 2]
    assert staged.columns == ["z", "y"]
    assert staged.params == {}
    assert staged.is_staged is True


def test_a_notebook_name_that_is_not_one_never_becomes_a_path(tmp_path: Path) -> None:
    staging = Staging(root=tmp_path)

    for name in ["../escape", "sub/dir", "Upper"]:
        with pytest.raises(StagingError):
            staging.input_path(name)


def test_columns_are_first_seen_order_rather_than_sorted() -> None:
    assert columns_of([{"run_id": 1, "agent": 2}, {"agent": 3, "text": 4}]) == [
        "run_id",
        "agent",
        "text",
    ]


def test_measuring_an_empty_root_is_a_reading_rather_than_an_absence(tmp_path: Path) -> None:
    # Zero is what a fresh installation holds, and the curve from there is the
    # whole point of taking the measurement. `oldest_days` is the one field that
    # is absent instead: "nothing staged" and "everything staged today" are
    # different states, and a zero would say the second.
    totals = Staging(root=tmp_path).measure()
    assert (totals.contexts, totals.files, totals.bytes) == (0, 0, 0)
    assert totals.oldest_days is None


def test_every_context_counts_because_nothing_here_cleans_one_up(tmp_path: Path) -> None:
    # The number this exists for. A stage overwrites its *own* context and
    # leaves every other one where it is, so two runs of one notebook are two
    # directories for ever — one per attempt of every managed step that reached
    # a notebook. Counting notebooks instead would report 1 and hide the growth.
    staging = Staging(root=tmp_path)
    staging.stage("detect", [{"a": 1}], context="exec-1/detect/1")
    staging.stage("detect", [{"a": 2}], context="exec-2/detect/1")
    staging.stage("detect", [{"a": 3}], context="exec-2/detect/1")

    totals = staging.measure()
    assert totals.contexts == 2, "a second run of one notebook is a second context"
    assert totals.files == 2
    assert totals.bytes > 0
    assert totals.oldest_days == 0


def test_the_latest_pointer_is_not_counted_as_a_context(tmp_path: Path) -> None:
    # It sits beside the context directories and is the one thing here that is
    # rewritten rather than accumulated, so counting it would report growth that
    # is not happening.
    staging = Staging(root=tmp_path)
    staging.stage("detect", [{"a": 1}], context="exec-1/detect/1")
    assert staging.measure().contexts == 1
