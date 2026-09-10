"""q2 over one 5 GB CSV file: the polars crate in a Rust process, Polars in Python, and marimo.

Run from services/ml_pipeline so marimo and ml_pipeline import:
    uv run python compare_5gb.py
Every variant runs in a process of its own, one after another, three times;
the median is reported with the peak resident memory of the process that did
the work. The three answers are compared before anything is printed as a result.
"""

import io
import json
import re
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import polars as pl

BENCH = Path(__file__).resolve().parent
CORPUS = BENCH / ".data" / "5GB"
FILE = CORPUS / "spans.csv" / "part-00000.csv"
RUST = BENCH / "rust" / "target" / "release" / "polars-bench"
PY = sys.executable
RUNS = 3

SCHEMA = (
    "{'run_id': pl.String, 'trace_id': pl.String, 'span_id': pl.String, 'parent_span_id': pl.String,"
    " 'name': pl.String, 'kind': pl.String, 'start': pl.String, 'end': pl.String,"
    " 'duration_ms': pl.Int64, 'operation': pl.String, 'agent_id': pl.String, 'model': pl.String,"
    " 'tool': pl.String, 'step_type': pl.String, 'status': pl.String, 'input_tokens': pl.Int64,"
    " 'output_tokens': pl.Int64, 'error': pl.String}"
)
Q2 = (
    f"pl.scan_csv('{FILE}', schema={SCHEMA}).filter(pl.col('kind') == 'llm').group_by('model')"
    ".agg(spans=pl.len(), input_tokens=pl.col('input_tokens').sum(),"
    " output_tokens=pl.col('output_tokens').sum(), avg_duration_ms=pl.col('duration_ms').mean())"
    ".sort('model').collect(engine='streaming')"
)
# A fresh Python process: import, query, answer on stdout, seconds on stderr.
PYTHON_FRESH = f"""
import sys, time
import polars as pl
started = time.perf_counter()
frame = {Q2}
seconds = time.perf_counter() - started
frame.write_csv(sys.stdout)
print('{{"seconds": %.3f}}' % seconds, file=sys.stderr)
"""
# A warm one: the query once to warm it, then timed; the process's own peak.
PYTHON_WARM = f"""
import resource, sys, time
import polars as pl
{Q2}
samples = []
for _ in range({RUNS}):
    started = time.perf_counter()
    frame = {Q2}
    samples.append(time.perf_counter() - started)
frame.write_csv(sys.stdout)
samples.sort()
print('{{"seconds": %.3f, "rss": %d}}' % (samples[len(samples) // 2],
      resource.getrusage(resource.RUSAGE_SELF).ru_maxrss), file=sys.stderr)
"""
# marimo: the production path, run_notebook → the step subprocess → App.run.
# RUSAGE_CHILDREN is read in a launcher whose only children are steps, so the
# peak is the step's.
MARIMO = f"""
import json, os, resource, statistics, sys, tempfile, time
from pathlib import Path
os.environ["AIWATCHER_CORPUS_DIR"] = "{CORPUS}"
from ml_pipeline.config import SERVICE_ROOT, Config
from ml_pipeline.notebooks import NotebookDirectory
from ml_pipeline.runner import run_notebook
from ml_pipeline.staging import Staging
scratch = Path(tempfile.mkdtemp())
config = Config(notebooks=SERVICE_ROOT / "notebooks", revisions=scratch / "r", data=scratch / "d",
                timeout_seconds=900.0)
notebook = NotebookDirectory(root=config.notebooks, revisions=config.revisions).get_notebook("flow_vs_polars")
walls, steps, queries = [], [], []
for _ in range({RUNS}):
    started = time.perf_counter()
    result = run_notebook(notebook, [], {{"format": "csv"}}, Staging(root=config.data), config)
    walls.append(time.perf_counter() - started)
    steps.append(result.took_ms / 1000)
    queries.append(float(result.rows[0]["polars_seconds"]))
rows = [{{"model": r["model"], "spans": r["polars_spans"], "input_tokens": r["polars_input_tokens"],
          "output_tokens": r["polars_output_tokens"], "avg_duration_ms": r["polars_avg_duration_ms"]}}
         for r in result.rows]
print(json.dumps(rows))
print(json.dumps({{"wall": statistics.median(walls), "step": statistics.median(steps),
                  "seconds": statistics.median(queries),
                  "rss": resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss}}), file=sys.stderr)
"""


def timed(command: list[str]) -> tuple[str, str, float, int]:
    """stdout, stderr, wall seconds and peak RSS in bytes, under /usr/bin/time -l."""
    started = time.perf_counter()
    done = subprocess.run(
        ["/usr/bin/time", "-l", *command], capture_output=True, text=True, check=True
    )
    wall = time.perf_counter() - started
    rss = int(re.search(r"(\d+)\s+maximum resident set size", done.stderr).group(1))
    return done.stdout, done.stderr, wall, rss


def last_json(stderr: str) -> dict:
    return json.loads(next(line for line in reversed(stderr.splitlines()) if line.startswith("{")))


def frame_of_csv(text: str) -> pl.DataFrame:
    return pl.read_csv(io.StringIO(text)).sort("model")


def gib(value: int) -> str:
    return f"{value / 1024**3:.2f} GiB" if value >= 1024**3 else f"{value / 1024**2:.0f} MiB"


results: dict[str, dict] = {}

print(f"q2 over {FILE.name}: {FILE.stat().st_size / 1024**3:.2f} GiB, one file", flush=True)

for label, binary in (
    ("Rust process, tuned build", RUST),
    ("Rust process, naive build", RUST.with_name("polars-bench-naive")),
):
    samples = [timed([str(binary), str(FILE)]) for _ in range(RUNS)]
    results[label] = {
        "wall": statistics.median(s[2] for s in samples),
        "seconds": statistics.median(last_json(s[1])["seconds"] for s in samples),
        "rss": max(s[3] for s in samples),
        "rows": frame_of_csv(samples[-1][0]),
    }
    print(f"  {label} done", flush=True)

# The query written to a file and run as a script: a new interpreter, `import
# polars`, the query, gone — what an engine that shells out per query pays.
with tempfile.NamedTemporaryFile("w", prefix="tmp_query_", suffix=".py", delete=False) as script:
    script.write(PYTHON_FRESH)
samples = [timed([PY, script.name]) for _ in range(RUNS)]
results["tmp_file.py, one process a query"] = {
    "wall": statistics.median(s[2] for s in samples),
    "seconds": statistics.median(last_json(s[1])["seconds"] for s in samples),
    "rss": max(s[3] for s in samples),
    "rows": frame_of_csv(samples[-1][0]),
}
print("  tmp_file.py done", flush=True)

# One interpreter kept alive, Polars imported once, each query exec()'d in a
# fresh namespace and answered over a pipe — what a warm worker pays. Timed
# from the sender's side, so the pipe and the CSV encoding are in the number.
WORKER = """
import io, json, resource, sys, time
import polars as pl
for line in sys.stdin:
    request = json.loads(line)
    if request.get("stop"):
        print(json.dumps({"rss": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss}), flush=True)
        break
    namespace = {"pl": pl}
    started = time.perf_counter()
    exec(compile(request["code"], "<query>", "exec"), namespace)
    seconds = time.perf_counter() - started
    buffer = io.StringIO()
    namespace["result"].write_csv(buffer)
    print(json.dumps({"seconds": seconds, "csv": buffer.getvalue()}), flush=True)
"""
worker = subprocess.Popen(
    [PY, "-u", "-c", WORKER], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True
)
if worker.stdin is None or worker.stdout is None:
    raise SystemExit("the interpreter worker started without its pipes")


def ask(code: str) -> tuple[float, dict]:
    started = time.perf_counter()
    worker.stdin.write(json.dumps({"code": code}) + "\n")
    worker.stdin.flush()
    answer = json.loads(worker.stdout.readline())
    return time.perf_counter() - started, answer


first_wall, first = ask(f"result = {Q2}")
later = [ask(f"result = {Q2}") for _ in range(RUNS)]
worker.stdin.write(json.dumps({"stop": True}) + "\n")
worker.stdin.flush()
worker_rss = json.loads(worker.stdout.readline())["rss"]
worker.wait()
results["interpreter, first query"] = {
    "wall": first_wall,
    "seconds": first["seconds"],
    "rss": worker_rss,
    "rows": frame_of_csv(first["csv"]),
}
results["interpreter, next queries"] = {
    "wall": statistics.median(w for w, _ in later),
    "seconds": statistics.median(a["seconds"] for _, a in later),
    "rss": worker_rss,
    "rows": frame_of_csv(later[-1][1]["csv"]),
}
print("  interpreter done", flush=True)

done = subprocess.run([PY, "-c", MARIMO], capture_output=True, text=True, check=True)
marimo = last_json(done.stderr)
results["marimo step (run_notebook)"] = {
    "wall": marimo["wall"],
    "step": marimo["step"],
    "seconds": marimo["seconds"],
    "rss": marimo["rss"],
    "rows": pl.DataFrame(json.loads(done.stdout.strip().splitlines()[-1])).sort("model"),
}
print("  marimo done\n", flush=True)

reference = results["Rust process, tuned build"]["rows"]
print(f"{'':32}{'wall':>10}{'query':>10}{'overhead':>10}{'peak RSS':>12}  answer")
for label, result in results.items():
    rows = result["rows"].select(reference.columns).cast(reference.schema)
    same = rows.equals(reference.with_columns(pl.col("avg_duration_ms").round(6))) or (
        all(
            rows[c].to_list() == reference[c].to_list()
            for c in ("model", "spans", "input_tokens", "output_tokens")
        )
        and (rows["avg_duration_ms"] - reference["avg_duration_ms"]).abs().max() < 1e-6
    )
    print(
        f"{label:<32}{result['wall']:>9.2f}s{result['seconds']:>9.2f}s"
        f"{result['wall'] - result['seconds']:>9.2f}s{gib(result['rss']):>12}  {'same' if same else 'DIFFERS'}"
    )
print(f"\nmarimo's step subprocess alone: {results['marimo step (run_notebook)']['step']:.2f} s")
print(reference)
