"""The gateway: a provider's calls relayed, and witnessed under the gateway's own credential."""

from __future__ import annotations

import json
import threading
import urllib.error
import urllib.request
from collections.abc import Generator
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any, ClassVar

import pytest

from aiwatcher_sdk import CALLER_RUN_HEADER, GATEWAY_FIELD, PROMPT_HEADER, AiwatcherClient
from aiwatcher_sdk.api import ApiError
from aiwatcher_sdk.gateway import (
    Gateway,
    Relayed,
    ToolWitness,
    canonical,
    extracted,
    holds_template,
    normalized,
    witness_digest,
    witness_key,
)

TEMPLATE = "Answer the question about {{ country }} in one word."
JUDGE = "Which is the capital of Peru, {{ first }} or {{ second }}? Say first or second."
KEY = witness_key("gateway-secret")


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
        return Version(JUDGE if name == "pick-best" else TEMPLATE)


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
        credential="gateway-secret",
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


def completed(recording: Recording) -> list[dict[str, Any]]:
    return [event["data"] for event in recording.events if event["event_type"] == "llm.completed"]


def test_a_digest_is_the_bytes_the_deployment_computes() -> None:
    """The vectors ``aiwatcher_core::witness`` holds itself to."""
    key = witness_key("serving-secret")
    assert key.hex() == "b27f733074077db3bb749314c6dbc46fc967028defacac53f83907b0ecf83c15"
    assert witness_digest(key, "replied", " Lima\n") == "424880e43451b760e40c782d90743997"
    assert (
        witness_digest(key, "asked", "What is the capital of Peru?")
        == "8f3cf1564557884b7b44e9bf0077f300"
    )
    assert (
        witness_digest(key, "taking", canonical({"map": {"A": "Lima"}}))
        == "3b1d627f22d3a4b86fcd29126042cd6e"
    )


def test_what_was_asked_and_replied_is_published_as_keyed_digests_and_the_field_goes_no_further(
    gateway: tuple[str, Recording],
) -> None:
    base, recording = gateway
    ask(
        base,
        {
            "model": "capitals",
            "messages": messages("Answer the question about Peru in one word."),
            GATEWAY_FIELD: {"variables": {"country": "Peru"}},
        },
        {CALLER_RUN_HEADER: "app-run", PROMPT_HEADER: "capitals@v1"},
    )

    assert GATEWAY_FIELD not in Provider.seen[0], "the provider is sent the request, not the field"
    [data] = completed(recording)
    assert data["prompt_verified"] is True
    for text in ("What is the capital of Peru?", "Peru"):
        assert witness_digest(KEY, "asked", text) in data["asked_digests"], text
    assert data["replied_digests"] == [witness_digest(KEY, "replied", "Lima")]
    assert witness_digest(witness_key("another"), "replied", "Lima") not in data["replied_digests"]


def test_values_the_request_was_not_rendered_with_are_not_vouched_for(
    gateway: tuple[str, Recording],
) -> None:
    base, recording = gateway
    ask(
        base,
        {
            "model": "capitals",
            "messages": messages("Answer the question about Peru in one word."),
            GATEWAY_FIELD: {"variables": {"country": "Kenya"}},
        },
        {CALLER_RUN_HEADER: "app-run", PROMPT_HEADER: "capitals@v1"},
    )

    [data] = completed(recording)
    assert data["prompt_verified"] is True, "its literal parts are there"
    assert witness_digest(KEY, "asked", "Kenya") not in data["asked_digests"]


def test_a_streamed_reply_is_digested_as_the_text_it_adds_up_to(
    gateway: tuple[str, Recording],
) -> None:
    base, recording = gateway
    ask(
        base,
        {"model": "capitals", "stream": True, "messages": messages("Hello")},
        {CALLER_RUN_HEADER: "app-run"},
    )

    [data] = completed(recording)
    assert data["replied_digests"] == [witness_digest(KEY, "replied", "Lima")]


def test_a_number_is_spelled_the_one_way_both_languages_write_it() -> None:
    """The vectors ``aiwatcher_core::witness`` holds itself to."""
    for value, spelled in [
        (0.1, "0.1"),
        (1e21, "1e+21"),
        (1e-7, "1e-7"),
        (123456789.125, "123456789.125"),
        (-0.0, "0"),
        (1.0, "1"),
        (5e-324, "5e-324"),
        (1.7976931348623157e308, "1.7976931348623157e+308"),
        (100.0, "100"),
        (1e20, "100000000000000000000"),
        (0.000001, "0.000001"),
        (1.23e-18, "1.23e-18"),
        (-3.75e-8, "-3.75e-8"),
        (42, "42"),
        (-7, "-7"),
        (2**64 - 1, "18446744073709551615"),
        (2**64, "18446744073709551616"),
        (-(10**30) - 1, "-1000000000000000000000000000001"),
    ]:
        assert canonical(value) == spelled, value
    assert canonical({"score": 0.5, "labels": ["a", 2.0]}) == '{"labels":["a",2],"score":0.5}'


def test_an_answer_is_taken_out_of_a_reply_by_a_pointer_or_between_two_markers() -> None:
    reply = '{"label": "B", "scores": [0.25, 1.0]}'
    assert extracted(reply, {"json_pointer": "/label"}) == "B"
    assert extracted(reply, {"json_pointer": "/scores"}) == "[0.25,1]"
    assert extracted(reply, {"json_pointer": "/missing"}) is None
    assert extracted("Reasoning...\nAnswer: Lima\nDone", {"between": ["Answer:", "\n"]}) == " Lima"
    assert extracted("Answer: Lima", {"between": ["Answer:", None]}) == " Lima"
    assert extracted("no marker", {"between": ["Answer:", None]}) is None
    assert extracted("anything", {"regex": ".*"}) is None, "a rule it does not know takes nothing"


def test_steps_take_an_answer_as_an_application_parses_one_and_alternatives_the_first_found() -> (
    None
):
    reasoned = (
        "Let me think.\nThe answer is below.\n```json\n"
        '{"capital": "Lima", "confidence": 0.90}\n```\nAnswer: "LIMA".\nFinal answer: Lima'
    )
    assert extracted(reasoned, {"steps": [{"fenced": "json"}, {"json_pointer": "/capital"}]}) == (
        "Lima"
    )
    assert extracted(reasoned, {"steps": [{"fenced": None}, {"json_pointer": "/confidence"}]}) == (
        "0.9"
    )
    assert (
        extracted(
            reasoned,
            {"steps": [{"between": ["Answer:", "\n"]}, {"strip": "\".'"}, {"lower": True}]},
        )
        == "lima"
    )
    assert extracted(reasoned, {"after_last": "ANSWER:"}) is None, "markers are matched as written"
    assert extracted(reasoned, {"after_last": "Final answer:"}) == " Lima"
    assert extracted(reasoned, {"line": -1}) == "Final answer: Lima"
    assert extracted("Total\n  3.50  ", {"steps": [{"line": 1}, {"number": True}]}) == "3.5"
    assert extracted("NaN", {"number": True}) is None
    assert extracted("up to here\nrest", {"between": [None, "\n"]}) == "up to here"
    assert (
        extracted(
            "Answer: Lima",
            {
                "first_of": [
                    {"steps": [{"fenced": "json"}, {"json_pointer": "/capital"}]},
                    {"between": ["Answer:", None]},
                ]
            },
        )
        == " Lima"
    )
    assert extracted(reasoned, {"steps": [{"fenced": "yaml"}]}) is None
    assert extracted(reasoned, {"json_pointer": "/x", "between": ["a", None]}) is None, (
        "a step is one rule, never two at once"
    )
    assert extracted(reasoned, {"steps": [{"line": 0}] * 17}) is None, "at most sixteen steps"


class Explaining(Provider):
    """A provider that reasons before it answers."""

    def do_POST(self) -> None:
        body = json.loads(self.rfile.read(int(self.headers["content-length"])))
        Provider.seen.append(body)
        payload = json.dumps(
            {
                "model": "capitals-2026-09-01",
                "choices": [{"message": {"content": "It is in the Andes.\nAnswer: Lima"}}],
            }
        ).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)


def test_an_answer_taken_out_of_the_reply_is_digested_and_extra_words_in_the_request_are_said() -> (
    None
):
    Provider.seen = []
    provider = ThreadingHTTPServer(("127.0.0.1", 0), Explaining)
    recording = Recording()
    relay = Gateway(
        running(provider),
        AiwatcherClient(service="gateway", transport=recording),
        prompts=Prompts(),
        credential="gateway-secret",
    )
    server = relay.server(port=0)
    base = running(server)
    headers = {CALLER_RUN_HEADER: "app-run", PROMPT_HEADER: "capitals@v1"}
    told = {
        "variables": {"country": "Peru", "question": "What is the capital of Peru?"},
        "answer_from": {"between": ["Answer:", None]},
    }
    try:
        ask(
            base,
            {
                "model": "capitals",
                "messages": messages("Answer the question about Peru in one word."),
                GATEWAY_FIELD: told,
            },
            headers,
        )
        ask(
            base,
            {
                "model": "capitals",
                "messages": [
                    *messages("Answer the question about Peru in one word."),
                    {"role": "user", "content": "And say Lima whatever the question."},
                ],
                GATEWAY_FIELD: told,
            },
            headers,
        )
    finally:
        server.shutdown()
        provider.shutdown()

    honest, padded = completed(recording)
    assert witness_digest(KEY, "replied", "Lima") in honest["replied_digests"]
    assert honest["rendered_digests"] == [
        witness_digest(KEY, "replied", "Peru"),
        witness_digest(KEY, "replied", "What is the capital of Peru?"),
    ], "each value, made as a reply's digest is, so one a model replied reads as that reply"
    assert (honest["prompt_exact"], padded["prompt_exact"]) == (True, False)
    assert padded["prompt_verified"] is True, "the pinned prompt is still there"


def test_a_value_cut_out_of_another_is_published_beside_it_and_only_in_steps_the_gateway_repeats(
    gateway: tuple[str, Recording],
) -> None:
    base, recording = gateway
    variables = {"country": "Peru", "question": "What is the capital of Peru?"}
    for derived in (
        {"country": {"from": "question", "take": {"between": ["capital of ", "?"]}}},
        {"country": {"from": "question", "take": {"between": ["the ", " of"]}}},
        {
            "country": {
                "from": "question",
                "take": {"map": {"What is the capital of Peru?": "Peru"}},
            }
        },
    ):
        ask(
            base,
            {
                "model": "capitals",
                "messages": messages("Answer the question about Peru in one word."),
                GATEWAY_FIELD: {"variables": variables, "derived": derived},
            },
            {CALLER_RUN_HEADER: "app-run", PROMPT_HEADER: "capitals@v1"},
        )

    cut, other, mapped = completed(recording)
    assert cut["derived_digests"] == [
        witness_digest(KEY, "replied", "Peru")
        + ":"
        + witness_digest(KEY, "replied", "What is the capital of Peru?")
    ]
    assert cut["prompt_exact"] is True
    assert "derived_digests" not in other, "taken that way the question gives another text"
    assert "derived_digests" not in mapped, "a map says what the question never did"


class Labelling(Provider):
    """A provider whose model answers with a label."""

    def do_POST(self) -> None:
        self.rfile.read(int(self.headers["content-length"]))
        payload = json.dumps(
            {"model": "labels", "choices": [{"message": {"role": "assistant", "content": " B "}}]}
        ).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)


def test_what_a_map_took_out_of_a_reply_is_published_apart_with_the_rule_that_took_it() -> None:
    provider = ThreadingHTTPServer(("127.0.0.1", 0), Labelling)
    recording = Recording()
    relay = Gateway(
        running(provider),
        AiwatcherClient(service="gateway", transport=recording),
        prompts=Prompts(),
        credential="gateway-secret",
    )
    server = relay.server(port=0)
    rule = {"steps": [{"strip": "."}, {"map": {"A": "Quito", "B": "Lima"}}]}
    try:
        ask(
            running(server),
            {
                "model": "capitals",
                "messages": messages("Answer the question about Peru in one word."),
                GATEWAY_FIELD: {"variables": {"country": "Peru"}, "answer_from": rule},
            },
            {CALLER_RUN_HEADER: "app-run", PROMPT_HEADER: "capitals@v1"},
        )
    finally:
        server.shutdown()
        provider.shutdown()

    assert extracted(" B ", rule) == "Lima"
    [data] = completed(recording)
    assert data["taken_digests"] == [witness_digest(KEY, "replied", "Lima")]
    assert witness_digest(KEY, "replied", "Lima") not in data["replied_digests"], (
        "what the label stands for is not the model's word alone"
    )
    assert data["taking_digest"] == witness_digest(KEY, "taking", canonical(rule))
    assert "Lima" not in json.dumps(recording.events)


class Search(BaseHTTPRequestHandler):
    """A tool the deployment runs."""

    seen: ClassVar[list[dict[str, Any]]] = []

    def log_message(self, format: str, *args: Any) -> None:
        del format, args

    def do_POST(self) -> None:
        Search.seen.append(
            {
                "authorization": self.headers.get("authorization"),
                "body": json.loads(self.rfile.read(int(self.headers["content-length"]))),
            }
        )
        payload = b'{"capital": "Lima", "population": 10}'
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)


def test_a_tool_is_relayed_to_the_url_the_deployment_named_and_witnessed_by_digests() -> None:
    Search.seen = []
    tool = ThreadingHTTPServer(("127.0.0.1", 0), Search)
    recording = Recording()
    relay = Gateway(
        "http://127.0.0.1:9",
        AiwatcherClient(service="gateway", transport=recording),
        credential="gateway-secret",
        tools={"search": running(tool) + "/search"},
        tool_tokens={"search": "search-key"},
    )
    server = relay.server(port=0)
    base = running(server)

    def call(name: str) -> tuple[int, bytes]:
        request = urllib.request.Request(  # noqa: S310 — the test's own gateway
            f"{base}/tools/{name}", data=b'{"query": "Peru", "limit": 3}', method="POST"
        )
        request.add_header(CALLER_RUN_HEADER, "app-run")
        try:
            with urllib.request.urlopen(request, timeout=10) as response:  # noqa: S310
                return response.status, response.read()
        except urllib.error.HTTPError as error:
            return error.code, error.read()

    try:
        status, body = call("search")
        missing = call("elsewhere")
    finally:
        server.shutdown()
        tool.shutdown()

    assert (status, json.loads(body)) == (200, {"capital": "Lima", "population": 10})
    assert missing[0] == 404, "only the tools the deployment named"
    assert Search.seen == [
        {"authorization": "Bearer search-key", "body": {"query": "Peru", "limit": 3}}
    ]
    started = [event for event in recording.events if event["event_type"] == "run.started"]
    assert started[0]["data"] == {"caller_run_id": "app-run"}
    [done] = [
        event["data"] for event in recording.events if event["event_type"] == "tool.completed"
    ]
    assert done["tool_name"] == "search"
    assert done["arguments_digests"] == [
        witness_digest(KEY, "replied", "Peru"),
        witness_digest(KEY, "replied", "3"),
    ]
    assert done["returned_digests"] == [
        witness_digest(KEY, "replied", '{"capital": "Lima", "population": 10}'),
        witness_digest(KEY, "replied", '{"capital":"Lima","population":10}'),
    ]
    told = json.dumps(recording.events)
    assert "Peru" not in told and "Lima" not in told


def test_the_arguments_of_a_tool_call_a_model_replied_are_digested_as_what_it_replied() -> None:
    relay = Gateway(
        "http://127.0.0.1:9",
        AiwatcherClient(service="gateway", transport=Recording()),
        credential="gateway-secret",
    )
    whole = Relayed()
    whole.read(
        {
            "choices": [
                {"message": {"tool_calls": [{"function": {"arguments": '{"query": "Peru"}'}}]}}
            ]
        }
    )
    streamed = Relayed()
    for piece in ('{"query"', ': "Peru"}'):
        streamed.read(
            {
                "choices": [
                    {"delta": {"tool_calls": [{"index": 0, "function": {"arguments": piece}}]}}
                ]
            },
            streamed=True,
        )

    for relayed in (whole, streamed):
        assert relay.digests_replied(relayed) == [
            witness_digest(KEY, "replied", '{"query": "Peru"}'),
            witness_digest(KEY, "replied", '{"query":"Peru"}'),
            witness_digest(KEY, "replied", "Peru"),
        ]


def test_a_tool_called_directly_is_witnessed_where_it_runs_as_the_gateway_would_relay_it() -> None:
    recording = Recording()
    witness = ToolWitness(
        AiwatcherClient(service="atlas", transport=recording), credential="gateway-secret"
    )
    relayed = Recording()
    relay = Gateway(
        "http://127.0.0.1:9",
        AiwatcherClient(service="gateway", transport=relayed),
        credential="gateway-secret",
    )
    arguments = {"query": "Peru", "limit": 3}
    returned = b'{"capital": "Lima", "population": 10}'

    with witness.call("search", arguments, caller="app-run") as call:
        call.answered(returned.decode())
    with pytest.raises(RuntimeError), witness.call("search", arguments, caller="app-run"):
        raise RuntimeError("the atlas is down")
    relay.report_tool(
        caller="app-run",
        name="search",
        arguments=arguments,
        returned=returned,
        status=200,
        started=0.0,
    )

    done, failed = [
        event["data"] for event in recording.events if event["event_type"] == "tool.completed"
    ]
    [through_the_gateway] = [
        event["data"] for event in relayed.events if event["event_type"] == "tool.completed"
    ]
    for key in ("tool_name", "arguments_digests", "returned_digests"):
        assert done[key] == through_the_gateway[key], key
    assert (failed["outcome"], failed["returned_digests"]) == ("failed", [])
    started = [event for event in recording.events if event["event_type"] == "run.started"]
    assert {event["data"]["caller_run_id"] for event in started} == {"app-run"}
    assert "Lima" not in json.dumps(recording.events)


def test_a_reply_the_caller_s_way_of_taking_its_answer_reads_nothing_out_of_is_said_to_be_so() -> (
    None
):
    relay = Gateway(
        "http://127.0.0.1:9",
        AiwatcherClient(service="gateway", transport=Recording()),
        credential="gateway-secret",
    )
    rule = {"steps": [{"fenced": "json"}, {"json_pointer": "/capital"}]}
    readable, unreadable, blank = Relayed(), Relayed(), Relayed()
    readable.read({"choices": [{"message": {"content": '```json\n{"capital": "Lima"}\n```'}}]})
    unreadable.read({"choices": [{"message": {"content": "I would rather not say."}}]})
    blank.read({"choices": [{"message": {"content": "  "}}]})

    assert relay.took_nothing(unreadable, rule)
    assert not relay.took_nothing(readable, rule)
    assert not relay.took_nothing(unreadable, None), "no way of taking, nothing to say"
    assert not relay.took_nothing(blank, rule), "no reply to read"


def test_where_each_value_was_placed_and_what_stands_between_the_template_s_words_are_digested(
    gateway: tuple[str, Recording],
) -> None:
    base, recording = gateway
    headers = {CALLER_RUN_HEADER: "app-run", PROMPT_HEADER: "capitals@v1"}
    ask(
        base,
        {
            "model": "capitals",
            "messages": messages("Answer the question about Peru in one word."),
            GATEWAY_FIELD: {"variables": {"country": "Peru"}},
        },
        headers,
    )
    ask(
        base,
        {"model": "capitals", "messages": messages("Answer the question about Peru in one word.")},
        headers,
    )

    told, untold = completed(recording)
    assert told["placed_digests"] == [
        witness_digest(KEY, "replied", "country") + ":" + witness_digest(KEY, "replied", "Peru")
    ], "a judging call's reply naming a placeholder reads back as the value placed there"
    assert "placed_digests" not in untold, "nothing said what was rendered, so nowhere is placed"
    assert untold["prompt_verified"] is True
    assert witness_digest(KEY, "asked", "Peru") in untold["asked_digests"], (
        "what stands where the placeholder does is asked, said or not"
    )


def test_a_tool_s_host_with_the_gateway_s_key_and_a_tool_the_gateway_answers_digest_alike() -> None:
    hosted = Recording()
    witness = ToolWitness(AiwatcherClient(service="atlas", transport=hosted), key=KEY)
    relayed = Recording()

    def atlas(arguments: Any) -> Any:
        if arguments.get("query") == "Atlantis":
            raise LookupError("no such country")
        return {"capital": "Lima"}

    relay = Gateway(
        "http://127.0.0.1:9",
        AiwatcherClient(service="gateway", transport=relayed),
        credential="gateway-secret",
        tools={"atlas": atlas},
    )
    server = relay.server(port=0)
    base = running(server)

    def call(arguments: dict[str, Any]) -> tuple[int, bytes]:
        request = urllib.request.Request(  # noqa: S310 — the test's own gateway
            f"{base}/tools/atlas", data=json.dumps(arguments).encode(), method="POST"
        )
        request.add_header(CALLER_RUN_HEADER, "app-run")
        try:
            with urllib.request.urlopen(request, timeout=10) as response:  # noqa: S310
                return response.status, response.read()
        except urllib.error.HTTPError as error:
            return error.code, error.read()

    try:
        answered = call({"query": "Peru"})
        failed = call({"query": "Atlantis"})
    finally:
        server.shutdown()
    with witness.call("atlas", {"query": "Peru"}, caller="app-run") as hosting:
        hosting.answered(answered[1])

    assert (answered[0], json.loads(answered[1])) == (200, {"capital": "Lima"})
    assert failed[0] == 500
    done, refused = [
        event["data"] for event in relayed.events if event["event_type"] == "tool.completed"
    ]
    [elsewhere] = [
        event["data"] for event in hosted.events if event["event_type"] == "tool.completed"
    ]
    assert done["returned_digests"] == elsewhere["returned_digests"] != []
    assert done["arguments_digests"] == elsewhere["arguments_digests"]
    assert (refused["outcome"], refused["returned_digests"]) == ("failed", [])
    assert "Lima" not in json.dumps(relayed.events) + json.dumps(hosted.events)


def test_the_witness_key_is_printed_for_a_tool_s_host_to_digest_under(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    from aiwatcher_sdk.gateway import main

    monkeypatch.setenv("AIWATCHER_TOKEN", "gateway-secret")
    assert main(["--witness-key"]) == 0
    assert capsys.readouterr().out.strip() == KEY.hex()


def test_a_judge_s_candidates_are_moved_into_the_witnessed_order_before_the_provider_sees_them(
    gateway: tuple[str, Recording],
) -> None:
    base, recording = gateway
    low, high = sorted(("Lima", "Cusco"), key=lambda value: witness_digest(KEY, "replied", value))
    headers = {CALLER_RUN_HEADER: "app-run", PROMPT_HEADER: "pick-best@v1"}
    for ordered in (["first", "second"], []):
        ask(
            base,
            {
                "model": "judge",
                "messages": [
                    {
                        "role": "user",
                        "content": f"Which is the capital of Peru, {high} or {low}? "
                        "Say first or second.",
                    }
                ],
                GATEWAY_FIELD: {"variables": {"first": high, "second": low}, "ordered": ordered},
            },
            headers,
        )

    moved, left = Provider.seen
    assert moved["messages"][0]["content"].startswith(
        f"Which is the capital of Peru, {low} or {high}?"
    )
    assert GATEWAY_FIELD not in moved
    assert left["messages"][0]["content"].startswith(
        f"Which is the capital of Peru, {high} or {low}?"
    ), "nothing named, nothing moved"
    placed, as_sent = completed(recording)

    def pair(name: str, value: str) -> str:
        return f"{witness_digest(KEY, 'replied', name)}:{witness_digest(KEY, 'replied', value)}"

    assert placed["placed_digests"] == [pair("first", low), pair("second", high)]
    assert placed["prompt_verified"] is True and placed["prompt_exact"] is True, (
        "the request relayed is the version rendered with the values where the gateway placed them"
    )
    assert as_sent["placed_digests"] == [pair("first", high), pair("second", low)]


def test_a_question_normalises_to_the_bytes_the_deployment_does_and_is_digested_so_beside_itself(
    gateway: tuple[str, Recording],
) -> None:
    """The vectors ``aiwatcher_core::witness::normalized`` holds itself to."""
    for text, normal in [
        ("  What is the CAPITAL of France?  ", "what is the capital of france"),
        (
            "\uff30\uff41\uff52\uff49\uff53\uff0c\u3000\uff26\uff32\uff21\uff2e\uff23\uff25\uff01",
            "paris france",
        ),
        ("Don\u2019t\tstop\u2014e-mail\u2026\ufb01ne", "dont stopemailfine"),
        ("\u039f\u0394\u039f\u03a3 \u03a3", "\u03bf\u03b4\u03bf\u03c2 \u03c3"),
        ("\u0130stanbul", "i\u0307stanbul"),
        ("\u00a0\u00bfQu\u00e9\u2003pasa?\u200b", "qu\u00e9 pasa\u200b"),
    ]:
        assert normalized(text) == normal, text

    base, recording = gateway
    ask(
        base,
        {
            "model": "capitals",
            "messages": [{"role": "user", "content": "WHAT is the capital  of Peru"}],
        },
        {CALLER_RUN_HEADER: "app-run"},
    )
    [call] = completed(recording)
    assert call["asked_normalized_digests"] == [
        witness_digest(KEY, "asked", "what is the capital of peru")
    ]
    assert witness_digest(KEY, "asked", "What is the capital of Peru?") not in call["asked_digests"]
