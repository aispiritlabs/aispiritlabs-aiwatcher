"""A query, run: parsed, executed as Python, and read back as rows.

This runs in the child (`child.main`), never in the service's own process. The text is
a Python module whose last statement is an expression, and that expression's value is
the answer — so a query can name an intermediate frame on one line and use it on the
next, and a compiled pipeline can be `df = read(…)`, `df = (<transform>)`, `df`. The
value has to be the engine's frame; anything else is refused naming what it was.

Failures are the query's by default. The same text over the same rows fails the same
way on every retry, so an exception raised while it runs — a column that is not there,
a type the engine will not compare — is a 422 located at the line that raised it. What
is *not* the query's is reaching aiwatcher, which `read()` raises as its own types.
"""

from __future__ import annotations

import ast
import time
import traceback
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import httpx

from aiwatcher_query.admission import admit
from aiwatcher_query.answer import rows_of
from aiwatcher_query.catalog import Catalog
from aiwatcher_query.engine import Engine, Session
from aiwatcher_query.errors import AiwatcherUnreachableError, QueryRefusedError, UpstreamFailedError
from aiwatcher_query.reading import Reader

#: The file name a query's frames carry, which is how a traceback is read back to a line.
QUERY_FILENAME = "<query>"


@dataclass(frozen=True)
class Job:
    """Everything the child needs, handed across the fork server as one pickled value."""

    engine: str
    text: str
    max_rows: int
    aiwatcher: str
    catalog: Catalog
    corpus: Path | None = None
    window_seconds: int | None = None
    as_of: int | None = None
    input_limit: int | None = None
    #: `strict` admission: admitted by the engine's vocabulary before anything runs.
    strict: bool = False


@dataclass(frozen=True)
class Outcome:
    """A query that answered, before the service adds what only it knows."""

    columns: list[str]
    rows: list[dict[str, Any]]
    truncated: bool
    dataset: str | None
    grain: str | None
    window_seconds: int | None
    windowed: bool
    deterministic: bool
    took_ms: int


def parse(text: str) -> ast.Module:
    """The text as Python, or a refusal carrying the line and column Python's parser gave."""
    if not text.strip():
        raise QueryRefusedError('The query is empty. Try read("runs").')
    try:
        return ast.parse(text, filename=QUERY_FILENAME)
    except SyntaxError as error:
        raise QueryRefusedError(
            f"This is not Python: {error.msg}.", line=error.lineno or 0, column=error.offset or 0
        ) from None


def last_expression(tree: ast.Module) -> ast.expr:
    """The expression a query answers with: its last statement."""
    if not tree.body:
        # Text that is only comments parses to a module with nothing in it.
        raise QueryRefusedError('The query is only comments. Try read("runs").')
    last = tree.body[-1]
    if not isinstance(last, ast.Expr):
        raise QueryRefusedError(
            "A query ends in an expression — the frame it answers with — and this one ends in "
            f"{type(last).__name__.lower()} statement. Put the frame on the last line.",
            line=last.lineno,
            column=last.col_offset + 1,
        )
    return last.value


def execute(job: Job, engine: Engine, session: Session, client: httpx.Client) -> Outcome:
    tree = parse(job.text)
    result = last_expression(tree)
    if job.strict:
        problems = admit(tree, engine.vocabulary())
        if problems:
            raise problems[0]
    reader = Reader(
        session=session,
        catalog=job.catalog,
        client=client,
        aiwatcher=job.aiwatcher,
        corpus_root=job.corpus,
        window_seconds=job.window_seconds,
        as_of=job.as_of,
        input_limit=job.input_limit,
    )
    namespace: dict[str, Any] = {**session.namespace(), "read": reader}
    # A strict query's frame keeps Python's builtins, and it cannot name one: admission
    # refused every name the engine did not give it. An empty `__builtins__` was tried as
    # a second wall and aborted the process inside pyarrow — a C extension reads builtins
    # from the frame that called it — so admission is the one wall, and it is complete.
    where = (result.lineno, result.col_offset + 1)
    started = time.perf_counter()
    try:
        body = ast.Module(body=tree.body[:-1], type_ignores=[])
        # The whole point of `open` admission: the query is Python, run as a notebook's
        # cell is, in a child with ceilings and no credentials (ADR_0028).
        exec(compile(body, QUERY_FILENAME, "exec"), namespace)  # noqa: S102
        value = eval(compile(ast.Expression(result), QUERY_FILENAME, "eval"), namespace)  # noqa: S307
        if not session.is_frame(value):
            raise QueryRefusedError(
                f"A query ends in {engine.frame}, and this one ended in {_described(value)}.",
                line=where[0],
                column=where[1],
            )
        table = session.collect(value, job.max_rows + 1)
    except QueryRefusedError as refused:
        raise refused.at(*_located(refused, where)) from None
    except UpstreamFailedError, AiwatcherUnreachableError:
        raise
    except MemoryError:
        raise QueryRefusedError(
            "The query ran out of memory: its child reached the address-space ceiling "
            "(AIWATCHER_QUERY_MEMORY_MB) or the container's limit.",
            line=where[0],
            column=where[1],
        ) from None
    except SystemExit:
        raise QueryRefusedError(
            "The query called exit(). A query ends in a frame.", line=where[0], column=where[1]
        ) from None
    except Exception as error:  # noqa: BLE001 — anything the query's own code raised is the query's
        line, column = _located(error, where)
        raise QueryRefusedError(
            f"The query failed: {type(error).__name__}: {error}", line=line, column=column
        ) from None
    took_ms = round((time.perf_counter() - started) * 1000)

    truncated = table.num_rows > job.max_rows
    columns, rows = rows_of(table.slice(0, job.max_rows))
    first = reader.reads[0] if reader.reads else None
    return Outcome(
        columns=columns,
        rows=rows,
        truncated=truncated,
        dataset=first.dataset.name if first else None,
        grain=first.dataset.grain if first else None,
        window_seconds=first.window_seconds if first else job.window_seconds,
        windowed=first.dataset.windowed if first else False,
        deterministic=deterministic(tree, reader, engine.volatile, engine.function_by_name),
        took_ms=took_ms,
    )


def deterministic(
    tree: ast.Module,
    reader: Reader,
    volatile: frozenset[str],
    function_by_name: str | None = None,
) -> bool:
    """Whether running this again would answer the same thing.

    Not when it read a corpus — files on disk are addressed by nothing, and the same name
    reads whatever the directory holds today — and not when it called a function whose
    value is the moment it ran. In `open` admission the scan is of what the text says,
    and a query that reaches the clock some other way is its author's claim, as a
    notebook's `deterministic` is.
    """
    if any(plan.dataset.corpus is not None for plan in reader.reads):
        return False
    return not any(
        isinstance(node, ast.Call) and _called(node, function_by_name) in volatile
        for node in ast.walk(tree)
    )


def _called(call: ast.Call, function_by_name: str | None) -> str | None:
    """The function a call calls: its own name, `f.now()`, or — through the constructor
    an engine calls a function by name with — the name it was given, `FunctionExpression
    ("now")`, which DuckDB reads case-insensitively."""
    match call.func:
        case ast.Name(id=name) | ast.Attribute(attr=name):
            pass
        case _:
            return None
    match call.args:
        case [ast.Constant(value=str(function)), *_] if name == function_by_name:
            return function.lower()
    return name


def _located(error: BaseException, fallback: tuple[int, int]) -> tuple[int, int]:
    """Where in the query an exception was raised: the innermost frame that is the query's."""
    if isinstance(error, QueryRefusedError) and error.line:
        return error.line, error.column
    for frame in reversed(traceback.extract_tb(error.__traceback__)):
        if frame.filename == QUERY_FILENAME and frame.lineno:
            return frame.lineno, (frame.colno or 0) + 1
    return fallback


def _described(value: object) -> str:
    shown = repr(value)
    kind = type(value).__qualname__
    return f"{kind} {shown}" if len(shown) <= 60 else f"a {kind}"
