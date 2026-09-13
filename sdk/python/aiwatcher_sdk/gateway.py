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
reference, a flag, tokens, a latency — and keyed digests of each message, of
the values the application says it rendered the prompt with where the gateway
found exactly that rendering (:meth:`~aiwatcher_sdk.LlmCall.caller_body`), and
of each reply. The key is derived from the gateway's own credential, which the
deployment issued, so the deployment can ask whether an answer is a reply the
gateway relayed and whether a case's input was in the request, while a reader
of the log cannot test a guess against a one-word answer. Holding the
provider's key itself, so the application holds none, is what makes a call the
gateway did not see a call the application could not make.
"""

from __future__ import annotations

import argparse
import contextlib
import decimal
import hashlib
import hmac
import json
import math
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

from aiwatcher_sdk import CALLER_RUN_HEADER, GATEWAY_FIELD, PROMPT_HEADER, AiwatcherClient

__all__ = [
    "Gateway",
    "PromptFound",
    "PromptSource",
    "canonical",
    "canonical_number",
    "extracted",
    "holds_template",
    "main",
    "witness_digest",
    "witness_key",
]

#: The same placeholder syntax the registry reads — see ``aiwatcher_sdk.prompts``.
_PLACEHOLDER = re.compile(r"(?<!\{)\{\{\s*([a-zA-Z][a-zA-Z0-9_]*)\s*\}\}")
#: A request body larger than this is refused rather than held in memory.
MAX_BODY_BYTES = 8 * 1024 * 1024
#: What a witness key is derived for — ``aiwatcher_core::witness``, byte for byte.
WITNESS_KEY_LABEL = b"aiwatcher.witness.v1"
#: How many digests of one side of a call are published.
MOST_DIGESTS = 64
#: A reply longer than this is relayed and not digested.
MOST_REPLY_CHARS = 1024 * 1024
#: The characters Rust's ``str::trim`` removes: Unicode's White_Space, which is
#: not quite what ``str.strip()`` removes.
_WHITE_SPACE = (
    "\t\n\x0b\x0c\r \x85\xa0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006"
    "\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000"
)


def witness_key(secret: str) -> bytes:
    """The witness key of a credential, from its secret."""
    return hmac.new(secret.encode(), WITNESS_KEY_LABEL, hashlib.sha256).digest()


def witness_digest(key: bytes, said: str, text: str) -> str:
    """The digest of one text on one side of a call — ``said`` is ``asked`` or ``replied``."""
    message = said.encode() + b"\0" + text.strip(_WHITE_SPACE).encode()
    return hmac.new(key, message, hashlib.sha256).hexdigest()[:32]


def canonical_number(value: int | float) -> str:
    """A number as JavaScript's ``String(number)`` spells it — ``aiwatcher_core::witness::number``.

    The shortest digits that read back as the same double, positional from a
    millionth up to 10²¹ and in exponent notation outside; an integer a double
    or a 64-bit integer holds exactly is spelled exactly.
    """
    if isinstance(value, int) and -(2**63) <= value < 2**64:
        return str(value)
    number = float(value)
    if number == 0 or not math.isfinite(number):
        return "0"
    sign = "-" if number < 0 else ""
    exact = decimal.Decimal(repr(abs(number)))
    digits_tuple, exponent = exact.as_tuple().digits, int(exact.as_tuple().exponent)
    digits = "".join(str(digit) for digit in digits_tuple).rstrip("0") or "0"
    exponent += len(digits_tuple) - len(digits)
    count = len(digits)
    point = exponent + count
    if count <= point <= 21:
        spelled = digits + "0" * (point - count)
    elif 0 < point <= 21:
        spelled = f"{digits[:point]}.{digits[point:]}"
    elif -6 < point <= 0:
        spelled = "0." + "0" * -point + digits
    else:
        rest = f".{digits[1:]}" if count > 1 else ""
        spelled = f"{digits[0]}{rest}e{'-' if point - 1 < 0 else '+'}{abs(point - 1)}"
    return sign + spelled


def canonical(value: Any) -> str:
    """A JSON value as one text, byte for byte as ``aiwatcher_core::witness::canonical``
    writes it: keys sorted by code point, nothing between tokens, JSON's string
    escapes, and every number as :func:`canonical_number` spells it."""
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, (int, float)):
        return canonical_number(value)
    if isinstance(value, str):
        return json.dumps(value, ensure_ascii=False)
    if isinstance(value, Mapping):
        return (
            "{"
            + ",".join(
                f"{json.dumps(str(key), ensure_ascii=False)}:{canonical(value[key])}"
                for key in sorted(value, key=str)
            )
            + "}"
        )
    if isinstance(value, (list, tuple)):
        return "[" + ",".join(canonical(item) for item in value) + "]"
    return json.dumps(str(value), ensure_ascii=False)


def extracted(text: str, rule: Mapping[str, Any]) -> str | None:
    """What an application says it takes as its answer out of a reply's text.

    ``{"json_pointer": "/label"}`` reads the reply as JSON and takes the value at
    that pointer (RFC 6901) — text as itself, anything else in its canonical
    form; ``{"between": ["Answer:", "\\n"]}`` takes what follows the first
    occurrence of the first marker, up to the first occurrence of the second
    after it (or the end, where the second is ``null``). ``None`` when the rule
    is not one of those or finds nothing.
    """
    pointer = rule.get("json_pointer")
    if isinstance(pointer, str) and (pointer == "" or pointer.startswith("/")):
        try:
            found: Any = json.loads(text)
        except ValueError:
            return None
        for token in pointer.split("/")[1:] if pointer else []:
            token = token.replace("~1", "/").replace("~0", "~")
            if isinstance(found, Mapping) and token in found:
                found = found[token]
            elif isinstance(found, list) and token.isdigit() and int(token) < len(found):
                found = found[int(token)]
            else:
                return None
        return found if isinstance(found, str) else canonical(found)
    between = rule.get("between")
    if (
        isinstance(between, list)
        and len(between) == 2
        and isinstance(between[0], str)
        and between[0]
        and (between[1] is None or (isinstance(between[1], str) and between[1]))
    ):
        start = text.find(between[0])
        if start < 0:
            return None
        rest = text[start + len(between[0]) :]
        if between[1] is not None:
            end = rest.find(between[1])
            if end < 0:
                return None
            rest = rest[:end]
        return rest
    return None


@dataclass(frozen=True)
class PromptFound:
    """What the gateway found of the prompt a request names."""

    name: str
    version: str
    #: The request's text holds the version's template: rendered, or its literal parts.
    verified: bool
    #: It holds it rendered with exactly the values the caller sent.
    rendered: bool
    #: And nothing else: every message is the rendered template or those values.
    exact: bool


@dataclass(frozen=True)
class Told:
    """What the caller said about its call, in the body field the provider never sees."""

    variables: Mapping[str, Any] | None = None
    #: How the caller takes its answer out of the reply — see :func:`extracted`.
    answer_from: Mapping[str, Any] | None = None

    @classmethod
    def read(cls, field: Any) -> Told:
        if not isinstance(field, Mapping):
            return cls()
        variables = field.get("variables")
        answer_from = field.get("answer_from")
        return cls(
            variables=variables if isinstance(variables, Mapping) else None,
            answer_from=answer_from if isinstance(answer_from, Mapping) else None,
        )


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


def _texts_asked(body: Mapping[str, Any]) -> list[str]:
    """Each text a request holds: every message's, or a completion's prompt."""
    messages = body.get("messages")
    texts = [_text_of(message) for message in messages] if isinstance(messages, list) else []
    if isinstance(body.get("prompt"), str):
        texts.append(body["prompt"])
    return [text for text in texts if text.strip(_WHITE_SPACE)]


def _rendered(template: str, variables: Mapping[str, Any]) -> str | None:
    """``template`` rendered with these values, as
    :meth:`~aiwatcher_sdk.prompts.PromptVersion.render` fills it; ``None`` when a
    placeholder has no value."""
    names = set(_PLACEHOLDER.findall(template))
    if not names <= set(variables):
        return None
    rendered = _PLACEHOLDER.sub(lambda match: str(variables[match.group(1)]), template)
    return rendered.strip(_WHITE_SPACE) or None


def _holds_rendered(template: str, variables: Mapping[str, Any], texts: Sequence[str]) -> bool:
    """Whether a text holds ``template`` rendered with exactly these values."""
    rendered = _rendered(template, variables)
    return rendered is not None and any(rendered in text for text in [*texts, "\n".join(texts)])


def _holds_only(template: str, variables: Mapping[str, Any], texts: Sequence[str]) -> bool:
    """Whether the request's text is nothing but ``template`` rendered and the
    values it was rendered with, with whitespace between them — so nothing the
    application added, such as an answer it wants repeated, rides beside them."""
    rendered = _rendered(template, variables)
    if rendered is None:
        return False
    rest = "\n".join(texts)
    if rendered not in rest:
        return False
    pieces = [
        rendered,
        *sorted({str(value) for value in variables.values()}, key=len, reverse=True),
    ]
    for piece in pieces:
        if piece.strip(_WHITE_SPACE):
            rest = rest.replace(piece, "\n")
    return not rest.strip(_WHITE_SPACE)


def _digested(key: bytes, said: str, texts: Sequence[str]) -> list[str]:
    digests: list[str] = []
    for text in texts:
        digest = witness_digest(key, said, text)
        if digest not in digests:
            digests.append(digest)
        if len(digests) == MOST_DIGESTS:
            break
    return digests


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
    #: Each choice's text, by its index, as far as it has arrived.
    replies: dict[int, str] | None = None

    def read(self, body: Mapping[str, Any], *, streamed: bool = False) -> None:
        if isinstance(body.get("model"), str):
            self.served_model = body["model"]
        choices = body.get("choices")
        if isinstance(choices, list):
            replies = self.replies if self.replies is not None else {}
            for position, choice in enumerate(choices):
                if not isinstance(choice, Mapping):
                    continue
                index = choice.get("index", position)
                part = choice.get("delta" if streamed else "message")
                text = _text_of(part) if isinstance(part, Mapping) else ""
                if isinstance(choice.get("text"), str):
                    text += choice["text"]
                if isinstance(index, int) and text:
                    joined = replies.get(index, "") + text
                    replies[index] = joined[: MOST_REPLY_CHARS + 1]
            self.replies = replies
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
        credential: str | None = None,
        timeout: float = 120.0,
    ) -> None:
        """``credential`` is the token ``telemetry`` publishes with: the witness
        key its digests are made under is derived from it, and without one the
        gateway publishes no digests.
        """
        self.upstream = upstream.rstrip("/")
        self.telemetry = telemetry
        self.prompts = prompts
        self.upstream_token = upstream_token
        self.token = token
        self.key = witness_key(credential) if credential else None
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

    def verified(
        self,
        named: str | None,
        body: Mapping[str, Any],
        variables: Mapping[str, Any] | None = None,
    ) -> PromptFound | None:
        """The prompt a request names, and what its text holds of that template.

        ``None`` when it names none, or the registry could not be asked.
        """
        if not named or "@" not in named:
            return None
        name, version = named.rsplit("@", 1)
        try:
            template = self.template(name, version)
        except PromptMissingError:
            return PromptFound(name, version, verified=False, rendered=False, exact=False)
        except Exception:  # noqa: BLE001 — an unreachable registry verifies nothing
            return None
        texts = _texts_asked(body)
        rendered = variables is not None and _holds_rendered(template, variables, texts)
        exact = rendered and variables is not None and _holds_only(template, variables, texts)
        messages = body.get("messages")
        literal = isinstance(messages, list) and holds_template(template, messages)
        return PromptFound(
            name, version, verified=rendered or literal, rendered=rendered, exact=exact
        )

    def digests_asked(
        self, body: Mapping[str, Any], variables: Mapping[str, Any] | None, rendered: bool
    ) -> list[str]:
        """Keyed digests of each text the request held — and of the values the
        template was found rendered with, only where it was."""
        if self.key is None:
            return []
        texts = _texts_asked(body)
        if rendered and variables is not None:
            texts.extend(
                value if isinstance(value, str) else canonical(value)
                for value in variables.values()
            )
        return _digested(self.key, "asked", texts)

    def digests_replied(
        self, relayed: Relayed, answer_from: Mapping[str, Any] | None = None
    ) -> list[str]:
        """Keyed digests of each reply — as text, where it is JSON as its
        canonical form, and as what the caller said it takes out of it
        (``answer_from``), taken here — which is how an answer is compared."""
        if self.key is None or not relayed.replies:
            return []
        texts: list[str] = []
        for text in relayed.replies.values():
            if len(text) > MOST_REPLY_CHARS:
                continue
            if answer_from is not None:
                taken = extracted(text, answer_from)
                if taken is not None and taken.strip(_WHITE_SPACE):
                    texts.append(taken)
            texts.append(text)
            with contextlib.suppress(ValueError):
                parsed = json.loads(text)
                if isinstance(parsed, (dict, list)):
                    texts.append(canonical(parsed))
        return _digested(self.key, "replied", texts)

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
        prompt: PromptFound | None,
        relayed: Relayed,
        status: int,
        started: float,
        asked: Sequence[str] = (),
        answer_from: Mapping[str, Any] | None = None,
    ) -> None:
        """One call, as the gateway saw it — with nothing that was said in it."""
        request: dict[str, Any] = {"provider": "aiwatcher-gateway"}
        if prompt is not None:
            request["prompt"] = (prompt.name, prompt.version)
            request["prompt_verified"] = prompt.verified
            request["prompt_exact"] = prompt.exact
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
            if asked:
                outcome["asked_digests"] = list(asked)
            replied = self.digests_replied(relayed, answer_from)
            if replied:
                outcome["replied_digests"] = replied
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
                # What the application says about the call is for the gateway,
                # never for the provider.
                told = Told()
                if GATEWAY_FIELD in body:
                    told = Told.read(body.pop(GATEWAY_FIELD))
                    raw = json.dumps(body, separators=(",", ":")).encode()
                prompt = gateway.verified(self.headers.get(PROMPT_HEADER), body, told.variables)
                asked = gateway.digests_asked(
                    body, told.variables, prompt is not None and prompt.rendered
                )
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
                    asked=asked,
                    answer_from=told.answer_from,
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
                relayed.read(event, streamed=True)


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
        credential=credential,
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
