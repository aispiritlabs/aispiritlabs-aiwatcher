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

# The rows the Titanic chain's Flow half hands to the block that runs after it:
# eight real passengers of `phihung/titanic`, with the status the query already
# recoded. Two of them report no age at all — one whose status group has another
# age to borrow, and one whose group has none — because those are the two
# branches the fill has.
PASSENGERS: list[Row] = [
    {"name": name, "status": status, "age": age, "sib_sp": siblings, "parch": parents, "fare": fare}
    for name, status, age, siblings, parents, fare in [
        ("Braund, Mr. Owen Harris", "mr", 22.0, 1, 0, 7.25),
        ("Cumings, Mrs. John Bradley", "mrs", 38.0, 1, 0, 71.2833),
        ("Heikkinen, Miss. Laina", "miss", 26.0, 0, 0, 7.925),
        ("Moran, Mr. James", "mr", None, 0, 0, 8.4583),
        ("Palsson, Master. Gosta Leonard", "master", 2.0, 3, 1, 21.075),
        ("Vestrom, Miss. Hulda Amanda Adolfina", "miss", 14.0, 0, 0, 7.8542),
        ("Sagesser, Mlle. Emma", "miss", 24.0, 0, 0, 69.3),
        ("Brewe, Dr. Arthur Jackson", "rare", None, 0, 0, 39.6),
    ]
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
        NotebookDirectory(root=config.notebooks, revisions=config.revisions).get_notebook(name),
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


def test_the_titanic_block_fills_a_missing_age_from_the_median_of_its_status(
    config: Config,
) -> None:
    """The statistic that is the reason this block is a notebook at all.

    A median is a fact about the other rows, so no row-at-a-time expression in
    the query that ran before it could have produced one.
    """
    result = run(config, "titanic_features", PASSENGERS, {})

    filled = {row["name"]: row["age_filled"] for row in result.rows}
    # The other `mr` reported 22, and that is what this one is given — not the
    # median over everybody, which is 23.
    assert filled["Moran, Mr. James"] == 22.0
    # `rare` is one passenger and reported no age at all, so there is no group
    # median to borrow; it falls back to the median over every reported age
    # rather than keeping nothing.
    assert filled["Brewe, Dr. Arthur Jackson"] == 23.0


def test_an_imputed_age_never_overwrites_a_reported_one(config: Config) -> None:
    """A measurement and a guess must stay distinguishable after publishing.

    Filling the column in place is what every notebook does and what a dataset
    version must not do: nothing downstream — a split, a metric, a model card —
    could tell the two apart afterwards.
    """
    result = run(config, "titanic_features", PASSENGERS, {})

    guessed = [row for row in result.rows if row["age_imputed"]]
    reported = [row for row in result.rows if not row["age_imputed"]]

    assert [row["age"] for row in guessed] == [None, None]
    assert all(row["age_filled"] is not None for row in guessed)
    assert all(row["age"] == row["age_filled"] for row in reported)


def test_family_size_is_siblings_plus_parents_plus_the_passenger(
    config: Config,
) -> None:
    """The arithmetic this block was once said to exist for.

    It is expressible in the query now — Flow's own `->plus(...)`, reachable
    since the query surface stopped being a hand-written list — so what this
    tests is the block, not a limit of the language.
    """
    result = run(config, "titanic_features", PASSENGERS, {})

    sizes = {row["name"]: (row["family_size"], row["is_alone"]) for row in result.rows}

    # Three siblings, one parent, and the passenger the query could not add.
    assert sizes["Palsson, Master. Gosta Leonard"] == (5, False)
    assert sizes["Brewe, Dr. Arthur Jackson"] == (1, True)
    # A fare is what the ticket cost, not what the passenger cost.
    per_person = {row["name"]: row["fare_per_person"] for row in result.rows}
    assert per_person["Palsson, Master. Gosta Leonard"] == 4.21
    assert per_person["Brewe, Dr. Arthur Jackson"] == 39.6


def test_leaving_a_missing_age_missing_is_a_setting_rather_than_a_second_block(
    config: Config,
) -> None:
    """`mo.ui` defaults come from the injected `params`, here as they do there.

    The band follows the filled age, so refusing to fill one has to leave the
    band unknown as well — a banded row with no age is the same claim made
    twice as quietly.
    """
    result = run(config, "titanic_features", PASSENGERS, {"age_strategy": "keep"})

    missing = [row for row in result.rows if row["age"] is None]

    assert missing
    assert all(row["age_filled"] is None for row in missing)
    assert all(row["age_imputed"] is False for row in missing)
    assert {row["age_band"] for row in missing} == {"unknown"}


def test_the_bands_move_with_the_block_settings(config: Config) -> None:
    """Where the cut-offs fall is a setting, so the same rows band differently."""
    default = run(config, "titanic_features", PASSENGERS, {})
    older = run(config, "titanic_features", PASSENGERS, {"child_max": 5, "senior_min": 30})

    bands = {row["name"]: row["age_band"] for row in default.rows}
    moved = {row["name"]: row["age_band"] for row in older.rows}

    assert bands["Cumings, Mrs. John Bradley"] == "adult"
    assert moved["Cumings, Mrs. John Bradley"] == "senior"
    # And two is a child at either cut-off, which is what says the difference
    # above is the setting rather than the rows.
    assert bands["Palsson, Master. Gosta Leonard"] == "child"
    assert moved["Palsson, Master. Gosta Leonard"] == "child"


def test_the_run_injects_its_rows_rather_than_letting_the_notebook_read_them(
    scratch: Config,
) -> None:
    """`App.run(defs=…)` replaces the cell that would have read the staged file.

    Staging the previous rows and passing new ones is exactly the state a
    second preview is in, and the run has to be about the second lot.
    """
    NotebookDirectory(root=scratch.notebooks, revisions=scratch.revisions).save_notebook(
        "demo", PASSTHROUGH
    )
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
    NotebookDirectory(root=scratch.notebooks, revisions=scratch.revisions).save_notebook(
        "crowded", crowded
    )

    with pytest.raises(NotebookFailedError) as failure:
        run(scratch, "crowded", [{"text": "fresh"}], {})

    assert "move everything else into a cell of its own" in failure.value.stderr


def test_a_block_that_names_nothing_says_which_definition_is_missing(scratch: Config) -> None:
    NotebookDirectory(root=scratch.notebooks, revisions=scratch.revisions).save_notebook(
        "silent", SILENT
    )

    with pytest.raises(NotebookFailedError) as failure:
        run(scratch, "silent", [{"text": "x"}], {})

    assert "output" in failure.value.stderr


def test_a_notebook_that_raises_comes_back_as_its_traceback(scratch: Config) -> None:
    NotebookDirectory(root=scratch.notebooks, revisions=scratch.revisions).save_notebook(
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
    NotebookDirectory(root=scratch.notebooks, revisions=scratch.revisions).save_notebook(
        "plain", PASSTHROUGH
    )

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
    NotebookDirectory(root=scratch.notebooks, revisions=scratch.revisions).save_notebook(
        "sampling", sampling
    )

    assert run(scratch, "sampling", [{"text": "a"}], {}).deterministic is False
