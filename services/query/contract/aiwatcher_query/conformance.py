"""The conformance suite: the same questions, asked of whichever engine answers at a URL.

    uv run python -m aiwatcher_query.conformance --url http://127.0.0.1:8091
    uv run python -m aiwatcher_query.conformance --url … --record    # against Flow only

It reads `/query/healthz` for the language, sends each question's text in that language
to `/query/query`, and compares the rows with the ones Flow gave, checked in beside the
texts under `conformance/<question>/`. Counts and sums compare exactly; a float compares
within the half-hundredth Flow's `average()` rounds to, because the rows are Flow's and
Flow rounds a mean half-up to two places. It also compares `/query/datasets` with Flow's
answer, all of it but `engine`, `language` and `source`: the catalog is one file, and this
is where an engine that served it differently is caught.

The questions are the benchmark's q1 to q4 over `corpus_spans`, bounded: each ends in a total
order, and the two that answer a row per run or per span end in a limit of a hundred, so
every engine stays inside the thousand-row answer (AW-3's job note says why, and that it
narrows the spec's scenario). The corpus is `benchmarks/curation/generate.py` at its fixed
seed and one megabyte, which `just query-conformance` generates before it starts an engine.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any

import httpx

from aiwatcher_query.config import CONTRACT_ROOT

CONFORMANCE = CONTRACT_ROOT / "conformance"
#: Where the recorded catalog is, beside the questions.
DATASETS = CONFORMANCE / "datasets.json"
#: Half of Flow's last decimal place, and a hair for binary floating point.
TOLERANCE = 0.005 + 1e-9
#: Which file holds a question's text in each language.
TEXTS = {
    "flow-dsl": "flow.txt",
    "datafusion-python": "datafusion.py",
    "duckdb-python": "duckdb.py",
}
#: The parts of `/query/datasets` that are the engine's rather than the catalog's.
_OWN = ("engine", "language", "source")


def questions() -> list[Path]:
    return sorted(path for path in CONFORMANCE.iterdir() if path.is_dir() and path.name[0] == "q")


def differences(expected: list[dict[str, Any]], actual: list[dict[str, Any]]) -> list[str]:
    """What differs between two answers, row by row; empty when they are the same answer."""
    if len(expected) != len(actual):
        return [f"{len(actual)} rows where Flow answered {len(expected)}"]
    found: list[str] = []
    for index, (want, got) in enumerate(zip(expected, actual, strict=True)):
        if set(want) != set(got):
            found.append(f"row {index} has columns {sorted(got)}, Flow's has {sorted(want)}")
            continue
        found.extend(
            f"row {index}, {column}: {got[column]!r} where Flow answered {value!r}"
            for column, value in want.items()
            if not same(value, got[column])
        )
    return found


def same(want: Any, got: Any) -> bool:
    if isinstance(want, bool) or isinstance(got, bool):
        return want is got
    if isinstance(want, float) or isinstance(got, float):
        return isinstance(got, int | float) and abs(float(want) - float(got)) <= TOLERANCE
    return bool(want == got)


def catalog_of(answer: dict[str, Any]) -> dict[str, Any]:
    return {key: value for key, value in answer.items() if key not in _OWN}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="python -m aiwatcher_query.conformance")
    parser.add_argument("--url", default="http://127.0.0.1:8091")
    parser.add_argument("--record", action="store_true", help="write Flow's answers as expected")
    args = parser.parse_args(argv)

    with httpx.Client(base_url=args.url, timeout=300.0) as client:
        health = client.get("/query/healthz").json()
        engine, language = health.get("engine", "flow"), health.get("language", "flow-dsl")
        if args.record and engine != "flow":
            print("The expected rows are Flow's: record against Flow.", file=sys.stderr)
            return 2
        print(f"conformance against {engine} ({language}) at {args.url}")

        failed = 0
        catalog = catalog_of(client.get("/query/datasets").json())
        if args.record:
            _write(DATASETS, catalog)
        elif catalog != json.loads(DATASETS.read_text()):
            failed += 1
            print("  datasets  DIFFERS from the catalog Flow serves")
        else:
            print("  datasets  same")

        for question in questions():
            text = (question / TEXTS[language]).read_text()
            response = client.post("/query/query", json={"pipeline": text})
            if response.status_code != 200:
                failed += 1
                print(f"  {question.name}  REFUSED {response.status_code}: {response.text[:500]}")
                continue
            answer = response.json()
            if args.record:
                _write(question / "expected.json", {"rows": answer["rows"]})
                print(f"  {question.name}  recorded {answer['row_count']} rows")
                continue
            expected = json.loads((question / "expected.json").read_text())["rows"]
            problems = differences(expected, answer["rows"])
            failed += bool(problems)
            verdict = "same" if not problems else "DIFFERS"
            print(
                f"  {question.name}  {verdict} ({answer['row_count']} rows, {answer['took_ms']} ms)"
            )
            for problem in problems[:10]:
                print(f"      {problem}")
    return 1 if failed else 0


def _write(path: Path, value: object) -> None:
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n")


if __name__ == "__main__":
    sys.exit(main())
