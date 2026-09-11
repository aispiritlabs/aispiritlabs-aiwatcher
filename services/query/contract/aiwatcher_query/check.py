"""What is wrong with a query, without running it.

Two readers, as Flow has two. Python's own parser says where the text stops being
Python, with the line and column it reports. The catalog then reads every `read()`
whose arguments are written as literals — which is nearly every one — through the same
`plan_read` a run uses, so a dataset that does not exist or an argument its route does
not take is found before anything runs, in the words the run would have used.

Nothing here executes the query. In `open` admission that is also the limit of what a
check can say: whether `col("spanid")` names a column is the engine's to discover, and
it does so when the query runs. In `strict` admission the check also walks the text
through admission, so a refusal is found here, with every other one, before a run.
"""

from __future__ import annotations

import ast
from typing import Any

from aiwatcher_query.admission import Vocabulary, admit
from aiwatcher_query.catalog import Catalog
from aiwatcher_query.errors import QueryRefusedError
from aiwatcher_query.evaluate import last_expression, parse
from aiwatcher_query.reading import plan_read


def check(text: str, catalog: Catalog, vocabulary: Vocabulary | None = None) -> dict[str, Any]:
    """Always an answer: "this query is wrong" is a successful check, not a failed request."""
    try:
        tree = parse(text)
    except QueryRefusedError as refused:
        return {"ok": False, "diagnostics": [diagnostic(text, refused)], "checked_by": ["python"]}

    diagnostics: list[dict[str, Any]] = []
    try:
        last_expression(tree)
    except QueryRefusedError as refused:
        diagnostics.append(diagnostic(text, refused))
    if vocabulary is not None:
        diagnostics.extend(diagnostic(text, refused) for refused in admit(tree, vocabulary))
    for call in _literal_reads(tree):
        name, arguments = call
        try:
            plan_read(catalog, name.value, arguments, None)
        except QueryRefusedError as refused:
            diagnostics.append(diagnostic(text, refused.at(name.lineno, name.col_offset + 1)))
    return {
        "ok": not diagnostics,
        "diagnostics": diagnostics,
        "checked_by": ["python", "aiwatcher", *(["strict"] if vocabulary else [])],
    }


def diagnostic(text: str, refused: QueryRefusedError) -> dict[str, Any]:
    """One finding, in the panel's shape: `offset` into the text, and Python's line and column."""
    return {
        "level": "error",
        "message": refused.message,
        "offset": offset_of(text, refused.line, refused.column),
        "line": refused.line,
        "column": refused.column,
        "help": None,
    }


def offset_of(text: str, line: int, column: int) -> int:
    """A 1-based line and column as a 0-based offset into the text; 0 when unknown."""
    if line < 1:
        return 0
    lines = text.splitlines(keepends=True)
    before = sum(len(one) for one in lines[: line - 1])
    return before + max(column - 1, 0)


def _literal_reads(tree: ast.Module) -> list[tuple[ast.Constant, dict[str, object]]]:
    """Every `read("name", key=literal, …)` whose whole call is literals.

    A call with a computed argument is left for the run: guessing its value here would
    be a second evaluator, and one that disagreed with Python.
    """
    found: list[tuple[ast.Constant, dict[str, object]]] = []
    for node in ast.walk(tree):
        if not (
            isinstance(node, ast.Call)
            and isinstance(node.func, ast.Name)
            and node.func.id == "read"
            and len(node.args) == 1
            and isinstance(node.args[0], ast.Constant)
            and all(
                keyword.arg is not None and isinstance(keyword.value, ast.Constant)
                for keyword in node.keywords
            )
        ):
            continue
        arguments: dict[str, object] = {
            str(keyword.arg): keyword.value.value
            for keyword in node.keywords
            if isinstance(keyword.value, ast.Constant)
        }
        found.append((node.args[0], arguments))
    return found
