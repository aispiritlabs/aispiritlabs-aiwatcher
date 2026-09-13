"""``python -m aiwatcher_scorers`` — the service on localhost.

Started by ``just scorers-serve``. It says which adapters it loaded and which
model its graded metrics ask, because both are pinned into every card measured
with them and somebody running this needs to have read them once.
"""

from __future__ import annotations

import logging
import os

import uvicorn

from aiwatcher_scorers.adapters import load
from aiwatcher_scorers.model import ModelSettings
from aiwatcher_scorers.service import create_app

log = logging.getLogger("aiwatcher_scorers")


def main() -> None:
    logging.basicConfig(level=logging.INFO, format="%(levelname)s %(name)s %(message)s")
    names = [
        name.strip()
        for name in os.environ.get("AIWATCHER_SCORERS_ADAPTERS", "deepeval,opik").split(",")
        if name.strip()
    ]
    model = ModelSettings.from_env()
    adapters = load(names, model)
    token = os.environ.get("AIWATCHER_SCORERS_TOKEN") or None
    host = os.environ.get("AIWATCHER_SCORERS_HOST", "127.0.0.1")
    port = int(os.environ.get("AIWATCHER_SCORERS_PORT", "8083"))
    log.info(
        "serving %s on http://%s:%s; graded metrics %s",
        ", ".join(f"{adapter.name} {adapter.version}" for adapter in adapters) or "no adapter",
        host,
        port,
        f"ask {model.name} {model.revision} at {model.url}" if model else "are not offered",
    )
    if token is None:
        log.warning(
            "unauthenticated (AIWATCHER_SCORERS_TOKEN is unset) and sent the cases it scores: "
            "bind it where only aiwatcher reaches"
        )
    uvicorn.run(create_app(adapters, token), host=host, port=port, log_level="warning")


if __name__ == "__main__":
    main()
