"""Arrow beside Polars: as an engine, as the format a step hands on, and as a free exchange.

Run from benchmarks/curation: `uv run python arrow_exchange.py`.

1. q2 over the same 5 GB, read by PyArrow (CSV and Parquet) and computed either in
   PyArrow's own engine (Acero) or handed to Polars — against Polars reading it.
2. What a step's output costs to hand on: JSON (today's contract), CSV, Arrow IPC
   stream, Arrow IPC file memory-mapped, Parquet — write, read, bytes.
3. Polars ↔ PyArrow through the PyCapsule interface: is it really free?
"""

import json
import resource
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

BENCH = Path(__file__).resolve().parent
sys.path.insert(0, str(BENCH))

import polars as pl  # noqa: E402
import pyarrow as pa  # noqa: E402
import pyarrow.compute as pc  # noqa: E402
import pyarrow.csv as pacsv  # noqa: E402
import pyarrow.dataset as ds  # noqa: E402
import pyarrow.parquet as pq  # noqa: E402

from corpus import SPANS  # noqa: E402

DATA = BENCH / ".data" / "5GB"
CSV = DATA / "spans.csv" / "part-00000.csv"
PARQUET = DATA / "formats" / "spans.parquet"
NEEDED = ["kind", "model", "input_tokens", "output_tokens", "duration_ms"]
TYPES = {
    "kind": pa.string(),
    "model": pa.string(),
    "input_tokens": pa.int64(),
    "output_tokens": pa.int64(),
    "duration_ms": pa.int64(),
}
HANDOVER_ROWS = 2_000_000
RUNS = 3


def polars_q2(frame: pl.LazyFrame) -> pl.DataFrame:
    return (
        frame.filter(pl.col("kind") == "llm")
        .group_by("model")
        .agg(
            spans=pl.len(),
            input_tokens=pl.col("input_tokens").sum(),
            output_tokens=pl.col("output_tokens").sum(),
            avg_duration_ms=pl.col("duration_ms").mean(),
        )
        .sort("model")
        .collect(engine="streaming")
    )


def acero_q2(table: pa.Table) -> pl.DataFrame:
    grouped = (
        table.filter(pc.equal(table["kind"], "llm"))
        .group_by("model")
        .aggregate(
            [
                ("model", "count"),
                ("input_tokens", "sum"),
                ("output_tokens", "sum"),
                ("duration_ms", "mean"),
            ]
        )
    )
    return (
        pl.from_arrow(grouped)
        .select(
            "model",
            pl.col("model_count").alias("spans"),
            pl.col("input_tokens_sum").alias("input_tokens"),
            pl.col("output_tokens_sum").alias("output_tokens"),
            pl.col("duration_ms_mean").alias("avg_duration_ms"),
        )
        .sort("model")
    )


def read_csv_arrow() -> pa.Table:
    return pacsv.read_csv(
        CSV, convert_options=pacsv.ConvertOptions(include_columns=NEEDED, column_types=TYPES)
    )


VARIANTS = {
    "Polars · CSV": lambda: polars_q2(pl.scan_csv(CSV, schema=SPANS)),
    "PyArrow CSV → Acero": lambda: acero_q2(read_csv_arrow()),
    "PyArrow CSV → Polars (zero-copy)": lambda: polars_q2(pl.from_arrow(read_csv_arrow()).lazy()),
    "Polars · Parquet": lambda: polars_q2(pl.scan_parquet(PARQUET)),
    "PyArrow Parquet → Acero": lambda: acero_q2(pq.read_table(PARQUET, columns=NEEDED)),
    "PyArrow dataset, filter pushed down → Acero": lambda: acero_q2(
        ds.dataset(PARQUET).to_table(columns=NEEDED, filter=pc.field("kind") == "llm")
    ),
}
TYPES_PL = {"input_tokens": pl.Int64, "output_tokens": pl.Int64, "duration_ms": pl.Int64}


def child(variant: str) -> None:
    work = VARIANTS[variant]
    started = time.perf_counter()
    result = work()
    first = time.perf_counter() - started
    warm = []
    for _ in range(RUNS):
        started = time.perf_counter()
        result = work()
        warm.append(time.perf_counter() - started)
    print(
        json.dumps(
            {
                "first": first,
                "warm": statistics.median(warm),
                "rss": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,
                "rows": result.to_dicts(),
            }
        )
    )


def gib(value: float) -> str:
    return f"{value / 1024**3:.2f} GiB" if value >= 1024**3 else f"{value / 1024**2:.0f} MiB"


def timed(work) -> float:
    samples = []
    for _ in range(RUNS):
        started = time.perf_counter()
        work()
        samples.append(time.perf_counter() - started)
    return statistics.median(samples)


def handover(frame: pl.DataFrame, scratch: Path) -> None:
    """Write then read one step's output in each format, as the next step would."""
    print(
        f"\nhanding on one step's output: {frame.height:,} rows × {frame.width} columns (median of {RUNS})"
    )
    print(f"  {'format':<34}{'write':>9}{'read':>9}{'total':>9}{'bytes':>12}")
    formats = {
        "JSON rows (today's contract)": (
            lambda path: frame.write_json(path),
            lambda path: pl.read_json(path),
        ),
        "CSV": (lambda path: frame.write_csv(path), lambda path: pl.read_csv(path)),
        "Arrow IPC stream": (
            lambda path: frame.write_ipc_stream(path),
            lambda path: pl.read_ipc_stream(path),
        ),
        "Arrow IPC file, memory-mapped": (
            lambda path: frame.write_ipc(path),
            lambda path: pl.read_ipc(path, memory_map=True),
        ),
        "Parquet (zstd)": (
            lambda path: frame.write_parquet(path),
            lambda path: pl.read_parquet(path),
        ),
    }
    for label, (write, read) in formats.items():
        path = scratch / label.split()[0].lower()
        # Bound as defaults: each lambda is timed in its own iteration, and
        # saying so beats relying on it.
        write_s = timed(lambda write=write, path=path: write(path))
        read_s = timed(lambda read=read, path=path: read(path))
        if not read(path).equals(frame):
            raise SystemExit(f"{label} did not hand back the same rows")
        size = gib(path.stat().st_size)
        print(f"  {label:<34}{write_s:>8.3f}s{read_s:>8.3f}s{write_s + read_s:>8.3f}s{size:>12}")
    # What the notebook runtime does today: rows as Python dicts through JSON.
    sample = frame.head(200_000)
    dicts_s = timed(lambda: json.loads(json.dumps(sample.to_dicts())))
    print(
        f"  {'Python dicts via json (200k rows)':<34}{'':>9}{'':>9}{dicts_s:>8.3f}s"
        f"{'':>12}  ≈ {dicts_s * frame.height / sample.height:.1f} s at {frame.height:,}"
    )


def exchange(frame: pl.DataFrame) -> None:
    print(f"\nPolars ↔ PyArrow, {frame.height:,} rows (median of {RUNS})")
    table = frame.to_arrow(compat_level=pl.CompatLevel.newest())
    for label, work in (
        ("pa.table(df) — PyCapsule", lambda: pa.table(frame)),
        (
            "df.to_arrow(compat newest)",
            lambda: frame.to_arrow(compat_level=pl.CompatLevel.newest()),
        ),
        ("pl.DataFrame(table) — PyCapsule", lambda: pl.DataFrame(table)),
        ("pl.from_arrow(table)", lambda: pl.from_arrow(table)),
    ):
        print(f"  {label:<34}{timed(work) * 1000:>9.2f} ms")
    before = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    tables = [pa.table(frame) for _ in range(5)]
    grew = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss - before
    print(
        f"  five exports held at once grew the peak by {gib(grew)} "
        f"(the frame itself is {gib(frame.estimated_size())})"
    )
    del tables


def main() -> None:
    print(f"pyarrow {pa.__version__}, polars {pl.__version__}\n")
    results = {}
    for variant in VARIANTS:
        done = subprocess.run(
            [sys.executable, __file__, "--child", variant],
            capture_output=True,
            text=True,
            check=True,
        )
        results[variant] = json.loads(done.stdout.strip().splitlines()[-1])
        print(f"  {variant} done", flush=True)
    reference = pl.DataFrame(results["Polars · Parquet"]["rows"])
    print(f"\n{'q2 over 5 GB':<46}{'first':>9}{'warm':>9}{'peak RSS':>11}  answer")
    for variant, result in results.items():
        rows = pl.DataFrame(result["rows"]).cast(reference.schema)
        same = all(
            rows[c].to_list() == reference[c].to_list()
            for c in ("model", "spans", "input_tokens", "output_tokens")
        )
        same = same and (rows["avg_duration_ms"] - reference["avg_duration_ms"]).abs().max() < 1e-6
        print(
            f"{variant:<46}{result['first']:>8.3f}s{result['warm']:>8.3f}s{gib(result['rss']):>11}"
            f"  {'same' if same else 'DIFFERS'}"
        )

    llm = (
        pl.scan_parquet(PARQUET)
        .filter(pl.col("kind") == "llm")
        .select("span_id", "model", "input_tokens", "output_tokens", "duration_ms")
        .collect()
    )
    with tempfile.TemporaryDirectory() as scratch:
        handover(llm.head(HANDOVER_ROWS), Path(scratch))
    exchange(llm)


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--child":
        child(sys.argv[2])
    else:
        main()
