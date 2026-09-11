"""The part of a tracer the engine calls.

A turn opens a workflow span and a consumer opens a step span per reactor
call; neither ever calls a model, so this protocol has no ``llm`` method and
names no model response type. That is the whole reason it exists apart from
`agentic.observability.LLMTracer`, which is a superset of it: any tracer an
agent already builds satisfies this one structurally, and the engine does not
import a provider stack to say so.
"""

from __future__ import annotations

from collections.abc import Generator, Mapping
from contextlib import AbstractContextManager, contextmanager
from typing import Any, Protocol

from aiwatcher_agentic.workflow.trace import TraceSnapshot, TracingContext


class SpanHandle(Protocol):
    """What a span context yields, so the code inside can annotate it.

    Tool execution supplies ``metadata["agentic.tool_status"]`` as
    ``success``, ``error`` or ``retry`` before returning a ToolRunResult.
    ERROR carries ``output["error"]``; RETRY carries ``output["retry"]``
    with level WARNING. A plain WARNING is not a failed tool result.
    Annotation failures must not escape into tool execution.
    """

    def update(
        self,
        *,
        output: Mapping[str, Any] | None = None,
        metadata: Mapping[str, Any] | None = None,
        level: str | None = None,
    ) -> None: ...


class NoopSpanHandle:
    def update(
        self,
        *,
        output: Mapping[str, Any] | None = None,
        metadata: Mapping[str, Any] | None = None,
        level: str | None = None,
    ) -> None:
        pass


NOOP_HANDLE = NoopSpanHandle()


class WorkflowTracer(Protocol):
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

    def step(
        self,
        *,
        name: str,
        input: Any | None = None,
        attributes: Mapping[str, Any] | None = None,
        span_type: str = "CHAIN",
    ) -> AbstractContextManager[SpanHandle]: ...

    @property
    def current_trace(self) -> TraceSnapshot | None: ...

    @property
    def current_trace_id(self) -> str | None: ...


class NoopWorkflowTracer:
    """What the engine traces with when it was handed no tracer."""

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
    def step(
        self,
        *,
        name: str,
        input: Any | None = None,
        attributes: Mapping[str, Any] | None = None,
        span_type: str = "CHAIN",
    ) -> Generator[SpanHandle, None, None]:
        yield NOOP_HANDLE

    @property
    def current_trace(self) -> TraceSnapshot | None:
        return None

    @property
    def current_trace_id(self) -> str | None:
        return None
