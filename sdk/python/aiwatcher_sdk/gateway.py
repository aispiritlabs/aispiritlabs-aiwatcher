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
found exactly that rendering (:meth:`~aiwatcher_sdk.LlmCall.caller_body`) —
among the asked texts, and again made as a reply's are, so a value that is what
a model already replied reads as that reply — and of each reply. The key is
derived from the gateway's own credential, which the deployment issued, so the
deployment can ask whether an answer is a reply the gateway relayed and whether
a case's input was in the request, while a reader of the log cannot test a
guess against a one-word answer. Holding the
provider's key itself, so the application holds none, is what makes a call the
gateway did not see a call the application could not make.

It relays the deployment's tools the same way (``--tool search=https://…``):
``POST /tools/search`` with the call's arguments is sent to the URL the
deployment named — never one a caller names — and the gateway publishes keyed
digests of the arguments and of what the tool returned, so a value a later
request renders that is a tool's result, relayed here, is the tool's word
rather than the application's. A tool the application calls directly is
witnessed where it runs instead, by :class:`ToolWitness` under the gateway's
own credential — the same key, so the same digests.
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
from collections.abc import Generator, Iterator, Mapping, Sequence
from dataclasses import dataclass
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any, Protocol

from aiwatcher_sdk import CALLER_RUN_HEADER, GATEWAY_FIELD, PROMPT_HEADER, AiwatcherClient

__all__ = [
    "Gateway",
    "PromptFound",
    "PromptSource",
    "ToolCall",
    "ToolWitness",
    "canonical",
    "canonical_number",
    "extracted",
    "holds_template",
    "knows_more_than_the_reply",
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
#: How many labels one ``map`` step may hold.
MOST_MAPPED = 256
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
    """The digest of one text on one side of a call — ``said`` is ``asked``,
    ``replied``, or ``taking`` for how a caller takes its answer out."""
    message = said.encode() + b"\0" + text.strip(_WHITE_SPACE).encode()
    return hmac.new(key, message, hashlib.sha256).hexdigest()[:32]


def canonical_number(value: int | float) -> str:
    """A number as JavaScript's ``String(number)`` spells it — ``aiwatcher_core::witness::number``.

    The shortest digits that read back as the same double, positional from a
    millionth up to 10²¹ and in exponent notation outside; an integer, however
    wide, is spelled exactly, digit for digit.
    """
    if isinstance(value, int):
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


#: How many steps, or alternatives, one rule may hold, and how deep rules nest.
MOST_STEPS = 16
MOST_NESTING = 4


def extracted(text: str, rule: Mapping[str, Any]) -> str | None:
    """What an application says it takes as its answer out of a reply's text.

    A rule is one step, a list of them taken in turn — ``{"steps": [...]}``,
    each reading what the one before took — or alternatives,
    ``{"first_of": [...]}``, the first of which that takes something non-blank.
    The steps:

    * ``{"json_pointer": "/label"}`` reads the text as JSON and takes the value
      at that pointer (RFC 6901) — text as itself, anything else canonical;
    * ``{"between": ["Answer:", "\\n"]}`` takes what follows the first
      occurrence of the first marker up to the first occurrence of the second
      after it — from the start where the first is ``null``, to the end where
      the second is;
    * ``{"after_last": "Answer:"}`` takes what follows the marker's last
      occurrence;
    * ``{"line": -1}`` takes one non-blank line, counted from the end when
      negative;
    * ``{"fenced": "json"}`` takes the body of the first fenced code block with
      that language — any language where it is ``null``;
    * ``{"strip": "\"'."}`` removes these characters, and white space, from
      both ends;
    * ``{"lower": true}`` lower-cases the text;
    * ``{"number": true}`` reads the text as a JSON number and spells it as
      :func:`canonical_number` does;
    * ``{"map": {"A": "Paris", "B": "Lima"}}`` takes the word a label stands for
      — the text, stripped, as one of the labels. What a label means is known
      somewhere other than the reply, so what a rule holding one takes is its
      author's word as much as the model's (:func:`knows_more_than_the_reply`),
      and it witnesses an answer only where the variant pins that rule.

    ``None`` when a rule is none of those, or a step finds nothing. Nothing here
    runs a pattern a caller wrote: each step reads the text once. An application
    takes its answer with this same function, so both take the same text.
    """
    return _taken(text, rule, 0)


def _taken(text: str, rule: Any, depth: int) -> str | None:
    if not isinstance(rule, Mapping) or len(rule) != 1 or depth > MOST_NESTING:
        return None
    ((kind, argument),) = rule.items()
    if kind in ("steps", "first_of"):
        if not isinstance(argument, list) or not 0 < len(argument) <= MOST_STEPS:
            return None
        if kind == "first_of":
            for alternative in argument:
                taken = _taken(text, alternative, depth + 1)
                if taken is not None and taken.strip(_WHITE_SPACE):
                    return taken
            return None
        read = text
        for step in argument:
            took = _taken(read, step, depth + 1)
            if took is None:
                return None
            read = took
        return read
    step = _STEPS.get(str(kind))
    return None if step is None else step(text, argument)


def knows_more_than_the_reply(rule: Any, depth: int = 0) -> bool:
    """Whether a rule holds a step whose result is not all in the text it reads — a ``map``."""
    if not isinstance(rule, Mapping) or depth > MOST_NESTING:
        return False
    for kind, argument in rule.items():
        if kind == "map":
            return True
        if (
            kind in ("steps", "first_of")
            and isinstance(argument, list)
            and any(knows_more_than_the_reply(step, depth + 1) for step in argument)
        ):
            return True
    return False


def _map(text: str, table: Any) -> str | None:
    if not isinstance(table, Mapping) or not 0 < len(table) <= MOST_MAPPED:
        return None
    word = table.get(text.strip(_WHITE_SPACE))
    return word if isinstance(word, str) else None


def _json_pointer(text: str, pointer: Any) -> str | None:
    if not isinstance(pointer, str) or not (pointer == "" or pointer.startswith("/")):
        return None
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


def _between(text: str, markers: Any) -> str | None:
    if not (isinstance(markers, list) and len(markers) == 2) or markers == [None, None]:
        return None
    if not all(marker is None or (isinstance(marker, str) and marker) for marker in markers):
        return None
    begin, end = markers
    rest = text
    if begin is not None:
        start = text.find(begin)
        if start < 0:
            return None
        rest = text[start + len(begin) :]
    if end is not None:
        stop = rest.find(end)
        if stop < 0:
            return None
        rest = rest[:stop]
    return rest


def _after_last(text: str, marker: Any) -> str | None:
    if not isinstance(marker, str) or not marker or marker not in text:
        return None
    return text.rpartition(marker)[2]


def _line(text: str, index: Any) -> str | None:
    if isinstance(index, bool) or not isinstance(index, int):
        return None
    lines = [line for line in text.splitlines() if line.strip(_WHITE_SPACE)]
    return lines[index] if -len(lines) <= index < len(lines) else None


def _fenced(text: str, language: Any) -> str | None:
    if language is not None and (not isinstance(language, str) or not language):
        return None
    fence: str | None = None
    wanted = False
    body: list[str] = []
    for line in text.splitlines():
        bare = line.strip(_WHITE_SPACE)
        if fence is None:
            if bare.startswith(("```", "~~~")):
                marker = bare[0]
                fence = marker * (len(bare) - len(bare.lstrip(marker)))
                info = bare[len(fence) :].strip(_WHITE_SPACE).split()
                wanted = language is None or (bool(info) and info[0] == language)
                body = []
        elif bare.startswith(fence) and not bare.strip(fence[0]):
            if wanted:
                return "\n".join(body)
            fence = None
        else:
            body.append(line)
    return None


def _strip(text: str, characters: Any) -> str | None:
    if not isinstance(characters, str) or not characters:
        return None
    return text.strip(_WHITE_SPACE + characters)


def _lower(text: str, on: Any) -> str | None:
    return text.lower() if on is True else None


def _number(text: str, on: Any) -> str | None:
    if on is not True:
        return None

    def refused(constant: str) -> float:
        raise ValueError(constant)

    try:
        read = json.loads(text.strip(_WHITE_SPACE), parse_constant=refused)
    except ValueError:
        return None
    if isinstance(read, bool) or not isinstance(read, (int, float)):
        return None
    return canonical_number(read)


_STEPS: dict[str, Any] = {
    "json_pointer": _json_pointer,
    "between": _between,
    "after_last": _after_last,
    "line": _line,
    "fenced": _fenced,
    "strip": _strip,
    "lower": _lower,
    "number": _number,
    "map": _map,
}


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
    #: Values the caller took out of another of its values, by name:
    #: ``{"country": {"from": "question", "take": {"between": [...]}}}``.
    derived: Mapping[str, Any] | None = None

    @classmethod
    def read(cls, field: Any) -> Told:
        if not isinstance(field, Mapping):
            return cls()
        variables = field.get("variables")
        answer_from = field.get("answer_from")
        derived = field.get("derived")
        return cls(
            variables=variables if isinstance(variables, Mapping) else None,
            answer_from=answer_from if isinstance(answer_from, Mapping) else None,
            derived=derived if isinstance(derived, Mapping) else None,
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


def _as_text(value: Any) -> str:
    return value if isinstance(value, str) else canonical(value)


def _values(variables: Mapping[str, Any]) -> list[str]:
    """Each distinct non-blank value, as text: itself where it is text, its
    canonical JSON where it is not."""
    texts: list[str] = []
    for value in variables.values():
        text = _as_text(value)
        if text.strip(_WHITE_SPACE) and text not in texts:
            texts.append(text)
    return texts


def _leaves(value: Any) -> list[str]:
    """Each non-blank text inside a JSON value, and each number and flag as
    its canonical JSON — what a value's parts are compared as."""
    if isinstance(value, Mapping):
        return [leaf for field in value.values() for leaf in _leaves(field)]
    if isinstance(value, list):
        return [leaf for item in value for leaf in _leaves(item)]
    if value is None:
        return []
    text = _as_text(value)
    return [text] if text.strip(_WHITE_SPACE) else []


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
    #: Each tool call's arguments, by its choice and its own index.
    tool_arguments: dict[tuple[int, int], str] | None = None

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
                if isinstance(index, int) and isinstance(part, Mapping):
                    self._read_tool_calls(index, part.get("tool_calls"))
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

    def _read_tool_calls(self, choice: int, calls: Any) -> None:
        """The arguments of each tool call a choice holds, joined as a stream sends them."""
        if not isinstance(calls, list):
            return
        held = self.tool_arguments if self.tool_arguments is not None else {}
        for position, call in enumerate(calls):
            if not isinstance(call, Mapping):
                continue
            index = call.get("index", position)
            function = call.get("function")
            arguments = function.get("arguments") if isinstance(function, Mapping) else None
            if isinstance(index, int) and isinstance(arguments, str):
                joined = held.get((choice, index), "") + arguments
                held[(choice, index)] = joined[: MOST_REPLY_CHARS + 1]
        self.tool_arguments = held


@dataclass
class ToolCall:
    """What a tool answered one call with, as its host sends it back."""

    returned: bytes = b""
    status: int = int(HTTPStatus.OK)

    def answered(self, returned: bytes | str, status: int = int(HTTPStatus.OK)) -> None:
        """The bytes the host sends the caller, exactly: what a later request renders."""
        self.returned = returned.encode() if isinstance(returned, str) else returned
        self.status = status


class ToolWitness:
    """A tool's calls, witnessed by keyed digests under a witness's credential.

    The gateway relays a deployment's tools and witnesses them with this. A
    tool the application calls directly — not through ``/tools/<name>`` — is
    witnessed where it runs: its host wraps each call, and the witness publishes
    a run naming the caller's run (:data:`~aiwatcher_sdk.CALLER_RUN_HEADER`)
    holding digests of each part of the arguments and of the bytes the host
    sends back, and nothing said in either::

        witness = ToolWitness(telemetry, credential=GATEWAY_TOKEN)
        with witness.call("atlas", arguments, caller=headers.get(CALLER_RUN_HEADER)) as call:
            call.answered(json.dumps(look_up(arguments)))

    ``credential`` is the token ``telemetry`` publishes with, and it must be the
    gateway's own: a digest is made under the key derived from it, and only
    digests under one key can say that a value a call the gateway relayed was
    rendered with is what this tool returned. A host holding that token is
    trusted as the gateway is; one holding the application's is no witness.
    """

    def __init__(self, telemetry: AiwatcherClient, *, credential: str | None) -> None:
        self.telemetry = telemetry
        self.key = witness_key(credential) if credential else None

    @contextlib.contextmanager
    def call(
        self, name: str, arguments: Any, *, caller: str | None
    ) -> Generator[ToolCall, None, None]:
        """One call of the tool ``name`` with ``arguments``, reported once it is answered.

        A body that raises is reported as a failed call, with nothing returned.
        """
        started = time.monotonic()
        answer = ToolCall()
        try:
            yield answer
        except BaseException:
            self.report(
                caller=caller,
                name=name,
                arguments=arguments,
                returned=b"",
                status=int(HTTPStatus.INTERNAL_SERVER_ERROR),
                started=started,
            )
            raise
        self.report(
            caller=caller,
            name=name,
            arguments=arguments,
            returned=answer.returned,
            status=answer.status,
            started=started,
        )

    def report(
        self,
        *,
        caller: str | None,
        name: str,
        arguments: Any,
        returned: bytes,
        status: int,
        started: float,
    ) -> None:
        """One tool call: keyed digests of each part of its arguments and of
        what it returned, and nothing said in either."""
        with (
            contextlib.suppress(Exception),
            self.telemetry.run(f"gateway-{uuid.uuid4().hex}", caller_run_id=caller) as run,
            run.agent("gateway") as agent,
        ):
            # Its start and its end together, once the tool has answered: what
            # it returned is only known then, and the start would say nothing.
            call = {"call_id": uuid.uuid4().hex, "tool_name": name}
            self.telemetry.emit("tool.started", agent.correlation, call)
            outcome: dict[str, Any] = {
                **call,
                "status_code": status,
                "outcome": "succeeded" if status < 400 else "failed",
            }
            if self.key is not None:
                outcome["arguments_digests"] = _digested(self.key, "replied", _leaves(arguments))
                texts: list[str] = []
                if len(returned) <= MAX_BODY_BYTES and status < 400:
                    text = returned.decode("utf-8", errors="replace")
                    texts.append(text)
                    with contextlib.suppress(ValueError):
                        texts.append(canonical(json.loads(text)))
                outcome["returned_digests"] = _digested(self.key, "replied", texts)
            outcome["duration_ms"] = round((time.monotonic() - started) * 1000, 3)
            self.telemetry.emit("tool.completed", agent.correlation, outcome)
        with contextlib.suppress(Exception):
            self.telemetry.flush()


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
        tools: Mapping[str, str] | None = None,
        tool_tokens: Mapping[str, str] | None = None,
    ) -> None:
        """``credential`` is the token ``telemetry`` publishes with: the witness
        key its digests are made under is derived from it, and without one the
        gateway publishes no digests. ``tools`` names the URL each tool the
        deployment relays is posted to, and ``tool_tokens`` the bearer token any
        of them needs, so the application holds none.
        """
        self.tools = dict(tools or {})
        self.tool_tokens = dict(tool_tokens or {})
        self.upstream = upstream.rstrip("/")
        self.telemetry = telemetry
        self.prompts = prompts
        self.upstream_token = upstream_token
        self.token = token
        self.key = witness_key(credential) if credential else None
        self.tool_witness = ToolWitness(telemetry, credential=credential)
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
        # Every value is accounted for by its digest, so a request rendering
        # more than a witness digests is not one it can say is only the prompt.
        exact = (
            rendered
            and variables is not None
            and len(_values(variables)) <= MOST_DIGESTS
            and _holds_only(template, variables, texts)
        )
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

    def digests_rendered(self, variables: Mapping[str, Any] | None) -> list[str]:
        """Keyed digests of each value the template was found rendered with,
        made as a reply's are: what the deployment accounts for each value by —
        a part of the case's input, or what a call already replied."""
        if self.key is None or not variables:
            return []
        return _digested(self.key, "replied", _values(variables))

    def digests_derived(
        self, variables: Mapping[str, Any] | None, derived: Mapping[str, Any] | None
    ) -> list[str]:
        """For each value the caller says it took out of another of its values,
        and that this gateway takes out of it the same way, the value's digest
        and the other's, as ``value:source`` — so a value the application only
        cut out of the case's input is accounted as that input. A way of taking
        that knows more than the text it reads (a ``map``) derives nothing."""
        if self.key is None or not variables or not derived:
            return []
        pairs: list[str] = []
        for name, spec in derived.items():
            if not isinstance(spec, Mapping) or name not in variables:
                continue
            source, take = spec.get("from"), spec.get("take")
            if not isinstance(source, str) or source not in variables or source == name:
                continue
            if not isinstance(take, Mapping) or knows_more_than_the_reply(take):
                continue
            value = _as_text(variables[name])
            took = extracted(_as_text(variables[source]), take)
            if took is None or took.strip(_WHITE_SPACE) != value.strip(_WHITE_SPACE):
                continue
            pair = (
                f"{witness_digest(self.key, 'replied', value)}:"
                f"{witness_digest(self.key, 'replied', _as_text(variables[source]))}"
            )
            if pair not in pairs:
                pairs.append(pair)
            if len(pairs) == MOST_DIGESTS:
                break
        return pairs

    def digests_replied(
        self, relayed: Relayed, answer_from: Mapping[str, Any] | None = None
    ) -> list[str]:
        """Keyed digests of each reply — as text, where it is JSON as its
        canonical form, as what the caller said it takes out of it
        (``answer_from``), taken here, unless taking it needs more than the reply
        (:meth:`digests_taken`) — which is how an answer is compared; and of the
        arguments of each tool call it holds, whole and by their parts."""
        if self.key is None or not (relayed.replies or relayed.tool_arguments):
            return []
        texts: list[str] = []
        closed = answer_from is not None and not knows_more_than_the_reply(answer_from)
        for text in (relayed.replies or {}).values():
            if len(text) > MOST_REPLY_CHARS:
                continue
            if closed and answer_from is not None:
                taken = extracted(text, answer_from)
                if taken is not None and taken.strip(_WHITE_SPACE):
                    texts.append(taken)
            texts.append(text)
            with contextlib.suppress(ValueError):
                parsed = json.loads(text)
                if isinstance(parsed, (dict, list)):
                    texts.append(canonical(parsed))
        for arguments in (relayed.tool_arguments or {}).values():
            if len(arguments) > MOST_REPLY_CHARS:
                continue
            texts.append(arguments)
            with contextlib.suppress(ValueError):
                parsed = json.loads(arguments)
                texts.append(canonical(parsed))
                texts.extend(_leaves(parsed))
        return _digested(self.key, "replied", texts)

    def digests_taken(
        self, relayed: Relayed, answer_from: Mapping[str, Any] | None
    ) -> tuple[list[str], str | None]:
        """What a rule that knows more than the reply took out of each reply,
        digested as a reply is, and the digest of the rule itself — which the
        deployment compares with the rule the variant pins before it counts
        either."""
        if self.key is None or answer_from is None:
            return [], None
        taking = witness_digest(self.key, "taking", canonical(answer_from))
        if not knows_more_than_the_reply(answer_from):
            return [], taking
        texts: list[str] = []
        for text in (relayed.replies or {}).values():
            if len(text) > MOST_REPLY_CHARS:
                continue
            taken = extracted(text, answer_from)
            if taken is not None and taken.strip(_WHITE_SPACE):
                texts.append(taken)
        return _digested(self.key, "replied", texts), taking

    def took_nothing(self, relayed: Relayed, answer_from: Mapping[str, Any] | None) -> bool:
        """Whether the way the caller takes its answer out took nothing out of any
        reply it came back with: a reply the caller could not read, so none it
        chose its answer against. Said only where there was a reply to read."""
        if self.key is None or answer_from is None:
            return False
        texts = [
            text
            for text in (relayed.replies or {}).values()
            if text.strip(_WHITE_SPACE) and len(text) <= MOST_REPLY_CHARS
        ]
        return bool(texts) and all(
            not (extracted(text, answer_from) or "").strip(_WHITE_SPACE) for text in texts
        )

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
        rendered: Sequence[str] = (),
        derived: Sequence[str] = (),
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
            if rendered:
                outcome["rendered_digests"] = list(rendered)
            if derived:
                outcome["derived_digests"] = list(derived)
            replied = self.digests_replied(relayed, answer_from)
            if replied:
                outcome["replied_digests"] = replied
            taken, taking = self.digests_taken(relayed, answer_from)
            if taken:
                outcome["taken_digests"] = taken
            if taking:
                outcome["taking_digest"] = taking
            if self.took_nothing(relayed, answer_from):
                outcome["took_nothing"] = True
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

    def forward_tool(self, name: str, body: bytes) -> tuple[int, str, bytes]:
        """Post a tool call to the URL the deployment named for it; the reply whole."""
        request = urllib.request.Request(  # noqa: S310 — the deployment's own tool
            self.tools[name], data=body, method="POST"
        )
        request.add_header("content-type", "application/json")
        if token := self.tool_tokens.get(name):
            request.add_header("authorization", f"Bearer {token}")
        try:
            with urllib.request.urlopen(request, timeout=self.timeout) as response:  # noqa: S310
                content_type = response.headers.get("content-type", "application/json")
                return response.status, content_type, response.read(MAX_BODY_BYTES + 1)
        except urllib.error.HTTPError as error:
            return (
                error.code,
                error.headers.get("content-type", "application/json"),
                error.read(MAX_BODY_BYTES + 1),
            )

    def report_tool(
        self,
        *,
        caller: str | None,
        name: str,
        arguments: Any,
        returned: bytes,
        status: int,
        started: float,
    ) -> None:
        """One tool call, as the gateway relayed it — see :meth:`ToolWitness.report`."""
        self.tool_witness.report(
            caller=caller,
            name=name,
            arguments=arguments,
            returned=returned,
            status=status,
            started=started,
        )

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

            def relay_tool(self, name: str) -> None:
                if name not in gateway.tools:
                    self.send_json(HTTPStatus.NOT_FOUND, {"error": f"no tool named {name!r}"})
                    return
                length = int(self.headers.get("content-length") or 0)
                if length > MAX_BODY_BYTES:
                    self.send_json(HTTPStatus.REQUEST_ENTITY_TOO_LARGE, {"error": "too large"})
                    return
                raw = self.rfile.read(length)
                try:
                    arguments = json.loads(raw or b"{}")
                except json.JSONDecodeError:
                    self.send_json(HTTPStatus.BAD_REQUEST, {"error": "the body is not JSON"})
                    return
                started = time.monotonic()
                try:
                    status, content_type, returned = gateway.forward_tool(name, raw or b"{}")
                except (urllib.error.URLError, TimeoutError) as error:
                    self.send_json(HTTPStatus.BAD_GATEWAY, {"error": f"the tool: {error}"})
                    return
                gateway.report_tool(
                    caller=self.headers.get(CALLER_RUN_HEADER),
                    name=name,
                    arguments=arguments,
                    returned=returned,
                    status=status,
                    started=started,
                )
                self.send_response(status)
                self.send_header("content-type", content_type)
                self.send_header("content-length", str(len(returned)))
                self.end_headers()
                self.wfile.write(returned)

            def do_POST(self) -> None:
                authorization = self.headers.get("authorization")
                if not gateway.authorised(authorization):
                    self.send_json(HTTPStatus.UNAUTHORIZED, {"error": "a bearer token is required"})
                    return
                if self.path.startswith("/tools/"):
                    self.relay_tool(self.path.removeprefix("/tools/"))
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
                    rendered=gateway.digests_rendered(told.variables)
                    if prompt is not None and prompt.rendered
                    else (),
                    derived=gateway.digests_derived(told.variables, told.derived)
                    if prompt is not None and prompt.rendered
                    else (),
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


def _env_name(tool: str) -> str:
    return re.sub(r"[^A-Za-z0-9]", "_", tool).upper()


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
    parser.add_argument(
        "--tool",
        action="append",
        default=[],
        metavar="NAME=URL",
        help="a tool relayed at /tools/NAME to URL; its bearer token, if it needs one, "
        "in AIWATCHER_GATEWAY_TOOL_TOKEN_<NAME>",
    )
    args = parser.parse_args(argv)
    tools: dict[str, str] = {}
    for named in args.tool:
        name, _, url = named.partition("=")
        if not name or not url.startswith(("http://", "https://")):
            parser.error(f"--tool takes NAME=URL, not {named!r}")
        tools[name] = url
    tool_tokens = {
        name: token
        for name in tools
        if (token := os.environ.get(f"AIWATCHER_GATEWAY_TOOL_TOKEN_{_env_name(name)}"))
    }
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
        tools=tools,
        tool_tokens=tool_tokens,
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
