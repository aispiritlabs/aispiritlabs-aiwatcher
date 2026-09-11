"""The DuckDB Python the panel writes, run on the engine under strict admission.

The panel generates DuckDB text in three places — Build mode on the Query tab
(`query-builder.ts`), a promotion on the Datasets tab and a hub import — and it is a
generator, not a judge: whether the text runs is this engine's answer (AW-3). These are
the shapes each emits, as the TypeScript produces them, so a change to the language they
lean on fails here; the TypeScript tests hold the emitters to the same shapes from the
other side. `test_panel_shapes.py` does this for DataFusion, `BuilderShapesTest.php` for
Flow.
"""

from __future__ import annotations

import ast
from pathlib import Path

import pytest

from aiwatcher_query.admission import admit
from aiwatcher_query.catalog import Catalog
from aiwatcher_query.config import DEFAULT_CATALOG
from aiwatcher_query.evaluate import Job, Outcome, execute
from conftest import FakeApi, write_corpus
from query_duckdb import ENGINE, REFERENCE

#: Build mode: `model` and the count on the spans grain — the spec's scenario.
MODEL_AND_COUNT = """(
    read("spans")
    .aggregate(
        [
            ColumnExpression("model"),
            FunctionExpression("count", ColumnExpression("span_id")).alias("spans"),
        ],
        "model",
    )
    .sort(ColumnExpression("spans").desc())
)"""

#: Build mode: grouped by agent, filtered on it, with a second metric.
BY_AGENT = """(
    read("runs")
    .project(StarExpression(), FunctionExpression("unnest", ColumnExpression("agents")).alias("agent"))
    .filter(ColumnExpression("agent") == ConstantExpression("planner"))
    .aggregate(
        [
            ColumnExpression("agent"),
            FunctionExpression("count", ColumnExpression("run_id")).alias("runs"),
            FunctionExpression("sum", ColumnExpression("input_tokens")).alias("input_tokens"),
        ],
        "agent",
    )
    .sort(ColumnExpression("runs").desc())
)"""  # noqa: E501 — as the emitter writes it, one line per step

#: Build mode: a flat list, two values on one attribute, a limit.
LISTED = """(
    read("spans")
    .filter((ColumnExpression("model") == ConstantExpression("gpt-5")) | (ColumnExpression("model") == ConstantExpression("claude-sonnet-5")))
    .select(ColumnExpression("span_id"), ColumnExpression("name"), ColumnExpression("agent_id"), ColumnExpression("model"), ColumnExpression("tool"), ColumnExpression("start"), ColumnExpression("duration_ms"))
    .sort(ColumnExpression("start").desc())
    .limit(3)
)"""  # noqa: E501 — as the emitter writes it, one line per step

#: The Datasets tab: runs promoted by any of two agents, one row per run.
PROMOTION = """(
    read("default", period="24h")
    .project(StarExpression(), FunctionExpression("unnest", ColumnExpression("agents")).alias("selected_agent"))
    .filter((ColumnExpression("selected_agent") == ConstantExpression("planner")) | (ColumnExpression("selected_agent") == ConstantExpression("coder")))
    .aggregate(
        [
            ColumnExpression("run_id").alias("source_run_id"),
            FunctionExpression("any_value", ColumnExpression("conversation_id")).alias("source_session_id"),
            FunctionExpression("any_value", ColumnExpression("trace_id")).alias("source_trace_id"),
            FunctionExpression("any_value", ColumnExpression("agents")).alias("agents"),
            FunctionExpression("any_value", ColumnExpression("status")).alias("status"),
            FunctionExpression("any_value", ColumnExpression("started_at")).alias("started_at"),
        ],
        "run_id",
    )
    .sort(ColumnExpression("source_run_id").asc())
)"""  # noqa: E501

#: A hub import of a corpus whose pictures are an `Image` column, with a caption.
PICTURES = """# image is where this corpus keeps its pictures, of: image, text
(
    read("hub_rows", dataset="owner/pictures", limit=100)
    .select(
        ColumnExpression("row", "image", "src").alias("uri"),
        ColumnExpression("row", "image", "width").alias("width"),
        ColumnExpression("row", "image", "height").alias("height"),
        FunctionExpression("concat", ConstantExpression("owner/pictures/"), ColumnExpression("row_index")).alias("group_id"),
        ColumnExpression("row", "text").alias("caption"),
    )
)"""  # noqa: E501

#: A hub search, one row per corpus, before one is chosen to read.
CORPORA = """(
    read("hub_datasets", q="floor plans", hub="huggingface")
    .filter(ColumnExpression("id") == ConstantExpression("owner/pictures"))
    .select(
        ColumnExpression("url").alias("uri"),
        ColumnExpression("id").alias("group_id"),
        ConstantExpression(0).alias("width"),
        ConstantExpression(0).alias("height"),
        ColumnExpression("claimed_license"),
        ColumnExpression("usage"),
    )
)"""

SHAPES = {
    "model and count": MODEL_AND_COUNT,
    "by agent": BY_AGENT,
    "listed": LISTED,
    "promotion": PROMOTION,
    "pictures": PICTURES,
    "corpora": CORPORA,
}

RUNS = [
    {"run_id": "r1", "conversation_id": "s1", "trace_id": "t1", "status": "succeeded",
     "agents": ["planner", "coder"], "started_at": "2026-09-10T10:00:00Z", "input_tokens": 10},
    {"run_id": "r2", "conversation_id": "s1", "trace_id": "t2", "status": "failed",
     "agents": ["planner"], "started_at": "2026-09-10T11:00:00Z", "input_tokens": 5},
    {"run_id": "r3", "conversation_id": "s2", "trace_id": "t3", "status": "succeeded",
     "agents": [], "started_at": "2026-09-10T12:00:00Z", "input_tokens": 1},
]  # fmt: skip
SPANS = [
    {
        "span_id": f"s{i}",
        "model": "gpt-5" if i % 2 else "claude-sonnet-5",
        "start": f"2026-09-10T10:0{i}",
    }
    for i in range(5)
]
PICTURE = {"src": "https://hub.test/0.png", "height": 64, "width": 32}


@pytest.fixture
def corpus(tmp_path: Path) -> Path:
    return write_corpus(tmp_path)


@pytest.fixture
def catalog(corpus: Path) -> Catalog:
    return Catalog.load(DEFAULT_CATALOG, corpus_root=corpus)


@pytest.fixture
def api() -> FakeApi:
    api = FakeApi()
    api.answer("/api/v1/runs", {"runs": RUNS})
    api.answer("/api/v1/spans", {"spans": SPANS})
    columns = [
        {"name": "image", "kind": "Image"},
        {"name": "text", "kind": "Value", "dtype": "string"},
    ]
    # Then an empty page, as a hub answers past its last row: the pager reads by
    # offset until the limit or an empty page, whichever comes first.
    api.answer(
        "/api/v1/dataset-hubs/rows",
        {
            "columns": columns,
            "rows": [{"row_index": 0, "row": {"image": PICTURE, "text": "a floor plan"}}],
        },
        {"columns": columns, "rows": []},
    )
    api.answer(
        "/api/v1/dataset-hubs/search",
        {
            "results": [
                {"hub": "huggingface", "id": "owner/pictures", "url": "https://hub.test/owner/pictures",
                 "claimed_license": "cc-by-4.0", "usage": "unclear"},
                {"hub": "huggingface", "id": "owner/other", "url": "https://hub.test/owner/other",
                 "claimed_license": "mit", "usage": "unclear"},
            ]
        },
    )  # fmt: skip
    return api


def _run(text: str, api: FakeApi, catalog: Catalog, corpus: Path) -> Outcome:
    job = Job(
        engine=REFERENCE,
        text=text,
        max_rows=1_000,
        aiwatcher="http://aiwatcher.test",
        catalog=catalog,
        corpus=corpus,
        strict=True,
    )
    with api.client() as client:
        return execute(job, ENGINE, ENGINE.open(corpus), client)


@pytest.mark.parametrize("text", SHAPES.values(), ids=list(SHAPES))
def test_every_shape_the_panel_writes_is_admitted(text: str) -> None:
    assert [problem.message for problem in admit(ast.parse(text), ENGINE.vocabulary())] == []


def test_build_mode_model_and_count_answers_one_row_per_model(
    api: FakeApi, catalog: Catalog, corpus: Path
) -> None:
    outcome = _run(MODEL_AND_COUNT, api, catalog, corpus)
    assert outcome.columns == ["model", "spans"]
    assert outcome.rows == [
        {"model": "claude-sonnet-5", "spans": 3},
        {"model": "gpt-5", "spans": 2},
    ]


def test_build_mode_expands_the_agents_before_it_groups_by_one(
    api: FakeApi, catalog: Catalog, corpus: Path
) -> None:
    outcome = _run(BY_AGENT, api, catalog, corpus)
    assert outcome.rows == [{"agent": "planner", "runs": 2, "input_tokens": 15}]


def test_build_mode_lists_newest_first_to_its_limit(
    api: FakeApi, catalog: Catalog, corpus: Path
) -> None:
    outcome = _run(LISTED, api, catalog, corpus)
    assert [row["span_id"] for row in outcome.rows] == ["s4", "s3", "s2"]


def test_a_promotion_by_agents_is_one_row_per_run(
    api: FakeApi, catalog: Catalog, corpus: Path
) -> None:
    outcome = _run(PROMOTION, api, catalog, corpus)
    assert [row["source_run_id"] for row in outcome.rows] == ["r1", "r2"]
    assert outcome.columns[:3] == ["source_run_id", "source_session_id", "source_trace_id"]


def test_a_hub_import_reads_a_picture_by_its_path(
    api: FakeApi, catalog: Catalog, corpus: Path
) -> None:
    outcome = _run(PICTURES, api, catalog, corpus)
    assert outcome.rows == [
        {
            "uri": PICTURE["src"],
            "width": 32,
            "height": 64,
            "group_id": "owner/pictures/0",
            "caption": "a floor plan",
        }
    ]


def test_a_hub_search_names_the_corpus_it_chose(
    api: FakeApi, catalog: Catalog, corpus: Path
) -> None:
    outcome = _run(CORPORA, api, catalog, corpus)
    assert [row["group_id"] for row in outcome.rows] == ["owner/pictures"]
