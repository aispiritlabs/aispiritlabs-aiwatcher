"""The tracer an agent calls: the engine's two spans, an agent span and a model call.

`aiwatcher_agentic.workflow.tracer.WorkflowTracer` is the part the engine
needs; this is everything an `Agent` and its tools call, so an `LLMTracer` is
a `WorkflowTracer` too. Implementations live with what they report to — the
MLflow one in `ai_spirit_agent`'s `agentic.observability`, aiwatcher's in
`aiwatcher_sdk.integrations.agentic` — and satisfy this structurally, so
neither is imported here.
"""

from __future__ import annotations

from collections.abc import Callable, Generator, Mapping
from contextlib import AbstractContextManager, contextmanager
from typing import Any, Protocol

from aiwatcher_agentic.model import ModelResponse
from aiwatcher_agentic.workflow.trace import TraceSnapshot, TracingContext
from aiwatcher_agentic.workflow.tracer import NOOP_HANDLE, SpanHandle


class LLMTracer(Protocol):
    def workflow(
        self,
        *,
        name: str,
        session_id: str,
        user_id: str = "",
        metadata: Mapping[str, Any] | None = None,
        tags: Mapping[str, str] | None = None,
        input: Any | None = None,
        tracing_context: TracingContext | None = None,
    ) -> AbstractContextManager[SpanHandle]: ...

    def agent(
        self,
        *,
        name: str,
        agent_id: str = "",
        input: Any | None = None,
        attributes: Mapping[str, Any] | None = None,
    ) -> AbstractContextManager[SpanHandle]: ...

    def step(
        self,
        *,
        name: str,
        input: Any | None = None,
        attributes: Mapping[str, Any] | None = None,
        span_type: str = "CHAIN",
    ) -> AbstractContextManager[SpanHandle]: ...

    def llm(
        self,
        *,
        name: str,
        model: str,
        messages: list[dict[str, Any]],
        invoke: Callable[..., ModelResponse],
        **kwargs: Any,
    ) -> ModelResponse: ...

    @property
    def current_trace(self) -> TraceSnapshot | None: ...

    @property
    def current_trace_id(self) -> str | None: ...

    def get_trace_url(self) -> str | None: ...

    def flush(self) -> None: ...

    def shutdown(self, timeout_seconds: float = 5.0) -> None: ...


class NoopLLMTracer:
    @contextmanager
    def workflow(
        self,
        *,
        name: str,
        session_id: str,
        user_id: str = "",
        metadata: Mapping[str, Any] | None = None,
        tags: Mapping[str, str] | None = None,
        input: Any | None = None,
        tracing_context: TracingContext | None = None,
    ) -> Generator[SpanHandle, None, None]:
        yield NOOP_HANDLE

    @contextmanager
    def agent(
        self,
        *,
        name: str,
        agent_id: str = "",
        input: Any | None = None,
        attributes: Mapping[str, Any] | None = None,
    ) -> Generator[SpanHandle, None, None]:
        yield NOOP_HANDLE

    @contextmanager
    def step(
        self,
        *,
        name: str,
        input: Any | None = None,
        attributes: Mapping[str, Any] | None = None,
        span_type: str = "CHAIN",
    ) -> Generator[SpanHandle, None, None]:
        yield NOOP_HANDLE

    def llm(
        self,
        *,
        name: str,
        model: str,
        messages: list[dict[str, Any]],
        invoke: Callable[..., ModelResponse],
        **kwargs: Any,
    ) -> ModelResponse:
        return invoke()

    @property
    def current_trace(self) -> TraceSnapshot | None:
        return None

    @property
    def current_trace_id(self) -> str | None:
        return None

    def get_trace_url(self) -> str | None:
        return None

    def flush(self) -> None:
        pass

    def shutdown(self, timeout_seconds: float = 5.0) -> None:
        pass
