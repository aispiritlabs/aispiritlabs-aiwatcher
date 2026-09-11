"""One place that decides what this service's output looks like.

The notebook runtime's shape: at a terminal the console renderer, because the reader
is a person who has just seen a query fail; not at a terminal, JSON, because then the
reader is whatever collects logs. Events are named for what happened — `query.run`
with `engine`, `rows` and `took_ms` as fields — rather than written as sentences.
"""

from __future__ import annotations

import logging
import sys

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
            structlog.dev.ConsoleRenderer() if interactive else structlog.processors.JSONRenderer(),
        ],
        wrapper_class=structlog.make_filtering_bound_logger(level),
        logger_factory=structlog.PrintLoggerFactory(file=sys.stderr),
        cache_logger_on_first_use=True,
    )


def logger(name: str) -> structlog.stdlib.BoundLogger:
    """A logger bound to the module that asked for it."""
    return structlog.get_logger(name)  # type: ignore[no-any-return]
