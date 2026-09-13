#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.13"
# dependencies = ["aiwatcher-sdk"]
#
# [tool.uv.sources]
# aiwatcher-sdk = { path = "../sdk/python", editable = true }
# ///
"""A baseline and a candidate, each generating its answers and scored on one cohort (C1).

Stage C's acceptance on a small pinned set: both variants go through the whole
process, the report leads to the execution and the trace, a repeated start does
not measure twice, missing observations lower the visible coverage, and the two
results compare. The template is

    cases ──► generate (a worker's task) ──► score

once per variant: the serve role reads the cohort's inputs under the pair's
admission, a worker answers each case with the variant it is told, and the same
score step as C0 reads what the worker wrote. The "application" is a
deterministic stand-in whose prompt decides how it answers, so the run costs
nothing and says the same thing every time:

- the **baseline** prompt answers in a sentence, and exact match fails it;
- the **candidate** prompt answers in one word;
- a second repetition of the candidate is told to decline one case.

What it checks:

1. both runs complete, three steps each, the generation on a worker;
2. the worker was handed each case's question and nothing it expected;
3. the candidate scores higher than the baseline, on the same context;
4. the comparison is `comparable`, and names the metric's rise;
5. the case the repetition declined is unscored, not a zero: coverage drops,
   and its comparison with the baseline is withheld as unverified;
6. a result names its execution and step, and a case names its trace;
7. starting a declaration again lands on the run it started, not a second one.

It starts **its own** aiwatcher, from `target/debug/aiwatcher` or
`AIWATCHER_BINARY`, on a free port with every byte under a temporary directory,
so the one on :8080 is not touched.

    cargo build --bin aiwatcher   # once
    just e2e-generate
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
import threading
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "sdk" / "python"))

from aiwatcher_sdk import AiwatcherClient  # noqa: E402
from aiwatcher_sdk.prompts import PromptRegistry  # noqa: E402
from aiwatcher_sdk.worker import (  # noqa: E402
    Case,
    Declined,
    Generated,
    Generation,
    JsonValue,
    Worker,
    generation_task,
    get_task_context,
)

ROOT = Path(__file__).resolve().parent.parent
BINARY = Path(os.environ.get("AIWATCHER_BINARY", ROOT / "target" / "debug" / "aiwatcher"))
READY_WITHIN = 30.0
RUN_WITHIN = 90.0
BASE = ""

QUEUE = "e2e-generate"
PROMPT = "e2e.capitals"
DATASET = "capitals"
TERMINAL = {"completed", "failed", "cancelled", "crashed"}

CAPITALS = (
    ("France", "Paris"),
    ("Japan", "Tokyo"),
    ("Peru", "Lima"),
    ("Kenya", "Nairobi"),
)

PROMPTS = {
    "baseline": "Answer the question about {{ country }} helpfully.",
    "candidate": "Answer the question about {{ country }} in one word.",
}

#: What the worker was handed, to check nothing it expected reached it.
HANDED: list[dict[str, Any]] = []


# ── The application, and the task a worker hosts. ────────────────────────────


def application(prompt: str, country: str, capital: str) -> str:
    """A model that answers in one word only when it is told to."""
    if "one word" in prompt:
        return capital
    return f"The capital of {country} is {capital}, as it happens."


@generation_task("e2e.capitals.answer", version="1")
def answer(case: Case, run: Generation) -> JsonValue | Generated | Declined:
    context = get_task_context()
    HANDED.extend(context.read_artifact("cases") if not HANDED else [])
    prompt = run.variant["prompt"]
    assert isinstance(prompt, dict)
    with PromptRegistry(BASE) as prompts:
        text = prompts.get_version(str(prompt["name"]), str(prompt["version"])).text
    assert isinstance(case.input, dict)
    question = str(case.input["question"])
    country = question.removeprefix("What is the capital of ").removesuffix("?")
    capital = dict(CAPITALS)[country]
    if run.params.get("decline") == country:
        return Declined("the application would not say")
    said = application(text, country, capital)
    # The trace this answer was made in, derived as the SDK derives a run's.
    trace = hashlib.sha256(f"{run.evaluation_id}/{case.case_id}".encode()).hexdigest()[:32]
    return Generated(said, trace_id=trace)


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


def call(
    method: str, path: str, body: Any = None, *, raw: bytes | None = None
) -> tuple[int, Any, bytes]:
    data = raw if raw is not None else None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(BASE + path, data=data, method=method)
    request.add_header(
        "Content-Type", "application/octet-stream" if raw is not None else "application/json"
    )
    try:
        with urllib.request.urlopen(request, timeout=15) as response:  # noqa: S310 — our own server
            content = response.read()
            status = response.status
    except urllib.error.HTTPError as error:
        content, status = error.read(), error.code
    except (urllib.error.URLError, ConnectionError):
        return 0, None, b""
    try:
        parsed = json.loads(content) if content else None
    except json.JSONDecodeError:
        parsed = None
    return status, parsed, content


def ok(status: int, body: Any, what: str) -> Any:
    if status not in (200, 201, 202, 204):
        raise SystemExit(f"{what} answered {status}: {json.dumps(body)[:600]}")
    return body


# ── What both variants share: a dataset version, a cohort and a card. ────────


def artifact(name: str, content: bytes) -> dict[str, Any]:
    return {
        "name": name,
        "uri": f"file://{name}",
        "digest": hashlib.sha256(content).hexdigest(),
        "size_bytes": len(content),
        "content_type": "application/json",
    }


def shared() -> tuple[dict[str, Any], dict[str, Any], dict[str, Any]]:
    rows = [
        {
            "case_id": f"capital-{country.lower()}",
            "input": {"question": f"What is the capital of {country}?"},
            "expected": {"answer": capital},
        }
        for country, capital in CAPITALS
    ]
    published = ok(
        *call(
            "POST",
            "/api/v1/datasets",
            {
                "name": DATASET,
                "pipeline": "data_frame()->read(capitals)",
                "columns": ["case_id", "input", "expected"],
                "items": rows,
                "source": hashlib.sha256(json.dumps(rows).encode()).hexdigest(),
            },
        )[:2],
        "publishing the dataset version",
    )
    dataset = {
        "kind": "curation",
        "name": DATASET,
        "version": published["dataset"]["latest"]["version"],
    }
    derived = ok(
        *call("POST", "/api/v1/evaluation-cohorts", {"dataset": dataset, "split": "test"})[:2],
        "deriving the cohort",
    )
    card = ok(
        *call(
            "POST",
            "/api/v1/evaluation-scorecards",
            {
                "name": "capitals-exact",
                "scorers": [
                    {
                        "metric": "exact",
                        "expected_path": "/answer",
                        "scorer": {"kind": "exact_match", "trim": True},
                    }
                ],
            },
        )[:2],
        "publishing the card",
    )
    return (
        dataset,
        derived["cohort"],
        {"name": card["scorecard"]["name"], "version": card["version"]},
    )


def declare(
    which: str,
    dataset: dict[str, Any],
    cohort: dict[str, Any],
    card: dict[str, Any],
    *,
    repetition: str = "measurement-1",
    params: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """Declare one variant's run, admit its pair, and return the view."""
    with PromptRegistry(BASE) as prompts:
        version = prompts.publish(PROMPT, PROMPTS[which], author="e2e").version_id
    code = f"# the {which} application\n".encode()
    generation = json.dumps({"temperature": 0, "variant": which}).encode()
    run = {
        "evaluation_id": f"capitals-{which}"
        + ("" if repetition == "measurement-1" else f"-{repetition}"),
        "repetition_id": repetition,
        "variant": {
            "schema_version": 1,
            "experiment_id": which,
            "dataset": dataset,
            "prompt": {"name": PROMPT, "version": version},
            "code": artifact("application.py", code),
            "generation_config": artifact("generation.json", generation),
        },
        "cohort": cohort,
        "scorecard": card,
        "answers": {
            "generated_by": {
                "task": "e2e.capitals.answer@1",
                "queue": QUEUE,
                **({"params": params} if params else {}),
            }
        },
        "settings": {"timeout_seconds": 300},
    }
    view = ok(*call("POST", "/api/v1/evaluation-runs", run)[:2], f"declaring the {which} run")
    approval = view["approval_id"]
    # The operator's act: the manifest and the two files the variant pins. The
    # cohort's three are derived again from the dataset version, never staged.
    for name, content in (
        ("manifest.json", json.dumps(view["manifest"]).encode()),
        ("application.py", code),
        ("generation.json", generation),
    ):
        ok(
            *call("PUT", f"/api/v1/evaluation-approvals/{approval}/bundle/{name}", raw=content)[:2],
            f"staging {name}",
        )
    ok(
        *call("POST", "/api/v1/evaluation-approvals", view["manifest"])[:2],
        f"admitting the {which} pair",
    )
    return view


def followed(execution_id: str) -> dict[str, Any]:
    deadline = time.monotonic() + RUN_WITHIN
    while time.monotonic() < deadline:
        status, body, _ = call("GET", f"/api/v1/executions/{execution_id}")
        if status == 200 and body["execution"]["state"]["state_type"] in TERMINAL:
            return body
        time.sleep(0.2)
    raise SystemExit(f"{execution_id} did not finish within {RUN_WITHIN:.0f}s")


# ── The run. ─────────────────────────────────────────────────────────────────


def main() -> int:
    failures: list[str] = []

    def check(number: int, claim: str, holds: bool, detail: object = "") -> None:
        mark = "✓" if holds else "✗"
        print(f"  {mark} {number}. {claim}" + (f" — {detail}" if detail not in ("", None) else ""))
        if not holds:
            failures.append(claim)

    home = Path(tempfile.mkdtemp(prefix="aiwatcher-e2e-generate-"))
    server = serve(home / "server")
    worker = Worker(
        BASE,
        queues=[QUEUE],
        tasks=[answer],
        name="e2e-generate-worker",
        poll_interval=0.1,
        telemetry=AiwatcherClient(service="e2e-generate", base_url=BASE),
    )
    serving = threading.Thread(target=worker.run, name="e2e-generate-worker", daemon=True)
    serving.start()
    try:
        dataset, cohort, card = shared()
        views = {
            which: declare(which, dataset, cohort, card) for which in ("baseline", "candidate")
        }
        views["declining"] = declare(
            "candidate",
            dataset,
            cohort,
            card,
            repetition="measurement-2",
            params={"decline": "Kenya"},
        )
        started = {
            which: ok(
                *call("POST", f"/api/v1/evaluation-runs/{view['declaration']['id']}/start")[:2],
                f"starting the {which} run",
            )
            for which, view in views.items()
        }
        runs = {
            which: followed(accepted["execution"]["execution_id"])
            for which, accepted in started.items()
        }
        print("\nbaseline, candidate and a declining repetition:")

        states = {which: run["execution"]["state"]["state_type"] for which, run in runs.items()}
        steps = {
            which: [step["step_id"] for step in run["execution"]["steps"]]
            for which, run in runs.items()
        }
        check(
            1,
            "both runs complete through cases, generate and score",
            set(states.values()) == {"completed"}
            and all(sorted(ids) == ["cases", "generate", "score"] for ids in steps.values()),
            {"states": states, "steps": steps},
        )
        check(
            2,
            "the worker is handed each case's question and nothing it expected",
            len(HANDED) >= len(CAPITALS)
            and all(set(row) == {"case_id", "input"} for row in HANDED),
            HANDED[:1],
        )

        results = {
            which: ok(
                *call(
                    "GET",
                    f"/api/v1/evaluation-results/{view['declaration']['run']['evaluation_id']}",
                )[:2],
                f"reading the {which} result",
            )
            for which, view in views.items()
        }
        exact = {which: result["metrics"].get("exact") for which, result in results.items()}
        contexts = {result["receipt"]["context_id"] for result in results.values()}
        check(
            3,
            "the candidate scores higher than the baseline, on one context",
            len(contexts) == 1 and (exact["candidate"] or 0) > (exact["baseline"] or 0),
            exact,
        )
        comparison = ok(
            *call(
                "GET",
                "/api/v1/evaluation-results/capitals-candidate/comparison?baseline=capitals-baseline",
            )[:2],
            "comparing the two",
        )
        deltas = {row["name"]: row.get("delta") for row in comparison.get("metrics", [])}
        check(
            4,
            "the comparison is comparable and names the rise",
            comparison.get("comparability") == "comparable" and (deltas.get("exact") or 0) > 0,
            {"comparability": comparison.get("comparability"), "deltas": deltas},
        )
        counts = results["declining"]["counts"]
        withheld = ok(
            *call(
                "GET",
                "/api/v1/evaluation-results/capitals-candidate-measurement-2/comparison"
                "?baseline=capitals-baseline",
            )[:2],
            "comparing the declining repetition",
        )
        check(
            5,
            "a declined case is unscored rather than a zero, and its comparison is withheld",
            counts["unscored"] == 1
            and counts["scored"] == len(CAPITALS) - 1
            and withheld.get("comparability") == "unverified"
            and all(row.get("delta") is None for row in withheld.get("metrics", [])),
            {"counts": counts, "reasons": withheld.get("reasons")},
        )

        manifest = results["candidate"]["manifest"]
        origin = manifest["origin"]
        _, page, _ = call(
            "GET",
            f"/api/v1/evaluation-results/capitals-candidate/cases?version={results['candidate']['receipt']['version']}",
        )
        traced = [
            case for case in (page or {}).get("cases", []) if case["measurement"].get("trace_id")
        ]
        check(
            6,
            "the result names its execution and step, and its cases their traces",
            origin.get("execution_id") == started["candidate"]["execution"]["execution_id"]
            and origin.get("step_id") == "score"
            and len(traced) == len(CAPITALS),
            {"origin": origin, "traced": len(traced)},
        )

        again = ok(
            *call(
                "POST", f"/api/v1/evaluation-runs/{views['candidate']['declaration']['id']}/start"
            )[:2],
            "starting the candidate again",
        )
        check(
            7,
            "starting a declaration again lands on the run it started",
            again["created"] is False
            and again["execution"]["execution_id"]
            == started["candidate"]["execution"]["execution_id"],
            again["created"],
        )
    finally:
        worker.stop()
        serving.join(timeout=5)
        server.terminate()
        try:
            server.wait(timeout=10)
        except subprocess.TimeoutExpired:
            server.kill()
        if failures:
            print(f"\nkept {home} for the server log")
        else:
            shutil.rmtree(home, ignore_errors=True)

    if failures:
        print(f"\n✗ {len(failures)} claim(s) did not hold")
        return 1
    print("\n✓ a baseline and a candidate generated, scored and compared")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
