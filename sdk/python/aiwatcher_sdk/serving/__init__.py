"""Serve the model version a label names, and keep serving it.

The consumer end of the model registry. Everything else in this SDK writes to
aiwatcher — telemetry, training runs, annotations, turns; this reads one
decision back out and acts on it, which is the only place in the library where
the registry's answer becomes something that happens rather than something
that is recorded.

    from aiwatcher_sdk.serving import Server, serve
    from aiwatcher_sdk.training import TrainingClient

    state = Server(TrainingClient("http://aiwatcher:8080"), "vision.edge-detector", telemetry)
    state.start()          # resolve `production`, verify every digest, warm it
    serve(state, port=8091)

Two halves, and the split is the whole design. :mod:`~aiwatcher_sdk.serving.server`
holds what every framework needs — resolve, verify, warm, bound, validate,
watch, roll back, report — and :mod:`~aiwatcher_sdk.serving.runtimes` holds
what one framework needs, which is four members. A runtime is selected by the
name the *package* declares (ADR_0023) and a name with no loader is refused
rather than attempted.

What this never does is put an inference's inputs or outputs on the event log.
A run per request carries the model, the version, the runtime, the row count,
the latency and the outcome; a runtime that wants to keep the content writes
turns to the conversation archive, with consent and a retention clock, exactly
as an agent does. See ADR_0021 and ADR_0023.
"""

from __future__ import annotations

from aiwatcher_sdk.serving.artifact import (
    ArtifactReader,
    FileReader,
    LoadError,
    S3Credentials,
    S3Reader,
    SchemeReader,
    VersionCacheReader,
    read_verified,
)
from aiwatcher_sdk.serving.loader import Loaded, Loader, Predictor, load
from aiwatcher_sdk.serving.runtimes import available
from aiwatcher_sdk.serving.server import (
    LABEL,
    MAX_BODY_BYTES,
    ModelSource,
    Server,
    resolve,
    resolve_label,
    serve,
    warm,
)

__all__ = [
    "LABEL",
    "MAX_BODY_BYTES",
    "ArtifactReader",
    "FileReader",
    "LoadError",
    "Loaded",
    "Loader",
    "ModelSource",
    "Predictor",
    "S3Credentials",
    "S3Reader",
    "SchemeReader",
    "Server",
    "VersionCacheReader",
    "available",
    "load",
    "read_verified",
    "resolve",
    "resolve_label",
    "serve",
    "warm",
]
