"""aiwatcher behind two of the `agentic` package's interfaces.

`tracer` is the `LLMTracer` a graph already routes every model call through.
`event_store` is `agentic.workflow.EventStore` over one hosted execution —
the shared history a fan-out needs when every agent worker holds its own
SQLite. `payloads` is where the words go while aiwatcher holds only the
reference.

Neither imports the agent's packages. The tracer matches a protocol
structurally; the event store is handed its message type through a codec, and
reaches for `agentic`'s `ConcurrencyConflictError` only if it is importable,
because that one class is caught by name rather than by shape.
"""

from __future__ import annotations

from .event_store import (
    AggregateStreamResult,
    AiwatcherEventStore,
    AppendResult,
    ConcurrencyConflictError,
    MessageCodec,
    MessageRecord,
    ReadAllResult,
    ReadStreamResult,
    SagaTimers,
    TimerPolicy,
    TimerRequest,
    dataclass_codec,
)
from .payloads import (
    FilePayloadStore,
    MemoryPayloadStore,
    PayloadStore,
    SealedPayloadStore,
    digest_of,
    encode_payload,
)
from .tracer import AiwatcherTracer, TeeTracer, aiwatcher_tracer, tee

__all__ = [
    "AggregateStreamResult",
    "AiwatcherEventStore",
    "AiwatcherTracer",
    "AppendResult",
    "ConcurrencyConflictError",
    "FilePayloadStore",
    "MemoryPayloadStore",
    "MessageCodec",
    "MessageRecord",
    "PayloadStore",
    "ReadAllResult",
    "ReadStreamResult",
    "SagaTimers",
    "SealedPayloadStore",
    "TeeTracer",
    "TimerPolicy",
    "TimerRequest",
    "aiwatcher_tracer",
    "dataclass_codec",
    "digest_of",
    "encode_payload",
    "tee",
]
