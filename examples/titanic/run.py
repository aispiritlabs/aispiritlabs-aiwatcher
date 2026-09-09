"""Run the bundled Python blocks against a local CSV, without aiwatcher servers."""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import subprocess
import tempfile
from pathlib import Path
from urllib.request import urlopen

from ml_pipeline.step import inject, load_app, rows_of

HERE = Path(__file__).resolve().parent
DATA_URL = "https://huggingface.co/datasets/phihung/titanic/resolve/9753139e0b9d454ab4fd22e884290260db5fc7b6/train.csv"
DATA_SHA256 = "4a437fde05fe5264e1701a7387ac6fb75393772ba38bb2c9c566405af5af4bd7"
INT_COLUMNS = {"PassengerId", "Survived", "Pclass", "SibSp", "Parch"}


def read_rows(path: Path) -> list[dict]:
    with path.open(newline="", encoding="utf-8") as stream:
        return [
            {
                key: None if value == "" else int(value) if key in INT_COLUMNS
                else float(value) if key in {"Age", "Fare"} else value
                for key, value in row.items()
            }
            for row in csv.DictReader(stream)
        ]


def run(rows: list[dict], output_dir: Path, bundle_path: Path = HERE / "titanic.flow.json") -> dict:
    bundle = json.loads(bundle_path.read_text())
    sources = {entry["name"]: entry for entry in bundle["notebooks"]}
    output_dir.mkdir(parents=True, exist_ok=True)
    metrics = {}
    # Execute the code inside the portable bundle, including its exact revisions.
    with tempfile.TemporaryDirectory(prefix="aiwatcher-titanic-") as scratch:
        for block in bundle["pipeline"]["blocks"]:
            spec = block["spec"]
            if spec["kind"] != "notebook":
                continue
            notebook = sources[spec["notebook"]]
            if hashlib.sha256(notebook["source"].encode()).hexdigest() != spec["revision"]:
                raise ValueError(f"Source checksum mismatch: {block['id']}")
            path = Path(scratch) / f"{notebook['name']}.py"
            path.write_text(notebook["source"])
            definitions = inject(load_app(path), rows, spec.get("params", {}), path.stem)
            rows = rows_of(definitions, path.stem)
            for key in ("encoder_state", "imputation_state", "feature_state", "metrics"):
                if key in definitions:
                    (output_dir / f"{block['id']}-{key}.json").write_text(
                        json.dumps(definitions[key], indent=2, allow_nan=False) + "\n"
                    )
            if "figure" in definitions:
                definitions["figure"].savefig(output_dir / "visualization.png", dpi=150)
            if "metrics" in definitions:
                metrics = definitions["metrics"]
            print(f"{block['title']}: {len(rows)} rows")
    (output_dir / "predictions.json").write_text(json.dumps(rows, indent=2, allow_nan=False) + "\n")
    return metrics


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--csv", type=Path, help="Kaggle train.csv with raw passenger columns")
    source.add_argument("--download", action="store_true", help="Fetch the pinned public CSV mirror")
    parser.add_argument("--output", type=Path, default=HERE / "output")
    parser.add_argument("--limit", type=int, help="25 exercises the preview path")
    parser.add_argument("--php", action="store_true", help="Prepare data with native FlowPHP / FlowAI before Python visualization and training")
    args = parser.parse_args()
    if args.download:
        with urlopen(DATA_URL, timeout=60) as response:
            data = response.read()
        if hashlib.sha256(data).hexdigest() != DATA_SHA256:
            raise SystemExit("The downloaded CSV does not match the recorded SHA-256.")
        args.output.mkdir(parents=True, exist_ok=True)
        args.csv = args.output / "train.csv"
        args.csv.write_bytes(data)
    rows = read_rows(args.csv)
    if args.limit is not None:
        if args.limit < 8:
            raise SystemExit("Use at least 8 rows for the stratified example.")
        rows = rows[:args.limit]
    if args.php:
        command = ["php", str(HERE / "prepare.php"), str(args.csv)]
        if args.limit is not None:
            command.append(str(args.limit))
        rows = json.loads(subprocess.check_output(command, text=True))
        args.output.mkdir(parents=True, exist_ok=True)
        for field in ("feature_encoder", "label_encoder"):
            (args.output / f"{field}.json").write_text(json.dumps(rows[0][field], indent=2) + "\n")
        print(f"FlowPHP / FlowAI preparation: {len(rows)} rows")
    run(rows, args.output, HERE / ("titanic-php.flow.json" if args.php else "titanic.flow.json"))


if __name__ == "__main__":
    main()
