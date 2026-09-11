import json
from pathlib import Path
from typing import cast

import httpx
import pytest

from aiwatcher_sdk.evaluation import EvaluationManifest
from aiwatcher_sdk.evaluation_registry import EvaluationRegistry, EvaluationRegistryError


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
