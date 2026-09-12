#!/usr/bin/env python3
"""Stage an evaluation bundle as the approval it would be admitted under.

An approval is addressed by the pair it admits, so its directory is named by
that address rather than by anything a person chose. That is what lets a second
variant be a second directory instead of a swap that hides the first, and it is
why this script exists: the name is a digest nobody can type.

The digest is asked of the binary rather than recomputed here. Writing the
normalisation a second time in Python looked easy and was wrong on the first
try — the fixture's own JSON is not what `serde` serialises the validated
structure back to — and a staging script that puts a bundle in the wrong
directory produces a 403 that reads as a permissions problem.

    scripts/stage-evaluation-approval.py <destination> [bundle-directory]
"""

from __future__ import annotations

import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / "contracts/fixtures/evaluation-v1"


def approval_id(manifest: Path) -> str:
    """Ask `aiwatcher-evaluation` what would admit this declaration."""
    printed = subprocess.run(
        [
            "cargo",
            "run",
            "--quiet",
            "--package",
            "aiwatcher-evaluation",
            "--example",
            "prepare",
            "--",
            str(manifest),
        ],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    for line in printed.splitlines():
        if line.startswith("approval_id:"):
            return line.split(":", 1)[1].strip()
    raise SystemExit(f"prepare printed no approval id for {manifest}")


def main(argv: list[str]) -> int:
    destination = Path(argv[0] if argv else ".data/evaluation-approvals")
    source = Path(argv[1]) if len(argv) > 1 else FIXTURE
    bundle = destination / approval_id(source / "manifest.json")
    bundle.mkdir(parents=True, exist_ok=True)
    for entry in sorted(source.iterdir()):
        if entry.is_file():
            shutil.copy2(entry, bundle / entry.name)
    print(f"staged {source.name} as {bundle}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
