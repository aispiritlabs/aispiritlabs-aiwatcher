"""aiwatcher behind two of the `agentic` package's interfaces.

`tracer` is the `LLMTracer` a graph already routes every model call through.
`event_store` is `agentic.workflow.EventStore` over one hosted execution —
the shared history a fan-out needs when every agent worker holds its own
SQLite. `payloads` is where the words go while aiwatcher holds only the
reference. Handed an outbox, the store writes a hop down before it sends it,
and `deliver` / `stream_sender` are what drains one.

Neither imports the agent's packages. The tracer matches a protocol
structurally; the event store is handed its message type through a codec, and
reaches for `agentic`'s `ConcurrencyConflictError` only if it is importable,
because that one class is caught by name rather than by shape.
"""

from __future__ import annotations

from .event_store import (
    STREAM_APPEND,
    AggregateStreamResult,
    AiwatcherEventStore,
    AppendResult,
    ConcurrencyConflictError,
    JoinTimers,
    MessageCodec,
    MessageRecord,
    ReadAllResult,
    ReadStreamResult,
    SagaTimers,
    TimerPolicy,
    TimerRequest,
    UndeliveredHopsError,
    dataclass_codec,
    deliver,
    stream_sender,
)
from .graph import as_topology, declare_graph
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
    "STREAM_APPEND",
    "AggregateStreamResult",
    "AiwatcherEventStore",
    "AiwatcherTracer",
    "AppendResult",
    "ConcurrencyConflictError",
    "FilePayloadStore",
    "JoinTimers",
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
    "UndeliveredHopsError",
    "aiwatcher_tracer",
    "as_topology",
    "dataclass_codec",
    "declare_graph",
    "deliver",
    "digest_of",
    "encode_payload",
    "stream_sender",
    "tee",
]
