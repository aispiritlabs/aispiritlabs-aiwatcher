from __future__ import annotations

from typing import Any

import pytest

from ml_pipeline.config import Config
from ml_pipeline.notebooks import NotebookDirectory
from ml_pipeline.runner import NotebookFailedError, RunResult, run_notebook
from ml_pipeline.staging import Row, Staging

TEXTS = [
    "Call me at +48 601 234 567 or write to anna.kowalska@example.com.",
    "Card 4111 1111 1111 1111 expires soon; the server is 192.168.1.20.",
    "Nothing sensitive here at all.",
]

# A block in its smallest honest form: one injected cell, one that names what it
# hands on.
PASSTHROUGH = """
import marimo

app = marimo.App()


@app.cell
def _():
    from ml_pipeline import Block

    return (Block,)


@app.cell
def _(Block):
    _block = Block.for_notebook(__file__)
    rows = _block.get_rows()
    params = _block.get_params()
    return params, rows


@app.cell
def _(rows):
    output = [{**row, "seen": True} for row in rows]
    return (output,)


if __name__ == "__main__":
    app.run()
"""

SILENT = """
import marimo

app = marimo.App()


@app.cell
def _():
    kept = 1
    return (kept,)


if __name__ == "__main__":
    app.run()
"""


def run(config: Config, name: str, rows: list[Row], params: dict[str, Any]) -> RunResult:
    return run_notebook(
        NotebookDirectory(root=config.notebooks).get_notebook(name),
        rows,
        params,
        Staging(root=config.data),
        config,
    )


def test_the_demo_block_masks_its_text_and_counts_what_it_found(config: Config) -> None:
    rows = [{"text": text} for text in TEXTS]
    result = run(config, "pii_detection", rows, {"text_column": "text"})

    assert result.row_count == 3
    assert [row["pii_count"] for row in result.rows] == [2, 2, 0]
    assert result.rows[0]["text"] == "Call me at [PHONE] or write to [EMAIL]."
    assert result.rows[1]["pii_kinds"] == ["credit_card", "ip_address"]
    assert result.rows[2]["text"] == TEXTS[2]
    assert result.revision


def test_a_finding_never_carries_the_text_it_matched(config: Config) -> None:
    """The rule ADR_0021 keeps for a conversation finding, kept here.

    A finding that quoted the address it found would put that address in every
    preview, every list and every published dataset version — which is the
    thing this whole block exists to take out.
    """
    result = run(config, "pii_detection", [{"text": TEXTS[0]}], {"text_column": "text"})

    findings = result.rows[0]["pii_findings"]
    assert findings
    for finding in findings:
        assert set(finding) == {"kind", "start", "end"}
        assert "anna.kowalska@example.com" not in str(finding)


def test_the_blocks_settings_reach_a_headless_run(config: Config) -> None:
    """`mo.ui` defaults come from the injected `params`, so a run honours what
    the block was configured with."""
    rows = [{"body": TEXTS[0]}, {"body": TEXTS[2]}]

    everything = run(config, "pii_detection", rows, {"text_column": "body"})
    only_email = run(config, "pii_detection", rows, {"text_column": "body", "kinds": ["email"]})
    matches_only = run(config, "pii_detection", rows, {"text_column": "body", "only_matches": True})

    assert [row["pii_count"] for row in everything.rows] == [2, 0]
    assert [row["pii_count"] for row in only_email.rows] == [1, 0]
    assert matches_only.row_count == 1


def test_the_run_injects_its_rows_rather_than_letting_the_notebook_read_them(
    scratch: Config,
) -> None:
    """`App.run(defs=…)` replaces the cell that would have read the staged file.

    Staging the previous rows and passing new ones is exactly the state a
    second preview is in, and the run has to be about the second lot.
    """
    NotebookDirectory(root=scratch.notebooks).save_notebook("demo", PASSTHROUGH)
    Staging(root=scratch.data).stage("demo", [{"text": "stale"}])

    result = run(scratch, "demo", [{"text": "fresh"}], {})

    assert result.rows == [{"text": "fresh", "seen": True}]


def test_a_crowded_injection_cell_is_answered_rather_than_reported_raw(scratch: Config) -> None:
    """The one mistake this contract invites, and marimo's own message for it
    names the missing definitions without saying what to do about them."""
    crowded = PASSTHROUGH.replace(
        """@app.cell
def _():
    from ml_pipeline import Block

    return (Block,)


@app.cell
def _(Block):
    _block""",
        """@app.cell
def _():
    from ml_pipeline import Block

    _block""",
    ).replace("    return params, rows", "    return Block, params, rows")
    NotebookDirectory(root=scratch.notebooks).save_notebook("crowded", crowded)

    with pytest.raises(NotebookFailedError) as failure:
        run(scratch, "crowded", [{"text": "fresh"}], {})

    assert "move everything else into a cell of its own" in failure.value.stderr


def test_a_block_that_names_nothing_says_which_definition_is_missing(scratch: Config) -> None:
    NotebookDirectory(root=scratch.notebooks).save_notebook("silent", SILENT)

    with pytest.raises(NotebookFailedError) as failure:
        run(scratch, "silent", [{"text": "x"}], {})

    assert "output" in failure.value.stderr


def test_a_notebook_that_raises_comes_back_as_its_traceback(scratch: Config) -> None:
    NotebookDirectory(root=scratch.notebooks).save_notebook(
        "angry", SILENT.replace("kept = 1", "raise RuntimeError('no')")
    )

    with pytest.raises(NotebookFailedError) as failure:
        run(scratch, "angry", [], {})

    assert "exited with status" in str(failure.value)
    assert "RuntimeError" in failure.value.stderr


def test_a_notebook_that_answers_the_same_thing_twice_says_nothing_and_is_believed(
    scratch: Config,
) -> None:
    """The default, and it is the common case: a curation block is a transform."""
    NotebookDirectory(root=scratch.notebooks).save_notebook("plain", PASSTHROUGH)

    assert run(scratch, "plain", [{"text": "a"}], {}).deterministic


def test_a_notebook_that_reads_the_clock_says_so_and_is_not_cached(scratch: Config) -> None:
    """The escape hatch. A managed chain caches this step by the rows it read,
    the parameters it was given and the revision it pinned — which is right for
    a transform and wrong for a notebook that samples, asks a model or reads
    the time. Nothing outside the notebook can tell those apart, so it says."""
    sampling = PASSTHROUGH.replace(
        """@app.cell
def _(rows):
    output = [{**row, "seen": True} for row in rows]
    return (output,)""",
        """@app.cell
def _(rows):
    import random

    output = [{**row, "sample": random.random()} for row in rows]
    deterministic = False
    return deterministic, output""",
    )
    NotebookDirectory(root=scratch.notebooks).save_notebook("sampling", sampling)

    assert run(scratch, "sampling", [{"text": "a"}], {}).deterministic is False
