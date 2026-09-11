from __future__ import annotations

from aiwatcher_query.catalog import Catalog
from aiwatcher_query.check import check, offset_of


def test_a_query_that_reads_a_dataset_that_exists_is_ok(catalog: Catalog) -> None:
    assert check('read("spans", period="1h").filter(x)', catalog) == {
        "ok": True,
        "diagnostics": [],
        "checked_by": ["python", "aiwatcher"],
    }


def test_a_syntax_error_carries_the_line_and_column_pythons_parser_reported(
    catalog: Catalog,
) -> None:
    answer = check('df = read("spans")\ndf.filter(', catalog)
    assert not answer["ok"]
    assert answer["checked_by"] == ["python"]
    (found,) = answer["diagnostics"]
    assert (found["line"], found["column"]) == (2, 10)
    assert found["offset"] == len('df = read("spans")\n') + 9


def test_a_query_that_is_only_comments_is_refused_rather_than_crashing(catalog: Catalog) -> None:
    (found,) = check("# nothing here yet", catalog)["diagnostics"]
    assert found["message"] == 'The query is only comments. Try read("runs").'


def test_a_dataset_that_does_not_exist_is_found_without_running_anything(catalog: Catalog) -> None:
    (found,) = check('x = 1\nread("spanz")', catalog)["diagnostics"]
    assert 'no dataset "spanz"' in found["message"]
    assert (found["line"], found["column"]) == (2, 6)


def test_an_argument_computed_at_run_time_is_left_for_the_run(catalog: Catalog) -> None:
    assert check('name = "spanz"\nread(name)', catalog)["ok"]


def test_a_query_ending_in_a_statement_is_found(catalog: Catalog) -> None:
    (found,) = check('df = read("spans")', catalog)["diagnostics"]
    assert "ends in an expression" in found["message"]


def test_an_offset_counts_every_line_before_it() -> None:
    assert offset_of("ab\ncd\nef", 3, 2) == 7
    assert offset_of("anything", 0, 0) == 0
