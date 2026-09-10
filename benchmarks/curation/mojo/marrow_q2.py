"""q2 through marrow's lazy Python frontend, over the 5 GB Parquet copy.

Run inside the marrow checkout, with its own interpreter:
    cd ~/Projects/ai_spirit/marrow && PYTHONPATH=python pixi run python <this file> <parquet>
Prints the answer as CSV lines on stdout and the time on stderr as JSON, the
shape every other variant reports in.
"""

import json
import resource
import statistics
import sys
import time

import marrow as ma
from marrow import col, lit

PATH = sys.argv[1]
RUNS = 3


def q2():
    return (
        ma.read_parquet(PATH)
        .filter(col("kind") == lit("llm"))
        .aggregate(
            by=["model"],
            spans=("count", "model"),
            input_tokens=("sum", "input_tokens"),
            output_tokens=("sum", "output_tokens"),
            avg_duration_ms=("mean", "duration_ms"),
        )
        .order_by("model")
        .collect()
    )


started = time.perf_counter()
batch = q2()
first = time.perf_counter() - started
warm = []
for _ in range(RUNS):
    started = time.perf_counter()
    batch = q2()
    warm.append(time.perf_counter() - started)

print("model,spans,input_tokens,output_tokens,avg_duration_ms")
for row in batch.to_pylist():
    print(
        ",".join(
            str(row[k])
            for k in ("model", "spans", "input_tokens", "output_tokens", "avg_duration_ms")
        )
    )
print(
    json.dumps(
        {
            "engine": "marrow-python",
            "first": round(first, 3),
            "seconds": round(statistics.median(warm), 3),
            "rss": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,
        }
    ),
    file=sys.stderr,
)
