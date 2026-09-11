"""The contract every Python query engine serves (AW-3).

An engine's package is its `Engine` and a `__main__` that calls `serve` with a reference
to it; everything else — the routes, the catalog, the API pager, the answer, the child
a query runs in — is here, once.
"""

from __future__ import annotations

import sys

from aiwatcher_query.catalog import Catalog, CatalogError
from aiwatcher_query.child import Limits
from aiwatcher_query.config import Admission, Config, ConfigError
from aiwatcher_query.engine import Engine, FileFormat, Session, load_engine
from aiwatcher_query.log import configure, logger
from aiwatcher_query.sandbox import Sandbox
from aiwatcher_query.service import PREFIX, create_app

__all__ = ["Engine", "FileFormat", "Session", "serve"]


def serve(engine_ref: str) -> None:
    """Read the configuration, start the fork server, and serve `/query` on it."""
    import uvicorn

    configure()
    log = logger(__name__)
    try:
        config = Config.from_env()
        catalog = Catalog.load(config.catalog, corpus_root=config.corpus)
    except (ConfigError, CatalogError) as error:
        sys.exit(f"aiwatcher-query: {error}")
    engine = load_engine(engine_ref)
    sandbox = Sandbox(engine_ref, Limits(config.timeout_seconds, config.memory_mb))
    sandbox.start()
    app = create_app(config, engine=engine, engine_ref=engine_ref, catalog=catalog, runner=sandbox)
    log.info(
        "service.start",
        engine=engine.name,
        version=engine.version(),
        url=f"http://{config.host}:{config.port}{PREFIX}/",
        aiwatcher=config.aiwatcher,
        catalog=str(config.catalog),
        corpus=str(config.corpus) if config.corpus else None,
        admission=config.admission.value,
        timeout_seconds=config.timeout_seconds,
    )
    if config.admission is Admission.OPEN:
        log.warning(
            "service.open_admission",
            detail="runs the Python a query is written in, in a child with ceilings and no "
            "credentials — localhost only (ADR_0028)",
        )
    uvicorn.run(app, host=config.host, port=config.port, log_level="warning")
