#!/usr/bin/env python3
"""End-to-end: a graph's join survives the worker that opened it.

The seam this checks was built on both sides and never met in the middle: the
SDK's `AiwatcherEventStore` talks to `POST …/stream` and `GET …/history`, and
`ai_spirit_agent`'s `StreamJoinLedger` folds a join out of whatever store it is
handed. Every test either side had used a stand-in for the other. This one uses
neither: a running server, and three separate operating-system processes.

What it checks, in order:

1. a hosted execution starts, owned by a worker rather than by this system;
2. worker A's three reservations and two completions reach the server;
3. a completion recorded twice is one arrival, not two;
4. worker A is killed with **SIGKILL** — nothing released, nothing flushed, the
   exact failure the in-memory join could not survive;
5. worker B, a fresh process that never shared A's memory, reads three expected
   and two arrived **from the stream**;
6. worker B's third completion completes the join, and it claims the summary;
7. a third process asking to fire the same summary is refused — the join fires
   once, whichever worker sees the last arrival;
8. **the words stay out of aiwatcher's history**: every completion's text went
   to the payload store, and the stream holds a reference, a digest and a size.

Runs under `ai_spirit_agent`'s environment, because the ledger is its code —
the coupling `docs/specs/AW-2-…` exists to remove, and why the recipe names a
sibling checkout.

    just run                     # in one terminal
    just e2e-agent-join          # in another
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
import uuid
from pathlib import Path
from typing import Any

BASE = os.environ.get("AIWATCHER_URL", "http://127.0.0.1:8080").rstrip("/")
if not BASE.startswith(("http://", "https://")):
    raise SystemExit(f"AIWATCHER_URL must be http(s), got {BASE!r}")
DEFINITION = "e2e.agent-join"
TURN = "turn-1"
SUMMARIZER = "summarizer-1"
SOURCES = ("search-1", "search-2", "search-3")
#: How long worker A is given to report before the run is called broken.
READY_WITHIN = 60.0


# ── The workers. Each is this file, run again as a separate process. ─────────


def ledger_for(execution: str, holder: str) -> Any:
    from agentic_graph.durable import durable_join

    ledger = durable_join(execution, holder=holder)
    if ledger is None:
        raise SystemExit("durable_join returned None: AIWATCHER_URL did not reach the worker")
    return ledger


def arrival(source: str, marker: str) -> Any:
    from agentic_graph.join import Completion

    return Completion(
        source_node_id=source,
        source_alias=source.replace("-", " ").title(),
        text=f"{marker}:{source}",
    )


def worker_a(execution: str, marker: str) -> None:
    ledger = ledger_for(execution, "worker-a")
    for source in SOURCES:
        ledger.reserve_fan_in(TURN, source, (SUMMARIZER,))
    ledger.record_completion(TURN, SUMMARIZER, arrival("search-1", marker))
    # The same agent saying the same thing again: a redelivered completion,
    # which must not count as the second of three.
    ledger.record_completion(TURN, SUMMARIZER, arrival("search-1", marker))
    ledger.record_completion(TURN, SUMMARIZER, arrival("search-2", marker))
    report = {
        "expected": ledger.get_expected(TURN, SUMMARIZER),
        "arrived": len(ledger.get_completions(TURN, SUMMARIZER)),
    }
    print("READY " + json.dumps(report), flush=True)
    # Held here until the orchestrator kills this process. Nothing after this
    # line runs, which is the point: no release, no flush, no goodbye.
    time.sleep(600)


def worker_b(execution: str, marker: str) -> None:
    ledger = ledger_for(execution, "worker-b")
    before = {
        "expected": ledger.get_expected(TURN, SUMMARIZER),
        "arrived": len(ledger.get_completions(TURN, SUMMARIZER)),
    }
    ledger.record_completion(TURN, SUMMARIZER, arrival("search-3", marker))
    arrived = len(ledger.get_completions(TURN, SUMMARIZER))
    # The compiler's own rule, reproduced: a summarizer is fired when every
    # reserved source has arrived, and only by whoever takes the claim.
    claimed = arrived == before["expected"] and ledger.claim_summary(TURN, SUMMARIZER)
    if claimed:
        ledger.complete_summary(TURN, SUMMARIZER)
    print("DONE " + json.dumps({"before": before, "arrived": arrived, "claimed": claimed}))


def worker_c(execution: str) -> None:
    ledger = ledger_for(execution, "worker-c")
    claimed = ledger.claim_summary(TURN, SUMMARIZER)
    state = ledger.get_claim(TURN, SUMMARIZER)
    print(
        "DONE "
        + json.dumps(
            {
                "claimed": claimed,
                "completed": state.completed,
                "holder": state.held.holder if state.held is not None else None,
            }
        )
    )


# ── The orchestrator ─────────────────────────────────────────────────────────


def call(method: str, path: str, body: Any = None, *, key: str | None = None) -> Any:
    data = None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(BASE + path, data=data, method=method)  # noqa: S310
    request.add_header("Accept", "application/json")
    if data is not None:
        request.add_header("Content-Type", "application/json")
    if key is not None:
        request.add_header("Idempotency-Key", key)
    token = os.environ.get("AIWATCHER_TOKEN")
    if token:
        request.add_header("Authorization", f"Bearer {token}")
    try:
        with urllib.request.urlopen(request, timeout=30) as response:  # noqa: S310 - http(s), checked above
            raw = response.read()
    except urllib.error.HTTPError as error:
        detail = error.read().decode(errors="replace")
        raise SystemExit(f"{method} {path} → {error.code}: {detail}") from error
    return json.loads(raw) if raw else None


def raw_history(execution: str) -> str:
    request = urllib.request.Request(  # noqa: S310 - http(s), checked above
        f"{BASE}/api/v1/executions/{execution}/history?limit=500"
    )
    with urllib.request.urlopen(request, timeout=30) as response:  # noqa: S310 - http(s), checked above
        return response.read().decode()


def spawn(role: str, execution: str, marker: str, env: dict[str, str]) -> subprocess.Popen[str]:
    # This file, under this interpreter, with arguments this file wrote.
    return subprocess.Popen(  # noqa: S603
        [sys.executable, __file__, "--role", role, "--execution", execution, "--marker", marker],
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )


def reported(line: str, prefix: str) -> dict[str, Any]:
    if not line.startswith(prefix + " "):
        raise SystemExit(f"a worker answered {line!r}, not a {prefix} line")
    return dict(json.loads(line[len(prefix) + 1 :]))


def finished(process: subprocess.Popen[str], role: str) -> dict[str, Any]:
    out, err = process.communicate(timeout=120)
    if process.returncode != 0:
        raise SystemExit(f"{role} exited {process.returncode}:\n{err}")
    last = [line for line in out.splitlines() if line.strip()][-1]
    return reported(last, "DONE")


class Checks:
    def __init__(self) -> None:
        self.failed = 0

    def check(self, number: int, claim: str, holds: bool, detail: str = "") -> None:
        mark = "ok " if holds else "FAIL"
        print(f"  [{mark}] {number}. {claim}" + (f" — {detail}" if detail else ""))
        self.failed += 0 if holds else 1


def orchestrate() -> int:
    try:
        urllib.request.urlopen(f"{BASE}/healthz", timeout=5).read()  # noqa: S310 - http(s), checked above
    except OSError as error:
        raise SystemExit(f"no aiwatcher at {BASE} ({error}); start it with `just run`") from error
    try:
        import agentic_graph.durable  # noqa: F401
    except ImportError as error:
        raise SystemExit(
            "agentic_graph is not importable: run this under ai_spirit_agent's environment "
            "(`just e2e-agent-join` does)"
        ) from error

    checks = Checks()
    marker = f"e2e-words-{uuid.uuid4().hex}"
    print(f"aiwatcher at {BASE}")

    call(
        "POST",
        "/api/v1/workflow-definitions",
        {
            "name": DEFINITION,
            "version": "1",
            "steps": [
                {
                    "id": "graph",
                    "task_ref": "e2e.agent-graph@1",
                    "queue": "e2e-agent-join",
                    "timeout_seconds": 600,
                }
            ],
        },
    )
    accepted = call(
        "POST",
        "/api/v1/executions",
        {"target": {"kind": "workflow", "name": DEFINITION}, "decided_by": "worker"},
        key=uuid.uuid4().hex,
    )
    run = accepted["execution"]
    execution = run["execution_id"]
    print(f"execution {execution}\n")
    checks.check(
        1,
        "a hosted execution starts, owned by a worker",
        run.get("mode") == "hosted" and run.get("owner") == "worker",
        f"owner={run.get('owner')} mode={run.get('mode')}",
    )

    with tempfile.TemporaryDirectory(prefix="aiwatcher-e2e-payloads-") as payloads:
        env = dict(os.environ, AIWATCHER_URL=BASE, AIWATCHER_PAYLOAD_ROOT=payloads)

        first = spawn("worker-a", execution, marker, env)
        if first.stdout is None:
            raise SystemExit("worker A was started without a stdout pipe")
        deadline = time.monotonic() + READY_WITHIN
        line = ""
        while time.monotonic() < deadline:
            line = first.stdout.readline().strip()
            if line or first.poll() is not None:
                break
        if not line.startswith("READY"):
            first.kill()
            _, err = first.communicate()
            raise SystemExit(f"worker A never reported:\n{err}")
        a = reported(line, "READY")
        checks.check(
            2,
            "worker A's reservations and completions reach the server",
            a["expected"] == 3,
            f"expected={a['expected']}",
        )
        checks.check(
            3,
            "a completion recorded twice is one arrival",
            a["arrived"] == 2,
            f"arrived={a['arrived']}",
        )

        first.kill()
        first.wait(timeout=10)
        checks.check(
            4,
            "worker A is killed with SIGKILL",
            first.returncode == -9,
            f"returncode={first.returncode}",
        )

        b = finished(spawn("worker-b", execution, marker, env), "worker B")
        checks.check(
            5,
            "a fresh process reads the join from the stream",
            b["before"] == {"expected": 3, "arrived": 2},
            f"before={b['before']}",
        )
        checks.check(
            6,
            "the third completion completes the join, and it is claimed",
            b["arrived"] == 3 and b["claimed"] is True,
            f"arrived={b['arrived']} claimed={b['claimed']}",
        )

        c = finished(spawn("worker-c", execution, marker, env), "worker C")
        checks.check(
            7,
            "a third process cannot fire it again",
            c["claimed"] is False and c["completed"] is True,
            f"claimed={c['claimed']} completed={c['completed']} holder={c['holder']}",
        )

        history = raw_history(execution)
        stored = sorted(p.name for p in Path(payloads).rglob("*.json"))
        checks.check(
            8,
            "the words stay out of aiwatcher's history",
            marker not in history and bool(stored),
            f"{len(stored)} payload file(s) held by the worker, "
            f"marker {'FOUND' if marker in history else 'absent'} in history",
        )

    print()
    if checks.failed:
        print(f"{checks.failed} check(s) failed")
        return 1
    print("the join survived its worker")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--role",
        default="orchestrator",
        choices=("orchestrator", "worker-a", "worker-b", "worker-c"),
    )
    parser.add_argument("--execution", default="")
    parser.add_argument("--marker", default="")
    args = parser.parse_args()
    if args.role == "worker-a":
        worker_a(args.execution, args.marker)
    elif args.role == "worker-b":
        worker_b(args.execution, args.marker)
    elif args.role == "worker-c":
        worker_c(args.execution)
    else:
        return orchestrate()
    return 0


if __name__ == "__main__":
    sys.exit(main())
