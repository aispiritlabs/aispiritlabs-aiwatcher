#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["aiwatcher-sdk"]
#
# [tool.uv.sources]
# aiwatcher-sdk = { path = "../sdk/python", editable = true }
# ///
"""Fill a development server with enough varied data to click around in.

``just dev`` runs this once the server answers, and what it writes has two
lifetimes, because the server keeps it in two places:

* **The event log** — runs, spans, evaluations, declared workflows. ``just dev``
  runs on the in-memory bus, so all of it is gone on a restart and all of it is
  published again on every start: several hundred runs across six workflows,
  six runtimes, a dozen agents and five models, spread over the last week and
  busiest in the last day — with the failed calls, retries, streamed responses,
  stalled runs and still-running ones that make a view worth opening.
* **The authored registries** under ``./.data`` — prompts, annotations,
  training, imports, conversations. They survive a restart, and seeding them
  twice would record a second optimisation and register a second model
  version, so each step runs once and is written down in
  ``./.data/dev-seed.json``. A step that failed is not written down and is
  tried again next time; ``--registries`` does every one of them again.

With ``--live`` it then keeps a run arriving every few seconds, in real time,
so the Live view has something moving.

Everything goes through the public API, and the existing ``seed-*`` scripts are
run rather than restated — this adds volume and variety around them.
Deterministic for a given ``--seed``, apart from the clock.
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import json
import math
import os
import random
import signal
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
import uuid
from collections import Counter
from dataclasses import dataclass, field
from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent.parent
DATA = Path(os.environ.get("AIWATCHER_DATA_DIR", ROOT / ".data"))
RECORD = DATA / "dev-seed.json"
LOG = DATA / "dev-seed.log"
PANEL = os.environ.get("AIWATCHER_PANEL_URL", "http://localhost:5173")


def say(line: str) -> None:
    print(f"seed │ {line}", flush=True)


def iso(moment: datetime) -> str:
    return moment.astimezone(UTC).isoformat(timespec="milliseconds").replace("+00:00", "Z")


# ── HTTP ─────────────────────────────────────────────────────────────────────


class Api:
    def __init__(self, base: str) -> None:
        self.base = base.rstrip("/")

    def call(
        self,
        method: str,
        path: str,
        body: Any = None,
        *,
        headers: dict[str, str] | None = None,
    ) -> tuple[int, Any]:
        request = urllib.request.Request(  # noqa: S310 — a URL this script was given
            self.base + path,
            data=None if body is None else json.dumps(body).encode(),
            method=method,
            headers={"content-type": "application/json", **(headers or {})},
        )
        try:
            with urllib.request.urlopen(request, timeout=30) as response:  # noqa: S310
                return response.status, decode(response.read())
        except urllib.error.HTTPError as error:
            return error.code, decode(error.read())

    def get(self, path: str) -> tuple[int, Any]:
        return self.call("GET", path)

    def post(self, path: str, body: Any, **kwargs: Any) -> tuple[int, Any]:
        return self.call("POST", path, body, **kwargs)

    def publish(self, events: list[dict[str, Any]]) -> None:
        for start in range(0, len(events), 400):
            status, answer = self.post("/api/v1/events", {"events": events[start : start + 400]})
            if status >= 300:
                raise RuntimeError(f"/api/v1/events answered {status}: {answer}")

    def alive(self) -> bool:
        try:
            with urllib.request.urlopen(self.base + "/livez", timeout=2):  # noqa: S310
                return True
        except (urllib.error.URLError, OSError):
            return False


def decode(raw: bytes) -> Any:
    if not raw:
        return None
    try:
        return json.loads(raw)
    except ValueError:
        return raw.decode(errors="replace")


# ── What there is to run ─────────────────────────────────────────────────────


@dataclass(frozen=True)
class Model:
    provider: str
    prompt_tokens: int
    completion_tokens: int
    seconds: float


MODELS = {
    "claude-opus-5": Model("anthropic", 2600, 900, 7.0),
    "claude-sonnet-5": Model("anthropic", 1800, 600, 3.5),
    "claude-haiku-4-5": Model("anthropic", 900, 220, 1.1),
    "gpt-4.1": Model("openai", 2000, 650, 4.0),
    "gemini-2.5-pro": Model("google", 2400, 800, 5.5),
}

#: How long each tool takes, low and high, in seconds.
TOOLS: dict[str, tuple[float, float]] = {
    "lookup_customer": (0.05, 0.4),
    "search_kb": (0.2, 1.2),
    "get_order": (0.1, 0.6),
    "issue_refund": (0.3, 1.5),
    "web_search": (0.8, 3.5),
    "fetch_url": (0.3, 4.0),
    "vector_search": (0.05, 0.5),
    "rerank": (0.1, 0.8),
    "read_file": (0.01, 0.1),
    "git_diff": (0.05, 0.3),
    "run_tests": (4.0, 40.0),
    "sql_query": (0.2, 6.0),
    "plot_chart": (0.3, 1.0),
    "ocr_page": (1.0, 5.0),
}

TOOL_ERRORS = {
    "web_search": "HTTP 503 from the search provider",
    "fetch_url": "timed out after 10s",
    "sql_query": "permission denied for table payments",
    "run_tests": "3 tests failed",
    "issue_refund": "refund exceeds the order total",
    "get_order": "order not found",
    "ocr_page": "page 3 is blank",
}

#: What a provider answers that the SDK retries, and what ends a run.
RETRYABLE = ("rate_limit_error: 429 Too Many Requests", "overloaded_error: 529 Overloaded")
FATAL = (
    "request timed out after 60s",
    "context_length_exceeded: 212341 tokens > 200000",
    "invalid_request_error: tool_use without a matching tool_result",
)


@dataclass(frozen=True)
class Agent:
    name: str
    models: tuple[str, ...]
    tools: tuple[str, ...] = ()
    calls: tuple[int, int] = (1, 2)


@dataclass(frozen=True)
class Scenario:
    workflow: str
    service: str
    sdk: str
    weight: int
    #: The registry prompt its first call names, when the registry holds it.
    prompt: str | None
    agents: tuple[Agent, ...]


SCENARIOS = (
    Scenario(
        "customer-support", "support-bot", "python", 30, "support.triage",
        (
            Agent("triage-agent", ("claude-haiku-4-5",), ("lookup_customer",), (1, 1)),
            Agent(
                "support-agent",
                ("claude-sonnet-5", "claude-sonnet-5", "gpt-4.1"),
                ("search_kb", "get_order", "issue_refund"),
                (1, 3),
            ),
        ),
    ),
    Scenario(
        "research-summary", "research-service", "python", 18, "research.summarize",
        (
            Agent("planner-agent", ("claude-opus-5",), (), (1, 1)),
            Agent("research-agent", ("claude-sonnet-5", "gemini-2.5-pro"), ("web_search", "fetch_url"), (2, 4)),
            Agent("writer-agent", ("claude-opus-5",), (), (1, 2)),
        ),
    ),
    Scenario(
        "rag-qa", "rag-api", "python", 20, "rag.answer",
        (
            Agent("retriever-agent", ("claude-haiku-4-5",), ("vector_search", "rerank"), (1, 1)),
            Agent("answer-agent", ("claude-sonnet-5", "gemini-2.5-pro"), (), (1, 1)),
        ),
    ),
    Scenario(
        "code-review", "code-review-bot", "typescript", 14, "code.review",
        (
            Agent("reviewer-agent", ("claude-opus-5", "gpt-4.1"), ("read_file", "git_diff", "run_tests"), (2, 4)),
            Agent("summarizer-agent", ("claude-haiku-4-5",), (), (1, 1)),
        ),
    ),
    Scenario(
        "house-import", "planner-import-service", "python", 10, "planner.floor-plan",
        (Agent("floor-plan", ("claude-opus-5",), ("ocr_page",), (1, 2)),),
    ),
    Scenario(
        "sql-analyst", "analytics-agent", "typescript", 8, None,
        (Agent("analyst-agent", ("claude-sonnet-5", "gpt-4.1"), ("sql_query", "plot_chart"), (2, 3)),),
    ),
)

#: How a run ends, and how often. A retried call and a failed tool still end
#: in success — the waterfall shows them, the status does not.
FATES = {"ok": 80, "llm_retry": 7, "tool_error": 6, "failed": 7}


def fate(rng: random.Random) -> str:
    return rng.choices(list(FATES), list(FATES.values()))[0]


# ── Events on a clock ────────────────────────────────────────────────────────


@dataclass
class Timeline:
    """Events on one clock. A run is one ``run_id`` on it; a workflow execution is several."""

    clock: datetime
    service: str
    sdk: str
    instance: str
    conversation: str | None = None
    workflow: str | None = None
    execution: str | None = None
    events: list[dict[str, Any]] = field(default_factory=list)
    moments: list[datetime] = field(default_factory=list)
    sequences: dict[str, int] = field(default_factory=dict)

    def emit(
        self,
        run_id: str,
        event_type: str,
        data: dict[str, Any] | None = None,
        agent: str | None = None,
        *,
        after: float = 0.0,
    ) -> None:
        self.clock += timedelta(seconds=after)
        sequence = self.sequences[run_id] = self.sequences.get(run_id, 0) + 1
        event: dict[str, Any] = {
            "event_id": f"{run_id}-{sequence}",
            "event_type": event_type,
            "occurred_at": iso(self.clock),
            "run_id": run_id,
            "sequence": sequence,
            "source": {"service": self.service, "sdk": self.sdk, "instance": self.instance},
            "data": data or {},
        }
        if self.conversation:
            event["conversation_id"] = self.conversation
        if self.workflow:
            event["workflow_id"] = self.workflow
        if self.execution:
            event["workflow_run_id"] = self.execution
        if agent:
            event["agent_id"] = agent
        self.events.append(event)
        self.moments.append(self.clock)

    def wait(self, seconds: float) -> None:
        self.clock += timedelta(seconds=seconds)

    def before(self, moment: datetime) -> list[dict[str, Any]]:
        """What had happened by ``moment`` — a run cut here is one still running."""
        return [event for event, at in zip(self.events, self.moments, strict=True) if at <= moment]


class Prompts:
    """What the registry holds, so a call names a version the panel can open."""

    def __init__(self, heads: dict[str, tuple[str, list[str]]]) -> None:
        self.heads = heads

    @classmethod
    def read(cls, api: Api, names: list[str]) -> Prompts:
        heads: dict[str, tuple[str, list[str]]] = {}
        for name in names:
            status, body = api.get(f"/api/v1/prompts/{name}")
            if status != 200:
                continue
            head = body["head"]
            versions = [version["version_id"] for version in head.get("versions", [])]
            production = head.get("labels", {}).get("production") or (versions[-1] if versions else None)
            if production:
                heads[name] = (production, versions)
        return cls(heads)

    def pick(self, rng: random.Random, name: str | None) -> dict[str, str]:
        if name not in self.heads:
            return {}
        production, versions = self.heads[name]
        older = [version for version in versions if version != production]
        # Most calls run what `production` names; a few are still on an older
        # version, which is what comparing two versions on real traffic needs.
        version = rng.choice(older) if older and rng.random() < 0.15 else production
        return {"prompt_name": name, "prompt_version": version}


def llm_call(
    tl: Timeline,
    rng: random.Random,
    run_id: str,
    agent: str,
    model_name: str,
    *,
    prompt: dict[str, str] | None = None,
    error: str | None = None,
) -> bool:
    model = MODELS[model_name]
    call = f"llm-{len(tl.events)}"
    request: dict[str, Any] = {
        "call_id": call,
        "provider": model.provider,
        "model": model_name,
        "temperature": rng.choice((0.0, 0.2, 0.2, 0.7, 1.0)),
        "max_tokens": rng.choice((1024, 2048, 4096)),
    }
    if rng.random() < 0.3:
        request["top_p"] = 0.9
    if rng.random() < 0.15:
        request["seed"] = rng.randrange(1, 10_000)
    request |= prompt or {}
    tl.emit(run_id, "llm.started", request, agent, after=rng.uniform(0.02, 0.3))

    seconds = model.seconds * rng.uniform(0.4, 2.2)
    if error:
        waited = 60.0 if "timed out" in error else rng.uniform(0.2, 1.5)
        failed = {"call_id": call, "provider": model.provider, "model": model_name, "error": error}
        tl.emit(run_id, "llm.failed", failed, agent, after=waited)
        return False

    streamed = rng.random() < 0.25
    if streamed:
        first = seconds * rng.uniform(0.1, 0.3)
        tl.emit(run_id, "llm.first_token", {"call_id": call}, agent, after=first)
        chunks = rng.randint(6, 14)
        for index in range(chunks):
            tl.emit(run_id, "llm.chunk", {"call_id": call, "index": index}, agent, after=(seconds - first) / chunks)
    prompt_tokens = int(model.prompt_tokens * rng.uniform(0.5, 1.8))
    completed: dict[str, Any] = {
        "call_id": call,
        "provider": model.provider,
        "model": model_name,
        "response_model": model_name,
        "prompt_tokens": prompt_tokens,
        "completion_tokens": int(model.completion_tokens * rng.uniform(0.3, 1.6)),
        "finish_reason": "max_tokens" if rng.random() < 0.03 else "stop",
    }
    if model.provider == "anthropic":
        completed["cached_tokens"] = int(prompt_tokens * rng.choice((0.0, 0.0, 0.3, 0.6)))
    tl.emit(run_id, "llm.completed", completed, agent, after=0.0 if streamed else seconds)
    return True


def tool_call(
    tl: Timeline, rng: random.Random, run_id: str, agent: str, tool: str, *, error: str | None = None
) -> None:
    call = f"tool-{len(tl.events)}"
    low, high = TOOLS[tool]
    tl.emit(run_id, "tool.started", {"call_id": call, "tool_name": tool}, agent, after=rng.uniform(0.02, 0.2))
    if error:
        failed = {"call_id": call, "tool_name": tool, "error": error}
        tl.emit(run_id, "tool.failed", failed, agent, after=rng.uniform(low, high))
        return
    tl.emit(run_id, "tool.completed", {"call_id": call, "tool_name": tool}, agent, after=rng.uniform(low, high))


def agent_run(
    tl: Timeline, rng: random.Random, run_id: str, scenario: Scenario, prompts: Prompts, outcome: str
) -> None:
    """One run of one scenario, to its end. A caller that cuts it short has a running one."""
    tl.emit(run_id, "run.started")
    doomed = rng.randrange(len(scenario.agents)) if outcome == "failed" else -1
    retry_pending = outcome == "llm_retry"
    tool_pending = outcome == "tool_error"
    for index, agent in enumerate(scenario.agents):
        tl.emit(run_id, "agent.started", {}, agent.name, after=rng.uniform(0.05, 0.4))
        model = rng.choice(agent.models)
        calls = rng.randint(*agent.calls)
        for n in range(calls):
            if agent.tools and rng.random() < 0.7:
                tool = rng.choice(agent.tools)
                error = TOOL_ERRORS.get(tool, "the tool raised an exception") if tool_pending else None
                tool_pending = False
                tool_call(tl, rng, run_id, agent.name, tool, error=error)
            prompt = prompts.pick(rng, scenario.prompt) if index == 0 and n == 0 else None
            if retry_pending:
                # The SDK's own retry: the same request again, as a new call.
                retry_pending = False
                llm_call(tl, rng, run_id, agent.name, model, prompt=prompt, error=rng.choice(RETRYABLE))
                tl.wait(rng.uniform(1.0, 8.0))
            if index == doomed and n == calls - 1:
                error = rng.choice(FATAL)
                llm_call(tl, rng, run_id, agent.name, model, prompt=prompt, error=error)
                tl.emit(run_id, "agent.failed", {"error": error}, agent.name)
                tl.emit(run_id, "run.failed", {"status": "failed", "error": f"{agent.name}: {error}"})
                return
            llm_call(tl, rng, run_id, agent.name, model, prompt=prompt)
        if index + 1 < len(scenario.agents):
            handoff = {"to": scenario.agents[index + 1].name, "kind": "handoff", "channel": scenario.workflow}
            tl.emit(run_id, "agent.message", handoff, agent.name)
        tl.emit(run_id, "agent.completed", {}, agent.name, after=rng.uniform(0.01, 0.2))
    tl.emit(run_id, "run.completed", {"status": "succeeded"})


# ── The history: runs over the last week ─────────────────────────────────────


def pods(seed: int) -> dict[str, list[str]]:
    """Two or three instances per runtime, so a runtime row is more than one process."""
    rng = random.Random(seed ^ 0x5EED)
    services = sorted({scenario.service for scenario in SCENARIOS} | {"rag-api", "code-review-bot"})
    return {
        service: [f"{service}-{rng.getrandbits(20):05x}-{rng.getrandbits(16):04x}" for _ in range(rng.randint(2, 3))]
        for service in services
    }


def started_at(rng: random.Random, now: datetime) -> datetime:
    """Four runs in five in the last day, busier in working hours; the rest over the week."""
    while True:
        age = rng.uniform(180, 86_400) if rng.random() < 0.8 else rng.uniform(86_400, 7 * 86_400)
        moment = now - timedelta(seconds=age)
        hour = moment.astimezone().hour
        busy = 0.3 + 0.7 * math.sin(math.pi * (hour - 6) / 14) if 6 <= hour <= 20 else 0.3
        if rng.random() < busy:
            return moment


def history(
    rng: random.Random, now: datetime, count: int, prompts: Prompts, instances: dict[str, list[str]]
) -> tuple[list[dict[str, Any]], Counter[str]]:
    weights = [scenario.weight for scenario in SCENARIOS]
    sessions = [f"sess-{rng.getrandbits(24):06x}" for _ in range(70)]
    events: list[dict[str, Any]] = []
    outcomes: Counter[str] = Counter()
    for index in range(count):
        scenario = rng.choices(SCENARIOS, weights)[0]
        stalled = index < 5
        if stalled:
            # Began twenty minutes to two hours ago and went quiet in the middle:
            # past the fifteen minutes after which the panel calls a run stalled.
            start = now - timedelta(minutes=rng.uniform(20, 120))
        elif index < 12:
            start = now - timedelta(seconds=rng.uniform(2, 40))
        else:
            start = started_at(rng, now)
        tl = Timeline(
            start,
            scenario.service,
            scenario.sdk,
            rng.choice(instances[scenario.service]),
            conversation=rng.choice(sessions),
            workflow=scenario.workflow,
        )
        run_id = f"run-{scenario.workflow}-{index:04d}"
        agent_run(tl, rng, run_id, scenario, prompts, fate(rng))
        kept = tl.before(now)
        if stalled:
            kept = kept[: rng.randint(3, max(3, len(kept) - 2))]
        events.extend(kept)
        last = kept[-1]["event_type"] if kept else ""
        outcomes[{"run.completed": "succeeded", "run.failed": "failed"}.get(last, "running")] += 1
    return events, outcomes


# ── Declared workflows and their executions ──────────────────────────────────


@dataclass(frozen=True)
class Node:
    id: str
    name: str
    kind: str
    seconds: tuple[float, float]
    agent: str | None = None
    #: Reached over a conditional edge, so not every execution takes it — and
    #: the one that does not leaves it `pending`.
    optional: bool = False


@dataclass(frozen=True)
class Topology:
    workflow: str
    title: str
    service: str
    sdk: str
    #: In an order every edge respects.
    nodes: tuple[Node, ...]
    edges: tuple[tuple[str, str, str | None], ...]
    errors: dict[str, str]


TOPOLOGIES = (
    Topology(
        "kb-ingest", "Knowledge base ingest", "rag-api", "python",
        (
            Node("fetch", "Fetch sources", "chain", (2, 20)),
            Node("chunk", "Chunk documents", "chain", (1, 8)),
            Node("embed", "Embed chunks", "chain", (5, 40)),
            Node("entities", "Extract entities", "agent", (10, 60), "entity-agent", optional=True),
            Node("index", "Write the index", "chain", (1, 6)),
        ),
        (
            ("fetch", "chunk", None),
            ("chunk", "embed", None),
            ("chunk", "entities", "if enabled"),
            ("embed", "index", None),
            ("entities", "index", None),
        ),
        {
            "fetch": "403 from confluence.internal",
            "chunk": "document 17 is not UTF-8",
            "embed": "embedding provider timed out",
            "index": "index write conflict on shard 3",
        },
    ),
    Topology(
        "pr-review", "Pull request review", "code-review-bot", "typescript",
        (
            Node("checkout", "Check out", "chain", (1, 5)),
            Node("lint", "Lint", "chain", (3, 15)),
            Node("test", "Run the tests", "chain", (20, 120)),
            Node("review", "Review the diff", "agent", (15, 90), "reviewer-agent"),
            Node("comment", "Post the review", "chain", (0.5, 2)),
        ),
        (
            ("checkout", "lint", None),
            ("checkout", "test", None),
            ("lint", "review", None),
            ("test", "review", None),
            ("review", "comment", None),
        ),
        {
            "checkout": "ref refs/pull/412/head not found",
            "lint": "eslint exited 2",
            "test": "4 tests failed in packages/api",
            "review": "the model answered with no findings block",
            "comment": "GitHub answered 502",
        },
    ),
)


def declaration(topology: Topology) -> dict[str, Any]:
    return {
        "name": topology.title,
        "version": "sha256:" + hashlib.sha256(topology.workflow.encode()).hexdigest()[:12],
        "nodes": [
            {"id": node.id, "name": node.name, "kind": node.kind, **({"agent": node.agent} if node.agent else {})}
            for node in topology.nodes
        ],
        "edges": [
            {"from": source, "to": target, **({"label": label} if label else {})}
            for source, target, label in topology.edges
        ],
    }


def execution(tl: Timeline, rng: random.Random, topology: Topology, execution_id: str, outcome: str) -> None:
    driver = f"{execution_id}-driver"
    tl.emit(driver, "run.started")
    tl.emit(driver, "workflow.declared", declaration(topology))
    required = [node.id for node in topology.nodes if not node.optional]
    failing = rng.choice(required) if outcome in ("failed", "retried") else None
    for node in topology.nodes:
        if node.optional and rng.random() < 0.5:
            continue
        run_id = f"{execution_id}-{node.id}"
        tl.emit(run_id, "run.started", after=rng.uniform(0.2, 2.0))
        attempts = 2 if node.id == failing and outcome == "retried" else 1
        for attempt in range(1, attempts + 1):
            # Two `step.started` with different call ids are two attempts, not a redelivery.
            call = f"{node.id}-{attempt}"
            step = {"node": node.id, "call_id": call, "step_type": node.kind}
            tl.emit(run_id, "step.started", step, node.agent)
            if node.agent:
                llm_call(tl, rng, run_id, node.agent, "claude-opus-5")
            duration = rng.uniform(*node.seconds)
            if node.id == failing and (outcome == "failed" or attempt < attempts):
                failed = {"node": node.id, "call_id": call, "error": topology.errors[node.id]}
                tl.emit(run_id, "step.failed", failed, node.agent, after=duration)
                if outcome == "failed":
                    tl.emit(run_id, "run.failed", {"status": "failed", "error": "the stage failed"})
                    tl.emit(driver, "run.failed", {"status": "failed", "error": f"{node.id} failed"})
                    return
                tl.wait(rng.uniform(5, 30))
                continue
            artifact = {
                "node": node.id,
                "name": f"{node.id}.json",
                "uri": f"s3://{topology.workflow}/{execution_id}/{node.id}.json",
                "media_type": "application/json",
                "size_bytes": rng.randint(2_000, 4_000_000),
                "digest": "sha256:" + hashlib.sha256(f"{execution_id}/{node.id}".encode()).hexdigest(),
            }
            tl.emit(run_id, "artifact.produced", artifact, node.agent, after=duration)
            tl.emit(run_id, "step.completed", step, node.agent)
        tl.emit(run_id, "run.completed", {"status": "succeeded"})
    tl.emit(driver, "run.completed", {"status": "succeeded"})


def workflows(rng: random.Random, now: datetime, instances: dict[str, list[str]]) -> tuple[list[dict[str, Any]], int]:
    outcomes = ("ok", "ok", "retried", "ok", "failed", "ok", "ok", "running")
    events: list[dict[str, Any]] = []
    executions = 0
    for topology in TOPOLOGIES:
        for index, outcome in enumerate(outcomes):
            if outcome == "running":
                start = now - timedelta(seconds=rng.uniform(20, 60))
            else:
                start = now - timedelta(hours=(len(outcomes) - index) * rng.uniform(6, 9))
            execution_id = f"exec-{topology.workflow}-{index + 1:02d}"
            tl = Timeline(
                start,
                topology.service,
                topology.sdk,
                rng.choice(instances[topology.service]),
                workflow=topology.workflow,
                execution=execution_id,
            )
            execution(tl, rng, topology, execution_id, outcome)
            events.extend(tl.before(now))
            executions += 1
    return events, executions


# ── Evaluation reports ───────────────────────────────────────────────────────


@dataclass(frozen=True)
class Suite:
    name: str
    dataset: str
    variants: tuple[str, ...]
    base: float
    #: What each variant gains on the one before it, on average.
    step: float


SUITES = (
    Suite(
        "support-quality",
        "support/tickets@v4",
        ("prompt-v1", "prompt-v2", "prompt-v2-kb", "prompt-v3", "prompt-v3-haiku", "prompt-v4"),
        0.60,
        0.035,
    ),
    Suite("rag-faithfulness", "kb/questions@2", ("bm25", "hybrid", "hybrid-rerank", "hybrid-rerank-v2"), 0.56, 0.06),
    Suite("code-review-precision", "prs/sample@1", ("opus-baseline", "opus-diff-context", "opus-tests-first"), 0.64, 0.04),
)

MISSES = (
    "cites a passage that does not say it",
    "answered a different question",
    "left out the refund policy",
    "flagged a style nit as a bug",
    "missed the null dereference",
    "hedged where the source is explicit",
)


def evaluations(rng: random.Random, now: datetime) -> tuple[list[dict[str, Any]], int]:
    events: list[dict[str, Any]] = []
    reports = 0
    for suite in SUITES:
        cases = [(f"case-{number:03d}", rng.uniform(-0.15, 0.2)) for number in range(1, 17)]
        cases[2] = (cases[2][0], -0.25)  # an easy case, so the one regression below is visible
        for index, variant in enumerate(suite.variants):
            evaluation_id = f"eval-{suite.name}-{index + 1}"
            start = now - timedelta(days=len(suite.variants) - index, hours=rng.uniform(0, 6))
            tl = Timeline(start, "eval-runner", "python", "eval-runner-0")
            params = {"judge": "claude-opus-5", "temperature": "0", "cases": str(len(cases))}
            started = {"suite": suite.name, "dataset": suite.dataset, "variant": variant, "params": params}
            tl.emit(evaluation_id, "eval.started", started)
            # The second-to-last report of each suite is better on the mean and
            # worse on one case that passed every time before — what a mean hides.
            regression = index == len(suite.variants) - 2
            # And one report never finished: the judge stopped answering.
            broken = suite.name == "rag-faithfulness" and index == 2
            scores: list[float] = []
            for position, (case_id, difficulty) in enumerate(cases):
                if broken and position == 5:
                    tl.emit(evaluation_id, "eval.failed", {"error": "the judge returned malformed JSON five times"}, after=3)
                    break
                score = suite.base + suite.step * index - difficulty + rng.gauss(0, 0.04)
                if regression and case_id == "case-003":
                    score = 0.31
                score = round(min(1.0, max(0.0, score)), 3)
                passed = score >= 0.7
                case = {
                    "case_id": case_id,
                    "score": score,
                    "passed": passed,
                    "reason": "meets the rubric" if passed else rng.choice(MISSES),
                }
                tl.emit(evaluation_id, "eval.case", case, after=rng.uniform(2, 12))
                scores.append(score)
            else:
                metrics = {
                    "mean_score": round(sum(scores) / len(scores), 4),
                    "pass_rate": round(sum(score >= 0.7 for score in scores) / len(scores), 4),
                    "cost_usd": round(len(scores) * rng.uniform(0.01, 0.04), 3),
                    "p95_latency_ms": rng.randint(1800, 9000),
                }
                report = {"judge": "claude-opus-5", "cases": len(scores)}
                tl.emit(evaluation_id, "eval.completed", {"metrics": metrics, "report": report}, after=1)
            events.extend(tl.events)
            reports += 1
    return events, reports


# ── Live traffic ─────────────────────────────────────────────────────────────


def replay(api: Api, tl: Timeline, stop: threading.Event) -> None:
    """Publish a run as it happens: the same gaps, compressed so none takes over half a minute."""
    if not tl.events:
        return
    span = (tl.moments[-1] - tl.moments[0]).total_seconds()
    speed = max(1.0, span / 30)
    previous = tl.moments[0]
    for event, moment in zip(tl.events, tl.moments, strict=True):
        gap = (moment - previous).total_seconds() / speed
        previous = moment
        if gap > 0 and stop.wait(min(gap, 5.0)):
            return
        try:
            api.publish([{**event, "occurred_at": iso(datetime.now(UTC))}])
        except (RuntimeError, OSError):
            return  # the server went away, and so does this


def live(api: Api, rng: random.Random, prompts: Prompts, instances: dict[str, list[str]], stop: threading.Event) -> None:
    say("live: a new run every few seconds until the server stops")
    weights = [scenario.weight for scenario in SCENARIOS]
    stamp = int(time.time())
    index = 0
    while not stop.is_set():
        index += 1
        scenario = rng.choices(SCENARIOS, weights)[0]
        tl = Timeline(
            datetime.now(UTC),
            scenario.service,
            scenario.sdk,
            rng.choice(instances[scenario.service]),
            conversation=f"sess-live-{rng.randrange(8)}",
            workflow=scenario.workflow,
        )
        agent_run(tl, rng, f"live-{stamp}-{index:05d}", scenario, prompts, fate(rng))
        threading.Thread(target=replay, args=(api, tl, stop), daemon=True).start()
        stop.wait(rng.uniform(3, 9))


# ── The authored registries, once ────────────────────────────────────────────


PROMPTS: dict[str, dict[str, Any]] = {
    "support.triage": {
        "description": "Routes a customer message to one of five queues.",
        "tags": ["support", "classification"],
        "model": "claude-haiku-4-5",
        "versions": [
            (
                "You triage customer messages for an online shop.\n"
                "Read {{ message }} and answer with one of: billing, shipping, returns, account, other.\n"
                "Answer with the category only.",
                "first version",
                None,
            ),
            (
                "You triage customer messages for an online shop.\n"
                "Read {{ message }} from a {{ tier }} customer and answer with one category: billing,\n"
                "shipping, returns, account or other. A message that mentions a refund is returns.\n"
                "Answer with the category only.",
                "refunds were landing in billing",
                "production",
            ),
            (
                "You triage customer messages for an online shop.\n"
                "Read {{ message }} from a {{ tier }} customer. Answer with one category — billing,\n"
                "shipping, returns, account or other — then a confidence from 0 to 1.\n"
                "A message that mentions a refund is returns.",
                "adds a confidence for the escalation rule",
                "staging",
            ),
        ],
    },
    "research.summarize": {
        "description": "Summarises a set of sources for a named audience, with citations.",
        "tags": ["research", "writing"],
        "model": "claude-opus-5",
        "versions": [
            (
                "Summarise the sources below for {{ audience }}.\n{{ sources }}\n"
                "Keep it under 300 words and cite every claim.",
                "first version",
                None,
            ),
            (
                "Summarise the sources below for {{ audience }}.\n{{ sources }}\n"
                "Lead with the answer, then the evidence. Keep it under 300 words, cite every claim,\n"
                "and mark any claim only one source makes.",
                "readers wanted the answer first",
                "production",
            ),
        ],
    },
    "rag.answer": {
        "description": "Answers a question from retrieved passages only.",
        "tags": ["rag"],
        "model": "claude-sonnet-5",
        "versions": [
            (
                "Answer {{ question }} using only the passages below.\n{{ passages }}\n"
                "If the passages do not contain the answer, say so.",
                "first version",
                None,
            ),
            (
                "Answer {{ question }} using only the passages below.\n{{ passages }}\n"
                "Quote the passage id after every sentence. If the passages do not contain the\n"
                "answer, say so and name what is missing.",
                "faithfulness suite: +0.06 held out",
                "production",
            ),
        ],
    },
    "code.review": {
        "description": "Reviews a diff for correctness first and style last.",
        "tags": ["code"],
        "model": "claude-opus-5",
        "versions": [
            (
                "Review the diff below for correctness bugs first and style last.\n{{ diff }}\n"
                "Return findings as a list, most severe first, each with a file and a line.",
                "first version",
                "production",
            ),
        ],
    },
}


def script(api: Api, *argv: str, env: dict[str, str] | None = None) -> None:
    """Run one of the existing seed scripts, into the log rather than the terminal."""
    DATA.mkdir(parents=True, exist_ok=True)
    with LOG.open("a") as log:
        log.write(f"\n── {' '.join(argv)}  {iso(datetime.now(UTC))}\n")
        log.flush()
        result = subprocess.run(  # noqa: S603 — this repository's own scripts
            list(argv),
            cwd=ROOT,
            stdout=log,
            stderr=subprocess.STDOUT,
            env={**os.environ, "AIWATCHER_URL": api.base, **(env or {})},
            timeout=900,
            check=False,
        )
    if result.returncode:
        raise RuntimeError(f"{Path(argv[0]).name} exited {result.returncode}")


def seed_prompts(api: Api) -> str:
    script(api, "./scripts/seed-demo-prompts.sh")
    for name, prompt in PROMPTS.items():
        for text, notes, label in prompt["versions"]:
            body: dict[str, Any] = {
                "name": name,
                "text": text,
                "author": "dev-seed",
                "model": prompt["model"],
                "notes": notes,
                "description": prompt["description"],
                "tags": prompt["tags"],
            }
            if label:
                body["label"] = label
            status, answer = api.post("/api/v1/prompts", body)
            if status >= 300:
                raise RuntimeError(f"publishing {name} answered {status}: {answer}")
    return f"{len(PROMPTS) + 1} prompts, their versions and labels, three optimisations"


def fit(run: Any, rng: random.Random, epochs: int, final: float) -> None:
    loss = 1.4
    for index in range(epochs):
        with run.epoch(index) as epoch:
            for _ in range(20):
                loss *= rng.uniform(0.95, 0.99)
                epoch.step(loss=loss)
            f1 = final - (final - 0.35) * math.exp(-index / max(1.0, epochs / 3))
            epoch.metrics(val_f1=round(f1 + rng.gauss(0, 0.004), 4), val_loss=round(loss * 1.1, 4))
        run.sample(lr=3e-4 * 0.9**index)


def seed_training(api: Api) -> str:
    from aiwatcher_sdk.training import TrainingClient, TrainingError, TrainingRun

    rng = random.Random(11)
    dataset = "support/intents@" + hashlib.sha256(b"support/intents 2026-09").hexdigest()
    common = {"model": "intent-clf", "framework": "pytorch", "device": "cuda:0"}
    # The SDK says on stderr when a dataset is a bare name — which one of these
    # is on purpose. That belongs in the log, not between vite's lines.
    with LOG.open("a") as log, contextlib.redirect_stderr(log), TrainingClient(api.base) as client:
        versions = []
        for number, (lr, epochs, final) in enumerate(((1e-3, 8, 0.81), (3e-4, 12, 0.86), (2e-4, 14, 0.89)), 1):
            run_id = f"intent-clf-{number}"
            params = {"lr": lr, "batch_size": 32, "epochs": epochs, "backbone": "deberta-v3-small"}
            with client.run(run_id, dataset=dataset, code=f"git:4f1c{number}a2", params=params, **common) as run:
                fit(run, rng, epochs, final)
                run.checkpoint(
                    f"s3://models/intent-clf/{run_id}/best.pt",
                    epoch=epochs - 1,
                    metric="val_f1",
                    value=final,
                    best=True,
                )
            registered = client.register_model(
                "intent-clf",
                run_id=run_id,
                checkpoint_uri=f"s3://models/intent-clf/{run_id}/best.pt",
                validation={"f1": final},
                test={"f1": round(final - 0.03, 3)},
                description="Routes a support message to one of five queues",
            )
            versions.append(registered["version"]["version"])
        client.promote("intent-clf", versions[-1])

        # One that ran out of memory at its fourth epoch.
        with contextlib.suppress(RuntimeError), client.run(
            "intent-clf-4", dataset=dataset, code="git:77d02e1", params={"lr": 2e-4, "batch_size": 128}, **common
        ) as run:
            fit(run, rng, 3, 0.8)
            raise RuntimeError("CUDA out of memory: tried to allocate 2.50 GiB")

        # One still going: opened and never closed, so the Training area polls it.
        client.request(
            "POST",
            "/api/v1/training-runs",
            {
                "run_id": "intent-clf-5",
                "dataset": dataset,
                "code": "git:9b7e0c1",
                "params": {"lr": 1.5e-4, "batch_size": 32, "epochs": 20},
                **common,
            },
        )
        running = TrainingRun(client, "intent-clf-5")
        fit(running, rng, 5, 0.9)
        running.flush()

        # And the two versions a promotion refuses, for its two reasons.
        with client.run("intent-clf-scratch", dataset="support/intents", code="git:dirty", **common) as run:
            fit(run, rng, 6, 0.9)
        refused = [
            client.register_model(
                "intent-clf",
                run_id="intent-clf-scratch",
                checkpoint_uri="s3://models/intent-clf/scratch/best.pt",
                validation={"f1": 0.9},
                test={"f1": 0.88},
                notes="trained on the mutable name",
            ),
            client.register_model(
                "intent-clf",
                run_id="intent-clf-2",
                checkpoint_uri="s3://models/intent-clf/intent-clf-2/last.pt",
                validation={"f1": 0.87},
                notes="the last epoch rather than the best; never held out",
            ),
        ]
        for version in refused:
            with contextlib.suppress(TrainingError):
                client.promote("intent-clf", version["version"]["version"], label="staging")
    return "six intent-clf runs (one failed, one still running), five versions, one promoted"


def read_record() -> dict[str, str]:
    try:
        return dict(json.loads(RECORD.read_text()))
    except (OSError, ValueError):
        return {}


def registries(api: Api, force: bool) -> None:
    done = {} if force else read_record()
    steps: list[tuple[str, Any]] = [
        ("prompts", lambda: seed_prompts(api)),
        (
            "annotations",
            lambda: script(api, "./scripts/seed-demo-annotations.py", "--base-url", api.base)
            or "24 plans in 12 families, an export, a training run and a promoted segmenter",
        ),
        ("training", lambda: seed_training(api)),
        (
            "imports",
            lambda: script(api, "python3", "./scripts/seed-staged-import.py")
            or "a staged batch imported by the queued job, refusals included",
        ),
        (
            "conversations",
            lambda: script(api, "./scripts/seed-demo-conversations.py", "--api", api.base)
            or "one reviewed exchange and an exported corpus",
        ),
    ]
    # Not curation: its recipes run through the query service, and one found
    # listening on :8081 may be serving another instance — the first run of this
    # filled two dataset versions with a benchmark server's rows. Which instance
    # a query service reads is its configuration, not something to guess here.
    already = [name for name, _ in steps if name in done]
    if already:
        say(f"– already seeded: {', '.join(already)} (--registries seeds them again)")
    for name, step in steps:
        if name in done:
            continue
        say(f"… {name}")
        try:
            detail = step()
        except Exception as error:  # noqa: BLE001 — one step failing must not stop the rest
            say(f"✗ {name}: {error} — {LOG.relative_to(ROOT)} has the output; tried again next start")
            continue
        done[name] = iso(datetime.now(UTC))
        DATA.mkdir(parents=True, exist_ok=True)
        RECORD.write_text(json.dumps(done, indent=2) + "\n")
        say(f"✓ {name}: {detail}")
    say("– curation recipes and datasets: `just seed-curation`, with a query service that reads this server")


# ── Managed runs, through the worker `just dev` starts ───────────────────────


def managed(api: Api) -> None:
    name = "demo.worker-import"
    for _ in range(60):
        status, _ = api.get(f"/api/v1/workflow-definitions/{name}")
        if status == 200:
            break
        time.sleep(1)
    else:
        say(f"– managed runs skipped: no worker registered {name} within a minute")
        return
    started = []
    for number in (3, 5, 8):
        body = {"target": {"kind": "workflow", "name": name}, "parameters": {"number": number}}
        status, answer = api.post("/api/v1/executions", body, headers={"idempotency-key": uuid.uuid4().hex})
        if status >= 300:
            say(f"✗ managed run of {name}: {status} {answer}")
            return
        started.append(answer["execution"]["execution_id"])
    say(f"✓ {len(started)} managed runs of {name}, each retrying its second step once")


# ── Main ─────────────────────────────────────────────────────────────────────


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--api", default=os.environ.get("AIWATCHER_URL", "http://127.0.0.1:8080"))
    parser.add_argument("--runs", type=int, default=500, help="agent runs in the history (default 500)")
    parser.add_argument("--seed", type=int, default=7, help="random seed (default 7)")
    parser.add_argument("--live", action="store_true", help="keep a run arriving every few seconds")
    parser.add_argument("--registries", action="store_true", help="seed the registries again")
    parser.add_argument("--no-registries", action="store_true", help="leave the registries alone")
    return parser.parse_args()


def main() -> int:
    args = arguments()
    api = Api(args.api)
    for _ in range(120):
        if api.alive():
            break
        time.sleep(1)
    else:
        say(f"✗ nothing answers on {api.base}")
        return 1

    stop = threading.Event()
    signal.signal(signal.SIGTERM, lambda *_: stop.set())
    rng = random.Random(args.seed)
    instances = pods(args.seed)

    if not args.no_registries:
        registries(api, args.registries)

    prompts = Prompts.read(api, [scenario.prompt for scenario in SCENARIOS if scenario.prompt])
    now = datetime.now(UTC)
    events, outcomes = history(rng, now, args.runs, prompts, instances)
    api.publish(events)
    say(
        f"✓ {sum(outcomes.values())} runs over the last week — {outcomes['succeeded']} succeeded, "
        f"{outcomes['failed']} failed, {outcomes['running']} running or stalled ({len(events)} events)"
    )
    if prompts.heads:
        say(f"  their calls name versions of {', '.join(sorted(prompts.heads))}")

    events, executions = workflows(rng, now, instances)
    api.publish(events)
    say(f"✓ {executions} executions of {len(TOPOLOGIES)} declared workflows, with retries, a failure each and one running")

    events, reports = evaluations(rng, now)
    api.publish(events)
    say(f"✓ {reports} evaluation reports across {len(SUITES)} suites, one regression each and one that never finished")

    for label, argv in (
        ("the streamed demo run", ("./scripts/seed-demo-run.sh",)),
        ("the two-report evaluation comparison", ("./scripts/seed-demo-evaluation.sh",)),
        ("two house-import executions", ("./scripts/seed-demo-workflow.sh",)),
    ):
        try:
            script(api, *argv)
            say(f"✓ {label}")
        except RuntimeError as error:
            say(f"✗ {label}: {error}")

    managed(api)
    say(f"done — {PANEL}")

    if args.live:
        try:
            live(api, rng, prompts, instances, stop)
        except KeyboardInterrupt:
            stop.set()
    return 0


if __name__ == "__main__":
    sys.exit(main())
