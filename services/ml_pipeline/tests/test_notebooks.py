from __future__ import annotations

import pytest

from ml_pipeline.config import Config
from ml_pipeline.notebooks import (
    NotebookDirectory,
    NotebookNotFoundError,
    NotebookRejectedError,
)

NOTEBOOK = """
import marimo

app = marimo.App()


@app.cell
def _():
    x = 1
    return (x,)


if __name__ == "__main__":
    app.run()
"""


def test_a_source_that_does_not_parse_is_refused_with_its_line(scratch: Config) -> None:
    directory = NotebookDirectory(root=scratch.notebooks)

    with pytest.raises(NotebookRejectedError) as refusal:
        directory.save_notebook("broken", "import marimo\napp = marimo.App(\n")

    assert "does not parse" in str(refusal.value)
    assert not (scratch.notebooks / "broken.py").exists()


def test_a_plain_script_is_refused_because_marimo_could_not_serve_it(scratch: Config) -> None:
    directory = NotebookDirectory(root=scratch.notebooks)

    with pytest.raises(NotebookRejectedError) as refusal:
        directory.save_notebook("script", "print('hello')\n")

    assert "marimo app" in str(refusal.value)


def test_editing_a_notebook_changes_the_revision_a_pipeline_pins(scratch: Config) -> None:
    directory = NotebookDirectory(root=scratch.notebooks)

    first = directory.save_notebook("demo", NOTEBOOK)
    same = directory.save_notebook("demo", NOTEBOOK)
    edited = directory.save_notebook("demo", NOTEBOOK.replace("x = 1", "x = 2"))

    assert first.revision == same.revision
    assert first.revision != edited.revision
    assert directory.get_notebook("demo").revision == edited.revision


def test_a_name_cannot_reach_outside_the_notebook_directory(scratch: Config) -> None:
    directory = NotebookDirectory(root=scratch.notebooks)

    for name in ["../secret", "/etc/passwd", "sub/dir", "Upper", "", "with space"]:
        with pytest.raises(NotebookRejectedError):
            directory.path_of(name)


def test_a_missing_notebook_is_its_own_failure(scratch: Config) -> None:
    with pytest.raises(NotebookNotFoundError):
        NotebookDirectory(root=scratch.notebooks).get_notebook("absent")


def test_the_title_is_the_first_line_of_the_docstring(scratch: Config) -> None:
    directory = NotebookDirectory(root=scratch.notebooks)

    saved = directory.save_notebook("titled", '"""What this block does.\n\nMore.\n"""\n' + NOTEBOOK)

    assert saved.title == "What this block does."
    assert directory.save_notebook("plain", NOTEBOOK).title == "plain"
