"""The notebook half of `curation/flow-vs-polars`, over a corpus small enough to check by hand.

The Flow half is `services/query/flow/tests/Dataset/CorpusTest.php`, over the same six
spans and asserting the same two rows — which is what lets this file hand the
notebook Flow's answer as a literal.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

import polars as pl
import pytest

from ml_pipeline.config import Config
from ml_pipeline.notebooks import NotebookDirectory
from ml_pipeline.runner import NotebookFailedError, RunResult, run_notebook
from ml_pipeline.staging import Row, Staging

COLUMNS = [
    "run_id", "trace_id", "span_id", "parent_span_id", "name", "kind", "start", "end",
    "duration_ms", "operation", "agent_id", "model", "tool", "step_type", "status",
    "input_tokens", "output_tokens", "error",
]  # fmt: skip


def span(
    span_id: str,
    kind: str,
    duration: int,
    model: str | None = None,
    tokens: tuple[int, int] | None = None,
    error: str | None = None,
) -> dict[str, Any]:
    return dict(
        zip(
            COLUMNS,
            [
                "run-1",
                "trace-1",
                span_id,
                None,
                f"{kind}.x",
                kind,
                "2026-08-01T00:00:00.000Z",
                "2026-08-01T00:00:01.000Z",
                duration,
                None,
                "planner",
                model,
                None,
                None,
                "ok" if error is None else "error",
                None if tokens is None else tokens[0],
                None if tokens is None else tokens[1],
                error,
            ],
            strict=True,
        )
    )


# Four model calls over two models, and two spans that are not model calls.
SPANS = pl.DataFrame(
    [
        span("s1", "llm", 100, "a", (10, 2)),
        span("s2", "llm", 301, "a", (20, 4)),
        span("s3", "tool", 10),
        span("s4", "llm", 50, "b", (5, 1)),
        span("s5", "llm", 71, "b", (7, 0), error="upstream 503, retrying later"),
        span("s6", "step", 3),
    ],
    schema={name: pl.Int64 if name.endswith(("_ms", "_tokens")) else pl.String for name in COLUMNS},
)

# What the Flow block answers over those spans.
FLOW_ANSWER: list[Row] = [
    {"model": "a", "spans": 2, "input_tokens": 30, "output_tokens": 6, "avg_duration_ms": 200.5},
    {"model": "b", "spans": 2, "input_tokens": 12, "output_tokens": 1, "avg_duration_ms": 60.5},
]


@pytest.fixture
def corpus(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    """Both layouts the generator writes, two CSV parts so a single-file read shows."""
    root = tmp_path / "corpus"
    (root / "spans.csv").mkdir(parents=True)
    (root / "spans.parquet").mkdir()
    SPANS.slice(0, 3).write_csv(root / "spans.csv" / "part-00000.csv")
    SPANS.slice(3).write_csv(root / "spans.csv" / "part-00001.csv")
    SPANS.write_parquet(root / "spans.parquet" / "part-00000.parquet")
    # The notebook runs in a subprocess, which inherits this environment.
    monkeypatch.setenv("AIWATCHER_CORPUS_DIR", str(root))
    return root


def run(config: Config, rows: list[Row], params: dict[str, Any]) -> RunResult:
    return run_notebook(
        NotebookDirectory(root=config.notebooks, revisions=config.revisions).get_notebook(
            "flow_vs_polars"
        ),
        rows,
        params,
        Staging(root=config.data),
        config,
    )


@pytest.mark.usefixtures("corpus")
@pytest.mark.parametrize("corpus_format", ["csv", "parquet"])
def test_polars_agrees_with_what_flow_answered_over_the_same_files(
    config: Config, corpus_format: str
) -> None:
    result = run(config, FLOW_ANSWER, {"format": corpus_format})

    assert [row["model"] for row in result.rows] == ["a", "b"]
    assert all(row["agrees"] for row in result.rows)
    assert [row["polars_spans"] for row in result.rows] == [2, 2]
    assert [row["polars_avg_duration_ms"] for row in result.rows] == [200.5, 60.5]


@pytest.mark.usefixtures("corpus")
def test_a_flow_answer_that_miscounts_is_a_disagreement_showing_both_numbers(
    config: Config,
) -> None:
    miscounted = [{**FLOW_ANSWER[0], "spans": 3}, FLOW_ANSWER[1]]

    result = run(config, miscounted, {"format": "csv"})

    assert [row["agrees"] for row in result.rows] == [False, True]
    assert (result.rows[0]["flow_spans"], result.rows[0]["polars_spans"]) == (3, 2)


@pytest.mark.usefixtures("corpus")
def test_a_mean_flow_rounded_to_two_places_still_agrees_and_a_wrong_one_does_not(
    config: Config,
) -> None:
    # Flow's `average()` keeps two decimals, half up; Polars keeps them all. The
    # first managed run over a real corpus reported 1651.19 against 1651.192985
    # as a disagreement, which is a rounding rule rather than a wrong answer.
    rounded = [{**FLOW_ANSWER[0], "avg_duration_ms": 200.504}, FLOW_ANSWER[1]]
    wrong = [{**FLOW_ANSWER[0], "avg_duration_ms": 200.52}, FLOW_ANSWER[1]]

    assert all(row["agrees"] for row in run(config, rounded, {"format": "csv"}).rows)
    assert [row["agrees"] for row in run(config, wrong, {"format": "csv"}).rows] == [False, True]


@pytest.mark.usefixtures("corpus")
def test_a_model_only_one_engine_found_is_a_disagreement_rather_than_a_missing_row(
    config: Config,
) -> None:
    result = run(config, FLOW_ANSWER[:1], {"format": "csv"})

    assert [(row["model"], row["agrees"]) for row in result.rows] == [("a", True), ("b", False)]
    assert result.rows[1]["flow_spans"] is None


@pytest.mark.usefixtures("corpus")
def test_the_comparison_is_never_remembered(config: Config) -> None:
    # The files are addressed by nothing and the time differs on every run, so
    # a managed chain must not serve this step from its cache.
    assert run(config, FLOW_ANSWER, {"format": "csv"}).deterministic is False


def test_without_a_corpus_root_the_block_fails_rather_than_finding_nothing(
    config: Config, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.delenv("AIWATCHER_CORPUS_DIR", raising=False)

    with pytest.raises(NotebookFailedError) as failure:
        run(config, FLOW_ANSWER, {"format": "csv"})

    assert (
        "AIWATCHER_CORPUS_DIR" in f"{failure.value} {failure.value.stdout} {failure.value.stderr}"
    )
