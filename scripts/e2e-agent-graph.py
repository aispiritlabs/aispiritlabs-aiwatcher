"""A composed graph's turn, drawn by aiwatcher against the shape it declared.

Phase A's last acceptance, from AW-2's investigation: the panel draws the graph
with `Pending` nodes and the dispatches as messages. This runs turns of a routed
graph — an entry that hands the question to one of three searchers, a
summarizer waiting for it, a file the answer is written to — through the real
compiler, with its model-backed agents stubbed, and reads back what aiwatcher
made of them:

1. a turn is one execution, under the graph's own id;
2. its shape is the six nodes a turn can reach, every one declared — no model
   provider and no search integration;
3. the four nodes the turn reached succeeded, and the two searchers the router
   passed over are `Pending`;
4. the hand-offs are messages between agents: the entry's dispatch, and the
   searcher's answer to the join;
5. the execution succeeded with two nodes pending — a branch not taken is not a
   stall;
6. a span the agent's tracer opened, through a client of its own, is a child of
   its node's step in the run's trace;
7. the tracer opened no run of its own: one run per turn, and nothing else;
8. a second turn is a second execution of the same declared version.

It starts **its own** aiwatcher, from `target/debug/aiwatcher` or
`AIWATCHER_BINARY`, on a free port with every byte under a temporary directory,
so the one on :8080 is not touched. It runs under `ai_spirit_agent`'s
environment, because the compiler is its code.

    cargo build --bin aiwatcher   # once
    just e2e-agent-graph
"""

from __future__ import annotations

import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

REPOSITORY = Path(__file__).resolve().parent.parent
BINARY = Path(os.environ.get("AIWATCHER_BINARY", REPOSITORY / "target" / "debug" / "aiwatcher"))
GRAPH = "e2e-graph-routed"
#: The searcher the router picks, and the two it passes over.
CHOSEN = "search-b"
PASSED_OVER = ("search-a", "search-c")
REACHED = ("entry", CHOSEN, "summarizer", "output")
READY_WITHIN = 60.0
SETTLE_WITHIN = 20.0
BASE = ""
#: The tracer the stubbed searcher opens its span through — the runtime's own,
#: publishing through a client that is not the declaration's.
TRACER: Any = None


# ── The graph, and the agents in it. ─────────────────────────────────────────


def routed_graph(output_path: str) -> Any:
    from agentic_graph.models import AgentGraph, AgentNode, Connection, NodePosition

    def node(
        node_id: str,
        agent_name: str,
        display_name: str,
        *,
        node_type: str = "agent",
        config: tuple[tuple[str, str], ...] = (),
    ) -> AgentNode:
        return AgentNode(
            node_id=node_id,
            agent_name=agent_name,
            display_name=display_name,
            description=f"{display_name} description",
            capabilities=(agent_name,),
            position=NodePosition(0, 0),
            node_type=node_type,  # type: ignore[arg-type]
            config=config,
        )

    return AgentGraph(
        graph_id=GRAPH,
        name="Routed search",
        nodes=(
            node("entry", "llm_chat", "Entry", config=(("dispatch_mode", "route_one"),)),
            *(node(f"search-{x}", "searcher", f"Search {x.upper()}") for x in "abc"),
            node("summarizer", "summarizer", "Summarizer"),
            node(
                "provider",
                "model_provider",
                "Shared Provider",
                node_type="provider",
                config=(("provider_type", "openai"), ("model_id", "e2e-model")),
            ),
            *(
                node(
                    f"tavily-{x}",
                    "tavily_search",
                    f"Tavily {x.upper()}",
                    node_type="integration",
                    config=(("api_key_env", "TAVILY_API_KEY"),),
                )
                for x in "abc"
            ),
            node(
                "output",
                "markdown_output",
                "Markdown",
                node_type="structural_output",
                config=(("path", output_path),),
            ),
        ),
        connections=(
            *(Connection(f"to-{x}", "entry", f"search-{x}") for x in "abc"),
            *(Connection(f"uses-{x}", f"search-{x}", f"tavily-{x}") for x in "abc"),
            *(Connection(f"joins-{x}", f"search-{x}", "summarizer") for x in "abc"),
            *(
                Connection(f"model-{n}", "provider", n)
                for n in ("entry", "search-a", "search-b", "search-c", "summarizer")
            ),
            Connection("writes", "summarizer", "output"),
        ),
        entry_node_id="entry",
    )


class Result:
    tool_calls = ()
    run_id = "e2e"
    prompt_snapshot = None
    trace = None
    attempt_no = None
    loop_iteration = None
    request_usage = type(
        "Usage",
        (),
        {
            "prompt_tokens": 0,
            "completion_tokens": 0,
            "total_tokens": 0,
            "latency_ms": 0.0,
            "model": "",
            "finish_reason": "",
        },
    )()


class Response:
    def __init__(self, output: str) -> None:
        self.output = output
        self.result = Result()
        self.tool_results = ()


class Integration:
    def __init__(self, api_key: str) -> None:
        self.api_key = api_key

    def close(self) -> None:
        return None


class Entry:
    def __init__(self, model_name: str, **_: object) -> None:
        self.model_name = model_name

    def respond(self, message: str) -> Response:
        return Response(f"asked: {message}")

    def close(self) -> None:
        return None


class Searcher:
    def __init__(self, model_id: str, search_provider: Any, **_: object) -> None:
        self.model_id = model_id

    def respond(self, message: str) -> Response:
        # What a real agent's tracer does around its turn, through the tracer the
        # runtime was handed — and so through a client of its own.
        with TRACER.agent(name="searcher"):
            return Response(f"found: {message}")

    def close(self) -> None:
        return None


class Summarizer:
    def __init__(self, model_id: str, **_: object) -> None:
        self.model_id = model_id

    def summarize(self, text: str) -> str:
        return f"summary of {len(text)} characters"

    def close(self) -> None:
        return None


class Router:
    def __init__(self, model_id: str, **_: object) -> None:
        self.model_id = model_id

    def route(self, message: str, available_agents: str) -> str:
        del message, available_agents
        return "search_b"

    def close(self) -> None:
        return None


# ── The server. ──────────────────────────────────────────────────────────────


def call(method: str, path: str) -> tuple[int, Any]:
    request = urllib.request.Request(BASE + path, method=method)  # noqa: S310 - our own server
    request.add_header("Accept", "application/json")
    try:
        with urllib.request.urlopen(request, timeout=10) as response:  # noqa: S310
            raw = response.read()
            try:
                return response.status, json.loads(raw) if raw else None
            except ValueError:
                # `/readyz` answers in words, not JSON.
                return response.status, raw.decode(errors="replace")
    except urllib.error.HTTPError as error:
        return error.code, None
    except OSError:
        return 0, None


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
    # Nothing inherited: an AIWATCHER_AUTH_MODE in somebody's shell would make
    # this a different test.
    env = {name: value for name, value in os.environ.items() if not name.startswith("AIWATCHER_")}
    env |= {
        "AIWATCHER_LISTEN": f"127.0.0.1:{port}",
        "AIWATCHER_DATA_DIR": str(home / ".data"),
        "AIWATCHER_BUS": "wal",
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


def rows(body: Any) -> list[Any]:
    """A list route's rows, whatever key its page puts them under."""
    if isinstance(body, list):
        return body
    if isinstance(body, dict):
        return next((value for value in body.values() if isinstance(value, list)), [])
    return []


def settled(turn: str) -> dict[str, Any]:
    """The execution once its run has ended — events arrive after `run` returns."""
    deadline = time.monotonic() + SETTLE_WITHIN
    body: Any = None
    while time.monotonic() < deadline:
        status, body = call("GET", f"/api/v1/workflow-executions/{turn}")
        if status == 200 and body["summary"]["status"] in {"succeeded", "failed"}:
            return dict(body)
        time.sleep(0.2)
    raise SystemExit(f"execution {turn} never settled: {body}")


def child_of(run_id: str, parent: str) -> dict[str, Any] | None:
    """A finished span in the run whose parent is `parent`, once it has arrived."""
    deadline = time.monotonic() + SETTLE_WITHIN
    while time.monotonic() < deadline:
        status, body = call("GET", f"/api/v1/runs/{run_id}")
        if status == 200:
            for span in body.get("spans", []):
                if span.get("parent_span_id") == parent:
                    return dict(span)
        time.sleep(0.2)
    return None


# ── The checks. ──────────────────────────────────────────────────────────────


class Checks:
    def __init__(self) -> None:
        self.passed = 0
        self.total = 0

    def check(self, number: int, claim: str, holds: bool, detail: object = "") -> None:
        self.total += 1
        self.passed += int(holds)
        mark = "✓" if holds else "✗"
        print(f"  {mark} {number:>2}. {claim}" + (f" — {detail}" if detail != "" else ""))


def orchestrate(root: Path) -> bool:
    global TRACER
    os.environ["AIWATCHER_URL"] = BASE
    os.environ.pop("AIWATCHER_TOKEN", None)
    os.environ["TAVILY_API_KEY"] = "e2e-key"
    work = root / "agent"
    work.mkdir()
    # The compiler refuses an output outside the working directory.
    os.chdir(work)

    from agentic.workflow import InMemoryMessageBus, WorkflowRuntime
    from agentic_graph import compiler
    from aiwatcher_sdk import AiwatcherClient
    from aiwatcher_sdk.integrations.agentic import AiwatcherTracer

    tracer_client = AiwatcherClient(service="e2e-agent-graph-tracer")
    TRACER = AiwatcherTracer(client=tracer_client)
    compiler.TavilySearchProvider = Integration  # type: ignore[assignment,misc]
    compiler.LLMCall = Entry  # type: ignore[assignment,misc]
    compiler.SearchAgent = Searcher  # type: ignore[assignment,misc]
    compiler.SummarizationAgent = Summarizer  # type: ignore[assignment,misc]
    compiler.GenericRouter = Router  # type: ignore[assignment,misc]

    graph = routed_graph(str(work / "answer.md"))
    # No declaration passed: the one `AIWATCHER_URL` asks for is the one tested.
    system = compiler.build_compiled_graph_system(
        graph, runtime=WorkflowRuntime(bus=InMemoryMessageBus(), tracer=TRACER)
    )
    if not system.declaration.declares:
        raise SystemExit("AIWATCHER_URL was set and the graph declares nothing")
    try:
        system.run("what changed overnight?")
        system.run("and since Monday?")
        turns = list(system.final_responses)
    finally:
        system.close()
        system.runtime.close()
        tracer_client.close()

    checks = Checks()
    first = settled(turns[0])
    summary = first["summary"]
    nodes = {node["node_id"]: node for node in first["nodes"]}

    checks.check(
        1,
        "a turn is one execution, under the graph's own id",
        summary["workflow_id"] == GRAPH and summary["workflow_run_id"] == turns[0],
        f"{summary['workflow_id']} / {turns[0]}",
    )
    expected = {"entry", "search-a", "search-b", "search-c", "summarizer", "output"}
    checks.check(
        2,
        "its shape is the six nodes a turn can reach, every one declared",
        set(nodes) == expected and all(node["declared"] for node in nodes.values()),
        sorted(nodes),
    )
    statuses = {node_id: node["status"] for node_id, node in nodes.items()}
    checks.check(
        3,
        "the nodes it reached succeeded, the searchers passed over are Pending",
        all(statuses.get(n) == "succeeded" for n in REACHED)
        and all(statuses.get(n) == "pending" for n in PASSED_OVER),
        statuses,
    )
    said = [(m["from"], m["to"], m.get("kind")) for m in first["messages"]]
    checks.check(
        4,
        "the hand-offs are messages between agents, not edges of the shape",
        said == [("entry", "search_b", "dispatch"), ("search_b", "summarizer", "completion")]
        and all((e["from"], e["to"]) != ("entry", "search_b") for e in first["edges"]),
        said,
    )
    checks.check(
        5,
        "the execution succeeded with two nodes pending",
        summary["status"] == "succeeded" and summary["nodes_pending"] == 2,
        f"{summary['status']}, {summary['nodes_pending']} pending",
    )
    runs = summary["runs"]
    step = nodes.get(CHOSEN, {}).get("span_id")
    child = child_of(runs[0], step) if runs and step else None
    checks.check(
        6,
        "the searcher's own span is a child of its node's step, across two clients",
        child is not None,
        f"{child['name']} under {step}" if child else f"no child of {step}",
    )
    _, listed = call("GET", "/api/v1/runs")
    checks.check(
        7,
        "the tracer opened no run of its own: one run per turn, nothing else",
        len(runs) == 1 and len(rows(listed)) == len(turns),
        f"{len(rows(listed))} run(s) for {len(turns)} turn(s)",
    )
    second = settled(turns[1])
    checks.check(
        8,
        "a second turn is a second execution of the same declared version",
        second["summary"]["workflow_run_id"] != turns[0]
        and second["summary"]["workflow_id"] == GRAPH
        and second["summary"]["version"] == summary["version"] is not None,
        second["summary"]["version"],
    )
    print(f"{checks.passed}/{checks.total}")
    return checks.passed == checks.total


def main() -> None:
    try:
        import agentic_graph.compiler  # noqa: F401
    except ImportError as error:
        raise SystemExit(
            "agentic_graph is not importable: run this under ai_spirit_agent's environment "
            "(`just e2e-agent-graph` does)"
        ) from error
    root = Path(tempfile.mkdtemp(prefix="aiwatcher-e2e-graph-"))
    server = serve(root)
    print(f"aiwatcher of our own at {BASE}; everything under {root}")
    passed = False
    try:
        passed = orchestrate(root)
    finally:
        server.terminate()
        server.wait(timeout=10)
        if passed:
            shutil.rmtree(root, ignore_errors=True)
        else:
            print(f"kept {root} — server.log is in it")
    sys.exit(0 if passed else 1)


if __name__ == "__main__":
    main()
