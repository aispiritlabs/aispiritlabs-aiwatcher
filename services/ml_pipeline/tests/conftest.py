from __future__ import annotations

from collections.abc import Iterator
from pathlib import Path

import pytest

from ml_pipeline.config import SERVICE_ROOT, Config

NOTEBOOKS = SERVICE_ROOT / "notebooks"


@pytest.fixture
def config(tmp_path: Path) -> Config:
    """The shipped notebooks, but staging and outputs in a directory of their own."""
    return Config(
        notebooks=NOTEBOOKS,
        # Never the shipped directory: a test that saves a notebook would
        # otherwise write its history into the repository.
        revisions=tmp_path / "revisions",
        data=tmp_path / "data",
        timeout_seconds=120.0,
    )


@pytest.fixture
def scratch(tmp_path: Path) -> Iterator[Config]:
    """An empty notebook directory, for tests that write one."""
    notebooks = tmp_path / "notebooks"
    notebooks.mkdir()
    yield Config(
        notebooks=notebooks,
        revisions=tmp_path / "revisions",
        data=tmp_path / "data",
        timeout_seconds=60.0,
    )
