#!/usr/bin/env python3
"""End-to-end: an agent's hops survive aiwatcher being unreachable.

`e2e-agent-join.py` proved a join outlives the worker that opened it. This is
the other half of AW-2's durability claim: that a worker outlives *aiwatcher*.
Every hop is written to a local outbox before it is sent, so a turn running
while the server is away goes on with what it knows, and the hops arrive — once
each — when it is back.

aiwatcher is not stopped: it is somebody's development server. The workers
reach it through a door this script can shut — a TCP proxy that, shut, accepts
and hangs up at once, which a client sees exactly as it sees a server that went
away.

What it checks, in order:

1. a hosted execution starts, owned by a worker;
2. with the door shut, the agent writes three hops and none of them raises;
3. it still reads its own three arrivals — it keeps answering;
4. it is refused the summary's claim while those hops wait, rather than
   deciding over a history nobody else can read;
5. an operator lists the three from outside the process, each with its
   execution, its message id and its attempts — the oldest tried, the others
   untried behind it, because a drain stops at the first unreachable row;
6. none of them is on aiwatcher's stream yet;
7. with the door open, the claim drains them first and then succeeds;
8. each hop is on the stream exactly once, and the outbox is empty;
9. a hop aiwatcher accepted, from a process SIGKILLed before its outbox heard,
   is on the stream once and still waiting in the outbox;
10. an operator's drain re-sends it under the same id, aiwatcher recognises the
    redelivery, and the stream still holds it once;
11. the words are in neither aiwatcher's history nor the outbox.

Runs under `ai_spirit_agent`'s environment, like the join's, and for the same
reason.

    just run                     # in one terminal
    just e2e-agent-outbox        # in another
"""

from __future__ import annotations

import argparse
import json
import os
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import urllib.error
import urllib.parse
import urllib.request
import uuid
from pathlib import Path
from typing import Any

BASE = os.environ.get("AIWATCHER_URL", "http://127.0.0.1:8080").rstrip("/")
if not BASE.startswith("http://"):
    # The door is a plain TCP proxy, and a worker speaking http to it would be
    # speaking http to a TLS server behind it.
    raise SystemExit(f"AIWATCHER_URL must be a plain http server, got {BASE!r}")
DEFINITION = "e2e.agent-outbox"
TURN = "turn-1"
LATE_TURN = "turn-2"
SUMMARIZER = "summarizer-1"
SOURCES = ("search-1", "search-2", "search-3")


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


def say(prefix: str, report: dict[str, Any]) -> None:
    print(f"{prefix} {json.dumps(report)}", flush=True)


def wait_for_the_orchestrator() -> None:
    sys.stdin.readline()


def agent(execution: str, marker: str) -> None:
    ledger = ledger_for(execution, "agent")
    for source in SOURCES:
        ledger.reserve_fan_in(TURN, source, (SUMMARIZER,))
    say("READY", {"expected": ledger.get_expected(TURN, SUMMARIZER)})

    wait_for_the_orchestrator()  # the door is shut now
    raised = None
    try:
        for source in SOURCES:
            ledger.record_completion(TURN, SUMMARIZER, arrival(source, marker))
    except Exception as failure:  # noqa: BLE001 - reported, and the report is the check
        raised = f"{type(failure).__name__}: {failure}"
    arrived = len(ledger.get_completions(TURN, SUMMARIZER))
    claimed: bool | None
    try:
        claimed, refused = ledger.claim_summary(TURN, SUMMARIZER), None
    except Exception as failure:  # noqa: BLE001 - the refusal is what is being checked
        claimed, refused = None, type(failure).__name__
    say(
        "OUTAGE",
        {"raised": raised, "arrived": arrived, "claimed": claimed, "refused": refused},
    )

    wait_for_the_orchestrator()  # and open again
    claimed = ledger.claim_summary(TURN, SUMMARIZER)
    if claimed:
        ledger.complete_summary(TURN, SUMMARIZER)
    say("DONE", {"claimed": claimed})


def lost_acknowledgement(execution: str, marker: str, base: str) -> None:
    from aiwatcher_sdk.api import Transport
    from aiwatcher_sdk.integrations.agentic import deliver
    from aiwatcher_sdk.outbox import SqliteOutbox

    ledger = ledger_for(execution, "late")
    # The door is shut, so this waits in the outbox.
    ledger.record_completion(LATE_TURN, SUMMARIZER, arrival("search-4", marker))
    with SqliteOutbox(os.environ["AIWATCHER_OUTBOX"]) as outbox:
        [row] = outbox.pending(execution_id=execution)
    # Around the door, straight to aiwatcher, which accepts it…
    deliver(Transport(base), row.delivery)
    say("SENT", {"message_id": row.delivery.message_id})
    # …and the process dies before its outbox hears. Nothing after this runs.
    os.kill(os.getpid(), signal.SIGKILL)


# ── The door ─────────────────────────────────────────────────────────────────


def hang_up(connection: socket.socket) -> None:
    try:
        connection.shutdown(socket.SHUT_RDWR)
    except OSError:
        pass
    connection.close()


def pump(source: socket.socket, sink: socket.socket) -> None:
    try:
        while chunk := source.recv(65536):
            sink.sendall(chunk)
    except OSError:
        pass
    finally:
        hang_up(source)
        hang_up(sink)


class Door:
    """A TCP proxy in front of aiwatcher, shut and opened by this script."""

    def __init__(self, upstream: tuple[str, int]) -> None:
        self._upstream = upstream
        self._listener = socket.create_server(("127.0.0.1", 0))
        self.url = f"http://127.0.0.1:{self._listener.getsockname()[1]}"
        self._open = True
        self._live: list[socket.socket] = []
        self._lock = threading.Lock()
        threading.Thread(target=self._accept, daemon=True).start()

    def shut(self) -> None:
        # Pooled connections go too: a keep-alive socket opened while the door
        # was open would otherwise walk through it after it shut.
        with self._lock:
            self._open = False
            live, self._live = self._live, []
        for connection in live:
            hang_up(connection)

    def open(self) -> None:
        with self._lock:
            self._open = True

    def close(self) -> None:
        self.shut()
        self._listener.close()

    def _accept(self) -> None:
        while True:
            try:
                client, _ = self._listener.accept()
            except OSError:
                return
            with self._lock:
                if not self._open:
                    hang_up(client)
                    continue
                try:
                    upstream = socket.create_connection(self._upstream, timeout=5)
                except OSError:
                    hang_up(client)
                    continue
                upstream.settimeout(None)
                self._live += [client, upstream]
            for source, sink in ((client, upstream), (upstream, client)):
                threading.Thread(target=pump, args=(source, sink), daemon=True).start()


# ── The orchestrator ─────────────────────────────────────────────────────────


def call(method: str, path: str, body: Any = None, *, key: str | None = None) -> Any:
    data = None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(BASE + path, data=data, method=method)  # noqa: S310 - http, checked above
    request.add_header("Accept", "application/json")
    if data is not None:
        request.add_header("Content-Type", "application/json")
    if key is not None:
        request.add_header("Idempotency-Key", key)
    token = os.environ.get("AIWATCHER_TOKEN")
    if token:
        request.add_header("Authorization", f"Bearer {token}")
    try:
        with urllib.request.urlopen(request, timeout=30) as response:  # noqa: S310 - http, checked above
            raw = response.read()
    except urllib.error.HTTPError as error:
        detail = error.read().decode(errors="replace")
        raise SystemExit(f"{method} {path} → {error.code}: {detail}") from error
    return json.loads(raw) if raw else None


def history(execution: str) -> str:
    return json.dumps(call("GET", f"/api/v1/executions/{execution}/history?limit=500"))


def recorded(execution: str, message_type: str) -> int:
    page = call("GET", f"/api/v1/executions/{execution}/history?limit=500")
    return sum(
        1 for row in page.get("messages", ()) if row["message"].get("message_type") == message_type
    )


def operator(*args: str) -> str:
    """`python -m aiwatcher_sdk.outbox`, the way a person would run it."""
    # This interpreter, a module name and arguments this file wrote.
    done = subprocess.run(  # noqa: S603
        [sys.executable, "-m", "aiwatcher_sdk.outbox", *args],
        capture_output=True,
        text=True,
        check=False,
    )
    if done.returncode != 0:
        raise SystemExit(f"the outbox CLI exited {done.returncode}:\n{done.stderr}")
    return done.stdout


def listed(path: Path) -> list[dict[str, Any]]:
    return list(json.loads(operator("list", str(path), "--json")))


def spawn(role: str, execution: str, marker: str, env: dict[str, str]) -> subprocess.Popen[str]:
    # This file, under this interpreter, with arguments this file wrote.
    return subprocess.Popen(  # noqa: S603
        [
            sys.executable,
            __file__,
            "--role",
            role,
            "--execution",
            execution,
            "--marker",
            marker,
            "--base",
            BASE,
        ],
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )


def expect(process: subprocess.Popen[str], prefix: str) -> dict[str, Any]:
    if process.stdout is None:
        raise SystemExit("a worker was started without a stdout pipe")
    while True:
        line = process.stdout.readline()
        if not line:
            _, err = process.communicate()
            raise SystemExit(f"a worker exited before saying {prefix}:\n{err}")
        if line.startswith(prefix + " "):
            return dict(json.loads(line[len(prefix) + 1 :]))


def tell(process: subprocess.Popen[str]) -> None:
    if process.stdin is None:
        raise SystemExit("a worker was started without a stdin pipe")
    process.stdin.write("go\n")
    process.stdin.flush()


class Checks:
    def __init__(self) -> None:
        self.failed = 0

    def check(self, number: int, claim: str, holds: bool, detail: str = "") -> None:
        mark = "ok " if holds else "FAIL"
        print(f"  [{mark}] {number}. {claim}" + (f" — {detail}" if detail else ""))
        self.failed += 0 if holds else 1


def orchestrate() -> int:
    try:
        urllib.request.urlopen(f"{BASE}/healthz", timeout=5).read()  # noqa: S310 - http, checked above
    except OSError as error:
        raise SystemExit(f"no aiwatcher at {BASE} ({error}); start it with `just run`") from error
    try:
        from agentic_graph.join import RECORDED
    except ImportError as error:
        raise SystemExit(
            "agentic_graph is not importable: run this under ai_spirit_agent's environment "
            "(`just e2e-agent-outbox` does)"
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
                    "queue": "e2e-agent-outbox",
                    "timeout_seconds": 600,
                }
            ],
        },
    )
    run = call(
        "POST",
        "/api/v1/executions",
        {"target": {"kind": "workflow", "name": DEFINITION}, "decided_by": "worker"},
        key=uuid.uuid4().hex,
    )["execution"]
    execution = run["execution_id"]
    checks.check(
        1,
        "a hosted execution starts, owned by a worker",
        run.get("mode") == "hosted" and run.get("owner") == "worker",
        f"{execution} owner={run.get('owner')} mode={run.get('mode')}",
    )

    split = urllib.parse.urlsplit(BASE)
    door = Door((split.hostname or "127.0.0.1", split.port or 80))
    try:
        with tempfile.TemporaryDirectory(prefix="aiwatcher-e2e-outbox-") as scratch:
            root = Path(scratch)
            outbox = root / "outbox.sqlite"
            env = dict(
                os.environ,
                AIWATCHER_URL=door.url,
                AIWATCHER_PAYLOAD_ROOT=str(root / "payloads"),
                AIWATCHER_OUTBOX=str(outbox),
            )

            worker = spawn("agent", execution, marker, env)
            expect(worker, "READY")
            door.shut()
            tell(worker)
            outage = expect(worker, "OUTAGE")
            checks.check(
                2,
                "with aiwatcher unreachable, three hops are written and none raises",
                outage["raised"] is None,
                outage["raised"] or "",
            )
            checks.check(
                3,
                "the agent still reads its own three arrivals",
                outage["arrived"] == 3,
                f"arrived={outage['arrived']}",
            )
            checks.check(
                4,
                "the claim is refused while those hops wait",
                outage["claimed"] is None and outage["refused"] == "UndeliveredHopsError",
                f"claimed={outage['claimed']} refused={outage['refused']}",
            )
            waiting = [row for row in listed(outbox) if row["execution_id"] == execution]
            attempts = [row["attempts"] for row in waiting]
            checks.check(
                5,
                "an operator lists them, with execution, message id and attempts",
                len(waiting) == 3
                and all(row["message_id"] and isinstance(row["attempts"], int) for row in waiting)
                and attempts[0] >= 1,
                f"{len(waiting)} row(s), attempts={attempts} (oldest first)",
            )
            before = recorded(execution, RECORDED)
            checks.check(
                6,
                "none of them is on the stream yet",
                before == 0,
                f"{before} recorded",
            )

            door.open()
            tell(worker)
            out, err = worker.communicate(timeout=120)
            if worker.returncode != 0:
                raise SystemExit(f"the agent exited {worker.returncode}:\n{err}")
            done = json.loads(out.strip().splitlines()[-1].removeprefix("DONE "))
            checks.check(
                7,
                "aiwatcher back, the claim drains them first and then succeeds",
                done["claimed"] is True,
                f"claimed={done['claimed']}",
            )
            after = recorded(execution, RECORDED)
            left = listed(outbox)
            checks.check(
                8,
                "each hop is on the stream exactly once, and the outbox is empty",
                after == 3 and left == [],
                f"{after} recorded, {len(left)} row(s) left",
            )

            door.shut()
            late = spawn("late", execution, marker, env)
            sent = expect(late, "SENT")
            late.wait(timeout=30)
            still = listed(outbox)
            checks.check(
                9,
                "a hop accepted by a process killed before its outbox heard: on "
                "the stream once, and still waiting",
                late.returncode == -signal.SIGKILL
                and recorded(execution, RECORDED) == 4
                and [row["message_id"] for row in still] == [sent["message_id"]],
                f"returncode={late.returncode} rows={[row['message_id'] for row in still]}",
            )

            drained = operator("drain", str(outbox), "--url", BASE).strip()
            final = recorded(execution, RECORDED)
            checks.check(
                10,
                "an operator's drain re-sends it and aiwatcher recognises it",
                drained.startswith("settled=1 ") and final == 4 and listed(outbox) == [],
                f"{drained!r}, {final} recorded",
            )

            words = [
                path.name
                for path in root.glob("outbox.sqlite*")
                if marker.encode() in path.read_bytes()
            ]
            stored = sorted(path.name for path in (root / "payloads").rglob("*.json"))
            checks.check(
                11,
                "the words are in neither aiwatcher's history nor the outbox",
                marker not in history(execution) and not words and bool(stored),
                f"{len(stored)} payload file(s) held by the workers, marker in {words or 'neither'}",
            )
    finally:
        door.close()

    print()
    if checks.failed:
        print(f"{checks.failed} check(s) failed")
        return 1
    print("the hops outlived the outage")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--role", default="orchestrator", choices=("orchestrator", "agent", "late")
    )
    parser.add_argument("--execution", default="")
    parser.add_argument("--marker", default="")
    parser.add_argument("--base", default=BASE)
    args = parser.parse_args()
    if args.role == "agent":
        agent(args.execution, args.marker)
    elif args.role == "late":
        lost_acknowledgement(args.execution, args.marker, args.base)
    else:
        return orchestrate()
    return 0


if __name__ == "__main__":
    sys.exit(main())
