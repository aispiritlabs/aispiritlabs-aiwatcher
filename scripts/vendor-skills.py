#!/usr/bin/env python3
"""Vendor the agent skills listed in `.claude/skills/vendor.json`.

Skills are copied in rather than fetched on demand, for the reason the prompt
registry is an object store rather than a fold: what an agent read has to stay
readable later. A pin is a commit, so `--check` can say whether the working
tree still is what the manifest claims — and `--update` is the only thing that
moves a pin, so a refresh is a reviewable diff rather than a surprise.

    ./scripts/vendor-skills.py            # re-vendor at the pinned commits
    ./scripts/vendor-skills.py --check    # is the tree what the manifest says?
    ./scripts/vendor-skills.py --update   # move every pin to upstream HEAD
"""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SKILLS = ROOT / ".claude" / "skills"
MANIFEST = SKILLS / "vendor.json"
LICENSES = SKILLS / "licenses"

# Everything under .claude/skills that this script does not own.
NOT_VENDORED = {"vendor.json", "README.md", "licenses"}


def run(*args: str, cwd: Path | None = None) -> str:
    result = subprocess.run(
        args, cwd=cwd, check=True, capture_output=True, text=True
    )
    return result.stdout.strip()


def slug(repo: str) -> str:
    return repo.rstrip("/").removeprefix("https://github.com/").replace("/", "_")


def fetch(repo: str, commit: str, into: Path) -> Path:
    """Check `repo` out at `commit` under `into`, without cloning its history."""
    work = into / slug(repo)
    work.mkdir(parents=True)
    run("git", "init", "--quiet", cwd=work)
    run("git", "remote", "add", "origin", repo, cwd=work)
    run("git", "fetch", "--quiet", "--depth", "1", "origin", commit, cwd=work)
    run("git", "checkout", "--quiet", "FETCH_HEAD", cwd=work)
    return work


def head_of(repo: str) -> str:
    for line in run("git", "ls-remote", repo, "HEAD").splitlines():
        return line.split()[0]
    raise SystemExit(f"{repo} has no HEAD")


def place(source: Path, destination: Path, keep: list[str] | None) -> None:
    if destination.exists():
        shutil.rmtree(destination)
    if keep is None:
        shutil.copytree(source, destination, ignore=shutil.ignore_patterns(".git"))
        return
    destination.mkdir(parents=True)
    for name in keep:
        item = source / name
        if item.is_dir():
            shutil.copytree(item, destination / name)
        else:
            shutil.copy2(item, destination / name)


def vendor(manifest: dict, into: Path) -> list[str]:
    """Write every skill the manifest names into `into`. Returns their names."""
    LICENSES.mkdir(parents=True, exist_ok=True)
    written: list[str] = []
    with tempfile.TemporaryDirectory() as scratch:
        for source in manifest["sources"]:
            repo, commit = source["repo"], source["commit"]
            print(f"  {repo} @ {commit[:12]}")
            checkout = fetch(repo, commit, Path(scratch))
            for upstream, name in source["skills"].items():
                skill = checkout / upstream
                if not (skill / "SKILL.md").is_file():
                    raise SystemExit(f"{repo}:{upstream} holds no SKILL.md")
                place(skill, into / name, source.get("keep"))
                (into / name / "PROVENANCE.md").write_text(
                    f"Vendored from {repo}\n"
                    f"Path:    {upstream}\n"
                    f"Commit:  {commit}\n"
                    f"License: {source['license']} "
                    f"(../licenses/{slug(repo)}.txt)\n\n"
                    "Edited copies drift from upstream with nothing to say so.\n"
                    "Change it upstream, or fork it under a different name.\n"
                    "Refresh with `just skills`.\n",
                    encoding="utf-8",
                )
                written.append(name)
                print(f"    -> {name}")
            license_file = checkout / source["license_file"]
            shutil.copy2(license_file, LICENSES / f"{slug(repo)}.txt")
    return written


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="verify, change nothing")
    parser.add_argument("--update", action="store_true", help="move pins to HEAD")
    args = parser.parse_args()

    manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))

    if args.update:
        for source in manifest["sources"]:
            head = head_of(source["repo"])
            if head != source["commit"]:
                print(f"{source['repo']}: {source['commit'][:12]} -> {head[:12]}")
                source["commit"] = head
        MANIFEST.write_text(
            json.dumps(manifest, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
        )
        print("pins updated; re-run without --update to vendor them")
        return 0

    if args.check:
        with tempfile.TemporaryDirectory() as scratch:
            expected = Path(scratch) / "skills"
            expected.mkdir()
            names = vendor(manifest, expected)
            drifted = [
                name
                for name in names
                if not (SKILLS / name).is_dir()
                or subprocess.run(
                    ["diff", "-r", "-q", str(expected / name), str(SKILLS / name)],
                    capture_output=True,
                ).returncode
            ]
        stray = sorted(
            item.name
            for item in SKILLS.iterdir()
            if item.name not in NOT_VENDORED and item.name not in names
        )
        for name in drifted:
            print(f"drifted from its pin: {name}", file=sys.stderr)
        for name in stray:
            print(f"not in the manifest: {name}", file=sys.stderr)
        if drifted or stray:
            print("run `just skills` to restore", file=sys.stderr)
            return 1
        print(f"{len(names)} skills match their pins")
        return 0

    print("vendoring into .claude/skills")
    names = vendor(manifest, SKILLS)
    print(f"{len(names)} skills vendored")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
