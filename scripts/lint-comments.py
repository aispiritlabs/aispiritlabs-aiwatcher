#!/usr/bin/env python3
"""Keep code comments short and free of references a reader cannot follow.

Two rules, both about the same thing: a comment explains the code beside it.

  1. A comment block is at most MAX_LINES lines of prose. Fenced blocks — a
     diagram, a key layout, a table of routes — are not counted, because they
     carry structure rather than argument. Anything longer is an essay, and an
     essay about a decision belongs in docs/ADR/.

  2. No references to a plan's sections, phases or review ids. They point at
     moving documents a reader of the code cannot open, and they rot silently
     — the document is edited, the comment is not, and nobody finds out. Say
     the rule the section decided, and cite an ADR when one decided it.

Python gets the second rule and not the first: a module docstring is where this
repository explains a module, and its length is the house style there.

Usage: scripts/lint-comments.py [--list]
"""

from __future__ import annotations

import ast
import io
import re
import subprocess
import sys
import tokenize
from pathlib import Path

MAX_LINES = 25

# Per language: the comment markers, and whether a doc block has a terminator
# that is not itself prose.
MARKERS = {
    ".rs": ("//!", "///", "//"),
    ".php": ("/**", "*/", "*", "//", "#"),
    ".ts": ("/**", "*/", "*", "//"),
    ".tsx": ("/**", "*/", "*", "//"),
}
# Checked for stale references only. Neither a docstring nor a comment is marked
# on every line, so these are found by parsing rather than by a leading marker.
PROSE_ONLY = (".py",)
SKIP = ("apps/panel/src/api/generated/", "routeTree.gen.ts")
ROT = re.compile(
    r"(?:[Ss]ection|§)\s*[0-9]+(?:\.[0-9]+)*"
    r"|[Pp]hase\s+[0-9]+"
    r"|[Rr]eview\s+[A-Z][0-9]+"
    r"|plan\.md",
)


def marker_of(line: str, markers: tuple[str, ...]) -> str | None:
    stripped = line.strip()
    for marker in markers:
        if stripped.startswith(marker):
            return marker
    return None


def blocks(lines: list[str], markers: tuple[str, ...]):
    """Yield (marker, start_line, prose_line_count) for each comment block.

    Adjacent markers of one language are one block: a PHP docblock opens with
    `/**` and continues with `*`, and `//` beside `///` in Rust is still one
    stretch of prose a reader has to get through.
    """
    kind, start, count, fenced = None, 0, 0, False
    for number, line in enumerate(lines, 1):
        marker = marker_of(line, markers)
        if (marker is None) != (kind is None):
            if kind is not None:
                yield kind, start, count
            kind, start, count, fenced = marker, number, 0, False
        if marker is None:
            continue
        kind = kind or marker
        body = line.strip()[len(marker) :].strip()
        if body.startswith("```"):
            fenced = not fenced
        elif not fenced:
            count += 1
    if kind is not None:
        yield kind, start, count


def python_prose_lines(source: str) -> set[int]:
    """The line numbers of a Python file's comments and docstrings.

    A docstring's middle lines start with prose rather than a marker, so the
    tree says where each one is; a `#` comment comes from the tokenizer, which
    also keeps a `#` inside a string from reading as one.
    """
    lines = {
        token.start[0]
        for token in tokenize.generate_tokens(io.StringIO(source).readline)
        if token.type == tokenize.COMMENT
    }
    owners = (ast.Module, ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)
    for node in ast.walk(ast.parse(source)):
        if not isinstance(node, owners) or not node.body:
            continue
        first = node.body[0]
        if (
            isinstance(first, ast.Expr)
            and isinstance(first.value, ast.Constant)
            and isinstance(first.value.value, str)
        ):
            lines.update(range(first.lineno, (first.end_lineno or first.lineno) + 1))
    return lines


def tracked_files() -> list[Path]:
    globs = [f"*{suffix}" for suffix in (*MARKERS, *PROSE_ONLY)]
    out = subprocess.run(
        ["git", "ls-files", *globs], capture_output=True, text=True, check=True
    ).stdout
    # `ls-files` lists the index, which still holds a file deleted or moved in
    # the working tree until the change is staged. There is nothing left of one
    # to lint, and crashing on it would fail every run of `just check` between a
    # `mv` and a commit.
    return [
        Path(p) for p in out.split() if not any(s in p for s in SKIP) and Path(p).exists()
    ]


def main() -> int:
    listing = "--list" in sys.argv
    long_blocks: list[tuple[Path, int, str, int]] = []
    refs: list[tuple[Path, int, str]] = []
    unparsed: list[tuple[Path, str]] = []

    for path in tracked_files():
        source = path.read_text(encoding="utf-8", errors="replace")
        lines = source.splitlines()
        if path.suffix in PROSE_ONLY:
            try:
                prose = python_prose_lines(source)
            except (SyntaxError, tokenize.TokenError) as error:
                # Skipping it would pass a file nobody checked, which reads as
                # one that was clean.
                unparsed.append((path, str(error)))
                continue
            for number in sorted(prose):
                if ROT.search(lines[number - 1]):
                    refs.append((path, number, lines[number - 1].strip()))
            continue
        markers = MARKERS[path.suffix]
        for kind, start, count in blocks(lines, markers):
            if count > MAX_LINES:
                long_blocks.append((path, start, kind, count))
        for number, line in enumerate(lines, 1):
            if marker_of(line, markers) and ROT.search(line):
                refs.append((path, number, line.strip()))

    failed = False

    if unparsed:
        failed = True
        version = ".".join(str(part) for part in sys.version_info[:2])
        print(f"Python files that {version} cannot parse:", file=sys.stderr)
        for path, reason in unparsed:
            print(f"  {path}  {reason}", file=sys.stderr)
        print(file=sys.stderr)

    if long_blocks:
        failed = True
        print(f"comment blocks over {MAX_LINES} lines of prose:", file=sys.stderr)
        for path, start, kind, count in sorted(long_blocks, key=lambda b: -b[3]):
            print(f"  {path}:{start}  {kind}  {count} lines", file=sys.stderr)
        print(
            "\n  A comment explains the code beside it. Argument, history and "
            "rejected\n  alternatives belong in docs/ADR/. Diagrams do not "
            "count — fence them.\n",
            file=sys.stderr,
        )

    if refs:
        failed = True
        print(
            f"{len(refs)} reference(s) to a plan's sections, phases or reviews:",
            file=sys.stderr,
        )
        print(
            "  They point at moving documents a reader of the code cannot open,"
            "\n  and they rot without anyone finding out. Say the rule instead,"
            "\n  and cite an ADR if one decided it.\n",
            file=sys.stderr,
        )
        listing = True

    if listing:
        for path, number, text in refs:
            print(f"  {path}:{number}  {text}", file=sys.stderr)

    if not failed:
        print(f"comments ok: no block over {MAX_LINES} lines, no stale references")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
