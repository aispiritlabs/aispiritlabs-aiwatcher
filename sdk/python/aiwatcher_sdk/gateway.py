"""``aiwatcher-gateway``: a model provider's calls, witnessed by a host that is not the application.

An application's telemetry is its own word, and a provider outside the
deployment publishes nothing — so what served a call, and what the call asked,
had no second witness. This is one: point the application's OpenAI-compatible
client at it, and it relays each request to the provider and publishes, under
**its own** credential, a run naming the application's run
(:data:`~aiwatcher_sdk.CALLER_RUN_HEADER`) with the model the provider says
answered and whether the request's text holds the template of the prompt
version the application names (:data:`~aiwatcher_sdk.PROMPT_HEADER`)::

    AIWATCHER_URL=https://aiwatcher.internal AIWATCHER_TOKEN=<the gateway's own token> \\
    AIWATCHER_GATEWAY_UPSTREAM_TOKEN=<the provider key> \\
        aiwatcher-gateway --upstream https://api.openai.com --listen 127.0.0.1:8085

It publishes neither the request nor the reply: a model, a version, a prompt
reference, a flag, tokens and a latency. Holding the provider's key itself, so
the application holds none, is what makes a call the gateway did not see a call
the application could not make.
"""

from __future__ import annotations

import argparse
import contextlib
import json
import os
import re
import threading
import time
import urllib.error
import urllib.request
import uuid
from collections.abc import Iterator, Mapping, Sequence
from dataclasses import dataclass
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any, Protocol

from aiwatcher_sdk import CALLER_RUN_HEADER, PROMPT_HEADER, AiwatcherClient

__all__ = [
    "Gateway",
    "PromptSource",
    "holds_template",
    "main",
]

#: The same placeholder syntax the registry reads — see ``aiwatcher_sdk.prompts``.
_PLACEHOLDER = re.compile(r"(?<!\{)\{\{\s*([a-zA-Z][a-zA-Z0-9_]*)\s*\}\}")
#: A request body larger than this is refused rather than held in memory.
MAX_BODY_BYTES = 8 * 1024 * 1024


class PromptSource(Protocol):
    """What the gateway reads a prompt version's text through."""

    def get_version(self, name: str, version_id: str) -> Any: ...


class PromptMissingError(LookupError):
    """The registry holds no such version, so no request can hold its template."""


def holds_template(template: str, messages: Sequence[Any]) -> bool:
    """Whether a message's text holds ``template`` with its variables filled.

    Every literal part of the template, in order, with anything where a
    placeholder stands — searched in each message and in all of them joined, so
    a template spanning the system and user messages matches too. A template
    that is only a placeholder holds nothing a request could show.
    """
    parts = _PLACEHOLDER.split(template)
    literals = parts[::2]
    if not any(literal.strip() for literal in literals):
        return False
    pattern = re.compile("(?s)" + ".*?".join(re.escape(literal) for literal in literals))
    texts = [_text_of(message) for message in messages]
    return any(pattern.search(text) for text in [*texts, "\n".join(texts)])


def _text_of(message: Any) -> str:
    if not isinstance(message, Mapping):
        return ""
    content = message.get("content")
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "\n".join(
            str(part.get("text", ""))
            for part in content
            if isinstance(part, Mapping) and part.get("type") in (None, "text")
        )
    return ""


@dataclass
class Relayed:
    """What the provider said about one call, read off its reply."""

    served_model: str | None = None
    prompt_tokens: int | None = None
    completion_tokens: int | None = None
    cached_tokens: int | None = None

    def read(self, body: Mapping[str, Any]) -> None:
        if isinstance(body.get("model"), str):
            self.served_model = body["model"]
        usage = body.get("usage")
        if isinstance(usage, Mapping):
            for field, key in (
                ("prompt_tokens", "prompt_tokens"),
                ("completion_tokens", "completion_tokens"),
            ):
                if isinstance(usage.get(key), int):
                    setattr(self, field, usage[key])
            details = usage.get("prompt_tokens_details")
            if isinstance(details, Mapping) and isinstance(details.get("cached_tokens"), int):
                self.cached_tokens = details["cached_tokens"]


class Gateway:
    """Relays calls to one provider and witnesses each under its own credential."""

    def __init__(
        self,
        upstream: str,
        telemetry: AiwatcherClient,
        *,
        prompts: PromptSource | None = None,
        upstream_token: str | None = None,
        token: str | None = None,
        timeout: float = 120.0,
    ) -> None:
        self.upstream = upstream.rstrip("/")
        self.telemetry = telemetry
        self.prompts = prompts
        self.upstream_token = upstream_token
        self.token = token
        self.timeout = timeout
        self._templates: dict[tuple[str, str], str | None] = {}
        self._lock = threading.Lock()

    # ── What a request asked ─────────────────────────────────────────────

    def template(self, name: str, version: str) -> str:
        """The version's text, read once: a version never changes.

        Raises :class:`PromptMissingError` for a version the registry does not hold,
        and lets a registry that could not be reached raise its own error —
        which the gateway reports as nothing verified rather than as a mismatch.
        """
        key = (name, version)
        with self._lock:
            if key in self._templates:
                known = self._templates[key]
                if known is None:
                    raise PromptMissingError(f"{name}@{version}")
                return known
        if self.prompts is None:
            raise RuntimeError("this gateway reads no prompt registry")
        try:
            text = str(self.prompts.get_version(name, version).text)
        except Exception as error:
            if getattr(error, "status", None) == HTTPStatus.NOT_FOUND:
                with self._lock:
                    self._templates[key] = None
                raise PromptMissingError(f"{name}@{version}") from error
            raise
        with self._lock:
            self._templates[key] = text
        return text

    def verified(self, named: str | None, body: Mapping[str, Any]) -> tuple[str, str, bool] | None:
        """The prompt a request names, and whether its text holds that template.

        ``None`` when it names none, or the registry could not be asked.
        """
        if not named or "@" not in named:
            return None
        name, version = named.rsplit("@", 1)
        messages = body.get("messages")
        try:
            template = self.template(name, version)
        except PromptMissingError:
            return name, version, False
        except Exception:  # noqa: BLE001 — an unreachable registry verifies nothing
            return None
        return name, version, isinstance(messages, list) and holds_template(template, messages)

    # ── Relaying ─────────────────────────────────────────────────────────

    def authorised(self, authorization: str | None) -> bool:
        if self.token is None:
            return True
        return authorization == f"Bearer {self.token}"

    def forward(
        self, path: str, body: bytes, authorization: str | None
    ) -> tuple[int, str, Iterator[bytes]]:
        """Send the request on; the reply's status, content type and bytes."""
        request = urllib.request.Request(  # noqa: S310 — the configured provider
            self.upstream + path, data=body, method="POST"
        )
        request.add_header("content-type", "application/json")
        credential = (
            f"Bearer {self.upstream_token}"
            if self.upstream_token
            else (authorization if self.token is None else None)
        )
        if credential:
            request.add_header("authorization", credential)
        try:
            response = urllib.request.urlopen(request, timeout=self.timeout)  # noqa: S310
        except urllib.error.HTTPError as error:
            return (
                error.code,
                error.headers.get("content-type", "application/json"),
                iter([error.read()]),
            )
        return (
            response.status,
            response.headers.get("content-type", "application/json"),
            _chunks(response),
        )

    def report(
        self,
        *,
        caller: str | None,
        requested: str | None,
        prompt: tuple[str, str, bool] | None,
        relayed: Relayed,
        status: int,
        started: float,
    ) -> None:
        """One call, as the gateway saw it — with nothing that was said in it."""
        request: dict[str, Any] = {"provider": "aiwatcher-gateway"}
        if prompt is not None:
            request["prompt"] = (prompt[0], prompt[1])
            request["prompt_verified"] = prompt[2]
        with (
            contextlib.suppress(Exception),
            self.telemetry.run(f"gateway-{uuid.uuid4().hex}", caller_run_id=caller) as run,
            run.agent("gateway") as agent,
            agent.llm(model=requested or "unknown", **request) as call,
        ):
            outcome: dict[str, Any] = {
                "status_code": status,
                "outcome": "succeeded" if status < 400 else "failed",
                "duration_ms": round((time.monotonic() - started) * 1000, 3),
            }
            if relayed.served_model:
                outcome["model_version"] = relayed.served_model
                outcome["response_model"] = relayed.served_model
            call.usage(
                prompt_tokens=relayed.prompt_tokens,
                completion_tokens=relayed.completion_tokens,
                cached_tokens=relayed.cached_tokens,
                **outcome,
            )
        # Posted before the reply ends: the caller reads to the connection's
        # close, so its call cannot end before the witness is on the log.
        with contextlib.suppress(Exception):
            self.telemetry.flush()

    def handler(self) -> type[BaseHTTPRequestHandler]:
        gateway = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, format: str, *args: Any) -> None:
                del format, args

            def send_json(self, status: HTTPStatus, body: Mapping[str, Any]) -> None:
                payload = json.dumps(body, separators=(",", ":")).encode()
                self.send_response(status)
                self.send_header("content-type", "application/json")
                self.send_header("content-length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)

            def do_GET(self) -> None:
                if self.path in ("/livez", "/healthz"):
                    self.send_json(HTTPStatus.OK, {"status": "ok"})
                    return
                self.send_json(HTTPStatus.NOT_FOUND, {"error": "not found"})

            def do_POST(self) -> None:
                authorization = self.headers.get("authorization")
                if not gateway.authorised(authorization):
                    self.send_json(HTTPStatus.UNAUTHORIZED, {"error": "a bearer token is required"})
                    return
                if not self.path.startswith("/v1/"):
                    self.send_json(HTTPStatus.NOT_FOUND, {"error": "not found"})
                    return
                length = int(self.headers.get("content-length") or 0)
                if length > MAX_BODY_BYTES:
                    self.send_json(HTTPStatus.REQUEST_ENTITY_TOO_LARGE, {"error": "too large"})
                    return
                raw = self.rfile.read(length)
                try:
                    body = json.loads(raw or b"{}")
                except json.JSONDecodeError:
                    self.send_json(HTTPStatus.BAD_REQUEST, {"error": "the body is not JSON"})
                    return
                if not isinstance(body, dict):
                    self.send_json(HTTPStatus.BAD_REQUEST, {"error": "the body is not an object"})
                    return
                started = time.monotonic()
                prompt = gateway.verified(self.headers.get(PROMPT_HEADER), body)
                relayed = Relayed()
                try:
                    status, content_type, chunks = gateway.forward(self.path, raw, authorization)
                except (urllib.error.URLError, TimeoutError) as error:
                    self.send_json(HTTPStatus.BAD_GATEWAY, {"error": f"the provider: {error}"})
                    status = int(HTTPStatus.BAD_GATEWAY)
                else:
                    self.send_response(status)
                    self.send_header("content-type", content_type)
                    self.end_headers()
                    streaming = content_type.startswith("text/event-stream")
                    held = bytearray()
                    for chunk in chunks:
                        self.wfile.write(chunk)
                        self.wfile.flush()
                        if streaming:
                            _read_events(chunk, held, relayed)
                        else:
                            held.extend(chunk)
                    if not streaming:
                        with contextlib.suppress(ValueError):
                            parsed = json.loads(bytes(held))
                            if isinstance(parsed, Mapping):
                                relayed.read(parsed)
                gateway.report(
                    caller=self.headers.get(CALLER_RUN_HEADER),
                    requested=body.get("model") if isinstance(body.get("model"), str) else None,
                    prompt=prompt,
                    relayed=relayed,
                    status=int(status),
                    started=started,
                )

        return Handler

    def server(self, host: str = "127.0.0.1", port: int = 8085) -> ThreadingHTTPServer:
        return ThreadingHTTPServer((host, port), self.handler())


def _chunks(response: Any) -> Iterator[bytes]:
    """The reply as it arrives, so a stream reaches the caller as it is sent."""
    with response:
        while True:
            chunk = response.read1(64 * 1024)
            if not chunk:
                return
            yield chunk


def _read_events(chunk: bytes, held: bytearray, relayed: Relayed) -> None:
    """Read what a stream's events say about the call, line by line."""
    held.extend(chunk)
    while b"\n" in held:
        line, _, rest = bytes(held).partition(b"\n")
        held[:] = rest
        text = line.strip()
        if not text.startswith(b"data:"):
            continue
        payload = text[5:].strip()
        if payload == b"[DONE]":
            continue
        with contextlib.suppress(ValueError):
            event = json.loads(payload)
            if isinstance(event, Mapping):
                relayed.read(event)


def main(argv: Sequence[str] | None = None) -> int:
    from aiwatcher_sdk.prompts import PromptRegistry

    parser = argparse.ArgumentParser(prog="aiwatcher-gateway", description=__doc__)
    parser.add_argument("--upstream", required=True, help="the provider's base URL")
    parser.add_argument("--listen", default="127.0.0.1:8085", help="host:port to serve on")
    parser.add_argument(
        "--upstream-token-env",
        default="AIWATCHER_GATEWAY_UPSTREAM_TOKEN",
        help="the variable holding the provider's key, which the application then need not hold",
    )
    parser.add_argument(
        "--token-env",
        default="AIWATCHER_GATEWAY_TOKEN",
        help="the variable holding the bearer token callers present; required off localhost",
    )
    args = parser.parse_args(argv)
    host, _, port = args.listen.rpartition(":")
    token = os.environ.get(args.token_env) or None
    if token is None and host not in ("127.0.0.1", "localhost", "::1"):
        parser.error(f"{args.token_env} is required to listen on {host}")
    url = os.environ.get("AIWATCHER_URL")
    credential = os.environ.get("AIWATCHER_TOKEN")
    telemetry = AiwatcherClient(service="aiwatcher-gateway", base_url=url, token=credential)
    prompts = PromptRegistry(url, token=credential) if url else None
    gateway = Gateway(
        args.upstream,
        telemetry,
        prompts=prompts,
        upstream_token=os.environ.get(args.upstream_token_env) or None,
        token=token,
    )
    server = gateway.server(host or "127.0.0.1", int(port))
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
        telemetry.flush()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
