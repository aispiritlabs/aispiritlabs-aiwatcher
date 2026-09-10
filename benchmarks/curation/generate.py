"""Generate the corpus both engines read, to a target size on disk.

Deterministic: a seed and a size name the same bytes on every machine, because
numpy's PCG64 stream is stable across releases and every chunk draws from its
own `default_rng([seed, chunk])`. A benchmark whose input differs between two
runs measures the input.

The CSV is the size that is quoted — "10GB" is ten GiB of CSV, so `du -h`
agrees with the label — and the Parquet beside it holds the same rows, which
makes it a fraction of that on disk. Files are split into parts of about a
million rows, the way a hub or an export lays a corpus out, so a reader that
streams file by file is not handed one 10 GB object.
"""

import argparse
import json
import math
import re
import time
from pathlib import Path

import numpy as np
import polars as pl

from corpus import PRICES, SPANS, prices_path

SPANS_PER_RUN = 12
ROWS_PER_PART = 1_000_000
EPOCH_MS = 1_785_542_400_000  # 2026-08-01T00:00:00Z

KINDS = ["llm", "tool", "agent", "step"]
KIND_P = [0.45, 0.30, 0.10, 0.15]
# Median duration and spread per kind: a model call is seconds, a tool call a
# fraction of one, an agent turn several, a bookkeeping step tens of ms.
DURATION_MEDIAN_MS = np.array([1200.0, 250.0, 6000.0, 40.0])
DURATION_SIGMA = np.array([0.8, 1.0, 0.7, 0.9])
ERROR_P = np.array([0.03, 0.06, 0.02, 0.01])

# model, provider, USD per input token, USD per output token. Invented prices:
# what matters is that the join has something to multiply.
MODELS = [
    ("claude-sonnet-5", "anthropic", "0.000003", "0.000015"),
    ("claude-haiku-4-5", "anthropic", "0.000001", "0.000005"),
    ("gpt-5", "openai", "0.00000125", "0.00001"),
    ("gemini-3-pro", "google", "0.000002", "0.000012"),
    ("llama-4-70b", "meta", "0.0000006", "0.0000008"),
    ("mistral-large-3", "mistral", "0.000002", "0.000006"),
    ("qwen-3-72b", "alibaba", "0.0000009", "0.0000009"),
    ("text-embedding-3", "openai", "0.00000013", "0"),
]
MODEL_P = [0.24, 0.20, 0.16, 0.12, 0.10, 0.08, 0.06, 0.04]
EMBEDDING = "text-embedding-3"

AGENTS = [
    "planner", "researcher", "coder", "reviewer", "critic", "summariser", "router",
    "retriever", "extractor", "classifier", "translator", "tester", "scheduler",
    "negotiator", "analyst", "writer", "editor", "curator", "auditor", "monitor",
    "tutor", "support", "billing", "triage",
]  # fmt: skip
TOOLS = [
    "web_search", "sql_query", "http_get", "python_exec", "vector_search", "file_read",
    "file_write", "calendar", "email_send", "ocr", "geocode", "weather", "calculator",
    "translate", "shell",
]  # fmt: skip
STEP_TYPES = ["plan", "act", "observe", "reflect", "retrieve"]
# Two of these need quoting in CSV — a comma and a double quote — so both
# parsers are exercised on the case a hand-rolled splitter gets wrong.
ERRORS = [
    "rate limited: retry after 30s",
    "context length exceeded",
    "tool timed out after 30000 ms",
    "upstream 503, retrying later",
    'invalid JSON in "arguments"',
    "connection reset by peer",
    "permission denied",
    "schema validation failed: missing field query",
    "model overloaded",
    "cancelled by user",
]

ISO_MS = "%Y-%m-%dT%H:%M:%S%.3fZ"


def parse_size(text: str) -> int:
    match = re.fullmatch(r"(\d+(?:\.\d+)?)\s*(KB|MB|GB)", text.strip(), re.IGNORECASE)
    if match is None:
        raise argparse.ArgumentTypeError(f"{text!r}: expected e.g. 100MB, 1GB, 10GB")
    unit = {"KB": 1024, "MB": 1024**2, "GB": 1024**3}[match.group(2).upper()]
    return int(float(match.group(1)) * unit)


def pick(values: list[str], codes: np.ndarray) -> pl.Series:
    return pl.Series(values, dtype=pl.String).gather(pl.Series(codes, dtype=pl.UInt32))


def chunk(seed: int, index: int, first_row: int, rows: int) -> pl.DataFrame:
    rng = np.random.default_rng([seed, index])
    row = np.arange(first_row, first_row + rows, dtype=np.int64)
    kind = rng.choice(len(KINDS), size=rows, p=KIND_P)
    duration = np.maximum(
        1,
        np.rint(
            np.exp(
                np.log(DURATION_MEDIAN_MS[kind]) + DURATION_SIGMA[kind] * rng.standard_normal(rows)
            )
        ),
    ).astype(np.int64)
    failed = rng.random(rows) < ERROR_P[kind]
    start = EPOCH_MS + row * 20 + rng.integers(0, 20, size=rows)

    frame = pl.DataFrame(
        {
            "row": row,
            "kind": pick(KINDS, kind),
            "model_pick": pick(
                [m[0] for m in MODELS], rng.choice(len(MODELS), size=rows, p=MODEL_P)
            ),
            "tool_pick": pick(TOOLS, rng.integers(0, len(TOOLS), size=rows)),
            "step_pick": pick(STEP_TYPES, rng.integers(0, len(STEP_TYPES), size=rows)),
            "agent_pick": pick(AGENTS, rng.integers(0, len(AGENTS), size=rows)),
            "agent_null": rng.random(rows) < 0.05,
            "error_pick": pick(ERRORS, rng.integers(0, len(ERRORS), size=rows)),
            "failed": failed,
            "start_ms": start,
            "duration_ms": duration,
            "in_tokens": np.rint(np.exp(np.log(1400.0) + 0.9 * rng.standard_normal(rows))).astype(
                np.int64
            ),
            "out_tokens": np.rint(np.exp(np.log(280.0) + 0.8 * rng.standard_normal(rows))).astype(
                np.int64
            ),
        }
    )

    run = pl.col("row") // SPANS_PER_RUN
    is_llm = pl.col("kind") == "llm"
    model = pl.when(is_llm).then(pl.col("model_pick"))
    tool = pl.when(pl.col("kind") == "tool").then(pl.col("tool_pick"))
    step_type = pl.when(pl.col("kind") == "step").then(pl.col("step_pick"))
    agent = pl.when(~pl.col("agent_null")).then(pl.col("agent_pick"))

    return frame.select(
        run_id=pl.lit("run-") + run.cast(pl.String).str.zfill(10),
        trace_id=((run * 2_654_435_761) % 10**16).cast(pl.String).str.zfill(16)
        + run.cast(pl.String).str.zfill(16),
        span_id=pl.lit("s") + pl.col("row").cast(pl.String).str.zfill(15),
        parent_span_id=pl.when(pl.col("row") % SPANS_PER_RUN != 0).then(
            pl.lit("s") + (run * SPANS_PER_RUN).cast(pl.String).str.zfill(15)
        ),
        name=pl.col("kind") + "." + pl.coalesce(model, tool, step_type, agent, pl.lit("turn")),
        kind=pl.col("kind"),
        start=pl.from_epoch("start_ms", time_unit="ms").dt.to_string(ISO_MS),
        end=pl.from_epoch(pl.col("start_ms") + pl.col("duration_ms"), time_unit="ms").dt.to_string(
            ISO_MS
        ),
        duration_ms=pl.col("duration_ms"),
        operation=pl.when(is_llm & (pl.col("model_pick") == EMBEDDING))
        .then(pl.lit("embed"))
        .when(is_llm)
        .then(pl.lit("chat"))
        .when(pl.col("kind") == "tool")
        .then(pl.lit("invoke"))
        .when(pl.col("kind") == "agent")
        .then(pl.lit("turn")),
        agent_id=agent,
        model=model,
        tool=tool,
        step_type=step_type,
        status=pl.when(pl.col("failed")).then(pl.lit("error")).otherwise(pl.lit("ok")),
        input_tokens=pl.when(is_llm).then(pl.col("in_tokens")),
        output_tokens=pl.when(is_llm & (pl.col("model_pick") == EMBEDDING))
        .then(pl.lit(0, dtype=pl.Int64))
        .when(is_llm)
        .then(pl.col("out_tokens")),
        error=pl.when(pl.col("failed")).then(pl.col("error_pick")),
    ).cast(SPANS)  # type: ignore[arg-type]


def write_prices(corpus: Path) -> None:
    lines = [",".join(PRICES), *(",".join(model) for model in MODELS)]
    prices_path(corpus).write_text("\n".join(lines) + "\n")


def generate(corpus: Path, target_bytes: int, seed: int, parquet: bool) -> dict[str, object]:
    csv_dir = corpus / "spans.csv"
    parquet_dir = corpus / "spans.parquet"
    for directory in (csv_dir, *([parquet_dir] if parquet else [])):
        directory.mkdir(parents=True, exist_ok=True)
        for stale in directory.glob("part-*"):
            stale.unlink()
    write_prices(corpus)

    started = time.perf_counter()
    written = rows = parquet_bytes = index = 0
    # A first guess at bytes per row, corrected by every part written.
    bytes_per_row = 230.0
    while written < target_bytes:
        remaining = math.ceil((target_bytes - written) / bytes_per_row)
        part_rows = max(1, min(ROWS_PER_PART, remaining))
        frame = chunk(seed, index, rows, part_rows)
        csv_path = csv_dir / f"part-{index:05d}.csv"
        frame.write_csv(csv_path)
        written += csv_path.stat().st_size
        if parquet:
            parquet_path = parquet_dir / f"part-{index:05d}.parquet"
            # Snappy, one row group per million rows: what a hub serves, and a
            # codec Flow reads in pure PHP when the extension is absent.
            frame.write_parquet(parquet_path, compression="snappy", row_group_size=ROWS_PER_PART)
            parquet_bytes += parquet_path.stat().st_size
        rows += part_rows
        index += 1
        bytes_per_row = written / rows
        print(f"  part {index:>3}  {rows:>12,} rows  {written / 1024**3:6.2f} GiB", flush=True)

    manifest: dict[str, object] = {
        "seed": seed,
        "rows": rows,
        "parts": index,
        "csv_bytes": written,
        "parquet_bytes": parquet_bytes if parquet else None,
        "bytes_per_row": round(bytes_per_row, 1),
        "runs": math.ceil(rows / SPANS_PER_RUN),
        "generated_in_s": round(time.perf_counter() - started, 1),
        "polars": pl.__version__,
        "numpy": np.__version__,
    }
    (corpus / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    return manifest


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--size", default="10GB", help="CSV size on disk, e.g. 100MB, 1GB, 10GB")
    parser.add_argument("--data-dir", type=Path, default=Path(__file__).parent / ".data")
    parser.add_argument("--seed", type=int, default=20260910)
    parser.add_argument("--no-parquet", action="store_true", help="write only the CSV")
    args = parser.parse_args()

    corpus = args.data_dir / args.size.upper().replace(" ", "")
    print(f"generating {args.size} into {corpus}")
    manifest = generate(corpus, parse_size(args.size), args.seed, not args.no_parquet)
    print(json.dumps(manifest, indent=2))


if __name__ == "__main__":
    main()
