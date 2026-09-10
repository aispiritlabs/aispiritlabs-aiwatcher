#!/usr/bin/env python3
"""End-to-end: lab 6's agents talk through aiwatcher, and through nothing else.

AW-2's Phase F took Apache Iggy out. What carries a message from one agent to
the next is now aiwatcher itself: a hop is a run of the target agent's one-step
workflow, claimed from that agent's own queue, and the chat's answer comes back
through a mailbox — a hosted execution it tails. This runs lab 6's own handlers
(planner → search → summary, with the model and the search engine stubbed) as
three worker processes against a server of its own, and checks:

1.  each agent is a workflow of its own, claimed from a queue of its own;
2.  there is no broker: `laser_sdk` is not installed, and no worker loaded it;
3.  a fresh process finds the search agent by its capability, from the
    definitions aiwatcher holds;
4.  the chat's question comes back answered, through its mailbox;
5.  a turn is three runs, one per agent, each completed — followed hop to hop
    through the ids each run reports;
6.  **the words are in none of them**: no run's history or projection, and not
    the mailbox's — every hop is a reference, a digest and a size, and the
    digest is of the bytes the reference names;
7.  a message sent twice is one run, and is handled once;
8.  a hand-off whose answer was lost is started again under the same id: the
    search step runs twice and the summary once;
9.  a hop waits in its agent's queue while no worker runs, and is answered
    when one starts — with nothing in between but aiwatcher;
10. a hop that cannot be handled fails its step, is not retried, and the chat
    hears so at once rather than at its timeout;
11. every worker stops on SIGTERM.

What it does not check is a worker killed *inside* a hop. aiwatcher recovers
that by lease expiry, which is five minutes, and it is the server's own rule
with its own tests rather than anything this path adds.

Runs under `ai_spirit_agent`'s environment, for lab 6's handlers.

    just e2e-agent-transport
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import shutil
import signal
import socket
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

REPOSITORY = Path(__file__).resolve().parent.parent
BINARY = Path(os.environ.get("AIWATCHER_BINARY", REPOSITORY / "target" / "debug" / "aiwatcher"))
PREFIX = "lab6e2e"
AGENTS = {
    "planner": ("plan", "query-planning"),
    "search": ("web-search", "langsearch"),
    "summary": ("summarize", "answer-synthesis"),
}
#: Words that must never reach aiwatcher. Every question carries them.
WORDS = "which rivers cross the Sahel, and who told you?"
#: In a question, tells the search worker to lose the answer to its first hand-off.
LOSE = "::lose-the-hand-off"
#: In a question, tells the planner to refuse it outright.
REFUSE = "::refuse"
#: On every answer, so its absence from aiwatcher can be looked for.
SIGNATURE = "::summarised-by-lab6"
READY_WITHIN = 60.0
RUN_WITHIN = 60.0
BASE = ""


def answer(question: str, results: int) -> str:
    return f"{results} sources on [{question}] {SIGNATURE}"


# ── A worker. This file again, as a separate process. ───────────────────────


def work(agent: str) -> None:
    from agentic.integrations.search_provider import SearchResult
    from aiwatcher_sdk.api import ApiError
    from aiwatcher_sdk.worker import get_task_context

    from agentic_runtime.distributed import AgenticServiceDiscovery, PermanentMessageError
    from workshops.lab6.runtime import PlannerHandler, SearchHandler, SummaryHandler

    class Planner:
        def plan(self, question: str) -> tuple[str, ...]:
            if REFUSE in question:
                raise PermanentMessageError("the planner will not plan this")
            return (f"{question} latest",)

        def close(self) -> None:
            pass

    class Search:
        def search(self, query: str, *, count: int = 5) -> list[Any]:
            del count
            return [
                SearchResult(title=f"About {query}", url="https://example.com/a", snippet="A"),
                SearchResult(title="More", url="https://example.com/b", snippet="B"),
            ]

        def close(self) -> None:
            pass

    class Summary:
        def summarize(self, question: str, results: Any) -> str:
            return answer(question, len(results))

        def close(self) -> None:
            pass

    handler: Any = {
        "planner": lambda: PlannerHandler(planner=Planner()),  # type: ignore[arg-type]
        "search": lambda: SearchHandler(search_provider=Search(), results_per_query=2),  # type: ignore[arg-type]
        "summary": lambda: SummaryHandler(summary=Summary()),  # type: ignore[arg-type]
    }[agent]()

    def counted(message: Any, discovery: Any) -> Any:
        context = get_task_context()
        attempt = context.assignment.attempt
        print(f"HANDLED {agent} {message.metadata.turn_id} {attempt} {context.run_id}", flush=True)
        return handler(message, discovery)

    discovery = AgenticServiceDiscovery.from_settings()
    if agent == "search":
        transport: Any = discovery.transport
        publish = transport.publish_message
        lost: set[str] = set()

        def losing(message: Any) -> str:
            landed = publish(message)
            question = message.data.get("question", "") if isinstance(message.data, dict) else ""
            if LOSE in question and message.metadata.turn_id not in lost:
                # aiwatcher started the summary's run; this process never heard.
                lost.add(message.metadata.turn_id)
                raise ApiError("aiwatcher took the hand-off and its answer was lost")
            return landed

        transport.publish_message = losing

    # The call `workshops.lab6.runtime.build_lab6_service` makes, with the model
    # and the search engine stubbed and nothing else changed.
    service = discovery.create_service(
        agent,
        capabilities=AGENTS[agent],
        handler=counted,
        role="lab6-worker",
        close_hook=handler.close,
    )
    service.register()  # type: ignore[attr-defined]
    print(f"READY {agent} laser_sdk_loaded={'laser_sdk' in sys.modules}", flush=True)
    service.run_forever()
    print(f"STOPPED {agent}", flush=True)


class Worker:
    def __init__(self, agent: str, root: Path) -> None:
        self.agent = agent
        env = {
            name: value
            for name, value in os.environ.items()
            if not name.startswith(("AIWATCHER_", "LASER_", "AGENTIC_", "DISTRIBUTED_"))
        }
        env |= {
            "AIWATCHER_URL": BASE,
            "AIWATCHER_PAYLOAD_ROOT": str(root / "payloads"),
            "AGENTIC_TRANSPORT": "aiwatcher",
            "DISTRIBUTED_PREFIX": PREFIX,
            "PYTHONUNBUFFERED": "1",
        }
        self.log = root / f"{agent}.log"
        self.process = subprocess.Popen(  # noqa: S603 — this file, as a worker
            [sys.executable, str(Path(__file__).resolve()), "--work", agent],
            cwd=root,
            env=env,
            stdout=subprocess.PIPE,
            stderr=self.log.open("a"),
            text=True,
        )
        self.lines: list[str] = []
        self._changed = threading.Condition()
        threading.Thread(target=self._pump, daemon=True).start()

    def _pump(self) -> None:
        assert self.process.stdout is not None
        with self.log.open("a") as mirror:
            for line in self.process.stdout:
                mirror.write(line)
                mirror.flush()
                with self._changed:
                    self.lines.append(line.strip())
                    self._changed.notify_all()

    def wait_for(self, prefix: str, within: float = READY_WITHIN) -> str:
        deadline = time.monotonic() + within
        with self._changed:
            while True:
                for line in self.lines:
                    if line.startswith(prefix):
                        return line
                left = deadline - time.monotonic()
                if left <= 0 or self.process.poll() is not None:
                    tail = self.log.read_text(errors="replace")[-1500:]
                    raise TimeoutError(f"{self.agent} never printed {prefix!r}:\n{tail}")
                self._changed.wait(min(left, 0.5))

    def handled(self, turn: str) -> list[int]:
        with self._changed:
            return [
                int(parts[3])
                for line in self.lines
                if (parts := line.split()) and parts[0] == "HANDLED" and parts[2] == turn
            ]

    def count(self) -> int:
        with self._changed:
            return sum(line.startswith("HANDLED") for line in self.lines)


# ── The server ───────────────────────────────────────────────────────────────


def serve(root: Path) -> subprocess.Popen[bytes]:
    """Start an aiwatcher of our own and wait for it."""
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
    env = {name: value for name, value in os.environ.items() if not name.startswith("AIWATCHER_")}
    env |= {
        "AIWATCHER_LISTEN": f"127.0.0.1:{port}",
        "AIWATCHER_DATA_DIR": str(home / ".data"),
        "AIWATCHER_BUS": "wal",
        # Every agent is a worker in another process and every mailbox is a
        # hosted run; the `file` store holds one process and refuses both.
        "AIWATCHER_WORKFLOW_STORE": "memory",
        "AIWATCHER_INGEST_ENABLED": "true",
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
        if call("GET", "/readyz")[0] == 200:
            return process
        time.sleep(0.2)
    process.kill()
    tail = (home / "server.log").read_text(errors="replace")[-2000:]
    raise SystemExit(f"aiwatcher did not come up on {BASE}:\n{tail}")


def call(method: str, path: str, body: Any = None) -> tuple[int, Any, bytes]:
    data = None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(BASE + path, data=data, method=method)
    request.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(request, timeout=15) as response:  # noqa: S310 — our own server
            raw, status = response.read(), response.status
    except urllib.error.HTTPError as error:
        raw, status = error.read(), error.code
    except (urllib.error.URLError, ConnectionError):
        return 0, None, b""
    try:
        return status, json.loads(raw) if raw else None, raw
    except json.JSONDecodeError:
        return status, None, raw


def settled(execution: str) -> tuple[str, Any, bytes]:
    deadline = time.monotonic() + RUN_WITHIN
    while True:
        status, body, raw = call("GET", f"/api/v1/executions/{execution}")
        state = (body or {}).get("execution", {}).get("state", {}).get("state_type")
        if state in ("completed", "failed", "cancelled", "crashed"):
            return state, body, raw
        if time.monotonic() > deadline:
            return f"still {state} (HTTP {status})", body, raw
        time.sleep(0.25)


def state_of(execution: str) -> str:
    _, body, _ = call("GET", f"/api/v1/executions/{execution}")
    return str((body or {}).get("execution", {}).get("state", {}).get("state_type"))


def history(execution: str) -> tuple[Any, bytes]:
    _, body, raw = call("GET", f"/api/v1/executions/{execution}/history?after=0&limit=500")
    return body, raw


def found(value: Any, keys: set[str]) -> list[dict[str, Any]]:
    """Every object inside *value* holding all of *keys*, in order."""
    hits: list[dict[str, Any]] = []

    def walk(item: Any) -> None:
        if isinstance(item, dict):
            if keys <= set(item):
                hits.append(item)
            for child in item.values():
                walk(child)
        elif isinstance(item, list):
            for child in item:
                walk(child)

    walk(value)
    return hits


def hop_result(execution: str) -> dict[str, Any]:
    """What the hop reported: which agent handled it, and where its answers went."""
    body, _ = history(execution)
    results = found(body, {"agent", "handled", "sent"})
    if not results:
        raise LookupError(f"no hop result in the history of {execution}")
    return results[-1]


def follow(execution: str, depth: int = 0) -> None:
    """Print a run's state and every run it handed off to, for a failure."""
    _, body, _ = call("GET", f"/api/v1/executions/{execution}")
    run = (body or {}).get("execution", {})
    steps = [(s.get("state"), s.get("current_attempt")) for s in run.get("steps", [])]
    print(f"   {'  ' * depth}{run.get('definition_name')} {execution[:8]}: {run.get('state')} {steps}")
    try:
        sent = hop_result(execution)["sent"]
    except LookupError:
        return
    for handed in sent:
        if not str(handed.get("to", "")).startswith("mailbox:"):
            follow(str(handed["landed"]), depth + 1)


def ok(number: int, claim: str) -> None:
    print(f"  ✓ {number:>2}. {claim}")


def fail(number: int, claim: str, detail: object) -> None:
    raise SystemExit(f"  ✗ {number:>2}. {claim}\n       {detail}")


# ── The orchestrator ─────────────────────────────────────────────────────────


def orchestrate() -> None:
    from aiwatcher_sdk.integrations.agentic import FilePayloadStore

    from agentic_runtime.distributed import (
        AgenticServiceDiscovery,
        AiwatcherServiceRegistry,
        AiwatcherTransport,
        DistributedAgenticRuntime,
    )
    from agentic_runtime.messaging.messages import (
        AssistantMessage,
        ConversationData,
        RecordedMessageMetadata,
        UserMessage,
    )
    import workshops.lab6.messages  # noqa: F401 — lab 6's record types, for reading replies

    root = Path(tempfile.mkdtemp(prefix="aiwatcher-e2e-transport-"))
    server = serve(root)
    workers: dict[str, Worker] = {}
    passed = False
    print(f"aiwatcher at {BASE}, scratch in {root}\n")
    try:
        for agent in AGENTS:
            workers[agent] = Worker(agent, root)
        ready = {agent: worker.wait_for("READY") for agent, worker in workers.items()}

        # 1 ── every agent a workflow of its own, on a queue of its own.
        _, saved, _ = call("GET", "/api/v1/workflow-definitions")
        steps = {
            entry["definition"]["name"]: entry["definition"]["steps"]
            for entry in saved or []
        }
        shapes = {
            agent: [(step["id"], step.get("queue")) for step in steps.get(f"{PREFIX}.{agent}", [])]
            for agent in AGENTS
        }
        if any(shape != [("hop", f"{PREFIX}.{agent}")] for agent, shape in shapes.items()):
            fail(1, "each agent a one-step workflow claimed from its own queue", shapes)
        ok(1, f"three workflows, one step each, each on its own queue: {', '.join(f'{PREFIX}.{a}' for a in AGENTS)}")

        # 2 ── no broker anywhere.
        installed = importlib.util.find_spec("laser_sdk") is not None
        loaded = [line for line in ready.values() if "laser_sdk_loaded=False" not in line]
        if installed or loaded:
            fail(2, "no broker: laser_sdk neither installed nor loaded", (installed, loaded))
        ok(2, "no broker: laser_sdk is not installed, and no worker loaded it")

        # 3 ── the registry is what aiwatcher holds.
        transport = AiwatcherTransport(
            BASE, prefix=PREFIX, payloads=FilePayloadStore(root / "payloads" / "hops")
        )
        registry = AiwatcherServiceRegistry(transport)
        names = [agent.agent_name for agent in registry.live_agents(max_age_seconds=0)]
        search = registry.find_by_capability("web-search", max_age_seconds=0)
        if names != sorted(AGENTS) or search is None or search.agent_name != "search":
            fail(3, "a fresh process finds the search agent by capability", (names, search))
        ok(3, f"a fresh process lists {names} and finds `web-search` at `{search.agent_name}`")

        # 4 ── the lab's own client, through the mailbox.
        runtime = DistributedAgenticRuntime(
            AgenticServiceDiscovery(transport, registry),
            entry_agent="planner",
            source="chat",
            timeout_seconds=RUN_WITHIN,
            domain="lab6",
        )
        reply = runtime.run(WORDS)
        if reply != answer(WORDS, 2):
            fail(4, "the chat's question comes back answered", reply)
        ok(4, "lab 6's chat client asked, and the summary's answer came back through its mailbox")

        def ask(question: str, *, turn: str, message_id: str) -> str:
            return transport.publish_message(
                UserMessage(
                    data=ConversationData(role="user", text=question),
                    metadata=RecordedMessageMetadata(
                        runtime_id=turn,
                        turn_id=turn,
                        domain="lab6",
                        source=transport.reply_address("chat"),
                        target="planner",
                        message_id=message_id,
                    ),
                )
            )

        def reply_to(turn: str, cursor: str, within: float = RUN_WITHIN) -> str | None:
            deadline = time.monotonic() + within
            while time.monotonic() < deadline:
                for record in transport.read_messages("chat", after_id=cursor, block_ms=500):
                    cursor = record.entry_id
                    message = record.record
                    if isinstance(message, AssistantMessage) and message.metadata.turn_id == turn:
                        return message.data.text
            return None

        # 5 ── one turn, three runs, followed hop to hop.
        turn, message_id = uuid.uuid4().hex, uuid.uuid4().hex
        question = f"{WORDS} {LOSE}"
        cursor = transport.last_message_id("chat")
        runs = {"planner": ask(question, turn=turn, message_id=message_id)}
        if reply_to(turn, cursor) != answer(question, 2):
            fail(5, "the turn is answered", turn)
        for agent, following in (("planner", "search"), ("search", "summary")):
            if settled(runs[agent])[0] != "completed":
                fail(5, f"the {agent} run completed", state_of(runs[agent]))
            (handed,) = [sent for sent in hop_result(runs[agent])["sent"] if sent["to"] == following]
            runs[following] = handed["landed"]
        states = {agent: settled(run)[0] for agent, run in runs.items()}
        agents = {agent: hop_result(run)["agent"] for agent, run in runs.items()}
        if set(states.values()) != {"completed"} or agents != {a: a for a in AGENTS}:
            fail(5, "three runs, one per agent, each completed", (states, agents))
        ok(5, "one turn, three runs — " + " → ".join(f"{a} {r[:8]}" for a, r in runs.items()))

        # 6 ── the words are nowhere in aiwatcher.
        mailbox = transport.mailbox(transport.reply_address("chat"))
        leaks: list[str] = []
        hops: list[dict[str, Any]] = []
        for label, execution in [*runs.items(), ("mailbox", mailbox)]:
            _, _, projection = call("GET", f"/api/v1/executions/{execution}")
            body, raw = history(execution)
            for blob in (projection, raw):
                if WORDS.encode() in blob or SIGNATURE.encode() in blob:
                    leaks.append(label)
            if label != "mailbox":
                hops.extend(found(body, {"message_id", "reference", "digest", "size"}))
        mismatched = [
            hop["reference"]
            for hop in hops
            if hashlib.sha256(Path(hop["reference"].removeprefix("file://")).read_bytes()).hexdigest()
            != hop["digest"]
        ]
        if leaks or not hops or mismatched:
            fail(6, "the words are in no run and no mailbox", (leaks, len(hops), mismatched))
        ok(6, f"no words in three runs or the mailbox; {len(hops)} hop references, each digest the bytes it names")

        # 7 ── the same message twice is one run.
        again = ask(question, turn=turn, message_id=message_id)
        time.sleep(2.0)
        planned = workers["planner"].handled(turn)
        if again != runs["planner"] or planned != [1]:
            fail(7, "a message sent twice is one run, handled once", (again, runs["planner"], planned))
        ok(7, f"sent twice under one id: one run ({again[:8]}), handled once")

        # 8 ── a lost answer: the hand-off repeated, the summary run once.
        _, search_view, _ = settled(runs["search"])
        [search_step] = search_view["execution"]["steps"]
        searched, summarised = workers["search"].handled(turn), workers["summary"].handled(turn)
        if search_step.get("current_attempt") != 2 or searched != [1, 2] or summarised != [1]:
            fail(8, "the lost hand-off repeated under one id", (search_step, searched, summarised))
        ok(8, "search lost the answer to its hand-off, ran again on attempt 2 under the same id; summary ran once")

        # 9 ── a hop waits for its agent with nothing but aiwatcher holding it.
        workers["summary"].process.kill()
        workers["summary"].process.wait(timeout=10)
        waiting, cursor = uuid.uuid4().hex, transport.last_message_id("chat")
        planner_run = ask(f"{WORDS} (asked while nobody summarises)", turn=waiting, message_id=uuid.uuid4().hex)
        settled(planner_run)
        search_run = next(s["landed"] for s in hop_result(planner_run)["sent"] if s["to"] == "search")
        settled(search_run)
        summary_run = next(s["landed"] for s in hop_result(search_run)["sent"] if s["to"] == "summary")
        time.sleep(3.0)
        parked = state_of(summary_run)
        early = reply_to(waiting, cursor, within=1.0)
        if parked in ("completed", "failed") or early is not None:
            fail(9, "the summary hop waits while no summary worker runs", (parked, early))
        workers["summary"] = Worker("summary", root)
        workers["summary"].wait_for("READY")
        late = reply_to(waiting, cursor)
        if late is None or settled(summary_run)[0] != "completed":
            fail(9, "answered once a summary worker started", (late, state_of(summary_run)))
        ok(9, f"the summary hop sat `{parked}` with no worker and no broker; a new worker took it")

        # 10 ── refused outright: the sink, and the chat told at once.
        before = workers["planner"].count()
        started = time.monotonic()
        try:
            refused: str | Exception = runtime.run(f"{WORDS} {REFUSE}")
        except RuntimeError as error:
            refused = error
        elapsed = time.monotonic() - started
        time.sleep(3.0)
        tries = workers["planner"].count() - before
        if "planner failed" not in str(refused) or elapsed > 15 or tries != 1:
            fail(10, "a refused hop fails once and the chat hears at once", (refused, elapsed, tries))
        ok(10, f"the planner refused; the chat heard in {elapsed:.1f}s of a {RUN_WITHIN:.0f}s timeout, and nothing retried it")

        # 11 ── SIGTERM stops every worker.
        for worker in workers.values():
            worker.process.send_signal(signal.SIGTERM)
        codes = {agent: worker.process.wait(timeout=20) for agent, worker in workers.items()}
        stopped = {agent: bool(worker.wait_for("STOPPED", within=1)) for agent, worker in workers.items()}
        if set(codes.values()) != {0} or not all(stopped.values()):
            fail(11, "every worker stops on SIGTERM", codes)
        ok(11, "SIGTERM: all three workers stopped cleanly")

        runtime.stop()
        passed = True
        print("\n11/11 — lab 6 ran on aiwatcher with no broker")
    finally:
        if not passed:
            # Before anything stops: the server is what knows why a run stalled.
            for worker in workers.values():
                print(f"\n── {worker.agent}, last lines:")
                print("\n".join(worker.lines[-12:]) or "(nothing)")
                for line in worker.lines:
                    parts = line.split()
                    if parts[:1] == ["HANDLED"] and len(parts) > 4:
                        follow(parts[4])
        for worker in workers.values():
            if worker.process.poll() is None:
                worker.process.kill()
        server.terminate()
        try:
            server.wait(timeout=10)
        except subprocess.TimeoutExpired:
            server.kill()
        if passed:
            shutil.rmtree(root, ignore_errors=True)
        else:
            print(f"\nkept {root} — server.log and one log per worker are in it")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--work", choices=sorted(AGENTS), help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.work:
        work(args.work)
        return
    if importlib.util.find_spec("workshops") is None:
        raise SystemExit(
            "workshops is not importable: run this under ai_spirit_agent's environment "
            "(`just e2e-agent-transport` does)"
        )
    orchestrate()


if __name__ == "__main__":
    main()
