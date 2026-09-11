"""DataFusion as the contract's engine: the spec's DataFusion scenarios and its vocabulary."""

from __future__ import annotations

import ast
from collections.abc import Iterator
from pathlib import Path

import pytest
from starlette.testclient import TestClient

from aiwatcher_query.admission import admit
from aiwatcher_query.catalog import Catalog
from aiwatcher_query.check import check
from aiwatcher_query.child import Limits
from aiwatcher_query.config import DEFAULT_CATALOG, Config
from aiwatcher_query.conformance import TEXTS, questions
from aiwatcher_query.errors import QueryRefusedError
from aiwatcher_query.evaluate import Job, Outcome, execute
from aiwatcher_query.sandbox import Sandbox
from aiwatcher_query.service import create_app
from conftest import CORPUS_ROWS, FakeApi, spans, write_corpus
from query_datafusion import ENGINE, REFERENCE

VOCABULARY = ENGINE.vocabulary()
TEXT = TEXTS[ENGINE.language]
#: q2, the per-model aggregation: the spec's own example and the benchmark's measured query.
PER_MODEL = (questions()[1] / TEXT).read_text()


@pytest.fixture
def corpus(tmp_path: Path) -> Path:
    return write_corpus(tmp_path)


@pytest.fixture
def catalog(corpus: Path) -> Catalog:
    return Catalog.load(DEFAULT_CATALOG, corpus_root=corpus)


def _run(
    text: str,
    catalog: Catalog,
    corpus: Path,
    *,
    api: FakeApi | None = None,
    max_rows: int = 1_000,
    strict: bool = False,
) -> Outcome:
    job = Job(
        engine=REFERENCE,
        text=text,
        max_rows=max_rows,
        aiwatcher="http://aiwatcher.test",
        catalog=catalog,
        corpus=corpus,
        strict=strict,
    )
    with (api or FakeApi()).client() as client:
        return execute(job, ENGINE, ENGINE.open(), client)


def _problems(text: str) -> list[str]:
    return [problem.message for problem in admit(ast.parse(text), VOCABULARY)]


def _spans_api(*rows: dict[str, object]) -> FakeApi:
    api = FakeApi()
    api.answer("/api/v1/spans", {"spans": list(rows)})
    return api


def test_the_query_tabs_first_question_answers_one_row_per_model(
    catalog: Catalog, corpus: Path
) -> None:
    api = _spans_api(
        {"span_id": "s1", "kind": "llm", "model": "claude-sonnet-5"},
        {"span_id": "s2", "kind": "llm", "model": "claude-sonnet-5"},
        {"span_id": "s3", "kind": "llm", "model": "gpt-5"},
        {"span_id": "s4", "kind": "tool", "model": None},
    )
    outcome = _run(
        'read("spans").filter(col("kind") == lit("llm"))'
        '.aggregate([col("model")], [f.count(col("model")).alias("spans")])',
        catalog,
        corpus,
        api=api,
    )
    assert (outcome.dataset, outcome.columns) == ("spans", ["model", "spans"])
    assert sorted(outcome.rows, key=lambda row: str(row["model"])) == [
        {"model": "claude-sonnet-5", "spans": 2},
        {"model": "gpt-5", "spans": 1},
    ]


def test_a_transform_reads_the_rows_the_chain_has_so_far(catalog: Catalog, corpus: Path) -> None:
    # The shape the compiler writes: the source bound to `df`, then each transform.
    script = 'df = read("corpus_spans")\ndf = (df.filter(col("status") == lit("error")))\ndf'
    outcome = _run(script, catalog, corpus)
    errors = [row for row in spans().to_pylist() if row["status"] == "error"]
    assert sorted(outcome.rows, key=lambda row: str(row["span_id"])) == errors


def test_a_result_that_is_not_a_frame_is_refused_naming_what_it_was(
    catalog: Catalog, corpus: Path
) -> None:
    with pytest.raises(QueryRefusedError) as refused:
        _run("42", catalog, corpus)
    assert refused.value.message == (
        "A query ends in a DataFusion DataFrame, and this one ended in int 42."
    )


def test_a_syntax_error_is_located_where_pythons_parser_put_it(catalog: Catalog) -> None:
    text = 'read("spans").filter('
    with pytest.raises(SyntaxError) as python:
        ast.parse(text)
    (found,) = check(text, catalog, VOCABULARY)["diagnostics"]
    assert (found["line"], found["column"]) == (python.value.lineno, python.value.offset)


def test_csv_and_parquet_answer_the_same_rows(catalog: Catalog, corpus: Path) -> None:
    csv = _run(PER_MODEL, catalog, corpus)
    parquet = _run(
        PER_MODEL.replace('read("corpus_spans")', 'read("corpus_spans", format="parquet")'),
        catalog,
        corpus,
    )
    assert csv.rows == parquet.rows
    assert [row["spans"] for row in csv.rows] == [CORPUS_ROWS // 3]


def test_an_answer_stops_at_the_row_ceiling_and_says_so(catalog: Catalog, corpus: Path) -> None:
    outcome = _run('read("corpus_spans")', catalog, corpus, max_rows=10)
    assert (len(outcome.rows), outcome.truncated) == (10, True)


def test_a_query_naming_a_clock_is_not_deterministic(catalog: Catalog, corpus: Path) -> None:
    api = _spans_api({"span_id": "s1"})
    assert _run('read("spans")', catalog, corpus, api=api).deterministic
    timed = 'read("spans").with_column("at", f.now())'
    assert not _run(timed, catalog, corpus, api=api).deterministic


def test_the_vocabulary_is_datafusions_own() -> None:
    assert {"filter", "aggregate", "join", "sort", "limit", "with_column", "alias"} <= (
        VOCABULARY.attributes
    )
    assert {"count", "sum", "avg", "split_part", "when", "otherwise"} <= VOCABULARY.attributes
    assert ENGINE.volatile <= VOCABULARY.attributes


def test_strict_refuses_a_function_by_naming_udf() -> None:
    problems = _problems('udf(lambda array: array, [pa.int64()], pa.int64(), "stable")')
    assert problems[0] == "udf is not admitted: it registers a Python function with the engine."


def test_strict_refuses_sql_saying_a_dataset_is_reached_through_read() -> None:
    problems = _problems('SessionContext().sql("SELECT 1")')
    assert problems[0] == (
        "sql is not admitted: it runs SQL, and a strict query is DataFusion's Python API; "
        "a dataset is reached through read()."
    )
    assert problems[1].startswith("SessionContext is not a name a strict query has")


def test_strict_refuses_everything_a_session_offers() -> None:
    assert _problems('df = read("corpus_spans")\ndf.register_csv("t", "/etc/passwd")\ndf') == [
        "register_csv is not admitted: it is the SessionContext's, which a query never holds; "
        "a dataset is reached through read()."
    ]


def test_strict_declines_by_signature_what_takes_a_path_or_a_callable() -> None:
    assert _problems('read("corpus_spans").write_parquet("/tmp/spans")') == [
        "write_parquet is not admitted: it takes a path, and a strict query reaches data only "
        "through read()."
    ]
    assert _problems('read("corpus_spans").transform(f.abs)') == [
        "transform is not admitted: it takes a callable, and a strict query passes no function."
    ]
    assert VOCABULARY.declined["array_filter"] == VOCABULARY.declined["transform"]


def test_strict_does_not_reach_what_functions_merely_imported() -> None:
    assert _problems("f.pa") == ["pa is not part of DataFusion's API."]


@pytest.mark.parametrize("question", questions(), ids=lambda question: question.name)
def test_strict_admits_and_runs_every_conformance_question(
    question: Path, catalog: Catalog, corpus: Path
) -> None:
    text = (question / TEXT).read_text()
    assert _problems(text) == []
    # Ran: a refusal raises, and an answer lists its columns even when it has no rows.
    assert _run(text, catalog, corpus, strict=True).columns


@pytest.fixture(scope="module")
def served(tmp_path_factory: pytest.TempPathFactory) -> Iterator[TestClient]:
    corpus = write_corpus(tmp_path_factory.mktemp("corpus"))
    sandbox = Sandbox(REFERENCE, Limits(cpu_seconds=30, memory_mb=0))
    sandbox.start()
    config = Config(aiwatcher="http://127.0.0.1:9", corpus=corpus)
    catalog = Catalog.load(config.catalog, corpus_root=corpus)
    app = create_app(config, engine=ENGINE, engine_ref=REFERENCE, catalog=catalog, runner=sandbox)
    with TestClient(app) as client:
        yield client


def test_healthz_names_datafusion_python(served: TestClient) -> None:
    answer = served.get("/query/healthz").json()
    assert (answer["engine"], answer["language"], answer["version"]) == (
        "datafusion",
        "datafusion-python",
        ENGINE.version(),
    )


def test_a_query_answers_through_the_fork_server(served: TestClient) -> None:
    response = served.post("/query/query", json={"pipeline": PER_MODEL})
    assert response.status_code == 200, response.text
    assert [row["spans"] for row in response.json()["rows"]] == [CORPUS_ROWS // 3]
