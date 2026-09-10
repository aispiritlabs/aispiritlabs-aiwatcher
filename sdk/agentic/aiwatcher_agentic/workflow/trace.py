"""The trace a message was recorded under, and the tags a workflow is traced with.

Plain values, carried on every recorded message's metadata and rebuilt by the
serializer. They name a span; they never open one — that is a tracer's, and
:mod:`aiwatcher_agentic.workflow.tracer` is the one the engine asks for.
"""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass, field
from typing import Any


@dataclass(frozen=True, slots=True)
class TracingContext:
    session_id: str = ""
    user_id: str = ""
    runtime_id: str = ""
    turn_id: str = ""
    workflow: str = ""
    domain: str = ""
    tags: Mapping[str, str] = field(default_factory=dict)
    metadata: Mapping[str, Any] = field(default_factory=dict)


@dataclass(frozen=True, slots=True)
class TraceSnapshot:
    session_id: str = ""
    trace_id: str = ""
    span_id: str = ""
    parent_span_id: str = ""
    span_name: str = ""
    span_type: str = ""


def build_trace_snapshot(
    trace: TraceSnapshot | None = None,
    *,
    session_id: str = "",
    trace_id: str | None = None,
    span_id: str | None = None,
    parent_span_id: str | None = None,
    span_name: str | None = None,
    span_type: str | None = None,
) -> TraceSnapshot | None:
    """`trace`'s fields where it has them, the keywords where it does not.

    None when nothing at all is known, so a message recorded outside any trace
    carries no empty snapshot that reads as one.
    """
    resolved_trace_id = trace.trace_id if trace is not None and trace.trace_id else (trace_id or "")
    resolved_span_id = trace.span_id if trace is not None and trace.span_id else (span_id or "")
    resolved_parent_span_id = (
        trace.parent_span_id
        if trace is not None and trace.parent_span_id
        else (parent_span_id or "")
    )
    resolved_span_name = (
        trace.span_name if trace is not None and trace.span_name else (span_name or "")
    )
    resolved_span_type = (
        trace.span_type if trace is not None and trace.span_type else (span_type or "")
    )
    resolved_session_id = trace.session_id if trace is not None and trace.session_id else session_id

    if not any(
        (
            resolved_session_id,
            resolved_trace_id,
            resolved_span_id,
            resolved_parent_span_id,
            resolved_span_name,
            resolved_span_type,
        )
    ):
        return None

    return TraceSnapshot(
        session_id=resolved_session_id,
        trace_id=resolved_trace_id,
        span_id=resolved_span_id,
        parent_span_id=resolved_parent_span_id,
        span_name=resolved_span_name,
        span_type=resolved_span_type,
    )
