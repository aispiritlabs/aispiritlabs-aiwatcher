import json
from pathlib import Path
from typing import cast

import httpx
import pytest

from aiwatcher_sdk.evaluation import EvaluationManifest
from aiwatcher_sdk.evaluation_registry import (
    CaseTarget,
    EvaluationRegistry,
    EvaluationRegistryError,
)


def manifest() -> EvaluationManifest:
    path = Path(__file__).resolve().parents[3] / "contracts/fixtures/evaluation-v1/manifest.json"
    return cast(EvaluationManifest, json.loads(path.read_text()))


def test_lost_response_retries_the_same_logical_publication() -> None:
    bodies: list[bytes] = []

    def handle(request: httpx.Request) -> httpx.Response:
        bodies.append(request.content)
        if len(bodies) == 1:
            raise httpx.ReadError("response lost", request=request)
        return httpx.Response(200, json={"evaluation_id": "fti-contract-eval-1", "version": "abc"})

    with (
        httpx.Client(transport=httpx.MockTransport(handle)) as http,
        EvaluationRegistry("http://localhost", client=http, attempts=2) as registry,
    ):
        result = registry.publish(manifest(), [])
    assert result["version"] == "abc"
    assert len(bodies) == 2 and bodies[0] == bodies[1]


def test_conflict_raises_and_is_not_retried_or_turned_into_telemetry() -> None:
    calls = 0

    def handle(request: httpx.Request) -> httpx.Response:
        nonlocal calls
        calls += 1
        return httpx.Response(
            409, json={"error": "evaluation_conflict", "message": "different result"}
        )

    with (
        httpx.Client(transport=httpx.MockTransport(handle)) as http,
        EvaluationRegistry("http://localhost", client=http, attempts=3) as registry,
        pytest.raises(EvaluationRegistryError),
    ):
        registry.publish(manifest(), [])
    assert calls == 1


def test_case_page_keeps_version_and_cursor_and_encodes_the_id() -> None:
    def handle(request: httpx.Request) -> httpx.Response:
        assert request.url.raw_path.startswith(
            b"/api/v1/evaluation-results/id%2Fwith%20space/cases?"
        )
        assert request.url.params["version"] == "v1"
        assert request.url.params["cursor"] == "v1:200"
        return httpx.Response(200, json={"version": "v1", "state": "expired", "cases": []})

    with (
        httpx.Client(transport=httpx.MockTransport(handle)) as http,
        EvaluationRegistry("http://localhost", client=http) as registry,
    ):
        assert registry.cases("id/with space", "v1", cursor="v1:200")["state"] == "expired"


def test_a_judge_names_itself_and_a_person_does_not() -> None:
    sent: list[dict[str, object]] = []

    def handle(request: httpx.Request) -> httpx.Response:
        sent.append(cast(dict[str, object], json.loads(request.content)))
        return httpx.Response(200, json={"revision": 1, "standing_id": "s", "target_id": "t"})

    target: CaseTarget = {
        "kind": "case",
        "evaluation_id": "after",
        "case_id": "two-plus-two",
        "repetition_id": "measurement-1",
    }
    with (
        httpx.Client(transport=httpx.MockTransport(handle)) as http,
        EvaluationRegistry("http://localhost", client=http) as registry,
    ):
        registry.assess(target, "helpfulness", {"type": "level", "value": "bad"})
        registry.assess(
            target,
            "helpfulness",
            {"type": "level", "value": "good"},
            source="judge",
            author="gpt-4o@sha256:abc",
            rubric_version="sha-1",
        )

    assert "author" not in sent[0], "a person's judgement is the session's"
    assert sent[0]["source"] == "human"
    assert sent[1]["author"] == "gpt-4o@sha256:abc"
    assert sent[1]["rubric_version"] == "sha-1"


def test_a_lost_judgement_is_sent_again_because_repeating_one_records_nothing() -> None:
    # The server lands a repeat on the revision that already says it, which is
    # what makes this retry safe — and what stops a nightly judge that has not
    # changed its mind from writing a revision a night forever.
    attempts = 0

    def handle(request: httpx.Request) -> httpx.Response:
        nonlocal attempts
        attempts += 1
        if attempts == 1:
            raise httpx.ReadError("response lost", request=request)
        return httpx.Response(200, json={"revision": 1})

    with (
        httpx.Client(transport=httpx.MockTransport(handle)) as http,
        EvaluationRegistry("http://localhost", client=http, attempts=2) as registry,
    ):
        recorded = registry.assess(
            {"kind": "trace", "trace_id": "t-1"},
            "helpfulness",
            {"type": "flag", "value": True},
        )
    assert (attempts, recorded["revision"]) == (2, 1)


def test_a_rubric_is_read_at_the_version_that_will_be_answered_under() -> None:
    def handle(request: httpx.Request) -> httpx.Response:
        assert request.url.raw_path.startswith(b"/api/v1/evaluation-rubrics/team%20helpfulness?")
        assert request.url.params["version"] == "sha-1"
        return httpx.Response(200, json={"version": "sha-1"})

    with (
        httpx.Client(transport=httpx.MockTransport(handle)) as http,
        EvaluationRegistry("http://localhost", client=http) as registry,
    ):
        assert registry.get_rubric("team helpfulness", version="sha-1")["version"] == "sha-1"
