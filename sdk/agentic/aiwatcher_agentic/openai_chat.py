"""One OpenAI-compatible chat endpoint, and the profile that says how it differs.

`model.py` says adapters stay with the application, and that rule is about a
model *stack*: importing an agent must not import torch, MLX or vLLM. This is
not a stack. It is a wire format — a POST to `/chat/completions` and the shape
of what comes back — and every application that reached an endpoint over HTTP
wrote the same hundred lines: build the body, read `choices[0]`, map `usage`
onto :class:`~aiwatcher_agentic.model.ModelResponse`, turn an error into a
message. Four copies of that existed across two repositories, and they differed
only in the handful of fields one endpoint wants said differently.

Those differences are a :class:`ModelProfile`, and a profile is **declared**,
never sniffed — the same rule ADR_0023 applies to a model's runtime, for the
same reason: a behaviour chosen by looking at the answer is a behaviour chosen
by whoever wrote the answer. `LLAMA_CPP` turns thinking off and tightens a JSON
schema because that is what a GGUF build needs, not because the SDK detected
one.

The model stack still never arrives: the transport is a callable the caller
supplies, so the endpoint is reached by whatever HTTP client the application
already has — with its own timeouts, retries and user agent — and the default
is the standard library.
"""

from __future__ import annotations

from collections.abc import Callable, Generator, Mapping
from contextlib import contextmanager
from copy import deepcopy
from dataclasses import dataclass, field
from time import monotonic
from types import MappingProxyType
from typing import Any

import orjson

from aiwatcher_agentic.exceptions import ModelResponseError
from aiwatcher_agentic.model import ModelResponse

__all__ = [
    "LLAMA_CPP",
    "OPENAI",
    "ModelProfile",
    "OpenAIChatModel",
    "Transport",
    "urllib_transport",
]

#: Send one request, get the response body back. Anything that is not a 2xx is
#: the transport's to raise, because the client the application already has is
#: the one that knows what its own failures mean.
type Transport = Callable[[str, bytes, Mapping[str, str], float], bytes]

_EMPTY: Mapping[str, Any] = MappingProxyType({})


@dataclass(frozen=True, slots=True)
class ModelProfile:
    """What one endpoint needs said differently from the generic case."""

    name: str = "openai"
    temperature: float = 0.0
    max_tokens: int = 2048
    #: Merged into every request body, under whatever the caller passes. This is
    #: where a non-standard knob lives — `chat_template_kwargs`, a provider's
    #: own tool — rather than as another boolean on this class.
    extra_body: Mapping[str, Any] = field(default=_EMPTY)
    #: Mark every property of a structured-output schema required. A quantised
    #: model reads an optional field as one it may stop before, so a response
    #: that is valid against the schema can still be missing the answer.
    require_complete_schema: bool = False
    #: Treat `finish_reason == "length"` as a failure. A truncated completion
    #: parses as often as it does not, and the half that parses is worse.
    fail_on_length: bool = True


OPENAI = ModelProfile()

LLAMA_CPP = ModelProfile(
    name="llamacpp",
    extra_body=MappingProxyType({"chat_template_kwargs": {"enable_thinking": False}}),
    require_complete_schema=True,
)


class OpenAIChatModel:
    """A `TextModel` and its own `ModelSource`, over one chat-completions URL.

    It lends itself, because there is nothing to load: the endpoint is either
    reachable at request time or it is not, and a session that pretends
    otherwise only moves the failure earlier.
    """

    def __init__(
        self,
        base_url: str,
        model: str,
        *,
        transport: Transport | None = None,
        api_key: str = "",
        profile: ModelProfile = OPENAI,
        tools: tuple[dict[str, Any], ...] = (),
        tool_choice: str = "auto",
        timeout: float = 180.0,
    ) -> None:
        self._url = base_url.rstrip("/") + "/chat/completions"
        self._transport = transport or urllib_transport()
        self._api_key = api_key
        self._profile = profile
        self._tools = tools
        self._tool_choice = tool_choice
        self._timeout = timeout
        #: Read off the model by `Agent` and by the tracer, by these names.
        self._model_name = model
        self._model_provider_type = profile.name
        #: Sources a provider-side tool attached to the last answer.
        self.annotations: tuple[dict[str, Any], ...] = ()

    @contextmanager
    def session(self, name: str = "model") -> Generator[OpenAIChatModel, None, None]:
        del name
        yield self

    def close(self) -> None:
        """The transport belongs to whoever supplied it."""

    def response(self, prompt: str | list[dict[str, str]], **kwargs: Any) -> ModelResponse:
        self.annotations = ()
        started = monotonic()
        body = self._body(prompt, kwargs)
        headers = {"content-type": "application/json"}
        if self._api_key:
            headers["authorization"] = f"Bearer {self._api_key}"

        raw = self._transport(self._url, orjson.dumps(body), headers, self._timeout)
        try:
            payload = orjson.loads(raw)
            choice = payload["choices"][0]
            message = choice["message"]
        except (orjson.JSONDecodeError, LookupError, TypeError) as error:
            raise ModelResponseError(
                f"{self._model_name} answered with no completion to read."
            ) from error

        finish_reason = str(choice.get("finish_reason") or "")
        if self._profile.fail_on_length and finish_reason == "length":
            raise ModelResponseError(
                f"{self._model_name} ran out of output tokens before finishing its answer."
            )

        text = self._content(message)
        annotations = message.get("annotations")
        self.annotations = (
            tuple(row for row in annotations if isinstance(row, dict))
            if isinstance(annotations, list)
            else ()
        )
        usage = payload.get("usage") or {}
        details = usage.get("prompt_tokens_details") or {}
        return ModelResponse(
            text=text,
            model=str(payload.get("model") or self._model_name),
            request_id=str(payload.get("id") or ""),
            finish_reason=finish_reason,
            prompt_tokens=usage.get("prompt_tokens", 0),
            completion_tokens=usage.get("completion_tokens", 0),
            total_tokens=usage.get("total_tokens", 0),
            cached_tokens=details.get("cached_tokens", 0),
            cost=usage.get("cost"),
            latency_ms=(monotonic() - started) * 1000,
            annotations=self.annotations,
        )

    def _body(self, prompt: str | list[dict[str, str]], kwargs: dict[str, Any]) -> dict[str, Any]:
        body: dict[str, Any] = {
            "model": self._model_name,
            "messages": (
                prompt if isinstance(prompt, list) else [{"role": "user", "content": prompt}]
            ),
            "temperature": self._profile.temperature,
            "max_tokens": self._profile.max_tokens,
            **self._profile.extra_body,
            **kwargs,
        }
        if self._tools:
            body.setdefault("tools", list(self._tools))
            body.setdefault("tool_choice", self._tool_choice)
            body.setdefault("parallel_tool_calls", False)
        if self._profile.require_complete_schema:
            _require_every_property(body)
        return body

    def _content(self, message: Mapping[str, Any]) -> str:
        """The answer as the agent's parser reads it, tool calls included.

        A native tool call arrives in the endpoint's own shape and the rest of
        this SDK speaks one portable `{"name", "parameters"}`. Translating here
        is what lets an agent use an endpoint's `tools` API at all without the
        caller reaching into the response.
        """
        calls = message.get("tool_calls") or []
        if not calls:
            text = message.get("content") or ""
            if not isinstance(text, str) or not text.strip():
                raise ModelResponseError(f"{self._model_name} returned an empty answer.")
            return text
        if len(calls) != 1:
            raise ModelResponseError(
                f"{self._model_name} asked for {len(calls)} tools at once and one turn carries "
                "one call."
            )
        function = calls[0].get("function") or {}
        name = function.get("name")
        try:
            arguments = orjson.loads(function.get("arguments") or "")
        except orjson.JSONDecodeError as error:
            raise ModelResponseError(
                f"{self._model_name} called {name!r} with arguments that are not JSON."
            ) from error
        if not isinstance(name, str) or not name or not isinstance(arguments, dict):
            raise ModelResponseError(
                f"{self._model_name} returned a tool call with no name or no argument object."
            )
        return orjson.dumps({"name": name, "parameters": arguments}).decode()


def _require_every_property(body: dict[str, Any]) -> None:
    """Mark every property of a JSON-schema response format required.

    Copied before it is changed: the schema usually belongs to the caller's
    structured output and is reused across calls.
    """
    response_format = body.get("response_format")
    if not isinstance(response_format, dict):
        return
    schema = (response_format.get("json_schema") or {}).get("schema")
    if not isinstance(schema, dict) or not isinstance(schema.get("properties"), dict):
        return
    body["response_format"] = deepcopy(response_format)
    tightened = body["response_format"]["json_schema"]["schema"]
    tightened["required"] = list(tightened["properties"])


def urllib_transport() -> Transport:
    """The standard library as a transport, so this module needs no dependency.

    Enough for a local endpoint and for a test. An application with an HTTP
    client of its own passes that instead, and keeps its own retry policy —
    which matters, because a retried POST to a paid endpoint is a second bill.
    """
    import urllib.error
    import urllib.request

    def post(url: str, body: bytes, headers: Mapping[str, str], timeout: float) -> bytes:
        request = urllib.request.Request(  # noqa: S310 - the caller names the endpoint
            url, data=body, headers=dict(headers), method="POST"
        )
        try:
            with urllib.request.urlopen(request, timeout=timeout) as response:  # noqa: S310
                return bytes(response.read())
        except urllib.error.HTTPError as error:
            detail = error.read()[:500].decode(errors="replace")
            raise ModelResponseError(
                f"the endpoint refused the call ({error.code}): {detail}"
            ) from error
        except urllib.error.URLError as error:
            raise ModelResponseError(f"cannot reach the endpoint: {error.reason}") from error

    return post
