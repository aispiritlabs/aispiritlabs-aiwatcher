"""The rows a notebook block reads, on disk, under a name both sides can compute.

A notebook here is two things at once: a step in a curation chain, and a live
app somebody is editing while the chain is being built. Those two need the same
rows or the second is useless — developing a detector against rows that are not
the rows it will run on is how a block passes every preview and fails on the
first real batch.

So a run stages the rows it was given before it runs them, and the live app
reads what was staged. Open a block's editor after a preview and the notebook is
showing the rows the chain actually produced. Nothing here is durable and
nothing here is a dataset: a staged file is scratch, it is overwritten by the
next stage, and the only artifact that outlives a session is the dataset version
the View block publishes through the Rust registry.

## Why a context, and why there is still a `latest`

One file per *notebook* was the first shape, and it is wrong as soon as two
pipelines use one notebook: each run overwrites the other's rows, and a block
that passed its preview reads somebody else's table on the next one. So a run
stages under its **context** — `<execution>/<step>/<attempt>` for a managed run,
which is what `ActivityContext::context_id` carries.

That alone would break the thing this module exists for, because the live app
knows a notebook's name and nothing else. `latest` is the join: a stage writes
its own context and then points `latest` at it, and the editor follows the
pointer. Written in that order, always — a `latest` naming a context that was
never written is a broken editor, while an unreferenced staged file is a few
kilobytes nobody reads.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import UTC, datetime
from hashlib import sha256
from pathlib import Path
from typing import Any

import orjson

Row = dict[str, Any]

# The same shape a notebook file's name has, because the directory is named
# after the notebook: anything else and a staged path could point out of the
# staging root.
NAME = re.compile(r"^[a-z][a-z0-9_]{0,63}$")

#: What a context directory may be called: the hash, or the reserved word.
SLUG = re.compile(r"^([0-9a-f]{32}|adhoc)$")

MAX_STAGED_BYTES = 64 * 1024 * 1024

#: What a stage with no context of its own is filed under.
#:
#: Every query the panel sends. Named rather than blank so the directory says
#: what it holds, and reserved so a real context id can never land on it — a
#: context is hashed, and a hash is never this word.
ADHOC = "adhoc"


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
    #: Which run's rows these are. `ADHOC` for anything the panel staged.
    context: str = ""

    @property
    def is_staged(self) -> bool:
        """Whether anything has ever been staged for this notebook."""
        return self.staged_at is not None


@dataclass(frozen=True)
class StagedTotals:
    """What the staging root is holding, right now.

    Nothing here cleans up. A stage overwrites its own context and leaves every
    other one where it is, so this directory only grows — one context per
    attempt of every managed step that reached a notebook, for as long as the
    volume lasts. Workflow retention is in another process and prunes
    executions, not this disk, and it could not reach here if it wanted to.

    So the question "does that matter yet" is a question about a number nobody
    had. This is the number: a reference-aware collector is worth building when
    the curve says so and not before.
    """

    contexts: int
    files: int
    bytes: int
    #: Whole days since the least recently written staged file. `None` when
    #: there is nothing staged — which is different from "everything here is
    #: from today", and a zero would say the second.
    oldest_days: int | None


@dataclass(frozen=True)
class Staging:
    """One directory holding every block's scratch rows."""

    root: Path

    def measure(self, now: datetime | None = None) -> StagedTotals:
        """Walk what is staged and say how much of it there is.

        On demand rather than on a health poll: this is a `stat` per staged
        file, and the number it answers moves over days. Somebody graphing it
        asks for it; nothing should pay for it by accident.
        """
        at = now or datetime.now(UTC)
        blocks = self.root / "blocks"
        contexts = 0
        files = 0
        total = 0
        oldest: float | None = None
        # `latest` pointers sit beside the context directories and are counted
        # as neither: they are one small file per notebook and they are the one
        # thing here that is rewritten rather than accumulated.
        for notebook in sorted(blocks.glob("*")) if blocks.is_dir() else []:
            if not notebook.is_dir():
                continue
            for context in sorted(notebook.glob("*")):
                if not context.is_dir() or not SLUG.match(context.name):
                    continue
                contexts += 1
                for staged in context.glob("*.json"):
                    stat = staged.stat()
                    files += 1
                    total += stat.st_size
                    oldest = stat.st_mtime if oldest is None else min(oldest, stat.st_mtime)
        age = None
        if oldest is not None:
            since = at - datetime.fromtimestamp(oldest, UTC)
            age = max(0, int(since.total_seconds() // 86400))
        return StagedTotals(contexts=contexts, files=files, bytes=total, oldest_days=age)

    def input_path(self, notebook: str, context: str | None = None) -> Path:
        return self._directory(notebook, context) / "input.json"

    def output_path(self, notebook: str, context: str | None = None) -> Path:
        return self._directory(notebook, context) / "output.json"

    def stage(
        self,
        notebook: str,
        rows: list[Row],
        columns: list[str] | None = None,
        params: dict[str, Any] | None = None,
        context: str | None = None,
    ) -> StagedInput:
        """Write the rows this notebook is to read for this context.

        Replaces whatever that context held, and leaves every other context
        alone — which is the difference between two pipelines sharing a
        notebook and two pipelines fighting over one.
        """
        staged = StagedInput(
            notebook=_checked(notebook),
            rows=rows,
            columns=columns if columns is not None else columns_of(rows),
            params=params or {},
            staged_at=datetime.now(UTC).isoformat(),
            context=context or ADHOC,
        )
        write_json(
            self.input_path(staged.notebook, context),
            {
                "notebook": staged.notebook,
                "staged_at": staged.staged_at,
                "context": staged.context,
                "params": staged.params,
                "columns": staged.columns,
                "rows": staged.rows,
            },
        )
        # And only then the pointer the editor follows. The other order leaves
        # `latest` naming rows that were never written.
        write_json(
            self._latest_path(staged.notebook),
            {"slug": _slug(context), "context": staged.context, "at": staged.staged_at},
        )
        return staged

    def get_input(self, notebook: str, context: str | None = None) -> StagedInput:
        """The staged rows, or an empty one when nothing has been staged yet.

        With no context — which is how the live app asks, because a notebook
        file knows its own name and nothing else — this follows `latest`.
        """
        notebook = _checked(notebook)
        slug = _slug(context) if context is not None else self._latest_slug(notebook)
        return read_input(self._at(notebook, slug) / "input.json", notebook)

    def _latest_path(self, notebook: str) -> Path:
        return self.root / "blocks" / _checked(notebook) / "latest.json"

    def _latest_slug(self, notebook: str) -> str:
        """Which context the editor should open on."""
        path = self._latest_path(notebook)
        if not path.is_file():
            return _slug(None)
        body = read_json(path)
        slug = body.get("slug") if isinstance(body, dict) else None
        # A pointer that is not a slug is a pointer nobody wrote: fall back to
        # the ad-hoc directory rather than refusing to open an editor.
        return slug if isinstance(slug, str) and SLUG.match(slug) else _slug(None)

    def _directory(self, notebook: str, context: str | None = None) -> Path:
        return self._at(_checked(notebook), _slug(context))

    def _at(self, notebook: str, slug: str) -> Path:
        return self.root / "blocks" / notebook / slug


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
        context=str(body.get("context") or ADHOC),
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


def _slug(context: str | None) -> str:
    """A directory name for a context id.

    Hashed rather than sanitised: a context is `<execution>/<step>/<attempt>`,
    so it holds separators, and a path built by replacing them would be one
    somebody could aim outside the staging root. A hash cannot be, and it
    cannot collide with [`ADHOC`] either.
    """
    if context is None or not context:
        return ADHOC
    return sha256(context.encode()).hexdigest()[:32]


def _checked(notebook: str) -> str:
    if not NAME.match(notebook):
        raise StagingError(
            f"'{notebook}' is not a notebook name: "
            "a name is lower-case letters, digits and underscores, starting with a letter"
        )
    return notebook
