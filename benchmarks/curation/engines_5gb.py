"""q2 over the same 5 GB in Polars, DataFusion, DuckDB and Daft — CSV and Parquet.

Run from benchmarks/curation: `uv run python engines_5gb.py`.
Every engine is written through its Python API — expressions and relations, no
SQL string anywhere — and handed the schema (none infers it). The answer is
brought back as a Polars frame inside the timed work, and each variant runs in a
process of its own so its peak RSS is its own. Answers are compared.
"""

import json
import os
import resource
import statistics
import subprocess
import sys
import time
from pathlib import Path

BENCH = Path(__file__).resolve().parent
sys.path.insert(0, str(BENCH))
os.environ.setdefault("DAFT_PROGRESS_BAR", "0")

import polars as pl  # noqa: E402

from corpus import SPANS  # noqa: E402

DATA = BENCH / ".data" / "5GB"
CSV = DATA / "spans.csv" / "part-00000.csv"
PARQUET = DATA / "formats" / "spans.parquet"
RUNS = 3


def polars(fmt: str) -> pl.DataFrame:
    frame = pl.scan_csv(CSV, schema=SPANS) if fmt == "csv" else pl.scan_parquet(PARQUET)
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


def arrow_schema():
    import pyarrow as pa

    return pa.schema(
        [(name, pa.int64() if dtype == pl.Int64 else pa.string()) for name, dtype in SPANS.items()]
    )


def datafusion_api(fmt: str) -> pl.DataFrame:
    from datafusion import SessionContext, col, lit
    from datafusion import functions as f

    ctx = SessionContext()
    frame = (
        ctx.read_csv(str(CSV), schema=arrow_schema())
        if fmt == "csv"
        else ctx.read_parquet(str(PARQUET))
    )
    return (
        frame.filter(col("kind") == lit("llm"))
        .aggregate(
            [col("model")],
            [
                f.count(col("model")).alias("spans"),
                f.sum(col("input_tokens")).alias("input_tokens"),
                f.sum(col("output_tokens")).alias("output_tokens"),
                f.avg(col("duration_ms")).alias("avg_duration_ms"),
            ],
        )
        .sort(col("model").sort(ascending=True))
        .to_polars()
    )


def duckdb_relational(fmt: str) -> pl.DataFrame:
    """DuckDB through its relational API: expressions and a relation, no SQL string.

    The one string is the group key, `"model"` — a column name, which is what
    `aggregate`'s `group_expr` takes. Sums come back as DuckDB's 128-bit integer
    and are compared after the cast every engine's answer goes through.
    """
    import duckdb
    from duckdb import ColumnExpression, ConstantExpression, FunctionExpression

    types = {name: "BIGINT" if dtype == pl.Int64 else "VARCHAR" for name, dtype in SPANS.items()}
    relation = (
        duckdb.read_csv(str(CSV), header=True, dtype=types)
        if fmt == "csv"
        else duckdb.read_parquet(str(PARQUET))
    )
    model = ColumnExpression("model")
    return (
        relation.filter(ColumnExpression("kind") == ConstantExpression("llm"))
        .aggregate(
            [
                model,
                FunctionExpression("count", model).alias("spans"),
                FunctionExpression("sum", ColumnExpression("input_tokens")).alias("input_tokens"),
                FunctionExpression("sum", ColumnExpression("output_tokens")).alias("output_tokens"),
                FunctionExpression("avg", ColumnExpression("duration_ms")).alias("avg_duration_ms"),
            ],
            "model",
        )
        .sort(model)
        .pl()
        .cast({"input_tokens": pl.Int64, "output_tokens": pl.Int64})
    )


def daft_api(fmt: str) -> pl.DataFrame:
    import daft

    schema = {
        name: daft.DataType.int64() if dtype == pl.Int64 else daft.DataType.string()
        for name, dtype in SPANS.items()
    }
    frame = (
        daft.read_csv(str(CSV), infer_schema=False, schema=schema)
        if fmt == "csv"
        else daft.read_parquet(str(PARQUET))
    )
    result = (
        frame.where(daft.col("kind") == "llm")
        .groupby("model")
        .agg(
            daft.col("model").count().alias("spans"),
            daft.col("input_tokens").sum().alias("input_tokens"),
            daft.col("output_tokens").sum().alias("output_tokens"),
            daft.col("duration_ms").mean().alias("avg_duration_ms"),
        )
        .sort("model")
    )
    return pl.from_arrow(result.to_arrow())


VARIANTS = {
    "Polars · CSV": lambda: polars("csv"),
    "DataFusion · CSV (Python API)": lambda: datafusion_api("csv"),
    "DuckDB · CSV (relational API)": lambda: duckdb_relational("csv"),
    "Daft · CSV": lambda: daft_api("csv"),
    "Polars · Parquet": lambda: polars("parquet"),
    "DataFusion · Parquet (Python API)": lambda: datafusion_api("parquet"),
    "DuckDB · Parquet (relational API)": lambda: duckdb_relational("parquet"),
    "Daft · Parquet": lambda: daft_api("parquet"),
}


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


def main() -> None:
    import daft
    import datafusion

    print(
        f"polars {pl.__version__}, datafusion {datafusion.__version__}, daft {daft.__version__}\n"
    )
    results, failures = {}, {}
    for variant in VARIANTS:
        done = subprocess.run(
            [sys.executable, __file__, "--child", variant], capture_output=True, text=True
        )
        if done.returncode != 0:
            failures[variant] = "\n".join(done.stderr.strip().splitlines()[-4:])
            print(f"  {variant} FAILED", flush=True)
            continue
        results[variant] = json.loads(done.stdout.strip().splitlines()[-1])
        print(f"  {variant} done", flush=True)

    reference = pl.DataFrame(results["Polars · Parquet"]["rows"])
    print(f"\n{'q2 over 5 GB':<40}{'first':>9}{'warm':>9}{'peak RSS':>11}  answer")
    for variant, result in results.items():
        rows = pl.DataFrame(result["rows"]).select(reference.columns).cast(reference.schema)
        same = all(
            rows[c].to_list() == reference[c].to_list()
            for c in ("model", "spans", "input_tokens", "output_tokens")
        )
        same = same and (rows["avg_duration_ms"] - reference["avg_duration_ms"]).abs().max() < 1e-6
        print(
            f"{variant:<40}{result['first']:>8.3f}s{result['warm']:>8.3f}s{gib(result['rss']):>11}"
            f"  {'same' if same else 'DIFFERS'}"
        )
    for variant, error in failures.items():
        print(f"\n{variant} failed:\n{error}")


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--child":
        child(sys.argv[2])
    else:
        main()
