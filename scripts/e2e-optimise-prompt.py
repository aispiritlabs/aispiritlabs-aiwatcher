#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.13"
# dependencies = ["aiwatcher-sdk"]
#
# [tool.uv.sources]
# aiwatcher-sdk = { path = "../sdk/python", editable = true }
# ///
"""One managed run optimises a prompt, evaluates it, and asks an admin (AW-5).

Phase 15's exit: an optimise → evaluate → promote definition runs end to end
with the verdict computed on the server. Every piece existed before; what this
proves is that they know about each other. The workflow is

    evaluate_baseline ─┐
    optimise ──────────┼─► evaluate_candidate ─► record ─► promote
                       └──────────────────────────┘

with a deterministic stand-in for a model and an optimiser, so it costs nothing
and says the same thing every time. It runs three times, each against a fresh
server:

- **approved** — the candidate improves the held-out score; an admin says
  "promote";
- **kept** — the same candidate; the admin says "keep";
- **rejected** — the candidate does not improve; nobody is asked.

What it checks, per run:

1. the run completes;
2. both evaluation reports are listed under this run, one per evaluating step;
3. the prompt holds one optimisation from the run, naming both reports;
4. the verdict is the server's: admitted, or rejected for no held-out gain;
5. a question was put to an `admin` exactly when the verdict admitted;
6. `production` names the candidate only after "promote";
7. the candidate's text is in no read of the run — its view and its history —
   and is in the registry, as the version the record stored.

It starts **its own** aiwatcher, from `target/debug/aiwatcher` or
`AIWATCHER_BINARY`, on a free port with every byte under a temporary directory,
so the one on :8080 is not touched.

    cargo build --bin aiwatcher   # once
    just e2e-optimise
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
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "sdk" / "python"))

from aiwatcher_sdk import AiwatcherClient  # noqa: E402
from aiwatcher_sdk.prompts import PromptRegistry, scores, version_id_of  # noqa: E402
from aiwatcher_sdk.runtime import ExecutionPool, Runtime  # noqa: E402
from aiwatcher_sdk.task import task  # noqa: E402
from aiwatcher_sdk.worker import TaskContext, get_task_context  # noqa: E402
from aiwatcher_sdk.workflow import Workflow, WorkflowInput, WorkflowStep  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
BINARY = Path(os.environ.get("AIWATCHER_BINARY", ROOT / "target" / "debug" / "aiwatcher"))
READY_WITHIN = 30.0
RUN_WITHIN = 90.0
BASE = ""

PROMPT = "e2e.answer"
BASELINE = "Answer the question: {{ question }}"
SUITE = "e2e-answer"
DATASET = "capitals@1/held-out"
QUEUE = "e2e-optimise"
TERMINAL = {"completed", "failed", "cancelled", "crashed"}

# Held out from whatever an optimiser searched on. The stand-in never searches,
# but the report and the record still say which cases they were measured on.
HELD_OUT = (
    ("the capital of France", "Paris"),
    ("the capital of Japan", "Tokyo"),
    ("the capital of Peru", "Lima"),
    ("the capital of Kenya", "Nairobi"),
)


# ── The work: a model, an optimiser and five tasks. ──────────────────────────


def model(prompt: str, question: str, expected: str) -> str:
    """A model that answers in one word only when it is told to."""
    if "one word" in prompt:
        return expected
    return f"Well, {question} is {expected}, as it happens."


def optimiser(baseline: str, kind: str) -> str:
    """Append an instruction. `improve` is the one that changes the answers."""
    return baseline + (" Reply in one word." if kind == "improve" else " Please be polite.")


def held_out_score(text: str) -> tuple[float, int]:
    passed = 0
    for question, expected in HELD_OUT:
        rendered = text.replace("{{ question }}", question)
        passed += int(model(rendered, question, expected) == expected)
    return passed / len(HELD_OUT), passed


def registry() -> PromptRegistry:
    return PromptRegistry(BASE)


@task("e2e.evaluate", version="1")
def evaluate(prompt: str, optimise_with: str, which: str) -> dict[str, Any]:
    del optimise_with
    ctx: TaskContext = get_task_context()
    if which == "candidate":
        text = str(ctx.read_artifact("candidate")[0]["text"])
    else:
        with registry() as prompts:
            text = prompts.resolve(prompt).text
    score, passed = held_out_score(text)
    evaluation_id = ctx.record_evaluation(
        suite=SUITE,
        dataset=DATASET,
        variant=version_id_of(text),
        metrics={"exact_match": score},
        cases_total=len(HELD_OUT),
        cases_passed=passed,
    )
    row = {"evaluation_id": evaluation_id, "version_id": version_id_of(text), "exact_match": score}
    ctx.write_artifact(f"{which}_eval", [row])
    return {"exact_match": score}


@task("e2e.optimise", version="1")
def optimise(prompt: str, optimise_with: str) -> None:
    ctx = get_task_context()
    with registry() as prompts:
        baseline = prompts.resolve(prompt).text
    # An artifact, never an inline result: a prompt's text stays out of every
    # workflow message.
    ctx.write_artifact("candidate", [{"text": optimiser(baseline, optimise_with)}])


@task("e2e.record", version="1")
def record(prompt: str, optimise_with: str) -> dict[str, Any]:
    del optimise_with
    ctx = get_task_context()
    baseline = ctx.read_artifact("baseline_eval")[0]
    candidate = ctx.read_artifact("candidate_eval")[0]
    text = str(ctx.read_artifact("candidate")[0]["text"])
    with registry() as prompts:
        recorded = prompts.record_optimization(
            prompt,
            algorithm="e2e/append",
            baseline=str(baseline["version_id"]),
            candidate_text=text,
            primary_metric="exact_match",
            test=scores(
                {"exact_match": float(baseline["exact_match"])},
                {"exact_match": float(candidate["exact_match"])},
            ),
            dataset=DATASET,
            baseline_evaluation=str(baseline["evaluation_id"]),
            candidate_evaluation=str(candidate["evaluation_id"]),
            # Filed under the step, so a retry lands on this record.
            optimization_id="e2e-" + hashlib.sha256(ctx.step_key.encode()).hexdigest()[:24],
            promote=False,
        )
    verdict = {
        "optimization_id": recorded.optimization_id,
        "outcome": "admitted" if recorded.admitted else "rejected",
        "candidate": recorded.candidate,
    }
    ctx.write_artifact("verdict", [verdict])
    return {"outcome": verdict["outcome"]}


@task("e2e.promote", version="1")
def promote(prompt: str, optimise_with: str) -> dict[str, Any]:
    del optimise_with
    ctx = get_task_context()
    verdict = ctx.read_artifact("verdict")[0]
    if verdict["outcome"] != "admitted":
        # Nothing to decide: the server turned it down, and a question about a
        # candidate that cannot be promoted is a button that does not work.
        return {"asked": False, "promoted": False}
    answer = ctx.ask(
        f"Promote {str(verdict['candidate'])[:12]} to production?",
        role="admin",
        choices=("promote", "keep"),
    )
    if answer == "promote":
        with registry() as prompts:
            prompts.set_label(prompt, "production", str(verdict["candidate"]))
    return {"asked": True, "promoted": answer == "promote"}


WORKFLOW = Workflow(
    name="e2e.optimise-prompt",
    version="1",
    steps=(
        WorkflowStep(
            "evaluate_baseline", evaluate, params={"which": "baseline"}, outputs=("baseline_eval",)
        ),
        WorkflowStep("optimise", optimise, outputs=("candidate",)),
        WorkflowStep(
            "evaluate_candidate",
            evaluate,
            params={"which": "candidate"},
            inputs=(WorkflowInput("optimise", "candidate"),),
            outputs=("candidate_eval",),
        ),
        WorkflowStep(
            "record",
            record,
            inputs=(
                WorkflowInput("evaluate_baseline", "baseline_eval"),
                WorkflowInput("optimise", "candidate"),
                WorkflowInput("evaluate_candidate", "candidate_eval"),
            ),
            outputs=("verdict",),
        ),
        WorkflowStep("promote", promote, inputs=(WorkflowInput("record", "verdict"),)),
    ),
)


# ── The server. ──────────────────────────────────────────────────────────────


def serve(home: Path) -> subprocess.Popen[bytes]:
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
    home.mkdir(parents=True)
    # Nothing inherited: an AIWATCHER_AUTH_MODE in somebody's shell would make
    # this a different test.
    env = {name: value for name, value in os.environ.items() if not name.startswith("AIWATCHER_")}
    env |= {
        "AIWATCHER_LISTEN": f"127.0.0.1:{port}",
        "AIWATCHER_DATA_DIR": str(home / ".data"),
        "AIWATCHER_BUS": "wal",
        # The worker is another thread claiming over HTTP, and the `file` store
        # refuses a plan that needs one.
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
        if call("GET", "/readyz")[0] == 200 and call("GET", "/api/v1/prompts")[0] == 200:
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


# ── The run. ─────────────────────────────────────────────────────────────────


def step_of(run: dict[str, Any], step_id: str) -> dict[str, Any] | None:
    return next(
        (step for step in run["execution"]["steps"] if step["step_id"] == step_id),
        None,
    )


def history_bytes(execution_id: str) -> bytes:
    """Every message of the run's stream, as the API serves them."""
    pages, after = [], 0
    while True:
        status, body, raw = call(
            "GET", f"/api/v1/executions/{execution_id}/history?after={after}&limit=500"
        )
        if status != 200:
            raise SystemExit(f"history answered {status}: {raw[:300]!r}")
        pages.append(raw)
        if not body.get("next_after"):
            return b"".join(pages)
        after = body["next_after"]


def reports_of(execution_id: str) -> list[dict[str, Any]]:
    """The run's evaluations, once the projector has folded both."""
    deadline = time.monotonic() + 15
    listed: list[dict[str, Any]] = []
    while time.monotonic() < deadline:
        status, body, _ = call("GET", "/api/v1/evaluations?limit=100")
        if status == 200:
            listed = [row for row in body["evaluations"] if row.get("execution_id") == execution_id]
            if len(listed) >= 2 and all(row["status"] == "succeeded" for row in listed):
                return listed
        time.sleep(0.2)
    return listed


def run_variant(root: Path, label: str, kind: str, answer: str | None) -> list[str]:
    """One run on a fresh server. Returns the failed claims, empty when green."""
    failures: list[str] = []

    def check(number: int, claim: str, holds: bool, detail: object = "") -> None:
        mark = "✓" if holds else "✗"
        print(f"  {mark} {number}. {claim}" + (f" — {detail}" if detail not in ("", None) else ""))
        if not holds:
            failures.append(f"{label}: {claim}")

    print(f"\n{label}:")
    server = serve(root / label)
    telemetry = AiwatcherClient(service="e2e-optimise", base_url=BASE)
    try:
        with registry() as prompts:
            baseline = prompts.publish(PROMPT, BASELINE, author="e2e", label="production")
        with Runtime(
            name="e2e-optimise",
            url=BASE,
            workflows=[WORKFLOW],
            pools=[ExecutionPool(name="e2e", queue=QUEUE)],
            placement={WORKFLOW.ref: "e2e"},
            telemetry=telemetry,
            poll_interval=0.1,
        ) as runtime:
            runtime.start()
            handle = runtime.run(
                WORKFLOW,
                parameters={"prompt": PROMPT, "optimise_with": kind},
                idempotency_key=f"e2e-optimise-{label}",
            )
            question: dict[str, Any] | None = None
            deadline = time.monotonic() + RUN_WITHIN
            run = handle.status()
            while time.monotonic() < deadline:
                run = handle.status()
                gate = step_of(run, "promote")
                if (
                    question is None
                    and gate is not None
                    and gate["state"]["state_type"] == "awaiting_input"
                ):
                    question = gate
                    status, body, _ = call(
                        "POST",
                        f"/api/v1/executions/{handle.execution_id}/steps/promote/input",
                        {"attempt": gate.get("attempt", 1), "response": answer},
                    )
                    if status != 200:
                        raise SystemExit(f"the answer was refused ({status}): {body}")
                if run["execution"]["state"]["state_type"] in TERMINAL:
                    break
                time.sleep(0.1)
            runtime.stop()

        execution_id = handle.execution_id
        state = run["execution"]["state"]["state_type"]
        check(1, "the run completes", state == "completed", state)

        reports = reports_of(execution_id)
        steps = sorted(str(row.get("step_id")) for row in reports)
        check(
            2,
            "both reports are listed under the run, one per evaluating step",
            steps == ["evaluate_baseline", "evaluate_candidate"],
            steps,
        )
        by_step = {row.get("step_id"): row["evaluation_id"] for row in reports}

        _, detail, _ = call("GET", f"/api/v1/prompts/{PROMPT}")
        optimisations = detail["head"]["optimizations"]
        with registry() as prompts:
            records = [
                prompts.get_optimization(PROMPT, row["optimization_id"]) for row in optimisations
            ]
        check(
            3,
            "one optimisation, naming the baseline's and the candidate's reports",
            len(records) == 1
            and records[0].baseline_evaluation == by_step.get("evaluate_baseline")
            and records[0].candidate_evaluation == by_step.get("evaluate_candidate"),
            f"{len(records)} recorded",
        )
        outcome = records[0].reason if records and not records[0].admitted else "admitted"
        expected = "admitted" if kind == "improve" else "no_held_out_improvement"
        check(4, f"the server's verdict is {expected}", outcome == expected, outcome)

        role = (question or {}).get("awaiting", {}).get("role")
        if kind == "improve":
            check(5, "an admin was asked whether to promote", role == "admin", role)
        else:
            check(5, "nobody was asked about a rejected candidate", question is None)

        candidate = records[0].candidate if records else ""
        with registry() as prompts:
            production = prompts.resolve(PROMPT).version_id
            stored = prompts.get_version(PROMPT, candidate).text if candidate else ""
        promoted = answer == "promote" and kind == "improve"
        check(
            6,
            "production names the candidate" if promoted else "production still names the baseline",
            production == (candidate if promoted else baseline.version_id),
            production[:12],
        )

        text = optimiser(BASELINE, kind).encode()
        _, _, view = call("GET", f"/api/v1/executions/{execution_id}")
        check(
            7,
            "the candidate's text is in no read of the run, and is in the registry",
            text not in view and text not in history_bytes(execution_id) and stored.encode() == text,
        )
    finally:
        telemetry.close()
        server.kill()
        server.wait(timeout=10)
    return failures


def main() -> None:
    root = Path(tempfile.mkdtemp(prefix="aiwatcher-e2e-optimise-"))
    failures: list[str] = []
    try:
        failures += run_variant(root, "approved", "improve", "promote")
        failures += run_variant(root, "kept", "improve", "keep")
        failures += run_variant(root, "rejected", "pad", None)
    finally:
        if not failures:
            shutil.rmtree(root, ignore_errors=True)
    if failures:
        print(f"\n{len(failures)} failed; the servers' logs are under {root}")
        raise SystemExit(1)
    print("\nall three runs hold")


if __name__ == "__main__":
    main()
