"""The adapters this service ships, and the one place that knows their names.

``KNOWN`` maps a name a deployment lists in ``AIWATCHER_SCORERS_ADAPTERS`` to a
loader. A loader answers ``None`` when its framework is not installed — an
extra nobody asked for is a working state, and the catalog then says what this
process does measure rather than failing to start.
"""

from __future__ import annotations

import logging
import os
from collections.abc import Callable, Sequence

from aiwatcher_scorers.adapter import Adapter
from aiwatcher_scorers.model import ModelSettings

log = logging.getLogger(__name__)

type Loader = Callable[[ModelSettings | None], Adapter | None]


def quiet() -> None:
    """Keep both frameworks from sending anything anywhere but the model.

    Each phones home by default — DeepEval's telemetry, Opik's tracing to its
    own server, LiteLLM's price table from GitHub — and a scorer service that
    is sent a conversation archive's words must send them nowhere else. Set
    before either is imported, because both read these at import.
    """
    os.environ.setdefault("DEEPEVAL_TELEMETRY_OPT_OUT", "YES")
    os.environ.setdefault("DEEPEVAL_UPDATE_WARNING_OPT_IN", "NO")
    os.environ.setdefault("DEEPEVAL_DISABLE_PROGRESS_BAR", "YES")
    os.environ.setdefault("OPIK_TRACK_DISABLE", "true")
    os.environ.setdefault("LITELLM_LOCAL_MODEL_COST_MAP", "True")
    # And LiteLLM's own log, which at its default level narrates every call.
    os.environ.setdefault("LITELLM_LOG", "ERROR")
    logging.getLogger("LiteLLM").setLevel(logging.WARNING)


def _deepeval(model: ModelSettings | None) -> Adapter | None:
    from aiwatcher_scorers.adapters import deepeval

    return deepeval.load(model)


def _opik(model: ModelSettings | None) -> Adapter | None:
    from aiwatcher_scorers.adapters import opik

    return opik.load(model)


KNOWN: dict[str, Loader] = {"deepeval": _deepeval, "opik": _opik}


def load(names: Sequence[str], model: ModelSettings | None) -> list[Adapter]:
    quiet()
    loaded: list[Adapter] = []
    for name in names:
        loader = KNOWN.get(name)
        if loader is None:
            raise ValueError(f"no adapter `{name}`; this service ships {', '.join(sorted(KNOWN))}")
        adapter = loader(model)
        if adapter is None:
            log.warning("adapter %s is not installed (uv sync --extra %s); left out", name, name)
            continue
        loaded.append(adapter)
    return loaded
