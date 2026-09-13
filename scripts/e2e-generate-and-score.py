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

    cases ──► generate (a worker's task) ──► traces ──► score

once per variant: the serve role reads the cohort's inputs under the pair's
admission, a worker answers each case with the variant it is told, and the same
score step as C0 reads what the worker wrote. The "application" is a
deterministic stand-in whose prompt decides how it answers, so the run costs
nothing and says the same thing every time:

- the **baseline** prompt answers in a sentence, and exact match fails it;
- the **candidate** prompt answers in one word;
- a second repetition of the candidate is told to decline one case.

What it checks:

1. both runs complete, four steps each, the generation on a worker;
2. the worker was handed each case's question and nothing it expected;
3. the candidate scores higher than the baseline, on the same context;
4. the comparison is `comparable`, and names the metric's rise;
5. the case the repetition declined is unscored, not a zero: coverage drops,
   and its comparison with the baseline is withheld as unverified;
6. a result names its execution and step, and a case names the trace its
   run was seen in — which the application never had to know;
7. starting a declaration again lands on the run it started, not a second one;
8. a variant pinning code the worker does not hold fails before a case is
   answered, naming both digests, and publishes nothing.
9. the experiment over that context sets the variants side by side: the
   candidate against the baseline, each with per-case latency and tokens over
   every case, and the whole run's duration from the log — and each result's
   cases priced by the model their runs' calls named, at the deployment's
   table, with the day the price was read.
10. what a variant was observed doing stands beside it: runs the application
    served naming the candidate are counted with their durations and tokens,
    while the runs it made answering the measurement's cases are counted apart
    and in no figure — and the baseline, served nowhere, is observed nowhere.
11. the traces of every generated answer showed it made on the pinned prompt,
    and the result says so: every answer seen, every one on the prompt;
12. a worker whose application renders another version of the pinned prompt
    fails at the traces step naming both versions, and publishes nothing —
    though what it generated with agrees with every pin it holds;
13. a variant that also pins a model nobody here registered and a workflow is
    seen, on every answer, executing the pinned workflow and calling the pinned
    model — and the gateway in front of the stand-in provider, publishing its own
    run under the one credential the server names a witness, is a second
    witness on every one to the model version the provider said served the call
    and to the prompt version whose template it found in the request: the log
    records each run as published by the token that sent it;
14. an application that steps through a node the pinned workflow does not
    declare fails at the traces step naming the node, and publishes nothing —
    and so does one that starts `answer` before `retrieve`, which the pinned
    declaration leads into it from, has completed;
15. what the candidate was observed serving is written down as each period
    closes, and a window asked of the experiment reads those periods with the
    live runs no written period holds: still five runs, not ten, each model
    call timed, and priced at the deployment's table, which says where and
    when the price was read;
16. a gateway holding the application's own token witnesses nothing: every
    answer is counted self-witnessed, and a gate requiring a witness holds the
    result incomplete, saying to give the serving host a token of its own;
17. a request naming the pinned prompt whose text does not hold its template
    fails at the traces step on the gateway's word, and publishes nothing.

The server runs behind a stand-in authenticating proxy: a person's requests
carry its headers, and the application, the gateway and the worker each publish
with a token of their own, which is what makes the gateway's word another
credential's. The served variant's application calls a stand-in provider over
HTTP, through `aiwatcher_sdk.gateway`.

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
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "sdk" / "python"))

from aiwatcher_sdk import AiwatcherClient  # noqa: E402
from aiwatcher_sdk.gateway import Gateway  # noqa: E402
from aiwatcher_sdk.prompts import PromptRegistry  # noqa: E402
from aiwatcher_sdk.worker import (  # noqa: E402
    Case,
    Declined,
    Generated,
    GeneratedWith,
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

#: Three credentials, as three hosts hold them: the application's, a model
#: server's, and the worker's, which may claim this queue's work.
APPLICATION_SECRET = "a1" * 16
SERVING_SECRET = "5e" * 16
WORKER_SECRET = "0b" * 16
#: What a person's request carries from the proxy in front of the server.
PERSON = {
    "x-authentik-uid": "e2e",
    "x-authentik-username": "e2e",
    "x-authentik-groups": "aiwatcher-admins",
}

#: The pinned workflow's declaration, and the model a model server serves.
WORKFLOW = json.dumps({"nodes": ["retrieve", "answer"], "edges": [["retrieve", "answer"]]}).encode()
WEIGHTS = b"the capitals model's weights\n"
PACKAGE = json.dumps(
    {
        "runtime": "weights",
        "artifacts": [
            {
                "name": "weights",
                "uri": "s3://elsewhere/capitals.bin",
                "digest": hashlib.sha256(WEIGHTS).hexdigest(),
                "size_bytes": len(WEIGHTS),
                "content_type": "",
                "kind": "model",
            }
        ],
    }
).encode()
MODEL = {"name": "capitals-served", "version": hashlib.sha256(PACKAGE).hexdigest()}
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

#: The application's own telemetry, pointed at the server once it is up.
TELEMETRY: list[AiwatcherClient] = []
#: The gateways in front of the stand-in provider: one holding a witness's
#: token, and one holding the application's own.
GATEWAYS: dict[str, str] = {}


class Provider(BaseHTTPRequestHandler):
    """A stand-in model provider: one word when its system message asks for one."""

    def log_message(self, format: str, *args: Any) -> None:
        del format, args

    def do_POST(self) -> None:
        body = json.loads(self.rfile.read(int(self.headers["content-length"])))
        system, question = (message["content"] for message in body["messages"])
        country = question.removeprefix("What is the capital of ").removesuffix("?")
        said = application(system, country, dict(CAPITALS)[country])
        payload = json.dumps(
            {
                # The version it served, as a provider names its snapshot.
                "model": MODEL["version"],
                "choices": [{"message": {"role": "assistant", "content": said}}],
                "usage": {
                    "prompt_tokens": len(question.split()),
                    "completion_tokens": len(said.split()),
                },
            }
        ).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)


def through_gateway(
    which: str, headers: dict[str, str], system: str, question: str
) -> dict[str, Any]:
    """One call to the provider, through a gateway."""
    request = urllib.request.Request(  # noqa: S310 — the e2e's own gateway
        GATEWAYS[which] + "/v1/chat/completions",
        data=json.dumps(
            {
                "model": MODEL["name"],
                "messages": [
                    {"role": "system", "content": system},
                    {"role": "user", "content": question},
                ],
            }
        ).encode(),
        method="POST",
    )
    request.add_header("content-type", "application/json")
    for name, value in headers.items():
        request.add_header(name, value)
    with urllib.request.urlopen(request, timeout=15) as response:  # noqa: S310
        reply: dict[str, Any] = json.loads(response.read())
    return reply


# ── The application, and the task a worker hosts. ────────────────────────────


def application(prompt: str, country: str, capital: str) -> str:
    """A model that answers in one word only when it is told to."""
    if "one word" in prompt:
        return capital
    return f"The capital of {country} is {capital}, as it happens."


def code_of(which: str) -> bytes:
    return f"# the {which} application\n".encode()


def generation_of(which: str) -> bytes:
    return json.dumps({"temperature": 0, "variant": which}).encode()


def holding(run: Generation) -> GeneratedWith:
    """What this worker was built with for the variant it is asked about."""
    which = str(run.variant["experiment_id"])
    return GeneratedWith.of(code=code_of(which), generation_config=generation_of(which))


@generation_task("e2e.capitals.answer", version="1", generated_with=holding)
def answer(case: Case, run: Generation) -> JsonValue | Generated | Declined:
    context = get_task_context()
    HANDED.extend(context.read_artifact("cases") if not HANDED else [])
    prompt = run.variant["prompt"]
    assert isinstance(prompt, dict)
    with PromptRegistry(BASE, token=APPLICATION_SECRET) as prompts:
        version = prompts.get_version(str(prompt["name"]), str(prompt["version"]))
    text = version.text
    assert isinstance(case.input, dict)
    question = str(case.input["question"])
    country = question.removeprefix("What is the capital of ").removesuffix("?")
    capital = dict(CAPITALS)[country]
    if run.params.get("decline") == country:
        return Declined("the application would not say")
    # The application's run for this case: the variant's, and a measurement's —
    # and the prompt version it renders, which is the variant's unless the run
    # was told to render another.
    rendered = str(run.params.get("render_version") or prompt["version"])
    if rendered != prompt["version"]:
        with PromptRegistry(BASE, token=APPLICATION_SECRET) as prompts:
            version = prompts.get_version(str(prompt["name"]), rendered)
        text = version.text
    model = run.variant.get("model")
    if isinstance(model, dict):
        # The application as an execution of the workflow the variant pins,
        # calling the pinned model through a gateway that reports its own runs
        # — with the prompt rendered, unless the run was told to drift from it.
        steps = ["retrieve", "answer"]
        if run.params.get("stray"):
            steps.append("improvise")
        if run.params.get("reorder"):
            steps.reverse()
        with run.traced_workflow(
            TELEMETRY[0],
            case,
            "capitals-app",
            nodes=["retrieve", "answer"],
            edges=[("retrieve", "answer")],
        ) as flow:
            for node in steps:
                if node == "retrieve":
                    with flow.node(node):
                        continue
                with (
                    flow.node(node) as stage,
                    stage.agent("capitals") as agent,
                    agent.llm(
                        model=str(model["name"]), prompt=(str(prompt["name"]), rendered)
                    ) as llm,
                ):
                    system = (
                        "Answer in one word."
                        if run.params.get("drift")
                        else version.render(country=country)
                    )
                    reply = through_gateway(
                        "shared" if run.params.get("shared") else "witness",
                        llm.caller_headers(),
                        system,
                        question,
                    )
                    said = str(reply["choices"][0]["message"]["content"])
                    llm.usage(
                        prompt_tokens=reply["usage"]["prompt_tokens"],
                        completion_tokens=reply["usage"]["completion_tokens"],
                        model_version=reply["model"],
                    )
        return Generated(said, run_id=flow.correlation.run_id)
    with (
        run.traced(TELEMETRY[0], case) as traced,
        traced.agent("capitals") as agent,
        agent.llm(model="capitals-stand-in", prompt=(str(prompt["name"]), rendered)) as llm,
    ):
        said = application(text, country, capital)
        llm.usage(prompt_tokens=len(question.split()), completion_tokens=len(said.split()))
    # What a model would have counted: the question in, the words out.
    return Generated(
        said,
        run_id=traced.correlation.run_id,
        input_tokens=len(question.split()),
        output_tokens=len(said.split()),
    )


# ── The server. ──────────────────────────────────────────────────────────────


def prices(home: Path) -> Path:
    """The deployment's price table: the stand-in model, per million tokens."""
    table = home / "prices.json"
    table.write_text(
        json.dumps(
            {
                "currency": "USD",
                "prices": [
                    {
                        "model": "capitals-stand-in",
                        "input_per_million": 1.0,
                        "output_per_million": 2.0,
                        "source": "https://example.com/capitals-stand-in/pricing",
                        "as_of": "2026-09-13",
                    }
                ],
            }
        )
    )
    return table


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
        # Periods of five seconds, so one closes while this runs; and a price
        # for the stand-in model, read from a page on a day.
        "AIWATCHER_OBSERVATION_PERIOD_SECONDS": "5",
        "AIWATCHER_MODEL_PRICES": str(prices(home)),
        # The one credential whose runs witness a generated answer.
        "AIWATCHER_WITNESSES": "serving",
        "AIWATCHER_AUTH_MODE": "proxy",
        "AIWATCHER_AUTH_INGEST_TOKENS": ",".join(
            [
                f"application={APPLICATION_SECRET}",
                f"serving={SERVING_SECRET}",
                f"worker[{QUEUE}]={WORKER_SECRET}",
            ]
        ),
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
    request = urllib.request.Request(  # noqa: S310 — our own server
        BASE + path, data=data, method=method, headers=PERSON
    )
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
    code: bytes | None = None,
    served: bool = False,
    suffix: str = "",
) -> dict[str, Any]:
    """Declare one variant's run, admit its pair, and return the view.

    ``served`` also pins the model a model server serves and the workflow the
    application executes, and stages what those imply beside the rest.
    """
    with PromptRegistry(BASE, token=APPLICATION_SECRET) as prompts:
        version = prompts.publish(PROMPT, PROMPTS[which], author="e2e").version_id
    code = code_of(which) if code is None else code
    generation = generation_of(which)
    run = {
        "evaluation_id": f"capitals-{which}"
        + ("" if repetition == "measurement-1" else f"-{repetition}")
        + ("" if code == code_of(which) else "-unheld")
        + suffix,
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
    staged = [("application.py", code), ("generation.json", generation)]
    if served:
        run["variant"]["model"] = MODEL
        run["variant"]["workflow"] = {
            "name": "capitals-app",
            "version": hashlib.sha256(WORKFLOW).hexdigest(),
        }
        staged += [
            ("model-package.json", PACKAGE),
            ("model-artifacts/weights", WEIGHTS),
            ("workflow.json", WORKFLOW),
        ]
    view = ok(*call("POST", "/api/v1/evaluation-runs", run)[:2], f"declaring the {which} run")
    approval = view["approval_id"]
    # The operator's act: the manifest and the files the variant pins. The
    # cohort's three are derived again from the dataset version, never staged.
    for name, content in (("manifest.json", json.dumps(view["manifest"]).encode()), *staged):
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
    TELEMETRY.append(
        AiwatcherClient(service="e2e-capitals", base_url=BASE, token=APPLICATION_SECRET)
    )
    provider = ThreadingHTTPServer(("127.0.0.1", 0), Provider)
    threading.Thread(target=provider.serve_forever, daemon=True).start()
    gateways = []
    for which, secret in (("witness", SERVING_SECRET), ("shared", APPLICATION_SECRET)):
        relay = Gateway(
            f"http://127.0.0.1:{provider.server_address[1]}",
            AiwatcherClient(service=f"e2e-gateway-{which}", base_url=BASE, token=secret),
            prompts=PromptRegistry(BASE, token=secret),
            upstream_token="provider-key",  # noqa: S106 — the stand-in provider's
        )
        gateway = relay.server(port=0)
        threading.Thread(target=gateway.serve_forever, daemon=True).start()
        GATEWAYS[which] = f"http://127.0.0.1:{gateway.server_address[1]}"
        gateways.append(gateway)
    worker = Worker(
        BASE,
        WORKER_SECRET,
        queues=[QUEUE],
        tasks=[answer],
        name="e2e-generate-worker",
        poll_interval=0.1,
        telemetry=AiwatcherClient(service="e2e-generate", base_url=BASE, token=WORKER_SECRET),
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
        unheld = declare(
            "candidate",
            dataset,
            cohort,
            card,
            code=b"# a commit this worker was never built from\n",
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
            and all(
                sorted(ids) == ["cases", "generate", "score", "traces"] for ids in steps.values()
            ),
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
            "the result names its execution and step, and its cases the traces their runs had",
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

        refused = ok(
            *call("POST", f"/api/v1/evaluation-runs/{unheld['declaration']['id']}/start")[:2],
            "starting the unheld run",
        )
        stale = followed(refused["execution"]["execution_id"])
        told = json.dumps(stale["execution"])
        pinned = unheld["declaration"]["run"]["variant"]["code"]["digest"]
        held = hashlib.sha256(code_of("candidate")).hexdigest()
        published = call(
            "GET",
            f"/api/v1/evaluation-results/{unheld['declaration']['run']['evaluation_id']}",
        )[0]
        check(
            8,
            "a variant pinning code the worker does not hold is refused before an answer",
            stale["execution"]["state"]["state_type"] == "failed"
            and pinned in told
            and held in told
            and published == 404,
            {"state": stale["execution"]["state"]["state_type"], "result": published},
        )

        context_id = results["candidate"]["receipt"]["context_id"]
        viewed = ok(
            *call("GET", f"/api/v1/experiments/{context_id}?baseline=capitals-baseline")[:2],
            "reading the experiment",
        )
        rows = {row["evaluation_id"]: row for row in viewed["experiment"]["rows"]}
        candidate = rows.get("capitals-candidate", {})
        usage = candidate.get("usage") or {}
        priced = candidate.get("cost") or {}
        timed = {execution["workflow_run_id"] for execution in viewed["executions"]}
        # Six words in and one out per case, on the stand-in at 1 and 2 per million.
        expected_cost = (6 * len(CAPITALS) * 1.0 + len(CAPITALS) * 2.0) / 1e6
        check(
            9,
            "the experiment sets the variants side by side with what their answers took",
            {"capitals-baseline", "capitals-candidate"} <= set(rows)
            and (candidate.get("comparison") or {}).get("comparability") == "comparable"
            and (usage.get("latency_ms") or {}).get("cases") == len(CAPITALS)
            and (usage.get("output_tokens") or {}).get("cases") == len(CAPITALS)
            and started["candidate"]["execution"]["execution_id"] in timed
            and priced.get("priced_calls") == len(CAPITALS)
            and abs(priced.get("amount", 0) - expected_cost) < 1e-12
            and [price.get("as_of") for price in priced.get("prices", [])] == ["2026-09-13"],
            {"rows": sorted(rows), "usage": usage, "cost": priced, "timed": len(timed)},
        )

        # The candidate, deployed: the same application serving somebody, each
        # run naming the variant the result was published as — a period after
        # the one the server's fold began in, which is the first it can vouch
        # for holding whole.
        time.sleep(6)
        variant_id = candidate.get("variant_id", "")
        production = AiwatcherClient(
            service="e2e-capitals",
            base_url=BASE,
            token=APPLICATION_SECRET,
            variant_id=variant_id,
        )
        served = 5
        for request in range(served):
            with (
                production.run(f"served-{request}") as traced,
                traced.agent("capitals") as agent,
                agent.llm(model="capitals-stand-in") as llm,
            ):
                time.sleep(0.02)
                llm.usage(prompt_tokens=6, completion_tokens=1)
        production.flush()
        TELEMETRY[0].flush()
        served_at = time.time()
        observed: dict[str, Any] = {}
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            watched = ok(
                *call("GET", f"/api/v1/experiments/{context_id}")[:2], "reading what was observed"
            )
            observed = {row["variant_id"]: row for row in watched["observed"]}
            if (observed.get(variant_id) or {}).get("runs") == served:
                break
            time.sleep(0.3)
        seen = observed.get(variant_id) or {}
        baseline_seen = observed.get(rows.get("capitals-baseline", {}).get("variant_id", "")) or {}
        check(
            10,
            "what the candidate was observed serving stands beside it, its measurement left out",
            seen.get("runs") == served
            and seen.get("failed") == 0
            and (seen.get("duration_ms") or {}).get("count") == served
            and seen.get("input_tokens") == 6 * served
            and seen.get("measured_runs", 0) >= len(CAPITALS)
            and baseline_seen.get("runs") == 0
            and baseline_seen.get("measured_runs", 0) >= len(CAPITALS),
            {"candidate": seen, "baseline": baseline_seen},
        )

        traces = {which: result.get("traces") or {} for which, result in results.items()}
        check(
            11,
            "every generated answer's trace showed it made on the pinned prompt",
            traces["candidate"]
            == {
                "answers": len(CAPITALS),
                "named": len(CAPITALS),
                "seen": len(CAPITALS),
                "on_prompt": len(CAPITALS),
                # Nobody but the application saw these calls.
                "witnessed_prompt": 0,
            }
            and traces["declining"].get("on_prompt") == len(CAPITALS) - 1,
            traces,
        )

        pins = {
            which: views[which]["declaration"]["run"]["variant"]["prompt"]["version"]
            for which in ("baseline", "candidate")
        }
        rendering = declare(
            "candidate",
            dataset,
            cohort,
            card,
            repetition="measurement-3",
            params={"render_version": pins["baseline"]},
        )
        misrendered = followed(
            ok(
                *call("POST", f"/api/v1/evaluation-runs/{rendering['declaration']['id']}/start")[
                    :2
                ],
                "starting the run that renders another prompt",
            )["execution"]["execution_id"]
        )
        told = json.dumps(misrendered["execution"])
        check(
            12,
            "answers whose traces show another version of the pinned prompt are never scored",
            misrendered["execution"]["state"]["state_type"] == "failed"
            and [
                step["step_id"]
                for step in misrendered["execution"]["steps"]
                if '"failed"' in json.dumps(step)
            ]
            == ["traces"]
            and pins["baseline"] in told
            and pins["candidate"] in told
            and call(
                "GET",
                f"/api/v1/evaluation-results/{rendering['declaration']['run']['evaluation_id']}",
            )[0]
            == 404,
            {
                "state": misrendered["execution"]["state"]["state_type"],
                "failed_at": [
                    step["step_id"]
                    for step in misrendered["execution"]["steps"]
                    if '"failed"' in json.dumps(step)
                ],
            },
        )

        # A variant also pinning a model nobody here registered and a workflow:
        # the application executes the workflow, and a model server reports
        # the call it served under its own credential.
        flowing = declare("candidate", dataset, cohort, card, served=True, suffix="-served")
        flowed = followed(
            ok(
                *call("POST", f"/api/v1/evaluation-runs/{flowing['declaration']['id']}/start")[:2],
                "starting the run on a served model and a pinned workflow",
            )["execution"]["execution_id"]
        )
        evaluation = flowing["declaration"]["run"]["evaluation_id"]
        witnessed = (call("GET", f"/api/v1/evaluation-results/{evaluation}")[1] or {}).get(
            "traces"
        ) or {}
        spans = call("GET", "/api/v1/runs?limit=200")[1] or {}
        publishers = {
            run.get("published_by")
            for run in spans.get("runs", [])
            if run["run_id"].startswith(("generate-capitals-candidate-served", "gateway-"))
        }
        check(
            13,
            "every answer is seen executing the pinned workflow on the pinned model, and the "
            "gateway's own run under the witness credential witnesses its model and prompt",
            flowed["execution"]["state"]["state_type"] == "completed"
            and witnessed
            == {
                "answers": len(CAPITALS),
                "named": len(CAPITALS),
                "seen": len(CAPITALS),
                "on_prompt": len(CAPITALS),
                "on_model": len(CAPITALS),
                "on_workflow": len(CAPITALS),
                "witnessed_model": len(CAPITALS),
                "witnessed_prompt": len(CAPITALS),
                "witnesses": ["serving"],
            }
            and publishers == {"application", "serving"},
            {
                "state": flowed["execution"]["state"]["state_type"],
                "traces": witnessed,
                "published_by": sorted(str(name) for name in publishers),
            },
        )

        straying = declare(
            "candidate",
            dataset,
            cohort,
            card,
            repetition="measurement-4",
            params={"stray": True},
            served=True,
            suffix="-served",
        )
        strayed = followed(
            ok(
                *call("POST", f"/api/v1/evaluation-runs/{straying['declaration']['id']}/start")[:2],
                "starting the run whose application steps off the pinned workflow",
            )["execution"]["execution_id"]
        )
        told = json.dumps(strayed["execution"])
        reordering = declare(
            "candidate",
            dataset,
            cohort,
            card,
            repetition="measurement-5",
            params={"reorder": True},
            served=True,
            suffix="-served",
        )
        reordered = followed(
            ok(
                *call("POST", f"/api/v1/evaluation-runs/{reordering['declaration']['id']}/start")[
                    :2
                ],
                "starting the run whose application answers before it retrieves",
            )["execution"]["execution_id"]
        )
        check(
            14,
            "answers whose run stepped off the pinned workflow, or out of its order, are never "
            "scored",
            strayed["execution"]["state"]["state_type"] == "failed"
            and [
                step["step_id"]
                for step in strayed["execution"]["steps"]
                if '"failed"' in json.dumps(step)
            ]
            == ["traces"]
            and "improvise" in told
            and call(
                "GET",
                f"/api/v1/evaluation-results/{straying['declaration']['run']['evaluation_id']}",
            )[0]
            == 404
            and reordered["execution"]["state"]["state_type"] == "failed"
            and "started answer before retrieve had completed"
            in json.dumps(reordered["execution"]),
            {
                "stray": strayed["execution"]["state"]["state_type"],
                "reordered": reordered["execution"]["state"]["state_type"],
            },
        )

        # The same runs, through a window: once the periods they ended in are
        # written, they are counted from those — and only from those.
        windowed: dict[str, Any] = {}
        # A period closes a second after its five, and the writer looks every
        # five: twelve seconds after the last run, its period is written.
        time.sleep(max(0.0, served_at + 12 - time.time()))
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            watched = ok(
                *call("GET", f"/api/v1/experiments/{context_id}?window_seconds=3600")[:2],
                "reading what was observed over a window",
            )
            windowed = next(
                (row for row in watched["observed"] if row["variant_id"] == variant_id), {}
            )
            if windowed.get("runs_from_periods") == served:
                break
            time.sleep(1)
        cost = windowed.get("cost") or {}
        check(
            15,
            "observations over a window read the written periods once, time each call, and price "
            "them at a dated price",
            windowed.get("runs") == served
            and windowed.get("runs_from_periods") == served
            and (windowed.get("duration_ms") or {}).get("bucketed") is True
            and (windowed.get("call_ms") or {}).get("count") == served
            and cost.get("priced_calls") == served
            and abs(cost.get("amount", 0) - (6 * served * 1.0 + served * 2.0) / 1e6) < 1e-12
            and [price.get("as_of") for price in cost.get("prices", [])] == ["2026-09-13"],
            {
                key: windowed.get(key)
                for key in (
                    "runs",
                    "runs_from_periods",
                    "periods",
                    "incomplete_periods",
                    "duration_ms",
                    "call_ms",
                    "cost",
                )
            },
        )

        # A gateway holding the application's own token.
        sharing = declare(
            "candidate",
            dataset,
            cohort,
            card,
            repetition="measurement-6",
            params={"shared": True},
            served=True,
            suffix="-served",
        )
        shared_run = followed(
            ok(
                *call("POST", f"/api/v1/evaluation-runs/{sharing['declaration']['id']}/start")[:2],
                "starting the run whose gateway holds the application's token",
            )["execution"]["execution_id"]
        )
        sharing_id = sharing["declaration"]["run"]["evaluation_id"]
        shared_traces = (call("GET", f"/api/v1/evaluation-results/{sharing_id}")[1] or {}).get(
            "traces"
        ) or {}
        gated = (
            call(
                "POST",
                f"/api/v1/evaluation-results/{sharing_id}/gate",
                {"baseline": evaluation, "policy": {"require_witness": True}},
            )[1]
            or {}
        )
        check(
            16,
            "a gateway holding the application's own token witnesses nothing, and a gate "
            "requiring a witness says so",
            shared_run["execution"]["state"]["state_type"] == "completed"
            and shared_traces.get("self_witnessed") == len(CAPITALS)
            and shared_traces.get("witnessed_model") == 0
            and gated.get("verdict") == "incomplete"
            and any("a token of its own" in reason for reason in gated.get("reasons", [])),
            {"traces": shared_traces, "gate": gated.get("reasons")},
        )

        # A request naming the pinned prompt with other words in it.
        drifting = declare(
            "candidate",
            dataset,
            cohort,
            card,
            repetition="measurement-7",
            params={"drift": True},
            served=True,
            suffix="-served",
        )
        drifted = followed(
            ok(
                *call("POST", f"/api/v1/evaluation-runs/{drifting['declaration']['id']}/start")[:2],
                "starting the run whose requests do not hold the pinned prompt",
            )["execution"]["execution_id"]
        )
        check(
            17,
            "a request whose text does not hold the pinned prompt is refused on the gateway's word",
            drifted["execution"]["state"]["state_type"] == "failed"
            and "does not hold that version's template" in json.dumps(drifted["execution"])
            and call(
                "GET",
                f"/api/v1/evaluation-results/{drifting['declaration']['run']['evaluation_id']}",
            )[0]
            == 404,
            {"state": drifted["execution"]["state"]["state_type"]},
        )
    finally:
        for relay in gateways:
            relay.shutdown()
        provider.shutdown()
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
