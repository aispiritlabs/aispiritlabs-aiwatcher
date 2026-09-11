#!/usr/bin/env python3
"""Check reviewed context edges, including optional, aliased and target dependencies.

Cargo's no-deps metadata still reports every declared dependency of each workspace
package. This intentionally checks inactive target/feature edges too. Build-time
edges follow production rules; dev edges may additionally use test_only fixtures.
"""

import json
from pathlib import Path
import subprocess
import sys
import unittest
from copy import deepcopy

ROOT = Path(__file__).resolve().parent.parent
POLICY = json.loads((ROOT / "scripts/rust-boundaries.json").read_text())


def violations(metadata, policy=POLICY):
    packages = [p for p in metadata["packages"] if p["id"] in metadata["workspace_members"]]
    names = {p["name"] for p in packages}
    failures = []
    for package in packages:
        owner = package["name"].removeprefix("aiwatcher-")
        if owner not in policy["production"]:
            failures.append(f"Unreviewed workspace crate: {package['name']}")
            continue
        for dependency in package["dependencies"]:
            # Cargo's name is the package name; rename is only the Rust alias.
            target = dependency["name"]
            if target not in names:
                continue
            kind = dependency.get("kind") or "normal"
            allowed = set(policy["production"][owner])
            if kind == "dev":
                allowed.update(policy["test_only"].get(owner, []))
            if target.removeprefix("aiwatcher-") not in allowed:
                failures.append(f"{package['name']} -> {target} ({kind}, alias={dependency.get('rename')}, target={dependency.get('target')})")
    return failures


class BoundaryTests(unittest.TestCase):
    def setUp(self):
        self.metadata = {"workspace_members": ["training", "api", "prompts"], "packages": [
            {"id": name, "name": f"aiwatcher-{name}", "dependencies": []}
            for name in ("training", "api", "prompts")
        ]}

    def test_an_aliased_inactive_target_edge_is_rejected(self):
        for kind in (None, "build", "dev"):
            with self.subTest(kind=kind):
                metadata = deepcopy(self.metadata)
                metadata["packages"][0]["dependencies"] = [{"name": "aiwatcher-api", "rename": "transport", "kind": kind, "target": 'cfg(target_os = "windows")', "optional": True}]
                self.assertEqual(len(violations(metadata)), 1)

    def test_fixture_permission_does_not_leak_into_production(self):
        dependency = {"name": "aiwatcher-prompts", "kind": "dev"}
        self.metadata["packages"][0]["dependencies"] = [dependency]
        self.assertEqual(violations(self.metadata), [])
        dependency["kind"] = None
        self.assertEqual(len(violations(self.metadata)), 1)

    def test_new_crates_require_review_even_without_edges(self):
        self.metadata["packages"][0]["name"] = "aiwatcher-evaluation"
        self.assertEqual(len(violations(self.metadata)), 1)


def main():
    if "--test" in sys.argv:
        unittest.main(argv=[sys.argv[0]])
    metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--no-deps", "--format-version", "1", "--locked"], cwd=ROOT))
    failures = violations(metadata)
    if failures:
        print("Rust boundary violations:\n" + "\n".join(failures), file=sys.stderr)
        return 1
    print("Rust context boundaries passed (production/build and test edges checked separately).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
