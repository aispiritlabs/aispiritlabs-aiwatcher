from __future__ import annotations

import json
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import httpx
import pyarrow as pa
import pyarrow.csv as pv
import pyarrow.parquet as pq
import pytest

from aiwatcher_query.admission import Vocabulary, admit
from aiwatcher_query.catalog import Catalog
from aiwatcher_query.check import check
from aiwatcher_query.config import DEFAULT_CATALOG
from aiwatcher_query.errors import QueryRefusedError
from aiwatcher_query.evaluate import parse

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


#: What the panel ships for each Python engine, in the one copy it is built from.
#: This file is services/query/contract/tests/conftest.py.
PANEL_CONTENT = Path(__file__).resolve().parents[4] / "apps/panel/src/shared/lib/content"

#: The rows a placeholder stands for: what `compilePython` reads when a chain has no source.
PLACEHOLDER_ROWS = 'read("default")'

#: The names a cheat-sheet line uses without binding them: the chain it is a step of, and
#: the second relation a join names.
PLACEHOLDERS = ("df", "other")


@dataclass(frozen=True)
class Piece:
    """One text the panel ships, as its engine is handed it."""

    #: Which piece of the file it is, which is what a failure names.
    name: str
    text: str
    #: A query or a transform, rather than a cheat-sheet line.
    whole: bool


def pieces(engine: str) -> list[Piece]:
    """Every text `content/<engine>.json` holds, each as it is used (AW-3).

    Read from the file the panel reads rather than copied, so the engine's own tests and
    the panel's screens hold the same bytes. A text of several lines is its lines, joined
    as the panel joins them, and the starter curation is one of the examples by name, so
    it is checked as that example. Each piece is checked as what it is to somebody using
    it:

    - a query stands alone, checked as `/query/check` checks one under `strict`;
    - a transform is handed `df`, so it is checked inside the script `compilePython`
      writes around it: `df = read("default")`, then `df = (<transform>)`, then `df`;
    - a cheat-sheet line is a step, not a query. One that starts with `.` continues a
      chain on `df`, and a join's `other` is the relation it joins; both placeholders are
      bound by a read before the line. It is admitted and not held to ending in a frame,
      because the window's line assigns the half it joins back.
    """
    content: dict[str, Any] = json.loads((PANEL_CONTENT / f"{engine}.json").read_text("utf-8"))
    examples = [
        Piece(f"example {example['name']}", "\n".join(example["query"]), whole=True)
        for example in content["examples"]
    ]
    lines = [
        Piece(f"cheat sheet {line['label']}", _step(line["code"]), whole=False)
        for line in content["transformations"]
    ]
    return [
        Piece("starter query", "\n".join(content["starterQuery"]), whole=True),
        *examples,
        Piece("new transform", _transform(content["newTransform"]), whole=True),
        Piece("transform help", _transform(content["transformExample"]), whole=True),
        *lines,
    ]


def refusals(piece: Piece, catalog: Catalog, vocabulary: Vocabulary) -> list[str]:
    """Every reason the engine gives for not running this piece, each at its line."""
    if piece.whole:
        diagnostics = check(piece.text, catalog, vocabulary)["diagnostics"]
        return [f"line {found['line']}: {found['message']}" for found in diagnostics]
    try:
        tree = parse(piece.text)
    except QueryRefusedError as refused:
        return [f"line {refused.line}: {refused.message}"]
    return [f"line {refused.line}: {refused.message}" for refused in admit(tree, vocabulary)]


def _transform(text: str) -> str:
    return f"df = {PLACEHOLDER_ROWS}\ndf = (\n{text.strip()}\n)\ndf"


def _step(code: str) -> str:
    bound = [f"{name} = {PLACEHOLDER_ROWS}" for name in PLACEHOLDERS]
    return "\n".join([*bound, f"df{code}" if code.startswith(".") else code])
