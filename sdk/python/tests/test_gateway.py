"""The gateway: a provider's calls relayed, and witnessed under the gateway's own credential."""

from __future__ import annotations

import json
import threading
import urllib.request
from collections.abc import Generator
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any, ClassVar

import pytest

from aiwatcher_sdk import CALLER_RUN_HEADER, PROMPT_HEADER, AiwatcherClient
from aiwatcher_sdk.api import ApiError
from aiwatcher_sdk.gateway import Gateway, holds_template

TEMPLATE = "Answer the question about {{ country }} in one word."


class Recording:
    def __init__(self) -> None:
        self.events: list[dict[str, Any]] = []

    def send(self, batch: list[dict[str, Any]]) -> None:
        self.events.extend(batch)

    def close(self) -> None:
        return None


class Version:
    def __init__(self, text: str) -> None:
        self.text = text


class Prompts:
    def get_version(self, name: str, version_id: str) -> Version:
        if version_id != "v1":
            raise ApiError("no such version", status=404)
        return Version(TEMPLATE)


class Provider(BaseHTTPRequestHandler):
    """A provider that names the snapshot it served, streamed or not."""

    seen: ClassVar[list[dict[str, Any]]] = []

    def log_message(self, format: str, *args: Any) -> None:
        del format, args

    def do_POST(self) -> None:
        body = json.loads(self.rfile.read(int(self.headers["content-length"])))
        Provider.seen.append({"authorization": self.headers.get("authorization"), **body})
        reply = {
            "model": "capitals-2026-09-01",
            "choices": [{"message": {"role": "assistant", "content": "Lima"}}],
            "usage": {
                "prompt_tokens": 12,
                "completion_tokens": 1,
                "prompt_tokens_details": {"cached_tokens": 4},
            },
        }
        if body.get("stream"):
            self.send_response(200)
            self.send_header("content-type", "text/event-stream")
            self.end_headers()
            for event in (
                {"model": "capitals-2026-09-01", "choices": [{"delta": {"content": "Li"}}]},
                {"model": "capitals-2026-09-01", "choices": [{"delta": {"content": "ma"}}]},
                {"model": "capitals-2026-09-01", "choices": [], "usage": reply["usage"]},
            ):
                self.wfile.write(b"data: " + json.dumps(event).encode() + b"\n\n")
                self.wfile.flush()
            self.wfile.write(b"data: [DONE]\n\n")
            return
        payload = json.dumps(reply).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)


def running(server: ThreadingHTTPServer) -> str:
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return f"http://127.0.0.1:{server.server_address[1]}"


@pytest.fixture
def gateway() -> Generator[tuple[str, Recording], None, None]:
    Provider.seen = []
    provider = ThreadingHTTPServer(("127.0.0.1", 0), Provider)
    recording = Recording()
    relay = Gateway(
        running(provider),
        AiwatcherClient(service="gateway", transport=recording),
        prompts=Prompts(),
        upstream_token="provider-key",
    )
    server = relay.server(port=0)
    try:
        yield running(server), recording
    finally:
        server.shutdown()
        provider.shutdown()


def ask(base: str, body: dict[str, Any], headers: dict[str, str]) -> tuple[int, bytes]:
    request = urllib.request.Request(  # noqa: S310 — the test's own gateway
        base + "/v1/chat/completions", data=json.dumps(body).encode(), method="POST"
    )
    request.add_header("content-type", "application/json")
    for name, value in headers.items():
        request.add_header(name, value)
    with urllib.request.urlopen(request, timeout=10) as response:  # noqa: S310
        return response.status, response.read()


def messages(system: str) -> list[dict[str, str]]:
    return [
        {"role": "system", "content": system},
        {"role": "user", "content": "What is the capital of Peru?"},
    ]


def test_a_relayed_call_is_witnessed_with_what_served_it_and_nothing_that_was_said(
    gateway: tuple[str, Recording],
) -> None:
    base, recording = gateway
    status, body = ask(
        base,
        {"model": "capitals", "messages": messages("Answer the question about Peru in one word.")},
        {CALLER_RUN_HEADER: "app-run", PROMPT_HEADER: "capitals@v1"},
    )

    assert status == 200
    assert json.loads(body)["choices"][0]["message"]["content"] == "Lima"
    assert Provider.seen[0]["authorization"] == "Bearer provider-key", "the gateway's own key"
    started = [event for event in recording.events if event["event_type"] == "run.started"]
    assert started[0]["data"] == {"caller_run_id": "app-run"}
    [completed] = [event for event in recording.events if event["event_type"] == "llm.completed"]
    data = completed["data"]
    assert data["model"] == "capitals"
    assert data["model_version"] == "capitals-2026-09-01"
    assert (data["prompt_name"], data["prompt_version"], data["prompt_verified"]) == (
        "capitals",
        "v1",
        True,
    )
    assert (data["prompt_tokens"], data["completion_tokens"], data["cached_tokens"]) == (12, 1, 4)
    told = json.dumps(recording.events)
    assert "Peru" not in told and "Lima" not in told, "no request and no reply on the log"


def test_a_request_that_does_not_hold_the_named_template_is_witnessed_as_such(
    gateway: tuple[str, Recording],
) -> None:
    base, recording = gateway
    ask(
        base,
        {"model": "capitals", "messages": messages("Answer briefly.")},
        {CALLER_RUN_HEADER: "app-run", PROMPT_HEADER: "capitals@v1"},
    )
    ask(
        base,
        {"model": "capitals", "messages": messages("Answer the question about Peru in one word.")},
        {CALLER_RUN_HEADER: "app-run", PROMPT_HEADER: "capitals@v9"},
    )

    verified = [
        event["data"]["prompt_verified"]
        for event in recording.events
        if event["event_type"] == "llm.completed"
    ]
    assert verified == [False, False], "another text, and a version the registry does not hold"


def test_a_stream_reaches_the_caller_and_what_it_says_about_the_call_is_read(
    gateway: tuple[str, Recording],
) -> None:
    base, recording = gateway
    status, body = ask(
        base,
        {"model": "capitals", "stream": True, "messages": messages("Hello")},
        {CALLER_RUN_HEADER: "app-run"},
    )

    assert status == 200
    assert body.count(b"data: ") == 4
    [completed] = [event for event in recording.events if event["event_type"] == "llm.completed"]
    assert completed["data"]["model_version"] == "capitals-2026-09-01"
    assert completed["data"]["completion_tokens"] == 1
    assert "prompt_verified" not in completed["data"], "a request naming no prompt claims none"


def test_a_template_is_found_with_its_variables_filled_and_a_placeholder_alone_holds_nothing() -> (
    None
):
    filled = messages("Answer the question about Peru in one word.")
    assert holds_template(TEMPLATE, filled)
    assert not holds_template(TEMPLATE, messages("Answer the question about Peru."))
    assert holds_template(
        "{{ system }}\nQ: {{ question }}",
        [{"role": "user", "content": [{"type": "text", "text": "rules\nQ: capital?"}]}],
    )
    assert not holds_template("{{ everything }}", filled)
