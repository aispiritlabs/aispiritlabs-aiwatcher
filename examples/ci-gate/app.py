"""The application under test, as a CI job runs it: every case's question in, its answers out.

Deterministic on purpose — a regression gate that needs a paid judge on day one
is one nobody installs. Its styles are the commits of the example: `one-word`
answers every case, `sentence` none, `terse-south` only Peru and Kenya, and
`confused` every case but Kenya. It reads only each case's ``input``; what a case
expected stays with aiwatcher, which scores the answers this writes.

    python app.py cases.json answers.json --style one-word
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

CAPITALS = {"France": "Paris", "Japan": "Tokyo", "Peru": "Lima", "Kenya": "Nairobi"}


def answer(question: str, style: str) -> str:
    country = question.removeprefix("What is the capital of ").removesuffix("?")
    capital = CAPITALS.get(country, "I do not know")
    if style == "sentence" or (style == "terse-south" and country in {"France", "Japan"}):
        return f"The capital of {country} is {capital}."
    if style == "confused" and country == "Kenya":
        return "Mombasa"
    return capital


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("cases", type=Path)
    parser.add_argument("answers", type=Path)
    parser.add_argument(
        "--style", choices=["one-word", "sentence", "terse-south", "confused"], default="one-word"
    )
    args = parser.parse_args()
    cases = json.loads(args.cases.read_text())
    answers = [
        {
            "case_id": case["case_id"],
            "answer": {"text": answer(case["input"]["question"], args.style)},
        }
        for case in cases
    ]
    args.answers.write_text(json.dumps({"answers": answers}, indent=2))


if __name__ == "__main__":
    main()
