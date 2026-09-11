#!/usr/bin/env python3
"""Publish the approved synthetic fixture through the durable registry client.

The explicit destination and logical ID make retries reviewable. This invokes
only the repository's pinned response function and scorer, never a model.
"""

import argparse
import importlib.util
import json
from pathlib import Path

from aiwatcher_sdk.evaluation_registry import EvaluationRegistry

ROOT = Path(__file__).resolve().parents[1]
BUNDLE = ROOT / "contracts/fixtures/evaluation-v1"


def load(name):
    spec = importlib.util.spec_from_file_location(name, BUNDLE / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--evaluation-id", required=True)
    parser.add_argument("--repetition-id", required=True)
    args = parser.parse_args()
    manifest = json.loads((BUNDLE / "manifest.json").read_text())
    manifest["origin"] = {
        "evaluation_id": args.evaluation_id,
        "repetition_id": args.repetition_id,
    }
    responses, scorer = load("responses"), load("scorer")
    measurements = []
    for case in json.loads((BUNDLE / "cases.json").read_text())["cases"]:
        actual = responses.answer(case["input"]["question"])
        measurements.append({
            "case_id": case["case_id"],
            "repetition_id": args.repetition_id,
            "actual": {"answer": actual},
            "metrics": {"accuracy": scorer.score(actual, case["expected"]["answer"])},
            "error": None,
            "trace_id": None,
            "span_id": None,
        })
    with EvaluationRegistry(args.base_url) as registry:
        print(json.dumps(registry.publish(manifest, measurements), indent=2))


if __name__ == "__main__":
    main()
