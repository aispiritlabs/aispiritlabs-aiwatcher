"""The DataFusion Python the panel writes, run on the engine under strict admission.

The panel generates DataFusion text in three places — Build mode on the Query tab
(`query-builder.ts`), a promotion on the Datasets tab and a hub import — and it is a
generator, not a judge: whether the text runs is this engine's answer (AW-3). These are
the shapes each emits, as the TypeScript produces them, so a change to the language they
lean on fails here; the TypeScript tests hold the emitters to the same shapes from the
other side. `BuilderShapesTest.php` does this for Flow.
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
from query_datafusion import ENGINE, REFERENCE

#: Build mode: `model` and the count on the spans grain — the spec's scenario.
MODEL_AND_COUNT = """(
    read("spans")
    .aggregate(
        [col("model")],
        [
            f.count(col("span_id")).alias("spans"),
        ],
    )
    .sort(col("spans").sort(ascending=False))
)"""

#: Build mode: grouped by agent, filtered on it, with a second metric.
BY_AGENT = """(
    read("runs")
    .with_column("agent", col("agents"))
    .unnest_columns("agent")
    .filter(col("agent") == lit("planner"))
    .aggregate(
        [col("agent")],
        [
            f.count(col("run_id")).alias("runs"),
            f.sum(col("input_tokens")).alias("input_tokens"),
        ],
    )
    .sort(col("runs").sort(ascending=False))
)"""

#: Build mode: a flat list, two values on one attribute, a limit.
LISTED = """(
    read("spans")
    .filter((col("model") == lit("gpt-5")) | (col("model") == lit("claude-sonnet-5")))
    .select(
        col("span_id"), col("name"), col("agent_id"), col("model"), col("tool"), col("start"),
        col("duration_ms"),
    )
    .sort(col("start").sort(ascending=False))
    .limit(3)
)"""

#: The Datasets tab: runs promoted by any of two agents, one row per run.
PROMOTION = """(
    read("default", period="24h")
    .with_column("selected_agent", col("agents"))
    .unnest_columns("selected_agent")
    .filter((col("selected_agent") == lit("planner")) | (col("selected_agent") == lit("coder")))
    .distinct_on(
        [col("run_id")],
        [
            col("run_id").alias("source_run_id"),
            col("conversation_id").alias("source_session_id"),
            col("trace_id").alias("source_trace_id"),
            col("agents"),
            col("status"),
            col("started_at"),
        ],
        [col("run_id").sort()],
    )
)"""

#: A hub import of a corpus whose pictures are an `Image` column, with a caption.
PICTURES = """# image is where this corpus keeps its pictures, of: image, text
(
    read("hub_rows", dataset="owner/pictures", limit=100)
    .with_column("uri", col("row")["image"]["src"])
    .with_column("width", col("row")["image"]["width"])
    .with_column("height", col("row")["image"]["height"])
    .with_column("group_id", f.concat(lit("owner/pictures/"), col("row_index")))
    .with_column("caption", col("row")["text"])
    .select(col("uri"), col("width"), col("height"), col("group_id"), col("caption"))
)"""

#: A hub search, one row per corpus, before one is chosen to read.
CORPORA = """(
    read("hub_datasets", q="floor plans", hub="huggingface")
    .filter(col("id") == lit("owner/pictures"))
    .with_column("uri", col("url"))
    .with_column("group_id", col("id"))
    .with_column("width", lit(0))
    .with_column("height", lit(0))
    .select(
        col("uri"), col("group_id"), col("width"), col("height"),
        col("claimed_license"), col("usage"),
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
        return execute(job, ENGINE, ENGINE.open(), client)


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


def test_a_hub_import_reads_a_picture_by_field(
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
