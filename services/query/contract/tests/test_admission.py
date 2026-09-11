from __future__ import annotations

import ast
from pathlib import Path

import pyarrow as pa
import pytest

from aiwatcher_query.admission import Vocabulary, admit
from aiwatcher_query.catalog import Catalog
from aiwatcher_query.check import check
from aiwatcher_query.errors import QueryRefusedError
from aiwatcher_query.evaluate import Job, Outcome, execute
from arrow_engine import ENGINE, REFERENCE, ArrowEngine
from conftest import CORPUS_ROWS, FakeApi

VOCABULARY = ENGINE.vocabulary()


def _problems(text: str, vocabulary: Vocabulary = VOCABULARY) -> list[str]:
    return [problem.message for problem in admit(ast.parse(text), vocabulary)]


def _strict(text: str, catalog: Catalog, corpus: Path, engine: ArrowEngine = ENGINE) -> Outcome:
    job = Job(
        engine=REFERENCE,
        text=text,
        max_rows=1_000,
        aiwatcher="http://aiwatcher.test",
        catalog=catalog,
        corpus=corpus,
        strict=True,
    )
    with FakeApi().client() as client:
        return execute(job, engine, engine.open(), client)


def test_a_query_in_the_engines_own_vocabulary_is_admitted() -> None:
    assert (
        _problems(
            'errors = read("corpus_spans").filter(field("status") == "error")\n'
            'errors.select(["span_id", "model"])\n'
        )
        == []
    )


def test_the_attributes_are_the_engines_and_not_a_list_somebody_wrote() -> None:
    assert {"filter", "select", "group_by", "sort_by"} <= VOCABULARY.attributes
    assert {"sum", "count", "equal"} <= VOCABULARY.attributes


def test_an_import_is_refused() -> None:
    assert _problems('import os\nread("runs")') == [
        "A strict query imports nothing; a dataset is reached through read()."
    ]


def test_a_function_is_refused_by_name_before_the_lambda_it_holds() -> None:
    problems = _problems('pc.register_scalar_function(lambda context, x: x, "f", {}, {}, {})')
    assert problems == [
        "register_scalar_function is not admitted: it registers a Python function with the engine.",
        "A strict query passes no function, so it writes no lambda.",
    ]


def test_a_name_the_namespace_does_not_hold_is_refused_saying_what_it_holds() -> None:
    (problem,) = _problems('open("/etc/passwd")')
    assert problem.startswith("open is not a name a strict query has. It has field, now, pc, read")


def test_a_builtin_is_not_a_name_a_strict_query_has() -> None:
    (problem,) = _problems('read("runs").slice(0, len([1]))')
    assert problem.startswith("len is not a name a strict query has")


def test_nothing_beginning_with_an_underscore_is_named() -> None:
    assert _problems('read("runs").__class__')[0].startswith("__class__ begins with an underscore")
    assert _problems('_x = read("runs")\n_x')[0].startswith("_x begins with an underscore")


def test_format_is_refused_in_every_engine() -> None:
    assert _problems('"{0.__class__}".format(read("runs"))')[0] == (
        "format is not admitted: a format string reaches attributes by name, which a strict "
        "query never does."
    )


def test_a_member_of_no_engine_class_is_refused() -> None:
    assert _problems('read("runs").gi_frame') == ["gi_frame is not part of pyarrow's API."]


def test_a_generator_is_refused_and_a_list_is_not() -> None:
    assert _problems('columns = (c for c in ["a"])\nread("runs")')[0].startswith(
        "A strict query builds no generator"
    )
    assert _problems('columns = [c for c in ["a"]]\nread("runs").select(columns)') == []


def test_a_name_the_query_assigns_is_its_own_whatever_a_member_of_that_name_does() -> None:
    # `to_pylist` is declined as a Table's way out; as a name the query binds, it is a
    # variable holding a frame — the case of DuckDB's `df()` beside the compiler's `df`.
    assert _problems('to_pylist = read("runs")\nto_pylist') == []
    assert _problems('read("runs").to_pylist()') == [
        "to_pylist is not admitted: it leaves the engine, and a query answers with a frame."
    ]


def test_a_call_of_anything_but_a_name_is_refused() -> None:
    assert _problems('[read][0]("runs")') == ["A strict query calls a function by its name."]


def test_an_engines_call_rule_sees_every_call_by_name() -> None:
    def no_strings(call: ast.Call, name: str, tree: ast.Module) -> str | None:
        strings = [arg for arg in call.args if isinstance(arg, ast.Constant)]
        return "filter was given a string" if name == "filter" and strings else None

    vocabulary = Vocabulary.of(
        "pyarrow", names={"field"}, members=[pa.Table], declined={}, call_rule=no_strings
    )
    assert _problems('read("runs").filter("kind = \'llm\'")', vocabulary) == [
        "filter was given a string."
    ]


def test_check_reports_every_refusal_with_where(catalog: Catalog) -> None:
    answer = check('import os\nopen("x")', catalog, VOCABULARY)
    assert answer["checked_by"] == ["python", "aiwatcher", "strict"]
    assert [(found["line"], found["column"]) for found in answer["diagnostics"]] == [(1, 1), (2, 1)]


def test_a_strict_run_refuses_before_anything_runs(catalog: Catalog, corpus: Path) -> None:
    with pytest.raises(QueryRefusedError, match="imports nothing") as refused:
        _strict("import os\nos.getcwd()", catalog, corpus)
    assert refused.value.line == 1


def test_a_strict_run_answers_what_it_admits(catalog: Catalog, corpus: Path) -> None:
    outcome = _strict('read("corpus_spans").filter(field("status") == "error")', catalog, corpus)
    assert len(outcome.rows) == CORPUS_ROWS // 3
