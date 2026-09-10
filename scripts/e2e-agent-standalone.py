#!/usr/bin/env python3
"""End-to-end: one agent, registered as a workflow of its own, with no graph.

AW-2's second success criterion. An agent that is not composed with anything —
a digest that runs at nine, a triage agent somebody starts from the panel — is
registered as a `WorkflowDefinition` of one step, of the kind `workflow`, and
started exactly the way the panel's launcher starts any registered workflow.
Nothing here builds a graph, and the host process never imports
`agentic_graph`.

What it checks, in order:

1. the host registers the agent with no graph around it — one step, no edges —
   and never imports `agentic_graph`;
2. a run is started the way the panel starts one: `POST /api/v1/executions`
   with `target.kind = "workflow"` and the revision the list returned;
3. the host's worker claims it and the run completes;
4. the reply is in the host's payload store, and what aiwatcher holds is its
   reference, a digest that matches the stored bytes, and a size;
5. **the reply's words are not in aiwatcher** — not in the history, not in the
   run's projection;
6. the Workflows view's fold lists the run as one node that succeeded;
7. a schedule is saved against the agent — compiled first, as the tick will —
   and `run_now` starts a second run from it;
8. that run, started with no parameters, answers the message the agent was
   registered with;
9. a schedule for an agent nobody registered is refused rather than saved;
10. the host stops cleanly on SIGTERM, closing the agent runtime it owns;
11. the agent's own records carry the attempt's context id as their turn id —
    what joins them to the run that asked.

Runs under `ai_spirit_agent`'s environment, because `AgenticRuntime` and
`hosted_runtime` are its code.

    just run                     # in one terminal
    just e2e-agent-standalone    # in another
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import queue
import shutil
import signal
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
import uuid
from pathlib import Path
from typing import Any

BASE = os.environ.get("AIWATCHER_URL", "http://127.0.0.1:8080").rstrip("/")
if not BASE.startswith(("http://", "https://")):
    raise SystemExit(f"AIWATCHER_URL must be http(s), got {BASE!r}")
AGENT = "e2e.digest"
QUEUE = "e2e-agent-standalone"
ASKED = "what changed in the notes since Monday?"
DEFAULT = "summarise what happened overnight"
#: Only the agent writes this, so finding it anywhere in aiwatcher is finding
#: the reply there.
SIGNATURE = "::answered-by-the-digest-agent"
READY_WITHIN = 60.0
RUN_WITHIN = 60.0


def answer(text: str) -> str:
    return f"{text[::-1]}{SIGNATURE}"


# ── The host. This file, run again as a separate process. ────────────────────


def host(root: Path) -> None:
    from agentic.metadata import Description
    from agentic.observability import build_tracer
    from aiwatcher_sdk.integrations.agentic import FilePayloadStore, aiwatcher_tracer, tee

    import agentic_runtime.runtime as runtime_module
    from agentic_runtime.hosted import hosted_runtime
    from agentic_runtime.runtime import AgenticRuntime
    from agentic_runtime.settings import Settings

    # MLflow is the application's other tracer. This run proves aiwatcher, and a
    # tracking server that is not running must not be what it waits on.
    runtime_module.init_tracing = lambda: "e2e-agent-standalone"
    runtime_module.create_tracer = lambda enabled=True: tee(
        build_tracer(enabled=False), aiwatcher_tracer(service="e2e-agent-standalone")
    )

    class Router:
        def route(self, message: str, available_workflows_summary: str) -> str:
            raise AssertionError("a hosted turn names its agent; the router is never asked")

        def start(self) -> str:
            return ""

        def close(self) -> None:
            pass

    class Digest:
        description = Description(AGENT, "answers with a digest", ())
        inputs = ("UserMessage",)

        def handle(self, message: Any) -> str:
            return answer(message.data.text)

        def close(self) -> None:
            pass

    app = AgenticRuntime(
        workflows=[Digest()],
        router=Router(),
        settings=Settings(
            event_store_path=str(root / "events.db"),
            message_store_path=str(root / "messages.db"),
        ),
    )
    runtime = hosted_runtime(
        app,
        version="1",
        name="e2e-agent-standalone",
        queue=QUEUE,
        default_messages={AGENT: DEFAULT},
        payloads=FilePayloadStore(root / "payloads"),
    )
    signal.signal(signal.SIGTERM, lambda *_: runtime.stop())
    runtime.register()
    print("READY", "graph" if "agentic_graph" in sys.modules else "graph-free", flush=True)
    runtime.serve()
    print("CLOSED", flush=True)


# ── The orchestrator. ────────────────────────────────────────────────────────


def call(method: str, path: str, body: Any = None, **headers: str) -> tuple[int, Any, bytes]:
    data = None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(BASE + path, data=data, method=method)
    request.add_header("Content-Type", "application/json")
    for name, value in headers.items():
        request.add_header(name.replace("_", "-"), value)
    try:
        with urllib.request.urlopen(request, timeout=15) as response:  # noqa: S310 — http(s) checked above
            raw = response.read()
            status = response.status
    except urllib.error.HTTPError as error:
        raw, status = error.read(), error.code
    try:
        parsed = json.loads(raw) if raw else None
    except json.JSONDecodeError:
        parsed = None
    return status, parsed, raw


def ok(number: int, claim: str) -> None:
    print(f"  ✓ {number:>2}. {claim}")


def fail(number: int, claim: str, detail: object) -> None:
    raise SystemExit(f"  ✗ {number:>2}. {claim}\n       {detail}")


def lines_of(process: subprocess.Popen[str]) -> queue.Queue[str]:
    lines: queue.Queue[str] = queue.Queue()

    def pump() -> None:
        assert process.stdout is not None
        for line in process.stdout:
            lines.put(line.strip())

    threading.Thread(target=pump, daemon=True).start()
    return lines


def expect_line(lines: queue.Queue[str], prefix: str, within: float) -> str:
    deadline = time.monotonic() + within
    while (left := deadline - time.monotonic()) > 0:
        try:
            line = lines.get(timeout=left)
        except queue.Empty:
            break
        if line.startswith(prefix):
            return line
    raise TimeoutError(f"the host did not print {prefix!r} within {within:.0f}s")


def settled(execution_id: str) -> tuple[str, Any, bytes]:
    deadline = time.monotonic() + RUN_WITHIN
    while True:
        status, body, raw = call("GET", f"/api/v1/executions/{execution_id}")
        state = (body or {}).get("execution", {}).get("state", {}).get("state_type")
        if state in ("completed", "failed", "cancelled", "crashed"):
            return state, body, raw
        if time.monotonic() > deadline:
            return f"still {state} (HTTP {status})", body, raw
        time.sleep(0.25)


def reply_of(execution_id: str) -> tuple[dict[str, Any], bytes]:
    _, body, raw = call("GET", f"/api/v1/executions/{execution_id}/history?after=0&limit=200")
    found: list[dict[str, Any]] = []

    def walk(value: Any) -> None:
        if isinstance(value, dict):
            if isinstance(value.get("reply"), dict) and "reference" in value["reply"]:
                found.append(value["reply"])
            for item in value.values():
                walk(item)
        elif isinstance(value, list):
            for item in value:
                walk(item)

    walk(body)
    if not found:
        raise LookupError(f"no reply reference in the history of {execution_id}")
    return found[-1], raw


def stored(reply: dict[str, Any]) -> tuple[Any, str]:
    path = Path(str(reply["reference"]).removeprefix("file://"))
    content = path.read_bytes()
    return json.loads(content), hashlib.sha256(content).hexdigest()


def orchestrate() -> None:
    root = Path(tempfile.mkdtemp(prefix="aiwatcher-e2e-standalone-"))
    env = os.environ | {"AIWATCHER_URL": BASE, "PYTHONUNBUFFERED": "1"}
    print(f"aiwatcher at {BASE}; the host keeps its data in {root}")
    process = subprocess.Popen(  # noqa: S603 — this file, run again
        [sys.executable, __file__, "--role", "host", "--root", str(root)],
        stdout=subprocess.PIPE,
        text=True,
        env=env,
    )
    lines = lines_of(process)
    schedule = f"/api/v1/workflow-definitions/{AGENT}/schedule"
    try:
        ready = expect_line(lines, "READY", READY_WITHIN)
        _, definitions, _ = call("GET", "/api/v1/workflow-definitions")
        entry = next(d for d in definitions or [] if d["definition"]["name"] == AGENT)
        [step] = entry["definition"]["steps"]
        if not (
            step["id"] == "turn"
            and step["after"] == []
            and step["inputs"] == []
            and step["queue"] == QUEUE
            and ready.endswith("graph-free")
        ):
            fail(1, "registered as one step, no graph", (ready, step))
        ok(1, f"`{AGENT}` is registered as one step with no edges; the host never imported agentic_graph")

        status, started, raw = call(
            "POST",
            "/api/v1/executions",
            {
                "target": {"kind": "workflow", "name": AGENT, "revision": entry["revision"]},
                "parameters": {"message": ASKED},
            },
            Idempotency_Key=uuid.uuid4().hex,
        )
        if status not in (200, 201, 202):
            fail(2, "started the way the panel starts one", (status, raw[:400]))
        execution_id = started["execution"]["execution_id"]
        ok(2, f"started from POST /api/v1/executions, target.kind = workflow — {execution_id}")

        state, view, view_raw = settled(execution_id)
        if state != "completed":
            fail(3, "the run completes", (state, json.dumps(view)[:600]))
        ok(3, "the host's worker claimed the turn and the run completed")

        reply, history_raw = reply_of(execution_id)
        words, digest = stored(reply)
        if words != answer(ASKED) or reply["digest"] != digest or reply["size"] <= 0:
            fail(4, "reference, digest and size", (reply, words, digest))
        ok(4, f"the reply is in the host's payload store; aiwatcher holds {reply['digest'][:12]}…, {reply['size']} bytes")

        if SIGNATURE.encode() in history_raw or SIGNATURE.encode() in view_raw:
            fail(5, "the reply's words stay out of aiwatcher", "the signature is in a response")
        ok(5, "the reply's words are in neither the history nor the run's projection")

        deadline = time.monotonic() + 15
        row = None
        while row is None and time.monotonic() < deadline:
            _, page, page_raw = call("GET", f"/api/v1/workflow-executions?workflow_id={AGENT}&limit=50")
            row = next(
                (
                    r
                    for r in (page or {}).get("executions", [])
                    if r["workflow_run_id"] == execution_id and r["nodes_succeeded"] == 1
                ),
                None,
            )
            time.sleep(0.25 if row is None else 0)
        if row is None or row["nodes_total"] != 1 or SIGNATURE.encode() in page_raw:
            fail(6, "the Workflows view lists it", row)
        ok(6, "the Workflows view's fold lists it as one node, succeeded")

        status, saved, raw = call(
            "PUT",
            schedule,
            {
                "cadence": {"every": "daily", "hour": 9, "minute": 0},
                "timezone": "Europe/Warsaw",
                "run_now": True,
                "request_id": uuid.uuid4().hex,
            },
        )
        if status != 200 or not (saved or {}).get("started"):
            fail(7, "a schedule is saved and starts a run", (status, raw[:400]))
        scheduled = saved["started"]
        ok(7, f"a schedule is saved against the agent and run_now started {scheduled}")

        state, _, _ = settled(scheduled)
        words, _ = stored(reply_of(scheduled)[0]) if state == "completed" else (None, "")
        if words != answer(DEFAULT):
            fail(8, "a scheduled run answers the registered message", (state, words))
        ok(8, "that run, started with no parameters, answered the message the agent was registered with")

        status, _, raw = call(
            "PUT",
            f"/api/v1/workflow-definitions/e2e.nobody-{uuid.uuid4().hex[:8]}/schedule",
            {"cadence": {"every": "daily", "hour": 9, "minute": 0}, "timezone": "Europe/Warsaw"},
        )
        if status != 404:
            fail(9, "an unregistered agent's schedule is refused", (status, raw[:300]))
        ok(9, "a schedule for an agent nobody registered is refused with 404, not saved")

        process.send_signal(signal.SIGTERM)
        try:
            closed = expect_line(lines, "CLOSED", 30)
            process.wait(timeout=30)
        except (TimeoutError, subprocess.TimeoutExpired) as error:
            fail(10, "the host stops cleanly", error)
        if process.returncode != 0 or not closed:
            fail(10, "the host stops cleanly", process.returncode)
        ok(10, "the host stopped on SIGTERM and closed the agent runtime it owns")

        turn = f"{execution_id}/turn/1"
        with sqlite3.connect(root / "messages.db") as db:
            rows = sum(
                db.execute(f"SELECT count(*) FROM {table} WHERE turn_id = ?", (turn,)).fetchone()[0]  # noqa: S608 — two fixed names
                for table in ("message_stream", "conversation_records")
            )
        if rows == 0:
            fail(11, "the agent's records name the attempt", turn)
        ok(11, f"{rows} of the agent's own records carry turn id {turn}")
    finally:
        call("DELETE", schedule)
        if process.poll() is None:
            process.kill()
            process.wait()
    shutil.rmtree(root, ignore_errors=True)
    print("11/11")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--role", choices=("host",))
    parser.add_argument("--root", type=Path)
    args = parser.parse_args()
    if args.role == "host":
        host(args.root)
    else:
        orchestrate()


if __name__ == "__main__":
    main()
