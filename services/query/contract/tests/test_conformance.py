from __future__ import annotations

import json
from pathlib import Path

from aiwatcher_query.catalog import Catalog
from aiwatcher_query.config import DEFAULT_CATALOG
from aiwatcher_query.conformance import DATASETS, TEXTS, differences, questions


def test_every_question_has_a_text_in_every_language_and_flows_answer() -> None:
    assert [question.name for question in questions()] == ["q1", "q2", "q3", "q4"]
    for question in questions():
        for text in TEXTS.values():
            assert (question / text).is_file(), f"{question.name} has no {text}"
        assert (question / "expected.json").is_file()


def test_counts_and_sums_compare_exactly() -> None:
    assert differences([{"spans": 3}], [{"spans": 4}]) == ["row 0, spans: 4 where Flow answered 3"]


def test_a_mean_compares_within_the_rounding_flow_applies() -> None:
    assert differences([{"mean": 1656.09}], [{"mean": 1656.0871287128714}]) == []
    assert differences([{"mean": 1656.09}], [{"mean": 1656.08}]) != []


def test_a_missing_row_is_one_problem_rather_than_a_hundred() -> None:
    assert differences([{"spans": 1}], []) == ["0 rows where Flow answered 1"]


def test_columns_are_compared_by_name() -> None:
    assert differences([{"spans": 1}], [{"count": 1}]) == [
        "row 0 has columns ['count'], Flow's has ['spans']"
    ]


def test_the_catalog_flow_serves_is_the_one_this_package_describes() -> None:
    # Recorded from Flow with a corpus configured, which is what offers corpus_spans.
    # One file, two loaders: this is where they would drift apart, and it needs no engine.
    catalog = Catalog.load(DEFAULT_CATALOG, corpus_root=Path("/any/corpus"))
    assert json.loads(DATASETS.read_text()) == {
        "datasets": [dataset.describe() for dataset in catalog.all()],
        "max_rows": 1_000,
    }
