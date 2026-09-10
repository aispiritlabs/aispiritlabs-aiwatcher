"""Run both engines over one corpus, check they agree, and write the numbers down.

    uv run python generate.py --size 10GB
    uv run python bench.py run --size 10GB
    uv run python bench.py run --size 10GB --format parquet
    uv run python bench.py report --size 10GB

Every (engine, query) is its own process under `/usr/bin/time`, so a peak
resident set belongs to one query and a warm interpreter carries nothing from
the one before. The corpus is read once before the first query so both engines
start from the same page cache — at 10 GB on a machine with room for it, the
comparison is the engines', not the disk's.

A time is only reported beside a verdict: each query's answer is compared
across engines, exactly for counts and sums and within a tolerance for means
and money, because a faster engine that computed something else has not been
measured at all.
"""

import argparse
import json
import os
import platform
import re
import subprocess
import sys
import time
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any

import polars as pl
from polars.testing import assert_frame_equal

HERE = Path(__file__).parent
QUERIES = {
    "q1": "filter + count",
    "q2": "group by model (8 groups)",
    "q3": "group by run (millions of groups) → CSV",
    "q4": "filter + join + derive → CSV",
}
# Flow at its best, and the report says what that was. Measured on the 100 MB
# corpus: tracing JIT takes q1 from 9.2 s to 7.0 s and q4 from 25.9 s to 19.1 s
# (`function` JIT lands between); Flow's own batch of 1000 beats 10 000, which
# costs 5× the memory and runs slower. AArch64 refuses a JIT buffer above 128M.
PHP_FLAGS = [
    "-d", "memory_limit=32G",
    "-d", "opcache.enable_cli=1",
    "-d", "opcache.jit=tracing",
    "-d", "opcache.jit_buffer_size=128M",
]  # fmt: skip
ENGINES: dict[str, dict[str, Any]] = {
    "polars": {"label": "Polars (all cores)"},
    "polars-1t": {"label": "Polars (1 thread)", "env": {"POLARS_MAX_THREADS": "1"}},
    "flow": {"label": "Flow PHP (JIT)"},
    "flow-nojit": {"label": "Flow PHP (no JIT)"},
}


@dataclass
class Run:
    engine: str
    query: str
    status: str  # ok | failed | timeout
    seconds: float | None  # what the engine measured around its query
    wall: float  # what this harness measured, process start included
    cpu: float | None  # user + sys
    max_rss: int | None  # bytes
    result: Any = None
    error: str | None = None


def command(engine: str, query: str, corpus: Path, fmt: str, out: Path) -> list[str]:
    common = ["--query", query, "--corpus", str(corpus), "--format", fmt, "--out", str(out)]
    if engine.startswith("polars"):
        return [sys.executable, str(HERE / "polars_bench.py"), *common]
    flags = [*PHP_FLAGS, "-d", "opcache.jit=disable"] if engine == "flow-nojit" else PHP_FLAGS
    return ["php", *flags, str(HERE / "flow_bench.php"), *common]


def timed(
    cmd: list[str], env: dict[str, str], timeout: float
) -> tuple[subprocess.CompletedProcess[str] | None, float, float | None, int | None]:
    """Run under /usr/bin/time; BSD's `-l` and GNU's `-v` report the same two facts."""
    darwin = platform.system() == "Darwin"
    wrapped = ["/usr/bin/time", "-l" if darwin else "-v", *cmd]
    started = time.perf_counter()
    try:
        done = subprocess.run(
            wrapped, capture_output=True, text=True, env=env, timeout=timeout, check=False
        )
    except subprocess.TimeoutExpired:
        return None, time.perf_counter() - started, None, None
    wall = time.perf_counter() - started
    err = done.stderr
    if darwin:
        cpu_match = re.search(r"([\d.]+) real\s+([\d.]+) user\s+([\d.]+) sys", err)
        rss_match = re.search(r"(\d+)\s+maximum resident set size", err)
        cpu = float(cpu_match.group(2)) + float(cpu_match.group(3)) if cpu_match else None
        rss = int(rss_match.group(1)) if rss_match else None
    else:
        user = re.search(r"User time \(seconds\): ([\d.]+)", err)
        system = re.search(r"System time \(seconds\): ([\d.]+)", err)
        rss_kb = re.search(r"Maximum resident set size \(kbytes\): (\d+)", err)
        cpu = float(user.group(1)) + float(system.group(1)) if user and system else None
        rss = int(rss_kb.group(1)) * 1024 if rss_kb else None
    return done, wall, cpu, rss


def execute(engine: str, query: str, corpus: Path, fmt: str, out: Path, timeout: float) -> Run:
    env = {**os.environ, **ENGINES[engine].get("env", {})}
    done, wall, cpu, rss = timed(command(engine, query, corpus, fmt, out), env, timeout)
    if done is None:
        return Run(engine, query, "timeout", None, wall, None, None, error=f"over {timeout:.0f} s")
    line = next((raw for raw in reversed(done.stdout.splitlines()) if raw.startswith("{")), None)
    if done.returncode != 0 or line is None:
        tail = "\n".join((done.stdout + done.stderr).strip().splitlines()[-6:])
        return Run(engine, query, "failed", None, wall, cpu, rss, error=tail)
    payload = json.loads(line)
    return Run(engine, query, "ok", payload["seconds"], wall, cpu, rss, result=payload["result"])


def warm(corpus: Path, fmt: str) -> float:
    """Read every part once, so the first engine does not pay for the disk alone."""
    started = time.perf_counter()
    for part in sorted((corpus / f"spans.{fmt}").glob("part-*")):
        with part.open("rb") as handle:
            while handle.read(16 * 1024 * 1024):
                pass
    return time.perf_counter() - started


# ── Agreement ─────────────────────────────────────────────────────────────────

Q3_SCHEMA = {
    "run_id": pl.String,
    "spans": pl.Int64,
    "input_tokens": pl.Int64,
    "output_tokens": pl.Int64,
    "max_duration_ms": pl.Int64,
}
Q4_SCHEMA = {
    "span_id": pl.String,
    "run_id": pl.String,
    "day": pl.String,
    "agent_id": pl.String,
    "model": pl.String,
    "provider": pl.String,
    "total_tokens": pl.Int64,
    "cost_usd": pl.Float64,
    "duration_ms": pl.Int64,
}


def comparable(run: Run) -> pl.DataFrame | int:
    match run.query:
        case "q1":
            return int(run.result["count"])
        case "q2":
            return (
                pl.DataFrame(run.result)
                .select("model", "spans", "input_tokens", "output_tokens", "avg_duration_ms")
                .sort("model")
            )
        case "q3":
            return pl.read_csv(run.result["output"], schema=Q3_SCHEMA).sort("run_id")
        case "q4":
            return pl.read_csv(run.result["output"], schema=Q4_SCHEMA).sort("span_id")
    raise ValueError(run.query)


def agree(reference: Run, other: Run) -> str | None:
    """None when the two answers are the same answer, else what differs."""
    expected, actual = comparable(reference), comparable(other)
    if isinstance(expected, int) or isinstance(actual, int):
        return None if expected == actual else f"count {actual} ≠ {expected}"
    try:
        # Integers exactly; floats to a millionth relative. Flow's arithmetic
        # is BigDecimal and Polars' is float64, so a cost can differ in the
        # last place, and a mean differs by whatever scale Flow rounds to.
        assert_frame_equal(expected, actual, check_exact=False, rel_tol=1e-6, abs_tol=1e-6)
    except AssertionError as difference:
        return str(difference).splitlines()[0][:300]
    return None


def verify(runs: list[Run]) -> dict[str, dict[str, str]]:
    verdicts: dict[str, dict[str, str]] = {}
    for query in QUERIES:
        finished = [run for run in runs if run.query == query and run.status == "ok"]
        if not finished:
            continue
        reference = next((run for run in finished if run.engine == "polars"), finished[0])
        verdicts[query] = {}
        for run in finished:
            if run.engine == reference.engine:
                verdicts[query][run.engine] = "reference"
                continue
            problem = agree(reference, run)
            verdicts[query][run.engine] = "agrees" if problem is None else f"DIFFERS: {problem}"
    return verdicts


# ── Report ────────────────────────────────────────────────────────────────────


def sysctl(key: str) -> str:
    return subprocess.run(
        ["sysctl", "-n", key], capture_output=True, text=True, check=False
    ).stdout.strip()


def machine() -> dict[str, str]:
    facts = {"os": f"{platform.system()} {platform.release()}", "python": platform.python_version()}
    if platform.system() == "Darwin":
        facts["cpu"] = sysctl("machdep.cpu.brand_string")
        facts["cores"] = sysctl("hw.ncpu")
        facts["memory_gib"] = str(int(sysctl("hw.memsize") or 0) // 1024**3)
    php = subprocess.run(
        ["php", "-r", "echo PHP_VERSION;"], capture_output=True, text=True, check=False
    )
    facts["php"] = php.stdout.strip()
    facts["polars"] = pl.__version__
    lock = json.loads((HERE / "composer.lock").read_text())
    facts["flow"] = next(p["version"] for p in lock["packages"] if p["name"] == "flow-php/etl")
    return facts


def human_bytes(value: int | None) -> str:
    if value is None:
        return "—"
    for unit, size in (("GiB", 1024**3), ("MiB", 1024**2)):
        if value >= size:
            return f"{value / size:.1f} {unit}"
    return f"{value / 1024:.0f} KiB"


def human_seconds(value: float | None) -> str:
    if value is None:
        return "—"
    if value >= 60:
        minutes, seconds = divmod(value, 60)
        return f"{int(minutes)} min {seconds:02.0f} s"
    return f"{value:.2f} s"


def render(results: list[dict[str, Any]]) -> str:
    first = results[0]
    corpus = first["corpus"]
    lines = [
        f"# Flow PHP and Polars over {first['size']} of spans",
        "",
        f"Generated by `bench.py` on {first['finished_at'][:10]}. "
        f"{corpus['rows']:,} rows, {human_bytes(corpus['csv_bytes'])} of CSV"
        + (
            f" and {human_bytes(corpus['parquet_bytes'])} of Parquet"
            if corpus.get("parquet_bytes")
            else ""
        )
        + f" in {corpus['parts']} parts; {corpus['runs']:,} runs.",
        "",
        "| machine | |",
        "|---|---|",
        *(f"| {key} | {value} |" for key, value in first["machine"].items()),
        "",
    ]
    for result in results:
        runs = [Run(**run) for run in result["runs"]]
        engines = [engine for engine in ENGINES if any(run.engine == engine for run in runs)]
        input_bytes = corpus["csv_bytes"] if result["format"] == "csv" else corpus["parquet_bytes"]
        lines += [
            f"## {result['format'].upper()}",
            "",
            "Query time as the engine measured it (process start-up excluded), "
            "then peak resident memory.",
            "",
            "| query | " + " | ".join(ENGINES[e]["label"] for e in engines) + " | Flow ÷ Polars |",
            "|---|" + "---|" * (len(engines) + 1),
        ]
        for query, title in QUERIES.items():
            cells = []
            by_engine = {run.engine: run for run in runs if run.query == query}
            if not by_engine:
                continue
            for engine in engines:
                run = by_engine.get(engine)
                if run is None:
                    cells.append("—")
                elif run.status != "ok":
                    cells.append(f"**{run.status}**")
                else:
                    verdict = result["verification"].get(query, {}).get(engine, "")
                    mark = "" if verdict in ("agrees", "reference") else " ⚠"
                    cells.append(f"{human_seconds(run.seconds)} · {human_bytes(run.max_rss)}{mark}")
            flow, polars = by_engine.get("flow"), by_engine.get("polars")
            ratio = (
                f"{flow.seconds / polars.seconds:,.0f}×"
                if flow and polars and flow.seconds and polars.seconds
                else "—"
            )
            lines.append(f"| {query} — {title} | " + " | ".join(cells) + f" | {ratio} |")
        lines += ["", "Throughput over the input, from the same runs:", ""]
        for engine in engines:
            finished = [
                run for run in runs if run.engine == engine and run.status == "ok" and run.seconds
            ]
            if finished:
                slowest = max(finished, key=lambda run: run.seconds or 0)
                fastest = min(finished, key=lambda run: run.seconds or 0)
                mb = input_bytes / 1024**2
                lines.append(
                    f"- {ENGINES[engine]['label']}: {mb / (slowest.seconds or 1):,.0f}–"
                    f"{mb / (fastest.seconds or 1):,.0f} MiB/s, "
                    f"{corpus['rows'] / (slowest.seconds or 1):,.0f}–"
                    f"{corpus['rows'] / (fastest.seconds or 1):,.0f} rows/s"
                )
        lines += ["", "Agreement, against Polars' answer:", ""]
        for query, verdicts in result["verification"].items():
            lines.append(f"- {query}: " + ", ".join(f"{e} {v}" for e, v in verdicts.items()))
        failures = [run for run in runs if run.status != "ok"]
        for run in failures:
            lines += [
                "",
                f"`{run.engine}` {run.query} {run.status}:",
                "",
                "```",
                run.error or "",
                "```",
            ]
        lines.append("")
    return "\n".join(lines)


# ── Commands ──────────────────────────────────────────────────────────────────


def results_path(size: str, fmt: str) -> Path:
    return HERE / "results" / f"{size}-{fmt}.json"


def cmd_run(args: argparse.Namespace) -> None:
    size = args.size.upper()
    corpus = (args.data_dir / size).resolve()
    manifest = corpus / "manifest.json"
    if not manifest.exists():
        sys.exit(f"no corpus at {corpus} — run: uv run python generate.py --size {args.size}")
    if any(e.startswith("flow") for e in args.engines) and not (HERE / "vendor").exists():
        sys.exit("Flow is not installed — run: composer install (in benchmarks/curation)")

    for fmt in args.format:
        print(f"── {size} {fmt}: warming the page cache", flush=True)
        print(f"   read in {warm(corpus, fmt):.1f} s", flush=True)
        runs: list[Run] = []
        for query in args.queries:
            for engine in args.engines:
                out = HERE / "results" / "outputs" / size / fmt / engine
                out.mkdir(parents=True, exist_ok=True)
                run = execute(engine, query, corpus, fmt, out, args.timeout)
                runs.append(run)
                shown = human_seconds(run.seconds) if run.status == "ok" else run.status
                print(
                    f"   {query} {engine:<11} {shown:>12}  peak {human_bytes(run.max_rss)}",
                    flush=True,
                )
                if run.error:
                    print("     " + run.error.replace("\n", "\n     "), flush=True)
        # Engines may be run separately — Flow for an hour, Polars for seconds on
        # a quiet machine — so a run replaces only its own (engine, query) pairs
        # and agreement is decided over everything recorded.
        if results_path(size, fmt).exists():
            recorded = json.loads(results_path(size, fmt).read_text())["runs"]
            runs = [
                Run(**run)
                for run in recorded
                if not (run["engine"] in args.engines and run["query"] in args.queries)
            ] + runs
        print("   verifying", flush=True)
        verdicts = verify(runs)
        for query, answer in verdicts.items():
            print(f"   {query}: {answer}", flush=True)
        results_path(size, fmt).parent.mkdir(parents=True, exist_ok=True)
        results_path(size, fmt).write_text(
            json.dumps(
                {
                    "size": size,
                    "format": fmt,
                    "finished_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
                    "machine": machine(),
                    "corpus": json.loads(manifest.read_text()),
                    "php_flags": PHP_FLAGS,
                    "runs": [asdict(run) for run in runs],
                    "verification": verdicts,
                },
                indent=2,
            )
            + "\n"
        )
    cmd_report(args)


def cmd_report(args: argparse.Namespace) -> None:
    size = args.size.upper()
    results = [
        json.loads(results_path(size, fmt).read_text())
        for fmt in ("csv", "parquet")
        if results_path(size, fmt).exists()
    ]
    if not results:
        sys.exit(f"no results for {size} yet — run: uv run python bench.py run --size {args.size}")
    target = HERE / "results" / f"{size}.md"
    target.write_text(render(results))
    print(f"wrote {target.relative_to(HERE)}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)

    run = commands.add_parser(
        "run", help="run the queries, verify, write results/<size>-<format>.json"
    )
    run.add_argument("--size", default="10GB")
    run.add_argument("--data-dir", type=Path, default=HERE / ".data")
    run.add_argument("--format", nargs="+", choices=["csv", "parquet"], default=["csv"])
    run.add_argument("--queries", nargs="+", choices=list(QUERIES), default=list(QUERIES))
    run.add_argument(
        "--engines", nargs="+", choices=list(ENGINES), default=["polars", "polars-1t", "flow"]
    )
    run.add_argument("--timeout", type=float, default=4 * 3600, help="seconds per query")
    run.set_defaults(handler=cmd_run)

    report = commands.add_parser("report", help="re-render results/<size>.md from the JSON")
    report.add_argument("--size", default="10GB")
    report.set_defaults(handler=cmd_report)

    args = parser.parse_args()
    args.handler(args)


if __name__ == "__main__":
    main()
