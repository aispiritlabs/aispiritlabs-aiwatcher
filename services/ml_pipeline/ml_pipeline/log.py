"""One place that decides what this service's output looks like.

Two renderings, chosen by whether anybody is watching. At a terminal — which is
`just ml-pipeline-serve`, and how this is nearly always run — the console
renderer, because the reader is a person who has just seen a notebook fail. Not
at a terminal, JSON through orjson, because then the reader is whatever is
collecting logs.

The events are named for what happened rather than written as sentences:
`notebook.run` with `notebook`, `rows_in`, `rows_out` and `took_ms` as fields
is greppable and comparable, and "Ran the notebook pii_detection over 50 rows"
is neither.
"""

from __future__ import annotations

import logging
import sys

import orjson
import structlog


def configure(level: int = logging.INFO) -> None:
    """Set up structlog. Called once, by the entry point."""
    interactive = sys.stderr.isatty()
    structlog.configure(
        processors=[
            structlog.contextvars.merge_contextvars,
            structlog.processors.add_log_level,
            structlog.processors.TimeStamper(fmt="iso", utc=True),
            structlog.processors.StackInfoRenderer(),
            structlog.processors.format_exc_info,
            structlog.dev.ConsoleRenderer()
            if interactive
            # orjson returns bytes, which is what `BytesLoggerFactory` writes —
            # pairing anything else with it is the mistake this comment exists
            # to prevent.
            else structlog.processors.JSONRenderer(serializer=orjson.dumps),
        ],
        wrapper_class=structlog.make_filtering_bound_logger(level),
        logger_factory=(
            structlog.PrintLoggerFactory(file=sys.stderr)
            if interactive
            else structlog.BytesLoggerFactory()
        ),
        cache_logger_on_first_use=True,
    )


def logger(name: str) -> structlog.stdlib.BoundLogger:
    """A logger bound to the module that asked for it."""
    return structlog.get_logger(name)  # type: ignore[no-any-return]
