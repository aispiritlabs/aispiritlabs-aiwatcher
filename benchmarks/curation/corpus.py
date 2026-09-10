"""What the benchmark reads: the synthetic `spans` corpus and the price table.

One row per completed span — the grain of the Flow query service's `spans`
dataset (`services/query/flow/src/Dataset/Catalog.php`), with the two token counts
and the status a curation step filters on. `flow_bench.php` restates this
schema in Flow's own terms; the two are kept side by side on purpose, because
each engine is handed its types rather than left to infer them, and inference
is a cost one of them would pay and the other would not.
"""

from pathlib import Path

import polars as pl

SPANS: dict[str, pl.DataType] = {
    "run_id": pl.String(),
    "trace_id": pl.String(),
    "span_id": pl.String(),
    "parent_span_id": pl.String(),
    "name": pl.String(),
    "kind": pl.String(),
    "start": pl.String(),
    "end": pl.String(),
    "duration_ms": pl.Int64(),
    "operation": pl.String(),
    "agent_id": pl.String(),
    "model": pl.String(),
    "tool": pl.String(),
    "step_type": pl.String(),
    "status": pl.String(),
    "input_tokens": pl.Int64(),
    "output_tokens": pl.Int64(),
    "error": pl.String(),
}

PRICES: dict[str, pl.DataType] = {
    "model": pl.String(),
    "provider": pl.String(),
    "usd_per_input_token": pl.Float64(),
    "usd_per_output_token": pl.Float64(),
}


def spans_glob(corpus: Path, fmt: str) -> Path:
    return corpus / f"spans.{fmt}" / f"part-*.{fmt}"


def prices_path(corpus: Path) -> Path:
    return corpus / "models.csv"
