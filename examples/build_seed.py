"""Build the service seed from example pipelines, pinning installed notebook sources.

No examples are compiled into the panel or server. Run with --check in CI to
detect stale pins. Add an example by adding a pipeline*.json file under examples/.
"""

import argparse
import hashlib
import json
from pathlib import Path

EXAMPLES = Path(__file__).resolve().parent
ROOT = EXAMPLES.parent


def build() -> str:
    pipelines = []
    names = set()
    for path in sorted(EXAMPLES.glob("*/pipeline*.json")):
        pipeline = json.loads(path.read_text())
        if pipeline["name"] in names:
            raise ValueError(f"Duplicate pipeline: {pipeline['name']}")
        names.add(pipeline["name"])
        for block in pipeline["blocks"]:
            spec = block["spec"]
            if spec["kind"] == "notebook":
                source = ROOT / "services/ml_pipeline/notebooks" / f"{spec['notebook']}.py"
                spec["revision"] = hashlib.sha256(source.read_bytes()).hexdigest()
        pipelines.append(pipeline)
    templates = json.loads((EXAMPLES / "blocks.json").read_text())
    for template in templates:
        spec = template["spec"]
        if spec["kind"] == "notebook":
            source = ROOT / "services/ml_pipeline/notebooks" / f"{spec['notebook']}.py"
            spec["revision"] = hashlib.sha256(source.read_bytes()).hexdigest()
    return json.dumps({"version": 1, "pipelines": pipelines, "templates": templates}, indent=2) + "\n"


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    path = EXAMPLES / "seed.json"
    content = build()
    if args.check:
        if not path.exists() or path.read_text() != content:
            raise SystemExit("Stale seed: run python3 examples/build_seed.py")
    else:
        path.write_text(content)
    print(path.relative_to(ROOT))
