from __future__ import annotations

from pathlib import Path

import pytest

from ml_pipeline import Block
from ml_pipeline.block import DATA_VARIABLE
from ml_pipeline.staging import Staging


def test_the_live_notebook_reads_the_rows_the_panel_staged(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The whole point of one staging file: open the block's editor and the
    widgets are being moved against the rows the block will run on."""
    Staging(root=tmp_path).stage("demo", [{"text": "hello"}], params={"text_column": "text"})
    monkeypatch.setenv(DATA_VARIABLE, str(tmp_path))

    block = Block.for_notebook("/wherever/demo.py")

    assert block.notebook == "demo"
    assert block.get_rows() == [{"text": "hello"}]
    assert block.get_params() == {"text_column": "text"}


def test_a_notebook_nobody_has_staged_rows_for_opens_empty_rather_than_raising(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv(DATA_VARIABLE, str(tmp_path))

    block = Block.for_notebook("demo.py")

    assert block.get_rows() == []
    assert block.get_params() == {}
    assert block.get_input().is_staged is False
