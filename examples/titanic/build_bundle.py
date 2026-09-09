"""Build portable flows from the authored pipeline and notebook files.

Run from any directory; --check refuses stale generated artifacts without edits.
"""

import argparse
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
EXAMPLE = Path(__file__).resolve().parent


def variant(php: bool = False) -> dict[Path, str]:
    pipeline = json.loads((EXAMPLE / ("pipeline-php.json" if php else "pipeline.json")).read_text())
    notebooks = []
    for block in pipeline["blocks"]:
        spec = block["spec"]
        if spec["kind"] != "notebook":
            continue
        source = (ROOT / "services/ml_pipeline/notebooks" / f"{spec['notebook']}.py").read_text()
        revision = hashlib.sha256(source.encode()).hexdigest()
        spec["revision"] = revision
        notebooks.append({"name": spec["notebook"], "revision": revision, "source": source})
    bundle = {
        "format": "aiwatcher.curation",
        "version": 1,
        "requirements": {
            "python": ">=3.14",
            "packages": ["aiwatcher-ml-pipeline==0.1.0 (includes FlowAI)", "marimo>=0.24,<0.25", "scikit-learn>=1.8,<1.9", "matplotlib>=3.10,<4", "pandas>=2.2,<4"],
        },
        "pipeline": pipeline,
        "notebooks": notebooks,
    }
    result = {
        EXAMPLE / ("titanic-php.flow.json" if php else "titanic.flow.json"): json.dumps(bundle, indent=2) + "\n",
    }
    if php:
        result[EXAMPLE / "preparation.flow"] = (
            "data_frame()->read(hub_rows, dataset: 'phihung/titanic', split: 'train', limit: 891)\n"
            + "\n".join(block["spec"]["steps"] for block in pipeline["blocks"] if block["spec"]["kind"] == "transform")
            + "\n->write(to_output(truncate: false))->run();\n"
        )
    return result


def artifacts() -> dict[Path, str]:
    return {**variant(), **variant(True)}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    for path, content in artifacts().items():
        if args.check:
            if not path.exists() or path.read_text() != content:
                raise SystemExit(f"Rebuild stale artifact: {path}")
        else:
            path.write_text(content)
        print(path.relative_to(ROOT))


if __name__ == "__main__":
    main()
