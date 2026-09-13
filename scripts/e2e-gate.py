#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.13"
# dependencies = ["aiwatcher-sdk"]
#
# [tool.uv.sources]
# aiwatcher-sdk = { path = "../sdk/python", editable = true }
# ///
"""A regression gate in CI, end to end (FTI C3), against a server of its own.

An admin publishes the cases and the card, measures `main` as the baseline and
admits the line — every variant of the experiment in that context — once. Then
four jobs run `examples/ci-gate/app.py` at four "commits" and `aiwatcher-gate`
over what it answered, each with nothing but the policy and the run declaration:

1. the baseline run is admitted through the line, from bytes the job staged,
   with no bundle anybody staged by hand, and the approval names the line;
2. the same answers at a new commit pass: exit 0;
3. answers that fix France and Japan but name Mombasa for Kenya — the
   critical case — are a regression though the mean rose: exit 1;
4. answers missing a case never pass: exit 2;
5. a variant of another experiment, which the line does not cover, is an error
   naming the line route: exit 3;
6. the job summary names the commit, the card, the variant and the evidence;
7. a variant of the same experiment naming a registered model and a workflow is
   admitted through the line too: the server stages the model's package from
   its own registry, the job sends the weights and the workflow's declaration
   by digest, and the job passes — while a job that forgot the weights is an
   error naming the member and the route to send it to;
8. a variant naming a model this deployment never registered is admitted too,
   by the digest of its own package: a job that sends the weights and not the
   package is an error naming `model-package.json`, and one that sends both
   passes, its approval carrying the package's digest.

    cargo build --bin aiwatcher   # once
    just e2e-gate
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

ROOT = Path(__file__).resolve().parent.parent
EXAMPLE = ROOT / "examples" / "ci-gate"
BINARY = Path(os.environ.get("AIWATCHER_BINARY", ROOT / "target" / "debug" / "aiwatcher"))
READY_WITHIN = 30.0
RUN_WITHIN = 90.0
BASE = ""
TERMINAL = {"completed", "failed", "cancelled", "crashed"}

sys.path.insert(0, str(ROOT / "sdk" / "python"))

from aiwatcher_sdk.evaluation_registry import EvaluationRegistry  # noqa: E402
from aiwatcher_sdk.prompts import PromptRegistry  # noqa: E402


def serve(home: Path) -> subprocess.Popen[bytes]:
    global BASE
    if not BINARY.exists():
        raise SystemExit(f"no server binary at {BINARY}: `cargo build --bin aiwatcher`")
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
    home.mkdir(parents=True)
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
        if call("GET", "/readyz")[0] == 200 and call("GET", "/api/v1/prompts")[0] == 200:
            return process
        time.sleep(0.2)
    process.kill()
    raise SystemExit(
        f"aiwatcher did not come up on {BASE}:\n{(home / 'server.log').read_text()[-2000:]}"
    )


def call(method: str, path: str, body: Any = None) -> tuple[int, Any]:
    data = None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(BASE + path, data=data, method=method)  # noqa: S310 — our own server
    request.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(request, timeout=15) as response:  # noqa: S310 — our own server
            content, status = response.read(), response.status
    except urllib.error.HTTPError as error:
        content, status = error.read(), error.code
    except (urllib.error.URLError, ConnectionError):
        return 0, None
    try:
        return status, json.loads(content) if content else None
    except json.JSONDecodeError:
        return status, None


def ok(status: int, body: Any, what: str) -> Any:
    if status not in (200, 201, 202, 204):
        raise SystemExit(f"{what} answered {status}: {json.dumps(body)[:600]}")
    return body


def setup(home: Path) -> dict[str, Any]:
    """The admin's once: cases, card, prompt, the baseline's declaration, the line."""
    cases = json.loads((EXAMPLE / "cases.json").read_text())
    capitals = {"France": "Paris", "Japan": "Tokyo", "Peru": "Lima", "Kenya": "Nairobi"}
    rows = [
        {
            "case_id": case["case_id"],
            "input": case["input"],
            "expected": {
                "answer": capitals[
                    case["input"]["question"]
                    .removeprefix("What is the capital of ")
                    .removesuffix("?")
                ]
            },
        }
        for case in cases
    ]
    published = ok(
        *call(
            "POST",
            "/api/v1/datasets",
            {
                "name": "capitals",
                "pipeline": "data_frame()->read(capitals)",
                "columns": ["case_id", "input", "expected"],
                "items": rows,
                "source": hashlib.sha256(json.dumps(rows).encode()).hexdigest(),
            },
        ),
        "publishing the cases",
    )
    dataset = {
        "kind": "curation",
        "name": "capitals",
        "version": published["dataset"]["latest"]["version"],
    }
    cohort = ok(
        *call("POST", "/api/v1/evaluation-cohorts", {"dataset": dataset, "split": "test"}),
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
                        "answer_path": "/text",
                        "expected_path": "/answer",
                        "scorer": {"kind": "exact_match", "trim": True},
                    }
                ],
            },
        ),
        "publishing the card",
    )
    with PromptRegistry(BASE) as prompts:
        prompt = prompts.publish(
            "capitals.app", "Name the capital of {{ country }}.", author="e2e"
        ).version_id
    return {
        "evaluation_id": "capitals-main",
        "repetition_id": "measurement-1",
        "variant": {
            "schema_version": 1,
            "experiment_id": "capitals-app",
            "dataset": dataset,
            "prompt": {"name": "capitals.app", "version": prompt},
            "code": {},
            "generation_config": {},
        },
        "cohort": cohort["cohort"],
        "scorecard": {"name": card["scorecard"]["name"], "version": card["version"]},
        "answers": {},
    }


def job(
    home: Path,
    run: dict[str, Any],
    *,
    commit: str,
    style: str,
    evaluation_id: str,
    drop: str | None = None,
    staged: tuple[Path, ...] = (),
) -> tuple[int, dict[str, Any], str]:
    """One CI job: answer every case, then `aiwatcher-gate`."""
    work = home / commit
    work.mkdir()
    subprocess.run(  # noqa: S603 — the example this repository ships
        [
            sys.executable,
            str(EXAMPLE / "app.py"),
            str(EXAMPLE / "cases.json"),
            str(work / "answers.json"),
            "--style",
            style,
        ],
        check=True,
    )
    if drop is not None:
        answers = json.loads((work / "answers.json").read_text())
        answers["answers"] = [answer for answer in answers["answers"] if answer["case_id"] != drop]
        (work / "answers.json").write_text(json.dumps(answers))
    (work / "run.json").write_text(json.dumps(run))
    (work / "generation.json").write_text(json.dumps({"style": style}))
    summary = work / "summary.md"
    env = dict(os.environ) | {
        "AIWATCHER_URL": BASE,
        "GITHUB_SHA": commit,
        "GITHUB_REPOSITORY": "example/capitals",
        "GITHUB_STEP_SUMMARY": str(summary),
        "PYTHONPATH": str(ROOT / "sdk" / "python"),
    }
    done = subprocess.run(  # noqa: S603 — the SDK's own entry point
        [
            sys.executable,
            "-m",
            "aiwatcher_sdk.gate",
            "--run",
            str(work / "run.json"),
            "--evaluation-id",
            evaluation_id,
            "--baseline",
            "capitals-main",
            "--policy",
            str(EXAMPLE / "policy.json"),
            "--code-commit",
            "--artifact",
            f"generation_config={work / 'generation.json'}",
            "--recording",
            str(work / "answers.json"),
            *[argument for path in staged for argument in ("--stage", str(path))],
            "--output",
            str(work / "gate.json"),
            "--timeout",
            str(RUN_WITHIN),
        ],
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )
    decision = json.loads((work / "gate.json").read_text()) if (work / "gate.json").exists() else {}
    return (
        done.returncode,
        decision,
        summary.read_text() if summary.exists() else done.stdout + done.stderr,
    )


def main() -> int:
    failures: list[str] = []

    def check(number: int, claim: str, holds: bool, detail: object = "") -> None:
        print(
            f"  {'✓' if holds else '✗'} {number}. {claim}"
            + (f" — {detail}" if detail not in ("", None) else "")
        )
        if not holds:
            failures.append(claim)

    home = Path(tempfile.mkdtemp(prefix="aiwatcher-e2e-gate-"))
    server = serve(home / "server")
    try:
        run = setup(home)
        with EvaluationRegistry(BASE) as registry:
            # The baseline's own declaration is the line's template: its context
            # and its experiment are what the admin admits, once.
            code = registry.stage_variant_artifact("commit.json", b'{"commit":"main"}')
            config = registry.stage_variant_artifact("generation.json", b'{"style":"terse-south"}')
            template = dict(run) | {
                "variant": dict(run["variant"]) | {"code": code, "generation_config": config}
            }
            app = home / "main"
            app.mkdir()
            subprocess.run(  # noqa: S603 — the example this repository ships
                [
                    sys.executable,
                    str(EXAMPLE / "app.py"),
                    str(EXAMPLE / "cases.json"),
                    str(app / "answers.json"),
                    "--style",
                    "terse-south",
                ],
                check=True,
            )
            template["answers"] = registry.stage_recording(
                "answers.json", json.loads((app / "answers.json").read_text())["answers"]
            )
            view = registry.declare_scoring_run(template)  # type: ignore[arg-type]
            line = registry.admit_line(view["manifest"])
            started = registry.start_scoring_run(view["declaration"]["id"])
            deadline = time.monotonic() + RUN_WITHIN
            while (
                registry.get_execution(started["execution"]["execution_id"])["execution"]["state"][
                    "state_type"
                ]
                not in TERMINAL
            ):
                if time.monotonic() > deadline:
                    raise SystemExit("the baseline did not finish")
                time.sleep(0.2)
            approvals = ok(*call("GET", "/api/v1/evaluation-approvals"), "listing the approvals")
        baseline = call("GET", "/api/v1/evaluation-results/capitals-main")
        named = [
            a
            for a in approvals["approvals"]
            if line["record"]["line_id"] in a["record"]["approved_by"]
        ]
        print("\na line, a baseline and four jobs:")
        check(
            1,
            "the baseline is admitted through the line from bytes the job staged",
            baseline[0] == 200 and baseline[1]["metrics"].get("exact") == 0.5 and len(named) == 1,
            {
                "exact": (baseline[1] or {}).get("metrics"),
                "approved_by": [a["record"]["approved_by"] for a in named],
            },
        )

        same, decided, _ = job(
            home, run, commit="c1", style="terse-south", evaluation_id="capitals-c1"
        )
        check(
            2,
            "the same answers at a new commit pass",
            same == 0 and decided.get("verdict") == "pass",
            decided.get("reasons"),
        )

        broke, decided, _ = job(
            home, run, commit="c2", style="confused", evaluation_id="capitals-c2"
        )
        metric = next(
            (m for m in (decided.get("decision") or {}).get("metrics", []) if m["name"] == "exact"),
            {},
        )
        check(
            3,
            "a lost critical case is a regression though the mean rose",
            broke == 1
            and decided.get("verdict") == "regression"
            and (metric.get("delta") or 0) > 0,
            {"delta": metric.get("delta"), "reasons": decided.get("reasons")},
        )

        missing, decided, _ = job(
            home,
            run,
            commit="c3",
            style="one-word",
            evaluation_id="capitals-c3",
            drop="capital-peru",
        )
        check(
            4,
            "a case nobody answered never passes",
            missing == 2 and decided.get("verdict") == "incomplete",
            decided.get("reasons"),
        )

        other = json.loads(json.dumps(run))
        other["variant"]["experiment_id"] = "capitals-rewrite"
        refused, decided, _ = job(
            home, other, commit="c4", style="one-word", evaluation_id="capitals-c4"
        )
        check(
            5,
            "a variant the line does not cover is an error naming the line route",
            refused == 3
            and any("evaluation-approval-lines" in reason for reason in decided.get("reasons", [])),
            decided.get("reasons"),
        )

        _, _, text = job(home, run, commit="c5", style="one-word", evaluation_id="capitals-c5")
        check(
            6,
            "the job summary names the commit, the card, the variant and the evidence",
            "`c5`" in text
            and "capitals-exact" in text
            and "- variant: `" in text
            and "evaluation?evidence=capitals-c5" in text,
            text.splitlines()[0] if text else "",
        )

        # A model this deployment's registry holds, and a workflow declaration.
        weights = home / "weights"
        weights.write_bytes(b"the capitals model's weights\n")
        declaration = home / "workflow.json"
        declaration.write_bytes(b'{"name":"capitals-app","steps":["answer"]}')
        ok(
            *call(
                "POST",
                "/api/v1/training-runs",
                {"run_id": "capitals-train", "model": "capitals", "dataset": "capitals@abc"},
            ),
            "opening the training run",
        )
        registered = ok(
            *call(
                "POST",
                "/api/v1/models",
                {
                    "name": "capitals",
                    "run_id": "capitals-train",
                    "checkpoint_uri": "s3://models/capitals",
                    "package": {
                        "runtime": "weights",
                        "artifacts": [
                            {
                                "name": "weights",
                                "uri": "s3://models/capitals.bin",
                                "digest": hashlib.sha256(weights.read_bytes()).hexdigest(),
                                "size_bytes": len(weights.read_bytes()),
                                "content_type": "",
                                "kind": "model",
                            }
                        ],
                    },
                },
            ),
            "registering the model",
        )
        modelled = json.loads(json.dumps(run))
        modelled["variant"]["model"] = {
            "name": "capitals",
            "version": registered["version"]["version"],
        }
        modelled["variant"]["workflow"] = {
            "name": "capitals-app",
            "version": hashlib.sha256(declaration.read_bytes()).hexdigest(),
        }
        forgot, told, _ = job(
            home,
            modelled,
            commit="c6",
            style="terse-south",
            evaluation_id="capitals-c6",
            staged=(declaration,),
        )
        passed, decided, _ = job(
            home,
            modelled,
            commit="c7",
            style="terse-south",
            evaluation_id="capitals-c7",
            staged=(weights, declaration),
        )
        approvals = ok(*call("GET", "/api/v1/evaluation-approvals"), "listing the approvals")
        # The model variant's own approval: named by the line, and carrying the
        # digest of the package the server staged from its registry.
        through = [
            approval["record"]
            for approval in approvals["approvals"]
            if approval["record"]["variant_id"] == decided.get("variant_id")
            and line["record"]["line_id"] in approval["record"]["approved_by"]
        ]
        check(
            7,
            "a variant naming a model and a workflow is admitted through the line too",
            forgot == 3
            and any(
                "model-artifacts/weights" in reason
                and "evaluation-variant-artifacts/weights" in reason
                for reason in told.get("reasons", [])
            )
            and passed == 0
            and decided.get("verdict") == "pass"
            and len(through) == 1
            and through[0].get("bundle_digest") is not None,
            {"forgot": told.get("reasons"), "passed": decided.get("reasons"), "through": through},
        )

        # A model nobody here registered: its version is its package's digest.
        package = home / "model-package.json"
        package.write_bytes(
            json.dumps(
                {
                    "runtime": "weights",
                    "artifacts": [
                        {
                            "name": "weights",
                            "uri": "s3://elsewhere/capitals.bin",
                            "digest": hashlib.sha256(weights.read_bytes()).hexdigest(),
                            "size_bytes": len(weights.read_bytes()),
                            "content_type": "",
                            "kind": "model",
                        }
                    ],
                }
            ).encode()
        )
        elsewhere = json.loads(json.dumps(run))
        elsewhere["variant"]["model"] = {
            "name": "capitals-elsewhere",
            "version": hashlib.sha256(package.read_bytes()).hexdigest(),
        }
        unsent, unsent_told, _ = job(
            home,
            elsewhere,
            commit="c8",
            style="terse-south",
            evaluation_id="capitals-c8",
            staged=(weights,),
        )
        sent, sent_decided, _ = job(
            home,
            elsewhere,
            commit="c9",
            style="terse-south",
            evaluation_id="capitals-c9",
            staged=(package, weights),
        )
        approvals = ok(*call("GET", "/api/v1/evaluation-approvals"), "listing the approvals")
        addressed = [
            approval["record"]
            for approval in approvals["approvals"]
            if approval["record"]["variant_id"] == sent_decided.get("variant_id")
        ]
        check(
            8,
            "a variant naming a model nobody here registered is admitted by its package's digest",
            unsent == 3
            and any("model-package.json" in reason for reason in unsent_told.get("reasons", []))
            and sent == 0
            and sent_decided.get("verdict") == "pass"
            and len(addressed) == 1
            and addressed[0].get("bundle_digest") is not None,
            {
                "unsent": unsent_told.get("reasons"),
                "sent": sent_decided.get("reasons"),
                "approval": addressed,
            },
        )
    finally:
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
    print("\n✓ a line admitted every commit's variant, and the gate exited by the server's verdict")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
