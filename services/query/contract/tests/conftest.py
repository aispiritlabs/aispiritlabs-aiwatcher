from __future__ import annotations

import json
from collections.abc import Callable
from pathlib import Path
from typing import Any

import httpx
import pyarrow as pa
import pyarrow.csv as pv
import pyarrow.parquet as pq
import pytest

from aiwatcher_query.catalog import Catalog
from aiwatcher_query.config import DEFAULT_CATALOG

CORPUS_ROWS = 60
KINDS = ["llm", "tool", "agent"]
MODELS = ["claude-sonnet-5", "gpt-5", None]
WHOLE_NUMBERS = frozenset({"duration_ms", "input_tokens", "output_tokens"})


def spans(rows: int = CORPUS_ROWS) -> pa.Table:
    """The corpus's columns, over a few deterministic rows: every third an error."""
    return pa.table(
        {
            "run_id": [f"run-{i // 6}" for i in range(rows)],
            "trace_id": [f"trace-{i // 6}" for i in range(rows)],
            "span_id": [f"span-{i:03d}" for i in range(rows)],
            "parent_span_id": [None] * rows,
            "name": ["call"] * rows,
            "kind": [KINDS[i % 3] for i in range(rows)],
            "start": ["2026-08-01T00:00:00Z"] * rows,
            "end": ["2026-08-01T00:00:01Z"] * rows,
            "duration_ms": [100 + i for i in range(rows)],
            "operation": [None] * rows,
            "agent_id": ["planner"] * rows,
            "model": [MODELS[i % 3] for i in range(rows)],
            "tool": [None] * rows,
            "step_type": [None] * rows,
            "status": ["error" if i % 3 == 0 else "ok" for i in range(rows)],
            "input_tokens": [10 * i for i in range(rows)],
            "output_tokens": list(range(rows)),
            "error": [None] * rows,
        },
        schema=pa.schema(
            [
                (name, pa.int64() if name in WHOLE_NUMBERS else pa.string())
                for name in (
                    "run_id", "trace_id", "span_id", "parent_span_id", "name", "kind", "start",
                    "end", "duration_ms", "operation", "agent_id", "model", "tool", "step_type",
                    "status", "input_tokens", "output_tokens", "error",
                )
            ]
        ),
    )  # fmt: skip


def write_corpus(root: Path) -> Path:
    """A corpus root holding `spans` as one CSV part and one Parquet part.

    A function as well as a fixture, because an engine's tests live in its own member and
    a second `conftest.py` there would be a second module named `conftest`.
    """
    table = spans()
    (root / "spans.csv").mkdir()
    (root / "spans.parquet").mkdir()
    pv.write_csv(table, root / "spans.csv" / "part-00000.csv")
    pq.write_table(table, root / "spans.parquet" / "part-00000.parquet")
    return root


@pytest.fixture
def corpus(tmp_path: Path) -> Path:
    return write_corpus(tmp_path)


@pytest.fixture
def catalog(corpus: Path) -> Catalog:
    return Catalog.load(DEFAULT_CATALOG, corpus_root=corpus)


type Handler = Callable[[httpx.Request], httpx.Response]


class FakeApi:
    """aiwatcher's API as a transport: answers from a table of paths, and remembers asks."""

    def __init__(self) -> None:
        self.requests: list[httpx.Request] = []
        self.pages: dict[str, list[tuple[int, Any]]] = {}

    def answer(self, path: str, *pages: Any, status: int = 200) -> None:
        self.pages[path] = [(status, page) for page in pages]

    def handle(self, request: httpx.Request) -> httpx.Response:
        self.requests.append(request)
        pages = self.pages.get(request.url.path)
        if not pages:
            return httpx.Response(404, json={"error": {"message": f"no {request.url.path}"}})
        status, body = pages.pop(0) if len(pages) > 1 else pages[0]
        return httpx.Response(status, content=json.dumps(body).encode())

    def client(self) -> httpx.Client:
        return httpx.Client(transport=httpx.MockTransport(self.handle))

    def params(self) -> list[dict[str, str]]:
        return [dict(request.url.params) for request in self.requests]


@pytest.fixture
def api() -> FakeApi:
    return FakeApi()
