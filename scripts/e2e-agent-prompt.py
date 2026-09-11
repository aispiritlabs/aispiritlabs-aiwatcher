"""A turn's LLM span names the registry version of the prompt it ran on.

AW-2 left this open. An agent named its prompt on its model call only as the
attribute `agentic.prompt_name`, which aiwatcher's tracer dropped, and passed no
version at all — so a trace could name the model and never the prompt, and
ADR_0011's promise that the version a run used stays readable could not be
followed from a trace. This runs real turns of a real `Agent`, whose builder
reads `sage` through the application's own `registry`, and reads back what
aiwatcher made of them:

1. `make registry`'s publish names each authored text by its `sha256`, which is
   what a builder derives from the text it read;
2. with `production` on a candidate, a turn's LLM span names `sage` and the
   candidate's version;
3. that reference resolves: the registry answers it with the candidate's text;
4. with no `AIWATCHER_URL` for the builder, the span names the authored version
   `make registry` published, and that resolves too;
5. a builder handed its text directly names no version, and so no prompt — the
   name alone would point at a text the model was not given;
6. the text is on neither the log nor the run: the reference, never the words.

It starts **its own** aiwatcher, from `target/debug/aiwatcher` or
`AIWATCHER_BINARY`, on a free port with a file prompt store under a temporary
directory, so the one on :8080 is not touched. It runs under `ai_spirit_agent`'s
environment, because the prompt source is its `registry`.

    cargo build --bin aiwatcher   # once
    just e2e-agent-prompt
"""

from __future__ import annotations

import hashlib
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
from collections.abc import Generator
from contextlib import contextmanager
from pathlib import Path
from typing import Any

REPOSITORY = Path(__file__).resolve().parent.parent
BINARY = Path(os.environ.get("AIWATCHER_BINARY", REPOSITORY / "target" / "debug" / "aiwatcher"))
MODEL = "pong-1"
#: A candidate's text, and a fragment of it no event or span may carry.
CANDIDATE = "You are the sage. Answer in one sentence, and say why.\n{tools}"
FRAGMENT = "Answer in one sentence"
READY_WITHIN = 60.0
SETTLE_WITHIN = 20.0
BASE = ""


# ── The agent. ───────────────────────────────────────────────────────────────


class Pong:
    def response(self, prompt: Any, **kwargs: Any) -> Any:
        from aiwatcher_agentic.model import ModelResponse

        return ModelResponse(text="Patience.", model=MODEL, prompt_tokens=3, completion_tokens=1)

    def close(self) -> None:
        return None


class Lends:
    """A model source whose one model answers without a model stack."""

    _model_name = MODEL

    @contextmanager
    def session(self, name: str = "model") -> Generator[Pong, None, None]:
        yield Pong()


def turn(builder: Any, session: str) -> str:
    """One turn of an agent on `builder`, under a run of its own; its run id."""
    from aiwatcher_agentic.agent import Agent
    from aiwatcher_sdk import AiwatcherClient
    from aiwatcher_sdk.integrations.agentic import AiwatcherTracer

    client = AiwatcherClient(service="e2e-agent-prompt", base_url=BASE)
    tracer = AiwatcherTracer(client=client)
    try:
        with tracer.workflow(name="e2e-agent-prompt", session_id=session):
            run_id = tracer.current_trace_id
            Agent(Lends(), prompt_builder=builder, tracer=tracer).run("what is patience?")
    finally:
        client.close()
    if run_id is None:
        raise SystemExit("the tracer opened no run")
    return run_id


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
    """Start an aiwatcher of our own, with a prompt store, and wait for it."""
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
        "AIWATCHER_PROMPT_STORE": "file",
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


def llm_span(run_id: str) -> dict[str, Any]:
    """The run's model call once its span has closed, attributes as a mapping."""
    deadline = time.monotonic() + SETTLE_WITHIN
    body: Any = None
    while time.monotonic() < deadline:
        status, body = call("GET", f"/api/v1/runs/{run_id}")
        if status == 200:
            for span in body.get("spans", []):
                attributes = dict(span.get("attributes", []))
                if attributes.get("gen_ai.request.model") == MODEL:
                    return attributes
        time.sleep(0.2)
    raise SystemExit(f"run {run_id} never closed a model call: {body}")


def resolves(name: str, version_id: str) -> str | None:
    """The text the registry holds under that reference, or `None`."""
    status, body = call("GET", f"/api/v1/prompts/{name}/versions/{version_id}")
    return body.get("text") if status == 200 and isinstance(body, dict) else None


def sha256(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


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


NAME = "aiwatcher.prompt.name"
VERSION = "aiwatcher.prompt.version_id"


def orchestrate() -> bool:
    os.environ["AIWATCHER_URL"] = BASE
    os.environ.pop("AIWATCHER_TOKEN", None)

    from agentic.prompts import QwenPromptBuilder
    from aiwatcher_sdk.prompts import PromptRegistry
    from registry import CATALOGUE, Prompts
    from registry.main import init_registry_prompt

    checks = Checks()
    with PromptRegistry(BASE) as registry:
        authored = {version.name: version for version in init_registry_prompt(registry)}
        checks.check(
            1,
            "make registry's publish names each authored text by its sha256",
            set(authored) == set(CATALOGUE)
            and all(authored[name].version_id == sha256(CATALOGUE[name]) for name in CATALOGUE),
            f"{len(authored)} published",
        )
        candidate = registry.publish(Prompts.SAGE, CANDIDATE, author="e2e-optimiser")
        registry.set_label(Prompts.SAGE, "production", candidate.version_id)

    promoted = llm_span(turn(QwenPromptBuilder(external_prompt_name=Prompts.SAGE), "promoted"))
    checks.check(
        2,
        "with production on a candidate, the span names sage and the candidate's version",
        promoted.get(NAME) == "sage" and promoted.get(VERSION) == candidate.version_id,
        f"{promoted.get(NAME)} @ {str(promoted.get(VERSION))[:12]}",
    )
    checks.check(
        3,
        "that reference resolves, to the candidate's text",
        resolves("sage", str(promoted.get(VERSION))) == CANDIDATE,
    )

    # The builder reads the authored text; the tracer still publishes, to the
    # address its client was handed.
    del os.environ["AIWATCHER_URL"]
    offline_run = turn(QwenPromptBuilder(external_prompt_name=Prompts.SAGE), "authored")
    offline = llm_span(offline_run)
    checks.check(
        4,
        "with no registry for the builder, the span names the authored version, which resolves",
        offline.get(VERSION) == authored["sage"].version_id
        and resolves("sage", authored["sage"].version_id) == CATALOGUE["sage"],
        str(offline.get(VERSION))[:12],
    )

    improvised = llm_span(
        turn(
            QwenPromptBuilder(system_prompt="Improvised.", external_prompt_name=Prompts.SAGE),
            "improvised",
        )
    )
    checks.check(
        5,
        "a builder handed its text directly names no version, and so no prompt",
        VERSION not in improvised and NAME not in improvised,
        sorted(key for key in improvised if key.startswith("aiwatcher.prompt")),
    )

    run_id = turn(QwenPromptBuilder(external_prompt_name=Prompts.SAGE), "words")
    llm_span(run_id)
    _, events = call("GET", f"/api/v1/runs/{run_id}/events")
    _, detail = call("GET", f"/api/v1/runs/{run_id}")
    checks.check(
        6,
        "the text is on neither the log nor the run: the reference, never the words",
        events is not None
        and detail is not None
        and FRAGMENT not in json.dumps(events)
        and FRAGMENT not in json.dumps(detail),
    )
    print(f"{checks.passed}/{checks.total}")
    return checks.passed == checks.total


def main() -> None:
    try:
        import agentic  # noqa: F401
        import registry  # noqa: F401
    except ImportError as error:
        raise SystemExit(
            "agentic and registry are not importable: run this under ai_spirit_agent's "
            "environment (`just e2e-agent-prompt` does)"
        ) from error
    root = Path(tempfile.mkdtemp(prefix="aiwatcher-e2e-prompt-"))
    server = serve(root)
    print(f"aiwatcher of our own at {BASE}; everything under {root}")
    passed = False
    try:
        passed = orchestrate()
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
