#!/usr/bin/env python3
"""End-to-end: one agent, registered as a workflow of its own, preparing
fine-tuning data at least once.

AW-2's second success criterion, and the three things a hosted agent is for.
An agent that is not composed with anything is registered as a
`WorkflowDefinition` of one step, of the kind `workflow`, and started exactly
the way the panel's launcher starts any registered workflow. Its turn is
delivered at least once, under control; its spans belong to the run that asked
for them; and its exchange lands in the conversation archive, where a reviewer
approves it into a corpus a trainer reads.

What it checks, in order:

1.  the host registers the agent with no graph — one step, no edges, the
    server's at-least-once budget — and never imports `agentic_graph`;
2.  a run is started the way the panel starts one: `POST /api/v1/executions`
    with `target.kind = "workflow"` and the revision the list returned;
3.  its first attempt is lost after the agent's tool wrote and its exchange was
    archived — the payload disk goes away once — and the server retries it:
    the run completes on attempt 2;
4.  **the tool wrote once**: it was called on both attempts, and the write keyed
    by the step's idempotency key happened on the first only;
5.  the reply is in the host's payload store, and aiwatcher holds its
    reference, a digest that matches the stored bytes, and a size;
6.  the reply's words are in neither the history nor the run's projection;
7.  **the agent's spans nest under the run**: both attempts' `agent.*` and
    `llm.*` events carry the execution's run id with a step's span as their
    parent, and the server holds no run the agent opened for itself;
8.  the Workflows view's fold lists the run as one node that succeeded;
9.  **the exchange is archived once**: two turns, under the step's ids, joined
    to the run's trace — though two attempts wrote them;
10. both turns approved, an `sft` export freezes them into one row whose
    completion is the reply — the fine-tuning data the agent was run to make;
11. a schedule is saved against the agent and `run_now` starts a run that,
    started with no parameters, answers the message it was registered with;
12. a schedule for an agent nobody registered is refused rather than saved;
13. the host stops cleanly on SIGTERM, closing the agent runtime and archive
    client it owns;
14. the agent's own records carry the completed attempt's context id as their
    turn id — what joins them to the run that asked.

It starts **its own** aiwatcher, from `target/debug/aiwatcher` or
`AIWATCHER_BINARY`, on a free port with the conversation archive on and every
byte under a temporary directory — the archive is off on `just run`, and a
server somebody is using is not where a test leaves schedules. It runs under
`ai_spirit_agent`'s environment, because `AgenticRuntime` and `hosted_runtime`
are its code.

    cargo build --bin aiwatcher   # once
    just e2e-agent-standalone
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
import queue
import shutil
import signal
import socket
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
from types import SimpleNamespace
from typing import Any

REPOSITORY = Path(__file__).resolve().parent.parent
BINARY = Path(os.environ.get("AIWATCHER_BINARY", REPOSITORY / "target" / "debug" / "aiwatcher"))
AGENT = "e2e.digest"
SERVICE = "e2e-agent-standalone"
QUEUE = "e2e-agent-standalone"
MODEL = "e2e-model"
EXPORT = "e2e-digest-sft"
ASKED = "what changed in the notes since Monday?"
DEFAULT = "summarise what happened overnight"
#: Only the agent writes this, so finding it anywhere in aiwatcher's history is
#: finding the reply there.
SIGNATURE = "::answered-by-the-digest-agent"
READY_WITHIN = 60.0
RUN_WITHIN = 90.0
BASE = ""


def answer(text: str) -> str:
    return f"{text[::-1]}{SIGNATURE}"


# ── The host. This file, run again as a separate process. ────────────────────


def host(root: Path) -> None:
    from agentic.metadata import Description
    from agentic.observability import build_tracer
    from aiwatcher_sdk.conversations import (
        Consent,
        ConversationArchive,
        PatternRedactor,
        Retention,
    )
    from aiwatcher_sdk.integrations.agentic import FilePayloadStore, aiwatcher_tracer, tee
    from aiwatcher_sdk.task_errors import TaskError

    import agentic_runtime.runtime as runtime_module
    from agentic_runtime.hosted import hosted_runtime
    from agentic_runtime.runtime import AgenticRuntime
    from agentic_runtime.settings import Settings

    # MLflow is the application's other tracer. This run proves aiwatcher, and a
    # tracking server that is not running must not be what it waits on.
    runtime_module.init_tracing = lambda: SERVICE
    runtime_module.create_tracer = lambda enabled=True: tee(
        build_tracer(enabled=False), aiwatcher_tracer(service=SERVICE)
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

        def __init__(self, tracer: Any) -> None:
            self.tracer = tracer

        def handle(self, message: Any) -> str:
            key = message.metadata.idempotency_key
            # The tool: a write keyed by the step. Called on every attempt, and
            # done on the first — which is what the key is for.
            with (root / "tool-calls.log").open("a") as calls:
                calls.write(key + "\n")
            done = root / "tool-writes.log"
            if key not in (done.read_text().split() if done.exists() else []):
                with done.open("a") as writes:
                    writes.write(key + "\n")
            with self.tracer.agent(name=AGENT):
                self.tracer.llm(
                    name="digest",
                    model=MODEL,
                    messages=[{"role": "user", "content": "(the message)"}],
                    invoke=lambda: SimpleNamespace(
                        model=MODEL, prompt_tokens=7, completion_tokens=5
                    ),
                )
            return answer(message.data.text)

        def close(self) -> None:
            pass

    class LosesItsFirstWrite(FilePayloadStore):
        """The payload disk goes away once — after the tool and the archive."""

        def store_payload(self, digest: str, data: Any) -> str:
            lost = root / "lost-once"
            if not lost.exists():
                lost.write_text("")
                raise TaskError("the payload disk went away", classification="infrastructure")
            return super().store_payload(digest, data)

    app = AgenticRuntime(
        workflows=lambda services: [Digest(services.tracer)],
        router=Router(),
        settings=Settings(
            event_store_path=str(root / "events.db"),
            message_store_path=str(root / "messages.db"),
        ),
    )
    runtime = hosted_runtime(
        app,
        version="1",
        name=SERVICE,
        queue=QUEUE,
        default_messages={AGENT: DEFAULT},
        payloads=LosesItsFirstWrite(root / "payloads"),
        archive=ConversationArchive(
            os.environ["AIWATCHER_URL"],
            redactor=PatternRedactor(),
            consent=Consent(
                subject=SERVICE,
                basis="synthetic",
                reference="scripts/e2e-agent-standalone.py",
                scope=["train"],
            ),
            retention=Retention(ttl_days=1, policy_id="e2e"),
        ),
    )
    signal.signal(signal.SIGTERM, lambda *_: runtime.stop())
    runtime.register()
    print("READY", "graph" if "agentic_graph" in sys.modules else "graph-free", flush=True)
    runtime.serve()
    print("CLOSED", flush=True)


# ── The server. ──────────────────────────────────────────────────────────────


def serve(root: Path) -> subprocess.Popen[bytes]:
    """Start an aiwatcher of our own, with the archive on, and wait for it."""
    global BASE
    if not BINARY.exists():
        raise SystemExit(
            f"no server binary at {BINARY}: `cargo build --bin aiwatcher`, "
            "or name one with AIWATCHER_BINARY"
        )
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
    home = root / "server"
    home.mkdir()
    key = "dev:" + base64.urlsafe_b64encode(os.urandom(32)).decode().rstrip("=")
    # Nothing inherited: an AIWATCHER_AUTH_MODE in somebody's shell would make
    # this a different test.
    env = {name: value for name, value in os.environ.items() if not name.startswith("AIWATCHER_")}
    env |= {
        "AIWATCHER_LISTEN": f"127.0.0.1:{port}",
        "AIWATCHER_DATA_DIR": str(home / ".data"),
        "AIWATCHER_BUS": "wal",
        # A worker in another process claims the turn, and the `file` store
        # refuses a plan that needs one — it holds a single process and says so.
        "AIWATCHER_WORKFLOW_STORE": "memory",
        "AIWATCHER_INGEST_ENABLED": "true",
        "AIWATCHER_CONVERSATION_ARCHIVE": "on",
        "AIWATCHER_CONVERSATION_KEYS": key,
        "AIWATCHER_SEED_FILE": "none",
        "AIWATCHER_LOG": "warn",
    }
    log = (home / "server.log").open("wb")
    process = subprocess.Popen(  # noqa: S603 — the binary this repository builds
        [str(BINARY)], cwd=home, env=env, stdout=log, stderr=subprocess.STDOUT
    )
    BASE = f"http://127.0.0.1:{port}"
    deadline = time.monotonic() + READY_WITHIN
    while time.monotonic() < deadline:
        if process.poll() is not None:
            break
        if call("GET", "/readyz")[0] == 200 and call("GET", "/api/v1/conversation-policy")[0] == 200:
            return process
        time.sleep(0.2)
    process.kill()
    tail = (home / "server.log").read_text(errors="replace")[-2000:]
    raise SystemExit(f"aiwatcher did not come up on {BASE}:\n{tail}")


# ── The orchestrator. ────────────────────────────────────────────────────────


def call(method: str, path: str, body: Any = None, **headers: str) -> tuple[int, Any, bytes]:
    data = None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(BASE + path, data=data, method=method)
    request.add_header("Content-Type", "application/json")
    for name, value in headers.items():
        request.add_header(name.replace("_", "-"), value)
    try:
        with urllib.request.urlopen(request, timeout=15) as response:  # noqa: S310 — our own server
            raw = response.read()
            status = response.status
    except urllib.error.HTTPError as error:
        raw, status = error.read(), error.code
    except (urllib.error.URLError, ConnectionError):
        return 0, None, b""
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


def poll(read: Any, within: float = 20.0) -> Any:
    deadline = time.monotonic() + within
    while True:
        found = read()
        if found or time.monotonic() > deadline:
            return found
        time.sleep(0.25)


def orchestrate() -> None:
    root = Path(tempfile.mkdtemp(prefix="aiwatcher-e2e-standalone-"))
    server = serve(root)
    print(f"aiwatcher of our own at {BASE}, archive on; everything under {root}")
    agent_home = root / "agent"
    agent_home.mkdir()
    process = subprocess.Popen(  # noqa: S603 — this file, run again
        [sys.executable, __file__, "--role", "host", "--root", str(agent_home)],
        stdout=subprocess.PIPE,
        text=True,
        env=os.environ | {"AIWATCHER_URL": BASE, "PYTHONUNBUFFERED": "1"},
    )
    lines = lines_of(process)
    passed = False
    try:
        ready = expect_line(lines, "READY", READY_WITHIN)
        _, definitions, _ = call("GET", "/api/v1/workflow-definitions")
        entry = next(d for d in definitions or [] if d["definition"]["name"] == AGENT)
        [step] = entry["definition"]["steps"]
        if not (
            step["id"] == "turn"
            and step["after"] == []
            and step["inputs"] == []
            and step["retry"]["max_attempts"] > 1
            and ready.endswith("graph-free")
        ):
            fail(1, "registered as one step, no graph, at least once", (ready, step))
        ok(1, f"`{AGENT}` is one step with no edges, {step['retry']['max_attempts']} attempts; no agentic_graph imported")

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
        [turn] = (view or {}).get("execution", {}).get("steps", [{}])
        if state != "completed" or turn.get("current_attempt") != 2:
            fail(3, "retried after the lost write, completed on attempt 2", (state, json.dumps(view)[:800]))
        ok(3, "attempt 1 lost its reply write after the tool and the archive; the server retried; attempt 2 completed")

        key = f"{execution_id}/turn"
        calls = (agent_home / "tool-calls.log").read_text().split().count(key)
        writes = (agent_home / "tool-writes.log").read_text().split().count(key)
        if (calls, writes) != (2, 1):
            fail(4, "the tool wrote once", {"calls": calls, "writes": writes})
        ok(4, f"the tool was called on both attempts and wrote once, keyed by {key}")

        reply, history_raw = reply_of(execution_id)
        words, digest = stored(reply)
        if words != answer(ASKED) or reply["digest"] != digest or reply["size"] <= 0:
            fail(5, "reference, digest and size", (reply, words, digest))
        ok(5, f"the reply is in the host's payload store; aiwatcher holds {reply['digest'][:12]}…, {reply['size']} bytes")

        if SIGNATURE.encode() in history_raw or SIGNATURE.encode() in view_raw:
            fail(6, "the reply's words stay out of aiwatcher's history", "the signature is there")
        ok(6, "the reply's words are in neither the history nor the run's projection")

        def spans() -> Any:
            _, page, _ = call("GET", f"/api/v1/runs/{execution_id}/events?limit=500")
            events = (page or {}).get("events", [])
            ours = [
                e for e in events if e["metadata"].get("source", {}).get("service") == SERVICE
            ]
            # Both attempts ran the agent, so both belong on the run's trace.
            if sum(e["event_type"] == "llm.completed" for e in ours) < 2:
                return None
            return events, ours

        found = poll(spans)
        if not found:
            _, page, _ = call("GET", f"/api/v1/runs/{execution_id}/events?limit=500")
            fail(7, "both attempts' spans reach the run", [
                (e["event_type"], e["metadata"].get("source", {}).get("service"),
                 e["metadata"].get("span_id"), e["metadata"].get("parent_span_id"))
                for e in (page or {}).get("events", [])
            ])
        events, ours = found
        step_spans = {
            e["metadata"].get("span_id") for e in events if e["event_type"].startswith("step.")
        }
        agents = [e for e in ours if e["event_type"] == "agent.started"]
        agent_spans = {e["metadata"].get("span_id") for e in agents}
        llms = [e for e in ours if e["event_type"] == "llm.started"]
        _, runs, _ = call("GET", "/api/v1/runs?limit=100")
        foreign = [
            r["run_id"] for r in (runs or {}).get("runs", []) if r["run_id"] != execution_id
        ]
        if not (
            len(agents) == 2
            and all(e["metadata"].get("parent_span_id") in step_spans for e in agents)
            and llms
            and all(e["metadata"].get("parent_span_id") in agent_spans for e in llms)
            and all(e["metadata"].get("run_id") == execution_id for e in ours)
            and not foreign
        ):
            fail(7, "the agent's spans nest under the run", {
                "agents": [e["metadata"] for e in agents][:2],
                "steps": sorted(filter(None, step_spans)),
                "runs": foreign,
            })
        ok(7, f"both attempts' agent and llm spans ({len(agents)} + {len(llms)}) nest under the turn's step; no run of the agent's own")

        def listed() -> Any:
            _, page, page_raw = call("GET", f"/api/v1/workflow-executions?workflow_id={AGENT}&limit=50")
            return next(
                (
                    (r, page_raw)
                    for r in (page or {}).get("executions", [])
                    if r["workflow_run_id"] == execution_id and r["nodes_succeeded"] == 1
                ),
                None,
            )

        row = poll(listed)
        if row is None or row[0]["nodes_total"] != 1 or SIGNATURE.encode() in row[1]:
            fail(8, "the Workflows view lists it", row)
        ok(8, "the Workflows view's fold lists it as one node, succeeded")

        trace_id = next(e["metadata"]["trace_id"] for e in ours)
        _, page, _ = call("GET", f"/api/v1/conversation-turns?conversation_id={execution_id}")
        archived = sorted((page or {}).get("turns", []), key=lambda t: t["ordinal"])
        ids = [t["message_id"] for t in archived]
        if ids != [f"{execution_id}.turn.user", f"{execution_id}.turn.assistant"] or any(
            t["provenance"].get("run_id") != execution_id
            or t["provenance"].get("trace_id") != trace_id
            for t in archived
        ):
            fail(9, "the exchange is archived once, joined to the trace", archived)
        ok(9, "the archive holds the exchange once — two turns under the step's ids, on the run's trace")

        for archived_turn in archived:
            status, _, raw = call(
                "POST",
                "/api/v1/conversation-turn-reviews",
                {
                    "conversation_id": execution_id,
                    "turn_id": archived_turn["turn_id"],
                    "review": {"state": "approved", "note": "e2e"},
                },
            )
            if status != 200:
                fail(10, "a reviewer approves the exchange", (status, raw[:300]))
        status, job, raw = call(
            "POST",
            "/api/v1/conversation-exports",
            {
                "name": EXPORT,
                "format": "sft",
                "required_scope": "train",
                "require_human_review": True,
                "selection": {"conversations": [execution_id]},
            },
        )
        if status not in (200, 201, 202):
            fail(10, "an sft export is queued", (status, raw[:300]))

        def exported() -> Any:
            _, current, _ = call("GET", f"/api/v1/conversation-exports/{job['job_id']}")
            return current if (current or {}).get("state") in ("completed", "failed", "cancelled") else None

        finished = poll(exported, within=60)
        if not finished or finished["state"] != "completed":
            fail(10, "the export completes", finished)
        _, rows, _ = call(
            "GET",
            f"/api/v1/conversation-dataset-rows?name={EXPORT}&version={finished['version']}",
        )
        corpus = (rows or {}).get("rows", [])
        if len(corpus) != 1 or corpus[0].get("completion") != answer(ASKED):
            fail(10, "one sft row whose completion is the reply", corpus)
        ok(10, f"approved and exported: `{EXPORT}` holds one sft row, completing with the reply")

        schedule = f"/api/v1/workflow-definitions/{AGENT}/schedule"
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
            fail(11, "a schedule is saved and starts a run", (status, raw[:400]))
        scheduled = saved["started"]
        state, _, _ = settled(scheduled)
        words = stored(reply_of(scheduled)[0])[0] if state == "completed" else None
        if words != answer(DEFAULT):
            fail(11, "a scheduled run answers the registered message", (state, words))
        ok(11, f"a schedule's run_now started {scheduled}, which answered the registered message")

        status, _, raw = call(
            "PUT",
            f"/api/v1/workflow-definitions/e2e.nobody-{uuid.uuid4().hex[:8]}/schedule",
            {"cadence": {"every": "daily", "hour": 9, "minute": 0}, "timezone": "Europe/Warsaw"},
        )
        if status != 404:
            fail(12, "an unregistered agent's schedule is refused", (status, raw[:300]))
        ok(12, "a schedule for an agent nobody registered is refused with 404, not saved")

        process.send_signal(signal.SIGTERM)
        try:
            expect_line(lines, "CLOSED", 30)
            process.wait(timeout=30)
        except (TimeoutError, subprocess.TimeoutExpired) as error:
            fail(13, "the host stops cleanly", error)
        if process.returncode != 0:
            fail(13, "the host stops cleanly", process.returncode)
        ok(13, "the host stopped on SIGTERM and closed the agent runtime and archive client it owns")

        completed = f"{execution_id}/turn/2"
        with sqlite3.connect(agent_home / "messages.db") as db:
            records = sum(
                db.execute(f"SELECT count(*) FROM {table} WHERE turn_id = ?", (completed,)).fetchone()[0]  # noqa: S608 — two fixed names
                for table in ("message_stream", "conversation_records")
            )
        if records == 0:
            fail(14, "the agent's records name the attempt", completed)
        ok(14, f"{records} of the agent's own records carry turn id {completed}")
        passed = True
    finally:
        for owned in (process, server):
            if owned.poll() is None:
                owned.send_signal(signal.SIGTERM)
                try:
                    owned.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    owned.kill()
                    owned.wait()
        if passed:
            shutil.rmtree(root, ignore_errors=True)
        else:
            print(f"kept for a look: {root}")
    print("14/14")


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
