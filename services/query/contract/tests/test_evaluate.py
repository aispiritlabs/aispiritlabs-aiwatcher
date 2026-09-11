from __future__ import annotations

from pathlib import Path

import pytest

from aiwatcher_query.catalog import Catalog
from aiwatcher_query.errors import QueryRefusedError
from aiwatcher_query.evaluate import Job, Outcome, execute
from arrow_engine import ENGINE, REFERENCE
from conftest import CORPUS_ROWS, FakeApi


def _run(
    text: str,
    catalog: Catalog,
    corpus: Path,
    api: FakeApi | None = None,
    *,
    max_rows: int = 1_000,
    window_seconds: int | None = None,
) -> Outcome:
    job = Job(
        engine=REFERENCE,
        text=text,
        max_rows=max_rows,
        aiwatcher="http://aiwatcher.test",
        catalog=catalog,
        corpus=corpus,
        window_seconds=window_seconds,
    )
    with (api or FakeApi()).client() as client:
        return execute(job, ENGINE, ENGINE.open(), client)


def test_the_last_expression_is_the_answer(catalog: Catalog, corpus: Path) -> None:
    outcome = _run(
        'errors = read("corpus_spans").filter(field("status") == "error")\nerrors',
        catalog,
        corpus,
    )
    assert outcome.dataset == "corpus_spans"
    assert len(outcome.rows) == CORPUS_ROWS // 3
    assert {row["status"] for row in outcome.rows} == {"error"}


def test_an_answer_over_the_cap_is_reported_not_cut_silently(
    catalog: Catalog, corpus: Path
) -> None:
    outcome = _run('import pyarrow as pa\npa.table({"n": list(range(1001))})', catalog, corpus)
    assert len(outcome.rows) == 1_000
    assert outcome.truncated


def test_a_result_that_is_not_a_frame_is_refused_naming_what_it_was(
    catalog: Catalog, corpus: Path
) -> None:
    with pytest.raises(
        QueryRefusedError, match=r"ends in a pyarrow Table, and this one ended in int 42"
    ) as refused:
        _run("42", catalog, corpus)
    assert (refused.value.line, refused.value.column) == (1, 1)


def test_a_query_whose_last_line_is_a_statement_is_refused_where_it_is(
    catalog: Catalog, corpus: Path
) -> None:
    with pytest.raises(QueryRefusedError, match="ends in an expression") as refused:
        _run('df = read("corpus_spans")\nx = df', catalog, corpus)
    assert refused.value.line == 2


def test_a_syntax_error_carries_pythons_line_and_column(catalog: Catalog, corpus: Path) -> None:
    with pytest.raises(QueryRefusedError, match="This is not Python") as refused:
        _run('df = read("corpus_spans")\ndf.filter(', catalog, corpus)
    assert refused.value.line == 2
    assert refused.value.column == 10


def test_an_engine_error_belongs_to_the_query_and_says_where(
    catalog: Catalog, corpus: Path
) -> None:
    with pytest.raises(QueryRefusedError, match="The query failed: ArrowInvalid") as refused:
        _run('df = read("corpus_spans")\ndf.filter(field("nope") == "x")', catalog, corpus)
    assert refused.value.line == 2


def test_a_read_refusal_is_located_at_the_read(catalog: Catalog, corpus: Path) -> None:
    with pytest.raises(QueryRefusedError, match='no dataset "spanz"') as refused:
        _run('x = 1\nread("spanz")', catalog, corpus)
    assert refused.value.line == 2


def test_a_corpus_read_is_never_remembered(catalog: Catalog, corpus: Path) -> None:
    assert not _run('read("corpus_spans")', catalog, corpus).deterministic


def test_an_api_read_is_deterministic_until_it_names_the_clock(
    catalog: Catalog, corpus: Path
) -> None:
    api = FakeApi()
    api.answer("/api/v1/runs", {"runs": []})
    assert _run('read("runs")', catalog, corpus, api).deterministic
    assert not _run('stamp = now()\nread("runs")', catalog, corpus, api).deterministic


def test_the_answer_says_which_window_its_first_read_used(catalog: Catalog, corpus: Path) -> None:
    api = FakeApi()
    api.answer("/api/v1/spans", {"spans": []})
    assert _run('read("spans")', catalog, corpus, api, window_seconds=900).window_seconds == 900
    assert (
        _run('read("spans", period="1h")', catalog, corpus, api, window_seconds=900).window_seconds
        == 3600
    )


def test_a_parquet_corpus_answers_the_same_rows(catalog: Catalog, corpus: Path) -> None:
    csv = _run('read("corpus_spans")', catalog, corpus)
    parquet = _run('read("corpus_spans", format="parquet")', catalog, corpus)
    assert csv.rows == parquet.rows


def test_two_columns_of_one_name_are_refused_rather_than_merged(
    catalog: Catalog, corpus: Path
) -> None:
    with pytest.raises(QueryRefusedError, match='two columns named "a"'):
        _run(
            'import pyarrow as pa\npa.table([pa.array([1]), pa.array([2])], names=["a", "a"])',
            catalog,
            corpus,
        )
