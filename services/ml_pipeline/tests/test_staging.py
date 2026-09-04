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
