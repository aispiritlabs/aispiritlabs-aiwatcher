"""The notebook files, and the two things that are checked before one is written.

A notebook here is the source of its own code. The panel edits it through these
routes rather than holding a copy: the file is what marimo serves, what a run
executes and what a test imports, and a block in a saved pipeline names it and
pins the revision it was saved against. One place to change it, one digest to
compare.

What that buys has to be paid for at the door, because a file that marimo
cannot open takes the live app down for everybody looking at it — and the panel
is then the only way back. So a save is refused unless the source **parses**
and unless it **is** a marimo notebook. Neither check runs the code: the first
compiles it, the second reads its syntax tree.
"""

from __future__ import annotations

import ast
import hashlib
import re
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path

NAME = re.compile(r"^[a-z][a-z0-9_]{0,63}$")
MAX_SOURCE_BYTES = 256 * 1024


class NotebookNotFoundError(LookupError):
    """A name no file answers to."""


class NotebookRejectedError(ValueError):
    """A source that was not written, and why."""


@dataclass(frozen=True)
class NotebookSummary:
    """A notebook as a list shows it."""

    name: str
    title: str
    revision: str
    size: int
    modified_at: str


@dataclass(frozen=True)
class Notebook:
    """One notebook, with its source."""

    name: str
    title: str
    revision: str
    source: str
    path: Path
    modified_at: str

    @property
    def summary(self) -> NotebookSummary:
        return NotebookSummary(
            name=self.name,
            title=self.title,
            revision=self.revision,
            size=len(self.source.encode("utf-8")),
            modified_at=self.modified_at,
        )


@dataclass(frozen=True)
class NotebookDirectory:
    """The directory the service serves, runs and edits."""

    root: Path

    def get_notebooks(self) -> list[NotebookSummary]:
        """Every notebook, by name."""
        if not self.root.is_dir():
            return []
        found = []
        for path in sorted(self.root.glob("*.py")):
            if not NAME.match(path.stem):
                continue
            found.append(self._read(path).summary)
        return found

    def get_notebook(self, name: str) -> Notebook:
        path = self.path_of(name)
        if not path.is_file():
            raise NotebookNotFoundError(f"there is no notebook called '{name}'")
        return self._read(path)

    def save_notebook(self, name: str, source: str) -> Notebook:
        """Write a notebook, after checking it is one."""
        path = self.path_of(name)
        size = len(source.encode("utf-8"))
        if size > MAX_SOURCE_BYTES:
            raise NotebookRejectedError(
                f"the notebook is {size} bytes; the limit is {MAX_SOURCE_BYTES}"
            )
        check_is_a_notebook(name, source)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(source, encoding="utf-8")
        return self._read(path)

    def path_of(self, name: str) -> Path:
        """Where a notebook of this name lives, refusing anything that is not one."""
        if not NAME.match(name):
            raise NotebookRejectedError(
                f"'{name}' is not a notebook name: lower-case letters, digits and "
                "underscores, starting with a letter"
            )
        path = (self.root / f"{name}.py").resolve()
        # Belt and braces behind the pattern: the pattern already excludes a
        # separator and a dot, and this is the check that stays correct if the
        # pattern is ever widened.
        if path.parent != self.root.resolve():
            raise NotebookRejectedError(f"'{name}' resolves outside the notebook directory")
        return path

    def _read(self, path: Path) -> Notebook:
        source = path.read_text(encoding="utf-8")
        return Notebook(
            name=path.stem,
            title=title_of(source, path.stem),
            revision=revision_of(source),
            source=source,
            path=path,
            modified_at=datetime.fromtimestamp(path.stat().st_mtime, tz=UTC).isoformat(),
        )


def revision_of(source: str) -> str:
    """The identity of this exact source. What a saved pipeline block pins."""
    return hashlib.sha256(source.encode("utf-8")).hexdigest()


def title_of(source: str, fallback: str) -> str:
    """The first line of the module docstring, or the name."""
    try:
        module = ast.parse(source)
    except SyntaxError:
        return fallback
    docstring = ast.get_docstring(module)
    if not docstring:
        return fallback
    first = docstring.strip().splitlines()[0].strip()
    return first or fallback


def check_is_a_notebook(name: str, source: str) -> None:
    """Refuse a source marimo could not serve, saying which half is missing.

    Two failures, reported apart because their fixes are different: a syntax
    error is a typo somebody can see the line of, and a file with no `app` is
    usually a plain script somebody pasted in — it is valid Python, it would be
    written without complaint, and the app view would then be blank with the
    reason in a server log.
    """
    try:
        module = ast.parse(source, filename=f"{name}.py")
    except SyntaxError as error:
        where = f" at line {error.lineno}" if error.lineno else ""
        raise NotebookRejectedError(f"the notebook does not parse{where}: {error.msg}") from error

    if not any(_assigns_app(statement) for statement in module.body):
        raise NotebookRejectedError(
            "the notebook does not create a marimo app: it needs a module-level "
            "`app = marimo.App()` and cells decorated with `@app.cell`"
        )


def _assigns_app(statement: ast.stmt) -> bool:
    targets: list[ast.expr] = []
    if isinstance(statement, ast.Assign):
        targets = list(statement.targets)
    elif isinstance(statement, ast.AnnAssign):
        targets = [statement.target]
    return any(isinstance(target, ast.Name) and target.id == "app" for target in targets)
