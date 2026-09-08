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
    directory = NotebookDirectory(root=scratch.notebooks, revisions=scratch.revisions)

    with pytest.raises(NotebookRejectedError) as refusal:
        directory.save_notebook("broken", "import marimo\napp = marimo.App(\n")

    assert "does not parse" in str(refusal.value)
    assert not (scratch.notebooks / "broken.py").exists()


def test_a_plain_script_is_refused_because_marimo_could_not_serve_it(scratch: Config) -> None:
    directory = NotebookDirectory(root=scratch.notebooks, revisions=scratch.revisions)

    with pytest.raises(NotebookRejectedError) as refusal:
        directory.save_notebook("script", "print('hello')\n")

    assert "marimo app" in str(refusal.value)


def test_editing_a_notebook_changes_the_revision_a_pipeline_pins(scratch: Config) -> None:
    directory = NotebookDirectory(root=scratch.notebooks, revisions=scratch.revisions)

    first = directory.save_notebook("demo", NOTEBOOK)
    same = directory.save_notebook("demo", NOTEBOOK)
    edited = directory.save_notebook("demo", NOTEBOOK.replace("x = 1", "x = 2"))

    assert first.revision == same.revision
    assert first.revision != edited.revision
    assert directory.get_notebook("demo").revision == edited.revision


def test_a_name_cannot_reach_outside_the_notebook_directory(scratch: Config) -> None:
    directory = NotebookDirectory(root=scratch.notebooks, revisions=scratch.revisions)

    for name in ["../secret", "/etc/passwd", "sub/dir", "Upper", "", "with space"]:
        with pytest.raises(NotebookRejectedError):
            directory.path_of(name)


def test_a_missing_notebook_is_its_own_failure(scratch: Config) -> None:
    with pytest.raises(NotebookNotFoundError):
        NotebookDirectory(root=scratch.notebooks, revisions=scratch.revisions).get_notebook(
            "absent"
        )


def test_the_title_is_the_first_line_of_the_docstring(scratch: Config) -> None:
    directory = NotebookDirectory(root=scratch.notebooks, revisions=scratch.revisions)

    saved = directory.save_notebook("titled", '"""What this block does.\n\nMore.\n"""\n' + NOTEBOOK)

    assert saved.title == "What this block does."
    assert directory.save_notebook("plain", NOTEBOOK).title == "plain"


def test_a_saved_source_can_be_read_back_by_the_digest_it_was_named_by(scratch: Config) -> None:
    directory = NotebookDirectory(root=scratch.notebooks, revisions=scratch.revisions)

    saved = directory.save_notebook("demo", NOTEBOOK)

    assert directory.get_revision("demo", saved.revision).source == NOTEBOOK


def test_an_earlier_revision_survives_the_edit_that_replaced_it(scratch: Config) -> None:
    """The whole point of the history: an edit must not strand what ran before it.

    Until this, a run pinning the first digest was refused because the head no
    longer matched — provenance protected by making the old run unrepeatable.
    """
    directory = NotebookDirectory(root=scratch.notebooks, revisions=scratch.revisions)

    first = directory.save_notebook("demo", NOTEBOOK)
    second = directory.save_notebook("demo", NOTEBOOK.replace("x = 1", "x = 2"))

    assert directory.get_revision("demo", first.revision).source == NOTEBOOK
    assert "x = 2" in directory.get_revision("demo", second.revision).source
    assert directory.get_notebook("demo").revision == second.revision


def test_a_revision_carries_the_notebook_s_own_name_and_not_its_digest(scratch: Config) -> None:
    """Staging, the app URL and every log line are about the notebook.

    The revision file is called by its digest, so reading the name off the path
    would key a run's staged rows under a hash — which two runs of one notebook
    would then not share, and no test of the rows themselves would notice.
    """
    directory = NotebookDirectory(root=scratch.notebooks, revisions=scratch.revisions)

    saved = directory.save_notebook("demo", NOTEBOOK)

    assert directory.get_revision("demo", saved.revision).name == "demo"


def test_a_revision_this_directory_never_held_is_refused_rather_than_the_head(
    scratch: Config,
) -> None:
    directory = NotebookDirectory(root=scratch.notebooks, revisions=scratch.revisions)
    directory.save_notebook("demo", NOTEBOOK)

    with pytest.raises(NotebookNotFoundError) as absent:
        directory.get_revision("demo", "0" * 64)

    assert "saving the notebook again" in str(absent.value)


def test_a_revision_that_is_not_a_digest_cannot_reach_a_file(scratch: Config) -> None:
    directory = NotebookDirectory(root=scratch.notebooks, revisions=scratch.revisions)

    for revision in ["../../etc/passwd", "0" * 63, "Z" * 64, "", "0" * 64 + "0"]:
        with pytest.raises(NotebookRejectedError):
            directory.revision_path("demo", revision)


def test_keeping_one_source_twice_writes_the_file_once(scratch: Config) -> None:
    """Idempotent by construction: the path is the digest of these bytes."""
    directory = NotebookDirectory(root=scratch.notebooks, revisions=scratch.revisions)

    revision = directory.keep("demo", NOTEBOOK)
    again = directory.keep("demo", NOTEBOOK)

    assert revision == again
    assert list((scratch.revisions / "demo").glob("*.py")) == [
        directory.revision_path("demo", revision)
    ]


def test_a_head_written_before_the_history_existed_is_kept_at_start_up(scratch: Config) -> None:
    """What an upgrade does, and the limit of what it can do.

    The head that is there is kept. What was there before it was never written
    down, and no amount of code can invent it — so this asserts the one and
    says nothing about the other.
    """
    (scratch.notebooks / "legacy.py").write_text(NOTEBOOK, encoding="utf-8")
    directory = NotebookDirectory(root=scratch.notebooks, revisions=scratch.revisions)
    head = directory.get_notebook("legacy")

    assert directory.keep_current() == ["legacy"]
    assert directory.get_revision("legacy", head.revision).source == NOTEBOOK
    # And again changes nothing: it is the same digest naming the same bytes.
    assert directory.keep_current() == []
