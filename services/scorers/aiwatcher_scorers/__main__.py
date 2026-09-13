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

LOOPBACK = frozenset({"127.0.0.1", "::1", "localhost"})


def refusal(token: str | None, host: str, unauthenticated: bool) -> str | None:
    """Why this process must not start, or ``None`` when it may.

    It is sent the cases it scores. Without a token it binds where only this
    machine reaches; anywhere else it wants one, or somebody saying in
    ``AIWATCHER_SCORERS_UNAUTHENTICATED`` that the network is the fence.
    """
    if token is not None or host in LOOPBACK or unauthenticated:
        return None
    return (
        f"AIWATCHER_SCORERS_TOKEN is unset and AIWATCHER_SCORERS_HOST is {host}: "
        "this service is sent the cases it scores, so off localhost it wants a token "
        "(set AIWATCHER_SCORERS_UNAUTHENTICATED=true where only aiwatcher reaches the port)"
    )


def main() -> None:
    logging.basicConfig(level=logging.INFO, format="%(levelname)s %(name)s %(message)s")
    token = os.environ.get("AIWATCHER_SCORERS_TOKEN") or None
    host = os.environ.get("AIWATCHER_SCORERS_HOST", "127.0.0.1")
    port = int(os.environ.get("AIWATCHER_SCORERS_PORT", "8083"))
    # Before the frameworks are imported, which takes seconds a refusal need not wait for.
    unauthenticated = os.environ.get("AIWATCHER_SCORERS_UNAUTHENTICATED", "").lower() == "true"
    refused = refusal(token, host, unauthenticated)
    if refused is not None:
        raise SystemExit(refused)
    names = [
        name.strip()
        for name in os.environ.get("AIWATCHER_SCORERS_ADAPTERS", "deepeval,opik").split(",")
        if name.strip()
    ]
    model = ModelSettings.from_env()
    adapters = load(names, model)
    log.info(
        "serving %s on http://%s:%s; graded metrics %s",
        ", ".join(f"{adapter.name} {adapter.version}" for adapter in adapters) or "no adapter",
        host,
        port,
        f"ask {model.name} {model.revision} at {model.url}" if model else "are not offered",
    )
    if token is None:
        log.warning(
            "unauthenticated (AIWATCHER_SCORERS_TOKEN is unset) and sent the cases it scores%s",
            "" if host in LOOPBACK else f": bound to {host} with the network as its only fence",
        )
    uvicorn.run(create_app(adapters, token), host=host, port=port, log_level="warning")


if __name__ == "__main__":
    main()
