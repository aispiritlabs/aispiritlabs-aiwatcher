from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import httpx

from aiwatcher_sdk.evaluation_registry import EvaluationRegistry
from aiwatcher_sdk.gate import Outcome, commit_note, run_gate, summary

DECISION = {
    "verdict": "regression",
    "reasons": ["critical case capital-kenya: it is worse than the baseline on a metric"],
    "comparability": "comparable",
    "candidate": {
        "evaluation_id": "answers-abc",
        "version": "v" * 64,
        "state": "complete",
        "suite": {"name": "capitals-exact", "version": "s" * 64},
    },
    "baseline": {"evaluation_id": "answers-main", "version": "b" * 64, "state": "complete"},
    "metrics": [
        {
            "name": "exact",
            "direction": "higher",
            "current": 0.75,
            "baseline": 0.5,
            "delta": 0.25,
            "held": True,
            "regressed": False,
        }
    ],
    "critical": [{"case_id": "capital-kenya", "change": "regressed", "held": False}],
}


def server(*, admitted: bool = True) -> tuple[list[tuple[str, str, Any]], httpx.MockTransport]:
    seen: list[tuple[str, str, Any]] = []
    polled = {"count": 0}

    def handle(request: httpx.Request) -> httpx.Response:
        body: Any = None
        if request.content and request.headers.get("content-type") == "application/json":
            body = json.loads(request.content)
        elif request.content:
            body = request.content
        seen.append((request.method, request.url.path, body))
        path = request.url.path
        if path.startswith("/api/v1/evaluation-variant-artifacts/"):
            name = path.rsplit("/", 1)[1]
            return httpx.Response(200, json={"name": name, "uri": "x", "digest": "c" * 64})
        if path.startswith("/api/v1/evaluation-recordings/"):
            return httpx.Response(
                200, json={"name": "answers.json", "uri": "y", "digest": "a" * 64}
            )
        if path == "/api/v1/evaluation-runs":
            return httpx.Response(
                200, json={"declaration": {"id": "d" * 64}, "variant_id": "e" * 64}
            )
        if path.endswith("/start"):
            if not admitted:
                return httpx.Response(
                    409,
                    json={"code": "pair_not_admitted", "message": "approval " + "f" * 64},
                )
            return httpx.Response(202, json={"execution": {"execution_id": "exec-1"}})
        if path == "/api/v1/executions/exec-1":
            polled["count"] += 1
            state = "running" if polled["count"] == 1 else "completed"
            return httpx.Response(200, json={"execution": {"state": {"state_type": state}}})
        if path == "/api/v1/evaluation-results/answers-abc/gate":
            return httpx.Response(200, json=DECISION)
        return httpx.Response(404, json={"code": "not_found", "message": path})

    return seen, httpx.MockTransport(handle)


def run() -> dict[str, Any]:
    return {
        "evaluation_id": "answers-abc",
        "repetition_id": "measurement-1",
        "variant": {"experiment_id": "capitals", "code": {}, "generation_config": {}},
        "answers": {},
    }


def gate(
    tmp_path: Path,
    *,
    admitted: bool = True,
    policy: dict[str, Any] | None = None,
    declared: dict[str, Any] | None = None,
) -> tuple[Outcome, list[tuple[str, str, Any]]]:
    seen, transport = server(admitted=admitted)
    recording = tmp_path / "answers.json"
    recording.write_text(
        json.dumps({"answers": [{"case_id": "capital-kenya", "answer": "Nairobi"}]})
    )
    config = tmp_path / "generation.json"
    config.write_text('{"temperature": 0}')
    weights = tmp_path / "weights"
    weights.write_bytes(b"\x00\x01")
    with EvaluationRegistry(
        "http://aiwatcher.invalid", client=httpx.Client(transport=transport)
    ) as registry:
        outcome = run_gate(
            registry,
            declared or run(),
            baseline="answers-main",
            policy=policy or {"critical_cases": ["capital-kenya"]},
            artifacts={"generation_config": config},
            recording=recording,
            staged=[weights],
            commit="abc123",
            repository="example/app",
            code_commit=True,
            panel_url="https://aiwatcher.example",
            timeout=60,
            sleep=lambda _: None,
        )
    return outcome, seen


def test_a_critical_case_lost_exits_as_a_regression_with_the_commit_card_and_evidence(
    tmp_path: Path,
) -> None:
    outcome, seen = gate(tmp_path)

    assert outcome.verdict == "regression"
    assert outcome.exit_code == 1
    assert outcome.evidence == "https://aiwatcher.example/evaluation?evidence=answers-abc"
    staged = [(method, path) for method, path, _ in seen if method == "PUT"]
    assert staged == [
        ("PUT", "/api/v1/evaluation-variant-artifacts/commit.json"),
        ("PUT", "/api/v1/evaluation-variant-artifacts/generation.json"),
        ("PUT", "/api/v1/evaluation-variant-artifacts/weights"),
        ("PUT", "/api/v1/evaluation-recordings/answers.json"),
    ], "a model's weights are sent by digest, pinning nothing in the declaration"
    commit = next(body for _, path, body in seen if path.endswith("/commit.json"))
    assert commit == commit_note("example/app", "abc123")
    declared = next(body for _, path, body in seen if path == "/api/v1/evaluation-runs")
    assert declared["variant"]["code"]["name"] == "commit.json", "the pin the staging answered"
    gated = next(body for _, path, body in seen if path.endswith("/gate"))
    assert gated == {"baseline": "answers-main", "policy": {"critical_cases": ["capital-kenya"]}}

    text = summary(outcome)
    assert text.startswith("## aiwatcher gate: regression")
    assert "- commit: `abc123`" in text
    assert f"- variant: `{'e' * 64}`" in text, "what the deployed application names on its runs"
    assert "- card: `capitals-exact` @ `ssssssssssss`" in text
    assert "| exact | 0.75 | 0.5 | 0.25 | yes |" in text
    assert "critical case capital-kenya" in text


def test_a_variant_nothing_admits_is_an_error_that_names_the_line(tmp_path: Path) -> None:
    outcome, _ = gate(tmp_path, admitted=False)

    assert outcome.verdict == "error"
    assert outcome.exit_code == 3
    assert "evaluation-approval-lines" in outcome.reasons[0]


def test_a_policy_holding_results_to_a_lookback_declares_the_run_with_at_least_as_much(
    tmp_path: Path,
) -> None:
    declared = run() | {"settings": {"asked_since_seconds": 60, "timeout_seconds": 600}}
    _, seen = gate(
        tmp_path,
        policy={"require_witnessed_answer": True, "asked_since_seconds": 3600},
        declared=declared,
    )
    [body] = [body for method, path, body in seen if path == "/api/v1/evaluation-runs"]
    assert body["settings"] == {"asked_since_seconds": 3600, "timeout_seconds": 600}
