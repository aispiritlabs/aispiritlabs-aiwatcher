# The curation benchmark's q2, as a compiled Mojo program on marrow.
#
# The same question Polars, DataFusion and Flow answer — the LLM spans per
# model, their token sums and mean latency — written against marrow's Mojo
# expression layer. Types are fixed at compile time: `col("model", string)` is
# a string column in the program's own type, not a name looked up at run time.
#
# The schema handed to `scan` *is* the projection: marrow reads only these five
# columns out of the Parquet file, so there is no separate `select`.
#
#   cd ~/Projects/ai_spirit/marrow
#   pixi run mojo build -O3 -I . <this file> -o q2
#   ./q2 <spans.parquet>

from std.sys import argv
from std.time import perf_counter_ns

from marrow.dtypes import field, int64, string
from marrow.expr import col, count_star, lit, render_csv, scan
from marrow.schema import schema


def main() raises:
    var args = argv()
    if len(args) < 2:
        print("usage: q2 <spans.parquet>")
        return
    var path = String(args[1])

    var started = perf_counter_ns()
    var spans = scan(
        path,
        schema(
            [
                field("kind", string),
                field("model", string),
                field("input_tokens", int64),
                field("output_tokens", int64),
                field("duration_ms", int64),
            ]
        ),
    )
    var per_model = spans.filter(col("kind", string) == lit("llm", string)).aggregate(
        [
            count_star().alias("spans"),
            col("input_tokens", int64).sum().alias("input_tokens"),
            col("output_tokens", int64).sum().alias("output_tokens"),
            col("duration_ms", int64).mean().alias("avg_duration_ms"),
        ],
        [col("model", string)],
    )
    var out = per_model.execute()
    var seconds = Float64(perf_counter_ns() - started) / 1e9

    print(render_csv(out))
    print('{"engine":"marrow-mojo","seconds":', seconds, "}")
