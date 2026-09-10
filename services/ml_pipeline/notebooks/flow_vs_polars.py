"""Run the Flow block's aggregation again in Polars, over the same files, and compare.

The notebook half of `curation/flow-vs-polars`. The block before it is Flow PHP
reading `corpus_spans` and aggregating it per model; this one reads the same
part files with Polars' streaming engine, computes the same aggregation, and
hands on one row per model carrying both answers and whether they are the same
answer. A faster engine that computed something else has not been measured.

What each engine took is in the Workflows waterfall, where the two steps sit
side by side. This block also reports the seconds Polars spent in its query
alone, because the step's span holds a subprocess start and a JSON round trip
that are not Polars.

The corpus root is `AIWATCHER_CORPUS_DIR` — the variable the Flow service reads
for the same dataset — so one setting points both engines at one set of files.
`deterministic = False`: the files are addressed by nothing, and a time differs
on every run.
"""

import marimo

__generated_with = "0.24.0"
app = marimo.App(width="medium")


@app.cell
def _():
    import os
    import time
    from pathlib import Path

    import marimo as mo
    import polars as pl

    from ml_pipeline import Block

    return Block, Path, mo, os, pl, time


@app.cell
def _(Block):
    # The one injected cell. Running as a step, `App.run(defs=…)` provides both
    # names and this cell is not executed at all; open in the panel, it reads
    # the rows the last run staged. `_block` is underscored so the cell defines
    # nothing else — an injected name replaces its whole cell.
    _block = Block.for_notebook(__file__)
    rows = _block.get_rows()
    params = _block.get_params()
    return params, rows


@app.cell
def _(Path, os, params, pl):
    # Typed on read with the columns the Flow dataset declares. Handing one
    # engine its types and leaving the other to infer them would measure
    # inference, which is not the question.
    _schema = {
        "run_id": pl.String,
        "trace_id": pl.String,
        "span_id": pl.String,
        "parent_span_id": pl.String,
        "name": pl.String,
        "kind": pl.String,
        "start": pl.String,
        "end": pl.String,
        "duration_ms": pl.Int64,
        "operation": pl.String,
        "agent_id": pl.String,
        "model": pl.String,
        "tool": pl.String,
        "step_type": pl.String,
        "status": pl.String,
        "input_tokens": pl.Int64,
        "output_tokens": pl.Int64,
        "error": pl.String,
    }
    _root = os.environ.get("AIWATCHER_CORPUS_DIR")
    if not _root:
        # Refused rather than answered with an empty table: "Polars found no
        # models" beside Flow's eight would read as a disagreement about data.
        raise RuntimeError(
            "AIWATCHER_CORPUS_DIR is not set for the notebook runtime. It has to name the "
            "directory the Flow service reads corpus_spans from; `just bench-curation-serve` "
            "sets it for both."
        )
    corpus_format = str(params.get("format") or "csv")
    _pattern = Path(_root) / f"spans.{corpus_format}" / f"part-*.{corpus_format}"
    spans = (
        pl.scan_csv(_pattern, schema=_schema)
        if corpus_format == "csv"
        else pl.scan_parquet(_pattern)
    )
    return corpus_format, spans


@app.cell
def _(pl, spans, time):
    # The Flow block's query, in Polars: the LLM spans, per model, how many,
    # how many tokens each way and the mean latency.
    _started = time.perf_counter()
    polars_rows = (
        spans.filter(pl.col("kind") == "llm")
        .group_by("model")
        .agg(
            spans=pl.len(),
            input_tokens=pl.col("input_tokens").sum(),
            output_tokens=pl.col("output_tokens").sum(),
            avg_duration_ms=pl.col("duration_ms").mean(),
        )
        .sort("model")
        .collect(engine="streaming")
        .to_dicts()
    )
    polars_seconds = time.perf_counter() - _started
    return polars_rows, polars_seconds


@app.cell
def _(polars_rows, polars_seconds, rows):
    # One row per model with both answers side by side. Counts and sums must be
    # equal; a mean within half a hundredth, because Flow's `average()` keeps
    # two decimals (half up, in BigDecimal) and Polars does not round at all.
    _flow = {str(row.get("model")): row for row in rows}
    _polars = {str(row["model"]): row for row in polars_rows}
    output = []
    for _model in sorted(_flow.keys() | _polars.keys()):
        _f = _flow.get(_model, {})
        _p = _polars.get(_model, {})
        _agrees = (
            bool(_f)
            and bool(_p)
            and all(_f.get(_key) == _p.get(_key) for _key in ("spans", "input_tokens", "output_tokens"))
            and abs(float(_f.get("avg_duration_ms") or 0) - float(_p.get("avg_duration_ms") or 0))
            <= 0.005 + 1e-9
        )
        output.append(
            {
                "model": _model,
                "flow_spans": _f.get("spans"),
                "polars_spans": _p.get("spans"),
                "flow_input_tokens": _f.get("input_tokens"),
                "polars_input_tokens": _p.get("input_tokens"),
                "flow_output_tokens": _f.get("output_tokens"),
                "polars_output_tokens": _p.get("output_tokens"),
                "flow_avg_duration_ms": _f.get("avg_duration_ms"),
                "polars_avg_duration_ms": (
                    None if _p.get("avg_duration_ms") is None else round(float(_p["avg_duration_ms"]), 6)
                ),
                "agrees": _agrees,
                "polars_seconds": round(polars_seconds, 3),
            }
        )
    deterministic = False
    return deterministic, output


@app.cell
def _(corpus_format, mo, output, polars_seconds, rows):
    _agreed = bool(output) and all(row["agrees"] for row in output)
    mo.hstack(
        [
            mo.stat(label="Models", value=len(output)),
            mo.stat(label="Rows from Flow", value=len(rows)),
            mo.stat(label=f"Polars over the {corpus_format.upper()}", value=f"{polars_seconds:.2f} s"),
            mo.stat(label="The two answers", value="agree" if _agreed else "differ"),
        ],
        gap=1,
    )
    return


@app.cell
def _(mo, output):
    mo.ui.table(output, page_size=10)
    return


if __name__ == "__main__":
    app.run()
