#!/usr/bin/env python3
"""Keep code comments short and free of references a reader cannot follow.

Two rules, both about the same thing: a comment explains the code beside it.

  1. A comment block is at most MAX_LINES lines of prose. Fenced blocks — a
     diagram, a key layout, a table of routes — are not counted, because they
     carry structure rather than argument. Anything longer is an essay, and an
     essay about a decision belongs in docs/ADR/.

  2. No references to plan.md sections, phases or review ids. They point at
     moving documents a reader of the code cannot open, and they rot silently
     — the document is edited, the comment is not, and nobody finds out. Say
     the rule the section decided, and cite an ADR when one decided it.

Usage: scripts/lint-comments.py [--list]
"""

from __future__ import annotations

import re
import subprocess
import sys
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


def tracked_files() -> list[Path]:
    globs = [f"*{suffix}" for suffix in MARKERS]
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

    for path in tracked_files():
        markers = MARKERS[path.suffix]
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
        for kind, start, count in blocks(lines, markers):
            if count > MAX_LINES:
                long_blocks.append((path, start, kind, count))
        for number, line in enumerate(lines, 1):
            if marker_of(line, markers) and ROT.search(line):
                refs.append((path, number, line.strip()))

    failed = False

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
            f"{len(refs)} reference(s) to plan.md sections, phases or reviews:",
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
