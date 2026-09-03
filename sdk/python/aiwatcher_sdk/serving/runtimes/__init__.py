"""One module per runtime, and the table that selects between them.

The table is the point. ADR_0023's rule is that a runtime is declared and
never sniffed, so the only way a loader is chosen here is by a string the
package wrote — and a string that is not a key is a refusal naming it, never a
fallback to whichever loader happens to be first.

:func:`available` is what a host registers. It takes ``weights`` for free,
because that loader has no dependency, and adds ``onnx`` only when the caller
asks for it: a process that will never serve a graph should not be a process
that can be talked into importing a runtime.
"""

from __future__ import annotations

from aiwatcher_sdk.serving.loader import Loader
from aiwatcher_sdk.serving.runtimes.weights import WeightsLoader

__all__ = ["WeightsLoader", "available"]


def available(*, onnx: bool = True, threads: int = 1) -> dict[str, Loader]:
    """The loaders this host is willing to select from.

    `onnx` may be off for a host that should be unable to load a graph at all,
    and `threads` bounds what one ONNX request may spend — the concurrency gate
    in front of it bounds how many are in flight.
    """
    loaders: dict[str, Loader] = {"weights": WeightsLoader()}
    if onnx:
        from aiwatcher_sdk.serving.runtimes.onnx import OnnxLoader

        loaders["onnx"] = OnnxLoader(threads=threads)
    return loaders
