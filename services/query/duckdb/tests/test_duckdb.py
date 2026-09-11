"""DuckDB as the contract's engine: the spec's DuckDB scenarios, its vocabulary and its lock."""

from __future__ import annotations

import ast
from collections.abc import Iterator
from pathlib import Path

import pyarrow.csv as pv
import pytest
from starlette.testclient import TestClient

from aiwatcher_query.admission import admit
from aiwatcher_query.catalog import Catalog
from aiwatcher_query.child import Limits
from aiwatcher_query.config import DEFAULT_CATALOG, Config
from aiwatcher_query.conformance import TEXTS, questions
from aiwatcher_query.errors import QueryRefusedError
from aiwatcher_query.evaluate import Job, Outcome, execute
from aiwatcher_query.sandbox import Sandbox
from aiwatcher_query.service import create_app
from conftest import CORPUS_ROWS, FakeApi, spans, write_corpus
from query_duckdb import ENGINE, REFERENCE, functions

VOCABULARY = ENGINE.vocabulary()
TEXT = TEXTS[ENGINE.language]
#: q2, the per-model aggregation: the benchmark's measured query.
PER_MODEL = (questions()[1] / TEXT).read_text()
#: The spec's own example, as it is written there.
FIRST_QUESTION = (
    'read("spans").filter(ColumnExpression("kind") == ConstantExpression("llm"))'
    '.aggregate([ColumnExpression("model"), '
    'FunctionExpression("count", ColumnExpression("model")).alias("spans")], "model")'
)
SQL_SENTENCE = (
    "which DuckDB would parse as SQL; a filter is written with ColumnExpression and "
    "ConstantExpression, and a computed column with FunctionExpression."
)


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
        return execute(job, ENGINE, ENGINE.open(corpus), client)


def _problems(text: str) -> list[str]:
    return [problem.message for problem in admit(ast.parse(text), VOCABULARY)]


def _spans_api(*rows: dict[str, object]) -> FakeApi:
    api = FakeApi()
    api.answer("/api/v1/spans", {"spans": list(rows)})
    return api


def test_the_first_question_answers_one_row_per_model(catalog: Catalog, corpus: Path) -> None:
    api = _spans_api(
        {"span_id": "s1", "kind": "llm", "model": "claude-sonnet-5"},
        {"span_id": "s2", "kind": "llm", "model": "claude-sonnet-5"},
        {"span_id": "s3", "kind": "llm", "model": "gpt-5"},
        {"span_id": "s4", "kind": "tool", "model": None},
    )
    outcome = _run(FIRST_QUESTION, catalog, corpus, api=api)
    assert (outcome.dataset, outcome.columns) == ("spans", ["model", "spans"])
    assert sorted(outcome.rows, key=lambda row: str(row["model"])) == [
        {"model": "claude-sonnet-5", "spans": 2},
        {"model": "gpt-5", "spans": 1},
    ]


def test_a_transform_reads_the_rows_the_chain_has_so_far(catalog: Catalog, corpus: Path) -> None:
    # The shape the compiler writes: the source bound to `df`, then each transform.
    # Strict, because a relation also has a `df()` — declined as a way out — and the
    # compiler's name for the rows so far must still be admitted.
    script = (
        'df = read("corpus_spans")\n'
        'df = (df.filter(ColumnExpression("status") == ConstantExpression("error")))\n'
        "df"
    )
    outcome = _run(script, catalog, corpus, strict=True)
    errors = [row for row in spans().to_pylist() if row["status"] == "error"]
    assert sorted(outcome.rows, key=lambda row: str(row["span_id"])) == errors


def test_an_expression_is_not_a_relation_and_is_refused_naming_it(
    catalog: Catalog, corpus: Path
) -> None:
    with pytest.raises(QueryRefusedError) as refused:
        _run('ColumnExpression("model")', catalog, corpus)
    assert refused.value.message.startswith(
        "A query ends in a DuckDB relation, and this one ended in Expression"
    )
    assert "model" in refused.value.message


def test_a_sum_of_a_bigint_comes_back_as_an_integer(catalog: Catalog, corpus: Path) -> None:
    # DuckDB sums a BIGINT into a HUGEINT, which reaches Arrow as a decimal.
    outcome = _run(
        'read("corpus_spans").aggregate([FunctionExpression("sum", '
        'ColumnExpression("input_tokens")).alias("input_tokens")])',
        catalog,
        corpus,
    )
    assert outcome.rows == [{"input_tokens": sum(10 * i for i in range(CORPUS_ROWS))}]
    assert type(outcome.rows[0]["input_tokens"]) is int


def test_a_sum_wider_than_64_bits_is_refused_naming_its_column(
    catalog: Catalog, corpus: Path
) -> None:
    with pytest.raises(QueryRefusedError, match=r'Column "wide" holds .* does not fit in 64 bits'):
        _run(
            'read("corpus_spans").aggregate([FunctionExpression("sum", '
            f'ConstantExpression({2**62})).alias("wide")])',
            catalog,
            corpus,
        )


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


@pytest.mark.parametrize("function", ["now", "NOW", "random", "uuid"])
def test_a_function_named_by_text_that_reads_the_clock_is_not_deterministic(
    function: str, catalog: Catalog, corpus: Path
) -> None:
    api = _spans_api({"span_id": "s1"})
    assert _run('read("spans")', catalog, corpus, api=api).deterministic
    timed = f'read("spans").project(StarExpression(), FunctionExpression("{function}").alias("at"))'
    assert not _run(timed, catalog, corpus, api=api).deterministic


def test_the_vocabulary_is_duckdbs_own() -> None:
    assert {"filter", "aggregate", "join", "sort", "limit", "project", "set_alias"} <= (
        VOCABULARY.attributes
    )
    assert {"alias", "isin", "cast", "desc", "when", "otherwise"} <= VOCABULARY.attributes
    callable_ = functions().callable
    assert {"count", "sum", "avg", "split_part", "struct_extract", "unnest"} <= callable_
    assert {"now", "random", "uuid", "current_date"} <= ENGINE.volatile <= callable_


def test_strict_refuses_a_file_read_naming_read_csv_and_read() -> None:
    assert _problems('duckdb.read_csv("/etc/passwd")')[0] == (
        "read_csv is not admitted: it is the connection's, which a query never holds; "
        "a dataset is reached through read()."
    )


def test_strict_refuses_a_sql_string_in_a_filter_naming_the_string() -> None:
    assert _problems('read("spans").filter("kind = \'llm\'")') == [
        f"filter was handed the text \"kind = 'llm'\", {SQL_SENTENCE}"
    ]


@pytest.mark.parametrize("text", ["SQLExpression(\"kind = 'llm'\")", 'duckdb.sql("SELECT 1")'])
def test_strict_refuses_sql_by_call_saying_how_a_filter_is_written(text: str) -> None:
    called = text.split("(")[0].rpartition(".")[2]
    assert _problems(text)[0] == (
        f"{called} is not admitted: it runs SQL, and a strict query is DuckDB's Python API; "
        "a filter is written with ColumnExpression and ConstantExpression."
    )


@pytest.mark.parametrize(
    "text",
    [
        'kind = "kind = \'llm\'"\nread("spans").filter(kind)',
        'read("spans").filter(f"kind = {1}")',
        'read("spans").project("span_id" + ", getvariable(1)")',
        'r = read("spans")\nr.filter(r.alias)',
        'read("spans").select(*["kind", "model"])',
        'where = [w for w in ["kind = \'llm\'"]]\nread("spans").filter(where[0])',
        'read("spans").filter(" and ".join(["true"]))',
        'read("spans").order("model")',
        'read("spans").sum("input_tokens")',
    ],
)
def test_strict_refuses_text_however_it_reaches_a_relation_method(text: str) -> None:
    (first, *_) = _problems(text)
    assert "DuckDB would parse" in first, first
    assert first.endswith(f"as SQL; {SQL_SENTENCE.partition('SQL; ')[2]}"), first


def test_strict_refuses_a_method_called_through_a_name_the_query_gave_it() -> None:
    assert _problems('keep = read("spans").filter\nkeep(ColumnExpression("kind"))')[0] == (
        "keep is called by a name the query gave it, so admission cannot see which method "
        "it is; a strict DuckDB query calls a method by its own name."
    )


def test_strict_admits_text_where_duckdb_reads_a_name() -> None:
    assert (
        _problems(
            'runs = read("runs").set_alias("runs")\n'
            'read("spans")'
            '.filter(ColumnExpression("kind").isin(ConstantExpression("llm")))'
            '.join(runs, ColumnExpression("run_id") == ColumnExpression("runs.run_id"), "left")'
            '.project(ColumnExpression("duration_ms").cast("DOUBLE").alias("seconds"))'
            '.select(StarExpression(exclude=["error"]))'
            '.aggregate([ColumnExpression("model")], "model, seconds")'
        )
        == []
    )


def test_strict_refuses_a_group_key_that_is_not_column_names() -> None:
    assert _problems('read("spans").aggregate([ColumnExpression("model")], "model || \'x\'")') == [
        "aggregate's group key is \"model || 'x'\", which DuckDB would parse as SQL; a strict "
        'query groups by column names — "model", or "run_id, model" — and names a computed '
        "key with project() first."
    ]


def test_strict_admits_functionexpression_against_duckdbs_catalog() -> None:
    assert _problems('FunctionExpression("unnest", ColumnExpression("agents"))') == []
    assert _problems('FunctionExpression("current_setting", ConstantExpression("home"))') == [
        "current_setting is not admitted: it reads the database's configuration, which names "
        "paths on this machine."
    ]
    assert _problems('FunctionExpression("read_csv", ConstantExpression("/etc/passwd"))') == [
        '"read_csv" is not one of DuckDB\'s scalar or aggregate functions.'
    ]
    assert _problems('name = "count"\nFunctionExpression(name)')[0].startswith(
        "FunctionExpression names its function as text written in the query"
    )


@pytest.mark.parametrize("question", questions(), ids=lambda question: question.name)
def test_strict_admits_and_runs_every_conformance_question(
    question: Path, catalog: Catalog, corpus: Path
) -> None:
    text = (question / TEXT).read_text()
    assert _problems(text) == []
    # Ran: a refusal raises, and an answer lists its columns even when it has no rows.
    assert _run(text, catalog, corpus, strict=True).columns


def test_the_lock_refuses_a_file_a_sql_string_names_even_in_open_mode(
    catalog: Catalog, corpus: Path
) -> None:
    text = 'read("corpus_spans").filter("span_id IN (SELECT * FROM read_csv(\'/etc/passwd\'))")'
    with pytest.raises(QueryRefusedError, match="file system operations are disabled"):
        _run(text, catalog, corpus)


def test_the_lock_refuses_a_part_that_links_out_of_the_corpus(
    catalog: Catalog, corpus: Path, tmp_path_factory: pytest.TempPathFactory
) -> None:
    outside = tmp_path_factory.mktemp("outside") / "spans.csv"
    pv.write_csv(spans(), outside)
    (corpus / "spans.csv" / "part-00001.csv").symlink_to(outside)
    with pytest.raises(QueryRefusedError, match="file system operations are disabled"):
        _run('read("corpus_spans")', catalog, corpus)


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


def test_healthz_names_duckdb_python(served: TestClient) -> None:
    answer = served.get("/query/healthz").json()
    assert (answer["engine"], answer["language"], answer["version"]) == (
        "duckdb",
        "duckdb-python",
        ENGINE.version(),
    )


def test_a_query_answers_through_the_fork_server(served: TestClient) -> None:
    response = served.post("/query/query", json={"pipeline": PER_MODEL})
    assert response.status_code == 200, response.text
    assert [row["spans"] for row in response.json()["rows"]] == [CORPUS_ROWS // 3]
