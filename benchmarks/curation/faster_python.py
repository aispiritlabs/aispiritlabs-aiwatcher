"""How much faster can the same Polars q2 go over the same 5 GB of spans?

Formats (CSV, Parquet, Parquet with categoricals, Arrow IPC), engines (streaming,
in-memory), a frame a warm worker keeps in memory, and a fork server that gives
each query a fresh process without paying for the interpreter again.

Run from benchmarks/curation: `uv run python faster_python.py`.
Each variant runs in a process of its own, so a peak RSS belongs to it alone.
"""

import json
import multiprocessing
import resource
import statistics
import subprocess
import sys
import time
from pathlib import Path

BENCH = Path(__file__).resolve().parent
sys.path.insert(0, str(BENCH))

import polars as pl  # noqa: E402

from corpus import SPANS  # noqa: E402

DATA = BENCH / ".data" / "5GB"
CSV = DATA / "spans.csv" / "part-00000.csv"
FORMATS = DATA / "formats"
PARQUET = FORMATS / "spans.parquet"
PARQUET_CAT = FORMATS / "spans-categorical.parquet"
IPC = FORMATS / "spans.arrow"
LOW_CARDINALITY = ["kind", "model", "status", "agent_id", "tool", "step_type", "operation"]
NEEDED = ["kind", "model", "input_tokens", "output_tokens", "duration_ms"]
RUNS = 3


def q2(frame: pl.LazyFrame) -> pl.LazyFrame:
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
    )


def source(variant: str) -> pl.LazyFrame:
    if variant.startswith("csv"):
        return pl.scan_csv(CSV, schema=SPANS)
    if variant.startswith("parquet+categorical"):
        return pl.scan_parquet(PARQUET_CAT)
    if variant.startswith("parquet"):
        return pl.scan_parquet(PARQUET)
    return pl.scan_ipc(IPC)


def engine(variant: str) -> str:
    return "streaming" if variant.endswith("streaming") else "in-memory"


VARIANTS = [
    "csv · streaming",
    "csv · in-memory",
    "parquet · streaming",
    "parquet · in-memory",
    "parquet+categorical · streaming",
    "parquet+categorical · in-memory",
    "arrow ipc · streaming",
    "arrow ipc · in-memory",
]


def child(variant: str) -> None:
    """One variant: first run, then the median of warm runs, and this process's peak."""
    if variant == "cached frame":
        started = time.perf_counter()
        frame = pl.read_parquet(PARQUET_CAT, columns=NEEDED)
        load = time.perf_counter() - started
        work = lambda: q2(frame.lazy()).collect()  # noqa: E731
    else:
        load = 0.0
        work = lambda: q2(source(variant)).collect(engine=engine(variant))  # noqa: E731
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
                "load": load,
                "first": first,
                "warm": statistics.median(warm),
                "rss": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,
                "rows": result.with_columns(pl.col("model").cast(pl.String)).to_dicts(),
            }
        )
    )


def nothing() -> None:
    pass


def q2_on_parquet() -> None:
    q2(pl.scan_parquet(PARQUET)).collect()


def process_cost(context: str, target, times: int = 5) -> float:
    ctx = multiprocessing.get_context(context)
    if context == "forkserver":
        ctx.set_forkserver_preload(["polars"])
    warmup = ctx.Process(target=nothing)
    warmup.start()
    warmup.join()
    samples = []
    for _ in range(times):
        started = time.perf_counter()
        process = ctx.Process(target=target)
        process.start()
        process.join()
        samples.append(time.perf_counter() - started)
    return statistics.median(samples)


def gib(value: int) -> str:
    return f"{value / 1024**3:.2f} GiB" if value >= 1024**3 else f"{value / 1024**2:.0f} MiB"


def main() -> None:
    FORMATS.mkdir(exist_ok=True)
    print("converting the 5 GB CSV once (reported, not counted in the queries)")
    for label, target, write in (
        ("parquet (zstd)", PARQUET, lambda: pl.scan_csv(CSV, schema=SPANS).sink_parquet(PARQUET)),
        (
            "parquet + categoricals",
            PARQUET_CAT,
            lambda: (
                pl.scan_csv(CSV, schema=SPANS)
                .with_columns(pl.col(LOW_CARDINALITY).cast(pl.Categorical))
                .sink_parquet(PARQUET_CAT)
            ),
        ),
        (
            "arrow ipc (uncompressed)",
            IPC,
            lambda: pl.scan_csv(CSV, schema=SPANS).sink_ipc(IPC, compression=None),
        ),
    ):
        if not target.exists():
            started = time.perf_counter()
            write()
            print(
                f"  {label:<26} {time.perf_counter() - started:6.1f} s  {gib(target.stat().st_size)}",
                flush=True,
            )
        else:
            print(f"  {label:<26} (already there)  {gib(target.stat().st_size)}", flush=True)
    print(f"  {'csv':<26} {'':>8}  {gib(CSV.stat().st_size)}")

    results = {}
    for variant in [*VARIANTS, "cached frame"]:
        done = subprocess.run(
            [sys.executable, __file__, "--child", variant],
            capture_output=True,
            text=True,
            check=True,
        )
        results[variant] = json.loads(done.stdout.strip().splitlines()[-1])
        print(f"  {variant} done", flush=True)

    reference = pl.DataFrame(results["csv · streaming"]["rows"])
    print(f"\n{'q2 over 5 GB':<34}{'first run':>11}{'warm run':>11}{'peak RSS':>12}  answer")
    for variant, result in results.items():
        rows = pl.DataFrame(result["rows"])
        same = all(
            rows[c].to_list() == reference[c].to_list()
            for c in ("model", "spans", "input_tokens", "output_tokens")
        )
        same = same and (rows["avg_duration_ms"] - reference["avg_duration_ms"]).abs().max() < 1e-6
        note = f"  (load {result['load']:.2f} s once)" if result["load"] else ""
        print(
            f"{variant:<34}{result['first']:>10.3f}s{result['warm']:>10.3f}s{gib(result['rss']):>12}"
            f"  {'same' if same else 'DIFFERS'}{note}"
        )

    print("\nwhat a fresh, isolated process costs before the query (median of 5)")
    print(
        f"  {'spawn: new interpreter, import polars, nothing':<52}{process_cost('spawn', nothing):7.3f} s"
    )
    print(
        f"  {'forkserver with polars preloaded, nothing':<52}{process_cost('forkserver', nothing):7.3f} s"
    )
    print(
        f"  {'forkserver with polars preloaded, q2 on parquet':<52}{process_cost('forkserver', q2_on_parquet, 3):7.3f} s"
    )


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--child":
        child(sys.argv[2])
    else:
        main()
