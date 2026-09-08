"""The notebook files, their history, and the two checks a save has to pass.

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

## The head is editable; a revision is not

There are two directories, and the split is the point. The **head** is the one
file per name that marimo serves and a person edits. A **revision** is one
exact source, named by its own `sha256`, written once — because a digest names
exactly one byte string, so writing it again writes the same bytes.

A managed run pins a revision and now resolves *that*, so editing a notebook no
longer strands every execution that came before the edit. Until this, the
runtime computed the head's digest and refused a run whose pin no longer
matched: provenance was protected by making the old run unrepeatable, which is
a strange thing to call protection.

Revisions live **outside** the notebook root on purpose. marimo's dynamic
directory turns every file under that root into a live app, and a hundred
copies of one notebook is not what somebody opening the picker wants to find —
nor is the history something the panel should offer to edit.

Two orderings hold, both for the reason ADR_0011 gives. A revision is written
**before** the head that names it: crash between them and there is an
unreferenced revision nobody minds, where the other order leaves a head whose
digest names nothing and a pipeline saved against it that can never run. And
`keep_current` runs at start-up, so a directory that predates any of this keeps
what it holds *now* — what it held before that is genuinely gone, and nothing
here can invent it.
"""

from __future__ import annotations

import ast
import hashlib
import os
import re
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path

NAME = re.compile(r"^[a-z][a-z0-9_]{0,63}$")
REVISION = re.compile(r"^[0-9a-f]{64}$")
MAX_SOURCE_BYTES = 256 * 1024


class NotebookNotFoundError(LookupError):
    """A name — or a name and a revision — that no file answers to."""


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
    """The directory the service serves, runs and edits, and the history beside it."""

    root: Path
    #: Where one exact source is kept forever, as `<name>/<sha256>.py`. Outside
    #: `root`, because everything under that root becomes a live marimo app.
    revisions: Path

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
        """Write a notebook, after checking it is one.

        The revision first, then the head: a head whose digest names no
        revision is a pipeline that cannot be saved against it, and an
        unreferenced revision is a file nobody looks at.
        """
        path = self.path_of(name)
        size = len(source.encode("utf-8"))
        if size > MAX_SOURCE_BYTES:
            raise NotebookRejectedError(
                f"the notebook is {size} bytes; the limit is {MAX_SOURCE_BYTES}"
            )
        check_is_a_notebook(name, source)
        self.keep(name, source)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(source, encoding="utf-8")
        return self._read(path)

    def get_revision(self, name: str, revision: str) -> Notebook:
        """The exact source a plan pinned, whatever the head says now.

        This is what a managed run executes. A revision this directory does not
        hold is a `NotebookNotFoundError` rather than a fallback to the head —
        running *something else* under a pinned run's name is the one outcome
        content addressing exists to prevent.
        """
        path = self.revision_path(name, revision)
        if not path.is_file():
            raise NotebookNotFoundError(
                f"'{name}' has no revision {revision[:12]} here. It was either saved "
                "before this runtime kept its history, or the revision directory was "
                "replaced; saving the notebook again pins what is there now"
            )
        # The logical name, not the digest the file is called by: staging, the
        # app URL and every log line are about the notebook, not the blob.
        return self._read(path, name=name)

    def keep(self, name: str, source: str) -> str:
        """Write one source into the history, and answer to what it is named.

        Idempotent by construction rather than by a check: the path *is* the
        digest of these bytes, so a second write writes the same file. Written
        through a temporary file and renamed, so a reader never opens a half of
        one.
        """
        revision = revision_of(source)
        path = self.revision_path(name, revision)
        if path.is_file():
            return revision
        path.parent.mkdir(parents=True, exist_ok=True)
        pending = path.with_suffix(f".{os.getpid()}.pending")
        pending.write_text(source, encoding="utf-8")
        pending.replace(path)
        return revision

    def keep_current(self) -> list[str]:
        """Put every head this directory holds into the history, and say which.

        Run at start-up. It is what makes an upgrade keep the sources that are
        there, and it is deliberately not able to do more than that: the
        revisions before the current one were never written down.
        """
        kept = []
        for summary in self.get_notebooks():
            notebook = self.get_notebook(summary.name)
            if not self.revision_path(notebook.name, notebook.revision).is_file():
                self.keep(notebook.name, notebook.source)
                kept.append(notebook.name)
        return kept

    def revision_path(self, name: str, revision: str) -> Path:
        """Where one exact source lives, refusing anything that is not a digest."""
        if not NAME.match(name):
            raise NotebookRejectedError(
                f"'{name}' is not a notebook name: lower-case letters, digits and "
                "underscores, starting with a letter"
            )
        if not REVISION.match(revision):
            raise NotebookRejectedError(
                f"'{revision}' is not a revision: a notebook revision is a sha256 "
                "digest, 64 lower-case hexadecimal characters"
            )
        # Both halves are already pattern-matched, so neither can hold a
        # separator; this is the check that survives either pattern widening.
        path = (self.revisions / name / f"{revision}.py").resolve()
        if path.parent.parent != self.revisions.resolve():
            raise NotebookRejectedError(f"'{name}' resolves outside the revision directory")
        return path

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

    def _read(self, path: Path, name: str | None = None) -> Notebook:
        source = path.read_text(encoding="utf-8")
        return Notebook(
            name=name or path.stem,
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
