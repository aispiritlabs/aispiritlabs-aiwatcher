"""aiwatcher behind the `agentic` package's `LLMTracer` interface.

The `agentic` SDK already routes every LLM call, agent execution and tool call
through one injected tracer, and defines that tracer as a `typing.Protocol`.
That makes it a structural interface: this module implements it by matching the
method signatures, with **no import from the agent's packages at all**. aiwatcher
does not depend on the agent, and the agent does not depend on aiwatcher's types.

## How it maps

| `LLMTracer`                       | aiwatcher            |
|-----------------------------------|----------------------|
| `workflow(session_id=…)`          | one run, one trace   |
| `agent(name=…, agent_id=…)`       | an agent span        |
| `llm(model=…, invoke=…)`          | an LLM span + tokens |
| `step(span_type="TOOL")`          | a tool span          |
| `step(span_type=…)`               | a step span, kind carried in the payload |

`session_id` becomes `conversation_id`, the workflow name becomes
`workflow_id`, and each `workflow` gets a fresh `run_id`. That is deliberate:
one chat session usually runs the agent several
times, and a trace that spanned the whole session would never close.

## Nesting is sent, not inferred

Every scope mints a span id and pushes it on a stack, so each event carries an
explicit `span_id` and `parent_span_id`. The backend can infer a parent, and
does for producers that cannot supply one, but inference has a blind spot it
cannot close: a leaf inside a leaf — a model calling a model, an embedding
inside a retrieval — looks the same from outside. A context-manager tracer
already knows the answer, so it says it.

The ids are generated here rather than derived on the backend, and that keeps
redelivery idempotent anyway: the id travels *with* the event, so processing the
same event twice lands on the same span.

An earlier version dropped every `step()` that was not a tool, on the grounds
that chain nodes add span count without adding meaning. That was wrong for this
agent: `step()` is also how retrievals, embeddings and parses are traced, and
those are exactly what a bad answer gets debugged against. They are steps now,
with the kind in `data.step_type`.

## What this cannot see

Two numbers stay empty, and neither is a bug here:

* **Cached tokens.** `agentic`'s `ModelResponse` does not carry them. Several
  likely field names are read speculatively, so this starts working the day the
  provider layer exposes one.
* **Time to first token.** The `llm()` hook wraps a single call and returns when
  it completes; there is no first-token callback to hang a timestamp on. Adding
  one to `LLMTracer` would make TTFT available — aiwatcher already records it
  from an `llm.first_token` event.

## Failure policy

Telemetry never breaks the agent. Every emit is wrapped, the transport is
non-blocking with a bounded queue, and an exception raised while recording is
logged and swallowed — an exception raised by the *agent* is recorded and
re-raised.

## Usage

    from aiwatcher_sdk.integrations.agentic import aiwatcher_tracer, tee

    tracer = tee(build_tracer(backend="mlflow"), aiwatcher_tracer())
"""

from __future__ import annotations

import contextlib
import os
import sys
import time
import uuid
from collections.abc import Callable, Iterator, Mapping
from typing import Any

from aiwatcher_sdk import AiwatcherClient, NullTransport, Transport, _Context

__all__ = ["AiwatcherTracer", "TeeTracer", "aiwatcher_tracer", "tee"]

# `span_type` the agentic toolset uses for a tool call.
_TOOL_SPAN_TYPE = "TOOL"


class _NoopSpan:
    """What every context manager here yields.

    The interface lets a caller annotate a span after the fact. aiwatcher builds
    its spans from start/end events, so there is nothing to annotate — but the
    method has to exist or callers that use it would break.
    """

    def update(
        self,
        *,
        output: Mapping[str, Any] | None = None,
        metadata: Mapping[str, Any] | None = None,
        level: str | None = None,
    ) -> None:
        del output, metadata, level


def _int_or_zero(value: Any) -> int:
    return value if isinstance(value, int) and value >= 0 else 0


class AiwatcherTracer:
    """Emits aiwatcher events from the `agentic` tracing hooks."""

    def __init__(
        self,
        *,
        client: AiwatcherClient | None = None,
        service: str = "ai-spirit-agent",
        base_url: str | None = None,
    ) -> None:
        self._client = client or AiwatcherClient(
            service=service,
            base_url=base_url or os.environ.get("AIWATCHER_URL"),
        )
        # A stack, not a single value: agents nest, and a tool call belongs to
        # the innermost one.
        self._runs: list[_Context] = []
        self._agents: list[_Context] = []
        # Open span ids, innermost last. The top is the parent of whatever
        # opens next; a leaf pushes and pops around its own body so that a
        # nested call sees it.
        self._spans: list[str] = []

    # -- context -----------------------------------------------------------

    @property
    def _run(self) -> _Context | None:
        return self._runs[-1] if self._runs else None

    def _scope(self, agent_id: str | None = None) -> _Context | None:
        """The context an event should be attributed to.

        Innermost agent if one is open, else the run. Returns `None` when
        neither is — an LLM call outside any workflow, which happens in tests
        and one-off scripts and should not crash.
        """
        base = self._agents[-1] if self._agents else self._run
        if base is None:
            return None
        if agent_id is None or agent_id == base.agent_id:
            return base
        return _Context(
            run_id=base.run_id,
            conversation_id=base.conversation_id,
            workflow_id=base.workflow_id,
            agent_id=agent_id,
            correlation_id=base.correlation_id,
            causation_id=base.causation_id,
        )

    @staticmethod
    def _new_span_id() -> str:
        """A W3C span id: 8 bytes as 16 lowercase hex digits."""
        return uuid.uuid4().hex[:16]

    def _emit(
        self,
        event_type: str,
        context: _Context | None,
        data: dict[str, Any],
        *,
        span_id: str | None = None,
        parent_span_id: str | None = None,
    ) -> None:
        if context is None:
            return
        try:
            self._client.emit(
                event_type,
                context,
                data,
                span_id=span_id,
                parent_span_id=parent_span_id,
            )
        except Exception as error:  # noqa: BLE001 - telemetry must not propagate
            print(f"[aiwatcher] dropped {event_type}: {error}", file=sys.stderr)

    @contextlib.contextmanager
    def _scoped_span(self) -> Iterator[tuple[str, str | None]]:
        """Mint a span id, make it the current parent, and pop it after.

        Popping in a `finally` matters: a scope that raises must not leave its
        id on the stack, or every later span in the run would nest under a span
        that already closed.
        """
        parent = self._spans[-1] if self._spans else None
        span_id = self._new_span_id()
        self._spans.append(span_id)
        try:
            yield span_id, parent
        finally:
            if self._spans and self._spans[-1] == span_id:
                self._spans.pop()

    # -- LLMTracer ---------------------------------------------------------

    @contextlib.contextmanager
    def workflow(
        self,
        *,
        name: str,
        session_id: str,
        user_id: str = "",
        metadata: Mapping[str, Any] | None = None,
        tags: Mapping[str, str] | None = None,
        input: Any | None = None,
        tracing_context: Any | None = None,
    ) -> Iterator[_NoopSpan]:
        del input, tracing_context
        # A fresh run per workflow, grouped by session. One chat session runs
        # the agent many times; a trace covering the whole session would never
        # close and would be unreadable in every trace UI.
        context = _Context(
            run_id=f"{session_id or 'session'}-{uuid.uuid4().hex[:8]}",
            conversation_id=session_id or None,
            workflow_id=name or None,
        )
        # Still in the payload as well: the backend reads `data.workflow` as a
        # fallback for producers that predate the field, and dropping it here
        # would break replays of logs written before this change.
        payload: dict[str, Any] = {"workflow": name}
        if user_id:
            payload["user_id"] = user_id
        if tags:
            payload.update({str(k): str(v) for k, v in tags.items()})
        if metadata:
            payload["metadata"] = {str(k): str(v) for k, v in metadata.items()}

        self._runs.append(context)
        with self._scoped_span() as (span_id, parent_span):
            self._emit("run.started", context, payload, span_id=span_id, parent_span_id=parent_span)
            try:
                yield _NoopSpan()
            except BaseException as error:
                # BaseException, not Exception: a cancelled run that reports
                # nothing is indistinguishable from a hung one.
                self._emit(
                    "run.failed",
                    context,
                    {**payload, "status": "failed", "error": str(error)},
                    span_id=span_id,
                )
                raise
            else:
                self._emit(
                    "run.completed",
                    context,
                    {**payload, "status": "succeeded"},
                    span_id=span_id,
                )
            finally:
                if self._runs and self._runs[-1] is context:
                    self._runs.pop()

    @contextlib.contextmanager
    def agent(
        self,
        *,
        name: str,
        agent_id: str = "",
        input: Any | None = None,
        attributes: Mapping[str, Any] | None = None,
    ) -> Iterator[_NoopSpan]:
        del input
        parent = self._scope()
        if parent is None:
            # No workflow open. Emitting an orphan agent span would produce a
            # trace with no root, so this one is simply not recorded.
            yield _NoopSpan()
            return

        context = _Context(
            run_id=parent.run_id,
            conversation_id=parent.conversation_id,
            workflow_id=parent.workflow_id,
            agent_id=agent_id or name,
            correlation_id=parent.correlation_id,
            causation_id=parent.correlation_id,
        )
        payload: dict[str, Any] = {"agent_name": name}
        if attributes:
            payload.update({str(k): str(v) for k, v in attributes.items()})

        self._agents.append(context)
        with self._scoped_span() as (span_id, parent_span):
            self._emit(
                "agent.started", context, payload, span_id=span_id, parent_span_id=parent_span
            )
            try:
                yield _NoopSpan()
            except BaseException as error:
                self._emit(
                    "agent.failed",
                    context,
                    {**payload, "error": str(error)},
                    span_id=span_id,
                )
                raise
            else:
                self._emit("agent.completed", context, payload, span_id=span_id)
            finally:
                if self._agents and self._agents[-1] is context:
                    self._agents.pop()

    @contextlib.contextmanager
    def step(
        self,
        *,
        name: str,
        input: Any | None = None,
        attributes: Mapping[str, Any] | None = None,
        span_type: str = "CHAIN",
    ) -> Iterator[_NoopSpan]:
        del input
        attributes = attributes or {}
        kind = span_type.upper()

        if kind == _TOOL_SPAN_TYPE:
            event_prefix = "tool"
            payload: dict[str, Any] = {
                # `call_id` is what separates two calls in one agent; without it
                # they would collapse into a single span.
                "call_id": uuid.uuid4().hex[:12],
                "tool_name": str(attributes.get("tool_name") or name.removeprefix("tool.")),
            }
        else:
            # Everything else — chains, retrievals, embeddings, parses,
            # guardrails — is a step, with the kind in the payload rather than
            # in the event type. A step kind aiwatcher has never seen still
            # produces a span; see the backend's `Subject::Step`.
            event_prefix = "step"
            payload = {
                "call_id": uuid.uuid4().hex[:12],
                "step_type": span_type.lower(),
                "name": name,
            }
        # Anything else the caller attached rides along; the backend reads the
        # retrieval-shaped keys and stores the rest.
        payload.update({str(key): value for key, value in attributes.items() if key != "tool_name"})

        context = self._scope()
        started = time.monotonic()

        with self._scoped_span() as (span_id, parent_span):
            self._emit(
                f"{event_prefix}.started",
                context,
                payload,
                span_id=span_id,
                parent_span_id=parent_span,
            )
            try:
                yield _NoopSpan()
            except BaseException as error:
                self._emit(
                    f"{event_prefix}.failed",
                    context,
                    {**payload, "error": str(error), "duration_ms": _elapsed_ms(started)},
                    span_id=span_id,
                )
                raise
            else:
                self._emit(
                    f"{event_prefix}.completed",
                    context,
                    {**payload, "duration_ms": _elapsed_ms(started)},
                    span_id=span_id,
                )

    def llm(
        self,
        *,
        name: str,
        model: str,
        messages: list[dict[str, Any]],
        invoke: Callable[..., Any],
        **kwargs: Any,
    ) -> Any:
        del name
        context = self._scope()
        payload: dict[str, Any] = {"call_id": uuid.uuid4().hex[:12], "model": model}
        if provider := kwargs.get("provider"):
            payload["provider"] = str(provider)
        if temperature := kwargs.get("temperature"):
            payload["temperature"] = temperature
        payload["message_count"] = len(messages)

        started = time.monotonic()
        # Scoped even though an LLM call is usually a leaf: a model that calls
        # a model nests, and that is exactly the shape backend inference cannot
        # see.
        with self._scoped_span() as (span_id, parent_span):
            self._emit("llm.started", context, payload, span_id=span_id, parent_span_id=parent_span)
            try:
                response = invoke(**kwargs)
            except BaseException as error:
                self._emit(
                    "llm.failed",
                    context,
                    {**payload, "error": str(error), "duration_ms": _elapsed_ms(started)},
                    span_id=span_id,
                )
                raise

            # `ModelResponse` is read by attribute rather than imported: this
            # module deliberately has no dependency on the agent's packages, and
            # anything carrying these fields works.
            completed = {
                **payload,
                "duration_ms": getattr(response, "latency_ms", None) or _elapsed_ms(started),
                "prompt_tokens": _int_or_zero(getattr(response, "prompt_tokens", 0)),
                "completion_tokens": _int_or_zero(getattr(response, "completion_tokens", 0)),
            }
            # `agentic`'s ModelResponse has no cached-token field today, so the
            # panel's cache-hit number stays empty for this agent. Read it
            # anyway: the moment the provider layer surfaces it under any of
            # these names it starts flowing with no change here.
            for source in ("cached_tokens", "cache_read_tokens", "cache_read_input_tokens"):
                if (cached := _int_or_zero(getattr(response, source, 0))) > 0:
                    completed["cached_tokens"] = cached
                    break
            if response_model := getattr(response, "model", ""):
                completed["response_model"] = str(response_model)
            if finish_reason := getattr(response, "finish_reason", ""):
                completed["finish_reason"] = str(finish_reason)
            if request_id := getattr(response, "request_id", ""):
                completed["response_id"] = str(request_id)

            self._emit("llm.completed", context, completed, span_id=span_id)
            return response

    # -- the rest of the interface ----------------------------------------

    @property
    def current_trace(self) -> Any | None:
        # aiwatcher derives trace and span ids on the backend, so there is no
        # id to hand back here. Returning `None` is what the agent's own noop
        # tracer does, and callers already handle it.
        return None

    @property
    def current_trace_id(self) -> str | None:
        run = self._run
        return run.run_id if run is not None else None

    def get_trace_url(self) -> str | None:
        run = self._run
        base = os.environ.get("AIWATCHER_PANEL_URL")
        if run is None or not base:
            return None
        return f"{base.rstrip('/')}/runs/{run.run_id}"

    def flush(self) -> None:
        with contextlib.suppress(Exception):
            self._client.close()

    def shutdown(self, timeout_seconds: float = 5.0) -> None:
        del timeout_seconds
        self.flush()


def _elapsed_ms(started: float) -> float:
    return (time.monotonic() - started) * 1000


class TeeTracer:
    """Fans every hook out to several tracers.

    So aiwatcher goes in alongside MLflow rather than replacing it.

    `llm` is the one that needs care: it takes an `invoke` callable and must run
    it **exactly once**. The tracers are therefore nested rather than looped
    over — each one's `invoke` calls the next, and the innermost calls the real
    one.
    """

    def __init__(self, *tracers: Any) -> None:
        self._tracers = [tracer for tracer in tracers if tracer is not None]

    @contextlib.contextmanager
    def workflow(self, **kwargs: Any) -> Iterator[Any]:
        with contextlib.ExitStack() as stack:
            handles = [stack.enter_context(t.workflow(**kwargs)) for t in self._tracers]
            yield handles[0] if handles else _NoopSpan()

    @contextlib.contextmanager
    def agent(self, **kwargs: Any) -> Iterator[Any]:
        with contextlib.ExitStack() as stack:
            handles = [stack.enter_context(t.agent(**kwargs)) for t in self._tracers]
            yield handles[0] if handles else _NoopSpan()

    @contextlib.contextmanager
    def step(self, **kwargs: Any) -> Iterator[Any]:
        with contextlib.ExitStack() as stack:
            handles = [stack.enter_context(t.step(**kwargs)) for t in self._tracers]
            yield handles[0] if handles else _NoopSpan()

    def llm(self, *, invoke: Callable[..., Any], **kwargs: Any) -> Any:
        def nest(index: int) -> Callable[..., Any]:
            if index >= len(self._tracers):
                return invoke

            def call(**call_kwargs: Any) -> Any:
                return self._tracers[index].llm(invoke=nest(index + 1), **kwargs | call_kwargs)

            return call

        return nest(0)()

    @property
    def current_trace(self) -> Any | None:
        for tracer in self._tracers:
            if (trace := tracer.current_trace) is not None:
                return trace
        return None

    @property
    def current_trace_id(self) -> str | None:
        # `str(...)` rather than a cast: these come out of a third-party tracer
        # whose own annotations are `Any`, and a trace id that is not a string
        # would fail later, in the middle of a request, instead of here.
        for tracer in self._tracers:
            if (trace_id := tracer.current_trace_id) is not None:
                return str(trace_id)
        return None

    def get_trace_url(self) -> str | None:
        for tracer in self._tracers:
            if (url := tracer.get_trace_url()) is not None:
                return str(url)
        return None

    def flush(self) -> None:
        for tracer in self._tracers:
            with contextlib.suppress(Exception):
                tracer.flush()

    def shutdown(self, timeout_seconds: float = 5.0) -> None:
        for tracer in self._tracers:
            with contextlib.suppress(Exception):
                tracer.shutdown(timeout_seconds)


def aiwatcher_tracer(
    *,
    service: str = "ai-spirit-agent",
    base_url: str | None = None,
    transport: Transport | None = None,
) -> AiwatcherTracer:
    """A tracer that publishes to aiwatcher, or discards if it is not configured.

    With no `AIWATCHER_URL` and no explicit transport the client uses a null
    transport, so importing and wiring this is safe in every environment.
    """
    resolved = base_url or os.environ.get("AIWATCHER_URL")
    client = AiwatcherClient(
        service=service,
        base_url=resolved,
        transport=transport or (None if resolved else NullTransport()),
    )
    return AiwatcherTracer(client=client)


def tee(*tracers: Any) -> TeeTracer:
    """Compose tracers so each one sees every hook."""
    return TeeTracer(*tracers)
