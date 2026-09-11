"""Run the benchmark pipeline on a running aiwatcher and print what each engine took.

    just bench-curation-serve 1GB     # one terminal: API, Flow, notebooks, panel
    just bench-curation-run           # another: start it, wait, print the steps

Starts a managed execution of the pipeline the seed installed and follows it by
re-reading `GET /executions/{id}` until it ends. Then it reads two answers
aiwatcher already holds: each step's duration from the workflow fold — the
numbers the Workflows waterfall draws — and the comparison rows from the
dataset version the view block published. Nothing here times anything; the
times are aiwatcher's, which is the point of running it there.

Which variant runs is the query engine's answer: `/query/healthz` names the
engine, and the pipeline written for it is `curation/flow-vs-polars` on Flow,
`curation/flow-vs-polars-datafusion` on DataFusion and
`curation/flow-vs-polars-duckdb` on DuckDB (AW-3). Asking beats a flag,
because a flag can disagree with the deployment and the engine cannot.
"""

import argparse
import json
import os
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from typing import Any

TERMINAL = {"completed", "failed", "crashed", "cancelled"}
#: The engines this benchmark has a variant for, in the words a table prints.
LABELS = {"flow": "Flow PHP", "datafusion": "DataFusion", "duckdb": "DuckDB"}


def roles(engine: str) -> dict[str, str]:
    """Which engine each step is. The query step is named for the source block,
    because the source and the transform behind it are one query."""
    return {
        "corpus": f"{LABELS.get(engine, engine)} (read + per-model aggregate)",
        "polars": "Polars notebook (same query, compare)",
        "result": "publish the comparison",
    }


class ApiError(RuntimeError):
    pass


def call(method: str, url: str, body: object | None = None) -> Any:
    data = None if body is None else json.dumps(body).encode()
    # S310: the URL is the operator's own --api, never something a response named.
    request = urllib.request.Request(url, data=data, method=method)  # noqa: S310
    request.add_header("content-type", "application/json")
    try:
        with urllib.request.urlopen(request, timeout=30) as response:  # noqa: S310
            raw = response.read()
    except urllib.error.HTTPError as error:
        raise ApiError(f"{method} {url} → {error.code}: {error.read().decode()[:500]}") from error
    # The health probes answer `ok` rather than JSON; everything else is JSON.
    try:
        return json.loads(raw or b"null")
    except json.JSONDecodeError:
        return raw.decode()


def follow(api: str, execution_id: str, timeout: float) -> dict[str, Any]:
    """Re-read the run until it ends, saying so each time its state changes."""
    started = time.monotonic()
    last = ""
    while True:
        execution: dict[str, Any] = call("GET", f"{api}/api/v1/executions/{execution_id}")[
            "execution"
        ]
        state = execution["state"]["state_type"]
        steps = ", ".join(
            f"{step.get('step_id', '?')}={step.get('state', {}).get('state_type', '?')}"
            for step in execution.get("steps", [])
        )
        line = f"{state}  [{steps}]"
        if line != last:
            print(f"  {time.monotonic() - started:7.1f} s  {line}", flush=True)
            last = line
        if state in TERMINAL:
            return execution
        if time.monotonic() - started > timeout:
            raise TimeoutError(f"{execution_id} still {state} after {timeout:.0f} s")
        time.sleep(2)


def folded(api: str, execution_id: str) -> dict[str, Any]:
    """The workflow fold for the run, once it has caught up with the ending."""
    detail: dict[str, Any] = {}
    for _ in range(30):
        try:
            detail = call("GET", f"{api}/api/v1/workflow-executions/{execution_id}")
        except ApiError:
            detail = {}
        if detail and detail["summary"]["status"] != "running":
            return detail
        time.sleep(1)
    return detail


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--api", default=os.environ.get("AIWATCHER_URL", "http://127.0.0.1:8080"))
    parser.add_argument(
        "--query", default=os.environ.get("AIWATCHER_QUERY_URL", "http://127.0.0.1:8081")
    )
    parser.add_argument("--pipeline", help="default: the variant for the engine --query runs")
    parser.add_argument("--dataset", help="default: the pipeline's own name")
    parser.add_argument("--panel", default="http://localhost:5173")
    parser.add_argument("--timeout", type=float, default=7200, help="seconds to wait for the run")
    args = parser.parse_args()
    api = args.api.rstrip("/")

    try:
        call("GET", f"{api}/livez")
    except (ApiError, urllib.error.URLError) as error:
        print(
            f"no aiwatcher at {api} ({error}) — start `just bench-curation-serve`", file=sys.stderr
        )
        return 2
    try:
        health = call("GET", f"{args.query.rstrip('/')}/query/healthz")
    except (ApiError, urllib.error.URLError) as error:
        print(f"no query engine at {args.query} ({error})", file=sys.stderr)
        return 2
    engine = str(health.get("engine", "flow")) if isinstance(health, dict) else "flow"
    suffix = "" if engine == "flow" else f"-{engine}"
    pipeline = args.pipeline or f"curation/flow-vs-polars{suffix}"
    dataset = args.dataset or pipeline

    started = call(
        "POST",
        f"{api}/api/v1/executions",
        {"target": {"kind": "curation_pipeline", "name": pipeline}},
    )
    execution_id = started["execution"]["execution_id"]
    query = urllib.parse.urlencode({"workflow": pipeline, "execution": execution_id})
    print(f"started {execution_id}\n  waterfall: {args.panel}/workflows?{query}", flush=True)

    execution = follow(api, execution_id, args.timeout)
    state = execution["state"]["state_type"]

    detail = folded(api, execution_id)
    print("\nstep                                        status      took      attempts")
    for node in detail.get("nodes", []):
        took = node.get("duration_ms")
        role = roles(engine).get(node["node_id"], node.get("name", node["node_id"]))
        shown = "—" if took is None else f"{took / 1000:8.2f} s"
        print(f"{role:<44}{node['status']:<12}{shown:>10}  {node.get('attempts', 0):>3}")
        if node.get("error"):
            print(f"    {node['error']}")

    if state != "completed":
        print(f"\nthe run ended {state}; the step above says why", file=sys.stderr)
        return 1

    page = call(
        "GET",
        f"{api}/api/v1/dataset-rows?" + urllib.parse.urlencode({"name": dataset, "limit": 50}),
    )
    rows = [entry["row"] for entry in page["rows"]]
    label = LABELS.get(engine, engine)
    print(f"\nmodel                 spans ({label} / Polars)   mean ms ({label} / Polars)   agrees")
    for row in rows:
        print(
            f"{row['model']:<20}  {row[f'{engine}_spans']!s:>12} / {row['polars_spans']!s:<12}"
            f"  {row[f'{engine}_avg_duration_ms']!s:>10} / {row['polars_avg_duration_ms']!s:<12}"
            f"  {'yes' if row['agrees'] else 'NO'}"
        )
    if rows:
        print(f"\nPolars' own query time inside the notebook: {rows[0]['polars_seconds']} s")
    agreed = bool(rows) and all(row["agrees"] for row in rows)
    print("both engines gave the same answer" if agreed else "the two answers DIFFER")
    return 0 if agreed else 1


if __name__ == "__main__":
    sys.exit(main())
