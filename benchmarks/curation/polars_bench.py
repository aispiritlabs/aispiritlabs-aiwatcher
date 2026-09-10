"""Polars' half of the benchmark: one query, one process, one JSON line out.

Every query is lazy and collected on the streaming engine, which is how Polars
reads a corpus larger than it means to hold; each has a twin in
`flow_bench.php` that computes the same answer, and `bench.py` checks that
they did before it reports a time for either.
"""

import argparse
import json
import time
from collections.abc import Callable
from pathlib import Path

import polars as pl

from corpus import PRICES, SPANS, prices_path, spans_glob

type Query = Callable[[pl.LazyFrame, Path, Path], object]


def scan(corpus: Path, fmt: str) -> pl.LazyFrame:
    if fmt == "csv":
        return pl.scan_csv(spans_glob(corpus, "csv"), schema=SPANS)
    return pl.scan_parquet(spans_glob(corpus, "parquet"))


def q1_filter_count(spans: pl.LazyFrame, _corpus: Path, _out: Path) -> object:
    """Failed spans slower than a second: a scan and a predicate, nothing held."""
    failed = spans.filter((pl.col("status") == "error") & (pl.col("duration_ms") > 1000))
    return {"count": failed.select(pl.len()).collect(engine="streaming").item()}


def q2_groupby_model(spans: pl.LazyFrame, _corpus: Path, _out: Path) -> object:
    """Tokens and latency per model: eight groups, the Query tab's first question."""
    per_model = (
        spans.filter(pl.col("kind") == "llm")
        .group_by("model")
        .agg(
            spans=pl.len(),
            input_tokens=pl.col("input_tokens").sum(),
            output_tokens=pl.col("output_tokens").sum(),
            avg_duration_ms=pl.col("duration_ms").mean(),
        )
        .sort("model")
    )
    return per_model.collect(engine="streaming").to_dicts()


def q3_groupby_run(spans: pl.LazyFrame, _corpus: Path, out: Path) -> object:
    """One row per run: millions of groups, which is what a hash table is for."""
    target = out / "q3.csv"
    spans.group_by("run_id").agg(
        spans=pl.len(),
        input_tokens=pl.col("input_tokens").sum(),
        output_tokens=pl.col("output_tokens").sum(),
        max_duration_ms=pl.col("duration_ms").max(),
    ).sink_csv(target)
    return {"output": str(target)}


def q4_curate(spans: pl.LazyFrame, corpus: Path, out: Path) -> object:
    """The curation shape: filter, enrich by a join, derive, project, write."""
    target = out / "q4.csv"
    prices = pl.scan_csv(prices_path(corpus), schema=PRICES)
    spans.filter((pl.col("status") == "ok") & (pl.col("kind") == "llm")).join(
        prices, on="model", how="inner"
    ).select(
        "span_id",
        "run_id",
        day=pl.col("start").str.slice(0, 10),
        agent_id="agent_id",
        model="model",
        provider="provider",
        total_tokens=pl.col("input_tokens") + pl.col("output_tokens"),
        cost_usd=pl.col("input_tokens") * pl.col("usd_per_input_token")
        + pl.col("output_tokens") * pl.col("usd_per_output_token"),
        duration_ms="duration_ms",
    ).sink_csv(target)
    return {"output": str(target)}


QUERIES: dict[str, Query] = {
    "q1": q1_filter_count,
    "q2": q2_groupby_model,
    "q3": q3_groupby_run,
    "q4": q4_curate,
}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--query", choices=QUERIES, required=True)
    parser.add_argument("--corpus", type=Path, required=True)
    parser.add_argument("--format", choices=["csv", "parquet"], default="csv")
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()

    args.out.mkdir(parents=True, exist_ok=True)
    started = time.perf_counter()
    result = QUERIES[args.query](scan(args.corpus, args.format), args.corpus, args.out)
    seconds = time.perf_counter() - started
    print(
        json.dumps(
            {
                "engine": "polars",
                "query": args.query,
                "format": args.format,
                "seconds": round(seconds, 3),
                "threads": pl.thread_pool_size(),
                "version": pl.__version__,
                "result": result,
            }
        )
    )


if __name__ == "__main__":
    main()
