"""The rows a notebook block reads, on disk, under a name both sides can compute.

A notebook here is two things at once: a step in a curation chain, and a live
app somebody is editing while the chain is being built. Those two need the same
rows or the second is useless — developing a detector against rows that are not
the rows it will run on is how a block passes every preview and fails on the
first real batch.

So there is one file per notebook and both sides use it. A run stages the rows
it was given before it runs them; the live app reads the same file. Open a
block's editor after a preview and the notebook is showing the rows the chain
actually produced. Nothing here is durable and nothing here is a dataset: a staged file
is scratch, it is overwritten by the next stage, and the only artifact that
outlives a session is the dataset version the View block publishes through the
Rust registry.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

import orjson

Row = dict[str, Any]

# The same shape a notebook file's name has, because the directory is named
# after the notebook: anything else and a staged path could point out of the
# staging root.
NAME = re.compile(r"^[a-z][a-z0-9_]{0,63}$")

MAX_STAGED_BYTES = 64 * 1024 * 1024


class StagingError(ValueError):
    """A staged file that cannot be written, read, or trusted."""


@dataclass(frozen=True)
class StagedInput:
    """What the previous block handed to this one."""

    notebook: str
    rows: list[Row] = field(default_factory=list)
    columns: list[str] = field(default_factory=list)
    params: dict[str, Any] = field(default_factory=dict)
    staged_at: str | None = None

    @property
    def is_staged(self) -> bool:
        """Whether anything has ever been staged for this notebook."""
        return self.staged_at is not None


@dataclass(frozen=True)
class Staging:
    """One directory holding every block's scratch rows."""

    root: Path

    def input_path(self, notebook: str) -> Path:
        return self._directory(notebook) / "input.json"

    def output_path(self, notebook: str) -> Path:
        return self._directory(notebook) / "output.json"

    def stage(
        self,
        notebook: str,
        rows: list[Row],
        columns: list[str] | None = None,
        params: dict[str, Any] | None = None,
    ) -> StagedInput:
        """Write the rows this notebook is to read, replacing whatever was there."""
        staged = StagedInput(
            notebook=_checked(notebook),
            rows=rows,
            columns=columns if columns is not None else columns_of(rows),
            params=params or {},
            staged_at=datetime.now(UTC).isoformat(),
        )
        path = self.input_path(staged.notebook)
        path.parent.mkdir(parents=True, exist_ok=True)
        write_json(
            path,
            {
                "notebook": staged.notebook,
                "staged_at": staged.staged_at,
                "params": staged.params,
                "columns": staged.columns,
                "rows": staged.rows,
            },
        )
        return staged

    def get_input(self, notebook: str) -> StagedInput:
        """The staged rows, or an empty one when nothing has been staged yet."""
        return read_input(self.input_path(_checked(notebook)), notebook)

    def _directory(self, notebook: str) -> Path:
        return self.root / "blocks" / _checked(notebook)


def read_input(path: Path, notebook: str) -> StagedInput:
    """Read one staged file. A missing file is a normal state, not a failure."""
    if not path.is_file():
        return StagedInput(notebook=notebook)
    body = read_json(path)
    if not isinstance(body, dict):
        raise StagingError(f"{path} is not a staged input document")
    rows = body.get("rows")
    columns = body.get("columns")
    params = body.get("params")
    return StagedInput(
        notebook=str(body.get("notebook") or notebook),
        rows=[row for row in rows if isinstance(row, dict)] if isinstance(rows, list) else [],
        columns=[str(name) for name in columns] if isinstance(columns, list) else [],
        params=params if isinstance(params, dict) else {},
        staged_at=str(body["staged_at"]) if body.get("staged_at") else None,
    )


def columns_of(rows: list[Row]) -> list[str]:
    """Every key any row carries, in the order the rows first mention it.

    First-seen order rather than sorted: a table whose columns are alphabetical
    reads as a different table from the one the previous block showed, and the
    row order it came in is the one thing the notebook did not choose.
    """
    names: list[str] = []
    seen: set[str] = set()
    for row in rows:
        for name in row:
            if name not in seen:
                seen.add(name)
                names.append(name)
    return names


def write_json(path: Path, body: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    # Written whole and then moved: a notebook reading the file while a stage is
    # half-written would see truncated JSON and blame its own parsing.
    temporary = path.with_suffix(".partial")
    # `default=str` is the one liberty taken with the encoding: a notebook that
    # hands on a datetime or a Decimal has produced something a row can hold,
    # and refusing it at the door would be refusing it after the work.
    temporary.write_bytes(orjson.dumps(body, default=str))
    temporary.replace(path)


def read_json(path: Path) -> Any:
    size = path.stat().st_size
    if size > MAX_STAGED_BYTES:
        raise StagingError(f"{path} is {size} bytes; the limit is {MAX_STAGED_BYTES}")
    try:
        return orjson.loads(path.read_bytes())
    except orjson.JSONDecodeError as error:
        raise StagingError(f"{path} is not JSON: {error}") from error


def _checked(notebook: str) -> str:
    if not NAME.match(notebook):
        raise StagingError(
            f"'{notebook}' is not a notebook name: "
            "a name is lower-case letters, digits and underscores, starting with a letter"
        )
    return notebook
