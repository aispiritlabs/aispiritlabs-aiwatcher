"""The registry client, against a stub of the real API.

A stub rather than a mock of `urllib`: what is worth testing is that the client
builds the request the Rust side accepts and reads the body it sends back, and
a mock that asserts on `urlopen` calls would pass while sending nonsense.
"""

from __future__ import annotations

import json
import threading
from collections.abc import Iterator
from http.server import BaseHTTPRequestHandler, HTTPServer
from typing import Any, ClassVar

import pytest

from aiwatcher_sdk.prompts import (
    PromptRegistry,
    PromptVersion,
    RegistryError,
    Score,
    scores,
    variables_of,
    version_id_of,
)

BASELINE = "Describe the floor plan on {{ page }} in {{ language }}."


class _Recorder(BaseHTTPRequestHandler):
    """Answers whatever the test put in `responses`, and records the requests."""

    # Class-level because `http.server` constructs a handler per request, so
    # there is no instance for a test to reach into.
    # Not `responses`: `BaseHTTPRequestHandler` already has one, holding the
    # status-line table.
    stubbed: ClassVar[dict[tuple[str, str], tuple[int, dict[str, Any]]]] = {}
    seen: ClassVar[list[dict[str, Any]]] = []

    def log_message(self, *args: Any) -> None:
        return

    def _handle(self, method: str) -> None:
        length = int(self.headers.get("content-length") or 0)
        raw = self.rfile.read(length) if length else b""
        type(self).seen.append(
            {
                "method": method,
                "path": self.path,
                "body": json.loads(raw) if raw else None,
            }
        )
        status, body = type(self).stubbed.get(
            (method, self.path.split("?")[0]), (404, {"code": "not_found", "message": "no stub"})
        )
        payload = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def do_GET(self) -> None:
        self._handle("GET")

    def do_POST(self) -> None:
        self._handle("POST")

    def do_PUT(self) -> None:
        self._handle("PUT")


@pytest.fixture
def api() -> Iterator[tuple[PromptRegistry, type[_Recorder]]]:
    _Recorder.stubbed = {}
    _Recorder.seen = []
    server = HTTPServer(("127.0.0.1", 0), _Recorder)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield PromptRegistry(f"http://127.0.0.1:{server.server_port}"), _Recorder
    finally:
        server.shutdown()
        server.server_close()


def version_body(text: str, **overrides: Any) -> dict[str, Any]:
    body = {
        "name": "planner.floor-plan",
        "version_id": version_id_of(text),
        "text": text,
        "created_at": "2026-08-28T10:00:00Z",
        "variables": variables_of(text),
        "origin": "authored",
    }
    body.update(overrides)
    return body


def test_a_version_id_is_the_sha256_the_server_computes() -> None:
    # The two sides have to agree, or the same prompt exists under two ids.
    assert version_id_of("test") == (
        "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
    )
    assert version_id_of(BASELINE) != version_id_of(BASELINE + " ")


def test_variables_are_read_from_the_text_the_way_the_server_reads_them() -> None:
    assert variables_of(BASELINE) == ["language", "page"]
    assert variables_of("{{ 1 + 2 }} {{{ raw }}} {{ unclosed") == []


def test_rendering_refuses_a_partial_substitution() -> None:
    # A missing value would ship a prompt with a literal `{{ page }}` in it,
    # which the model reads as an instruction and nothing catches.
    version = PromptVersion.from_json(version_body(BASELINE))
    assert version.render(page="p-1", language="pl").startswith("Describe the floor plan on p-1")

    with pytest.raises(KeyError, match="page"):
        version.render(language="pl")
    with pytest.raises(KeyError, match="tone"):
        version.render(page="p-1", language="pl", tone="formal")


def test_publishing_sends_only_the_fields_that_were_given(
    api: tuple[PromptRegistry, type[_Recorder]],
) -> None:
    registry, recorder = api
    recorder.stubbed[("POST", "/api/v1/prompts")] = (
        201,
        {"version": version_body(BASELINE, author="ci"), "created": True, "head": {}},
    )

    version = registry.publish("planner.floor-plan", BASELINE, author="ci", label="production")

    assert version.version_id == version_id_of(BASELINE)
    assert version.variables == ("language", "page")
    body = recorder.seen[-1]["body"]
    assert body == {
        "name": "planner.floor-plan",
        "text": BASELINE,
        "author": "ci",
        "label": "production",
    }, "an unset field is absent rather than null — the server treats the two differently"


def test_resolve_reads_the_current_version_in_one_request(
    api: tuple[PromptRegistry, type[_Recorder]],
) -> None:
    registry, recorder = api
    recorder.stubbed[("GET", "/api/v1/prompts/planner.floor-plan")] = (
        200,
        {"head": {"labels": {}}, "current": version_body(BASELINE)},
    )

    version = registry.resolve("planner.floor-plan")

    assert version.text == BASELINE
    assert [request["path"] for request in recorder.seen] == ["/api/v1/prompts/planner.floor-plan"]


def test_resolving_a_label_nobody_moved_is_an_error_not_a_silent_fallback(
    api: tuple[PromptRegistry, type[_Recorder]],
) -> None:
    # Falling back to the newest version here would deploy an unreviewed prompt
    # to a service that explicitly asked for `staging`.
    registry, recorder = api
    recorder.stubbed[("GET", "/api/v1/prompts/planner.floor-plan")] = (
        200,
        {"head": {"labels": {}}, "current": version_body(BASELINE)},
    )

    with pytest.raises(RegistryError) as raised:
        registry.resolve("planner.floor-plan", label="staging")
    assert raised.value.status == 404


def test_recording_an_optimisation_sends_both_splits(
    api: tuple[PromptRegistry, type[_Recorder]],
) -> None:
    registry, recorder = api
    candidate = "Read {{ page }} closely; describe every room in {{ language }}."
    recorder.stubbed[("POST", "/api/v1/prompts/planner.floor-plan/optimizations")] = (
        201,
        {
            "optimization_id": "opt-1",
            "prompt": "planner.floor-plan",
            "algorithm": "deepeval/SIMBA",
            "baseline": version_id_of(BASELINE),
            "candidate": version_id_of(candidate),
            "primary_metric": "mean_score",
            "outcome": "admitted",
            "dev": [{"metric": "mean_score", "baseline": 0.61, "candidate": 0.79}],
            "test": [{"metric": "mean_score", "baseline": 0.60, "candidate": 0.67}],
        },
    )

    record = registry.record_optimization(
        "planner.floor-plan",
        algorithm="deepeval/SIMBA",
        baseline=version_id_of(BASELINE),
        candidate_text=candidate,
        primary_metric="mean_score",
        dev=scores({"mean_score": 0.61}, {"mean_score": 0.79}),
        test=scores({"mean_score": 0.60}, {"mean_score": 0.67}),
        promote=True,
    )

    assert record.admitted
    assert record.reason is None
    # 0.18 on dev against 0.07 held out.
    assert record.overfit_gap == pytest.approx(0.11)

    body = recorder.seen[-1]["body"]
    assert body["dev"] == [{"metric": "mean_score", "baseline": 0.61, "candidate": 0.79}]
    assert body["test"] == [{"metric": "mean_score", "baseline": 0.60, "candidate": 0.67}]
    assert body["promote"] is True


def test_a_rejection_carries_the_reason_rather_than_raising(
    api: tuple[PromptRegistry, type[_Recorder]],
) -> None:
    # A refused promotion is a result, not an error: the caller decides whether
    # to fail the build over it, and either way the experiment is recorded.
    registry, recorder = api
    recorder.stubbed[("POST", "/api/v1/prompts/planner.floor-plan/optimizations")] = (
        201,
        {
            "optimization_id": "opt-2",
            "prompt": "planner.floor-plan",
            "algorithm": "deepeval/SIMBA",
            "baseline": version_id_of(BASELINE),
            "candidate": version_id_of("x"),
            "primary_metric": "mean_score",
            "outcome": "rejected",
            "reason": "variables_lost",
            "variables_lost": ["page"],
        },
    )

    record = registry.record_optimization(
        "planner.floor-plan",
        algorithm="deepeval/SIMBA",
        baseline=version_id_of(BASELINE),
        candidate_text="x",
        primary_metric="mean_score",
    )

    assert not record.admitted
    assert record.reason == "variables_lost"
    assert record.variables_lost == ("page",)


def test_an_instance_without_a_registry_says_which_variable_is_unset(
    api: tuple[PromptRegistry, type[_Recorder]],
) -> None:
    registry, recorder = api
    recorder.stubbed[("GET", "/api/v1/prompts/planner.floor-plan")] = (
        501,
        {"code": "registry_disabled", "message": "this instance has no prompt registry"},
    )

    with pytest.raises(RegistryError) as raised:
        registry.get("planner.floor-plan")

    assert raised.value.code == "registry_disabled"
    assert "AIWATCHER_PROMPT_STORE" in str(raised.value)
    assert not raised.value.is_retryable


def test_an_unreachable_registry_raises_rather_than_returning_nothing() -> None:
    # The whole difference from the telemetry transport, which swallows this.
    registry = PromptRegistry("http://127.0.0.1:1", timeout=0.5)
    with pytest.raises(RegistryError) as raised:
        registry.get("planner.floor-plan")
    assert raised.value.is_retryable


def test_a_metric_only_one_side_reported_has_no_delta() -> None:
    paired = scores({"mean_score": 0.6, "gone": 0.4}, {"mean_score": 0.7, "new": 0.9})
    by_metric = {score.metric: score for score in paired}
    assert by_metric["mean_score"].delta == pytest.approx(0.1)
    assert by_metric["gone"].delta is None
    assert by_metric["new"].delta is None
    assert Score("x").as_json() == {"metric": "x"}
