"""What this service was told, read once at start-up and passed down.

Nothing below reads the environment again, and that is a boundary rather than a
habit: once the configuration is read, `AIWATCHER_*` is removed from the process's
environment before the fork server starts (`sandbox.Sandbox.start`), so the child a
query runs in inherits none of it. A value a query needs — where aiwatcher is, where
the corpus is — reaches the child as a field of the job it is handed.

A malformed value is refused naming the variable, where the notebook runtime falls
back to its default: a ceiling somebody set and this service quietly ignored is a
ceiling nobody has.
"""

from __future__ import annotations

import os
from collections.abc import Mapping
from dataclasses import dataclass
from enum import StrEnum
from pathlib import Path

CONTRACT_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_CATALOG = CONTRACT_ROOT / "catalog.json"

DEFAULT_HOST = "127.0.0.1"
DEFAULT_PORT = 8081
DEFAULT_AIWATCHER = "http://127.0.0.1:8080"
#: Flow's own default, as CPU seconds. A backstop against a query that walks far more
#: than it meant to; a deployment reading a corpus from disk raises it above the
#: managed step's limit, so the step's clock is the one that stops first.
DEFAULT_TIMEOUT_SECONDS = 30
#: The child's address-space ceiling, on Linux. Address space rather than resident
#: memory — the engines reserve far more than they touch — so this is a backstop
#: against a runaway, and the container's limit is the budget. 0 turns it off.
DEFAULT_MEMORY_MB = 4096
#: Queries running at once, each in its own child.
DEFAULT_CONCURRENCY = 4


class ConfigError(Exception):
    """A variable this service reads holds something it cannot use."""


class Admission(StrEnum):
    """How typed Python is let run (ADR_0028).

    `open` runs it as a notebook runs: a child process with ceilings, no credentials
    and a scratch directory. `strict` also admits it first — the engine's own
    constructors and methods, and nothing that reads a file, imports, takes a
    callable or runs SQL.
    """

    OPEN = "open"
    STRICT = "strict"


@dataclass(frozen=True)
class Config:
    host: str = DEFAULT_HOST
    port: int = DEFAULT_PORT
    aiwatcher: str = DEFAULT_AIWATCHER
    catalog: Path = DEFAULT_CATALOG
    corpus: Path | None = None
    admission: Admission = Admission.OPEN
    timeout_seconds: int = DEFAULT_TIMEOUT_SECONDS
    memory_mb: int = DEFAULT_MEMORY_MB
    concurrency: int = DEFAULT_CONCURRENCY

    @classmethod
    def from_env(cls, environ: Mapping[str, str] | None = None) -> Config:
        env = os.environ if environ is None else environ
        corpus = _text(env, "AIWATCHER_CORPUS_DIR")
        catalog = _text(env, "AIWATCHER_QUERY_CATALOG")
        return cls(
            host=_text(env, "AIWATCHER_QUERY_HOST") or DEFAULT_HOST,
            port=_whole(env, "AIWATCHER_QUERY_PORT", DEFAULT_PORT, minimum=1),
            aiwatcher=(_text(env, "AIWATCHER_URL") or DEFAULT_AIWATCHER).rstrip("/"),
            catalog=Path(catalog).expanduser() if catalog else DEFAULT_CATALOG,
            corpus=Path(corpus).expanduser() if corpus else None,
            admission=_admission(env),
            timeout_seconds=_whole(
                env, "AIWATCHER_QUERY_TIMEOUT_SECONDS", DEFAULT_TIMEOUT_SECONDS, minimum=1
            ),
            memory_mb=_whole(env, "AIWATCHER_QUERY_MEMORY_MB", DEFAULT_MEMORY_MB, minimum=0),
            concurrency=_whole(env, "AIWATCHER_QUERY_CONCURRENCY", DEFAULT_CONCURRENCY, minimum=1),
        )


def _text(env: Mapping[str, str], name: str) -> str | None:
    value = env.get(name, "").strip()
    return value or None


def _whole(env: Mapping[str, str], name: str, default: int, *, minimum: int) -> int:
    raw = _text(env, name)
    if raw is None:
        return default
    try:
        value = int(raw)
    except ValueError:
        value = minimum - 1
    if value < minimum:
        raise ConfigError(f"{name} is '{raw}'; it takes a whole number of at least {minimum}")
    return value


def _admission(env: Mapping[str, str]) -> Admission:
    raw = _text(env, "AIWATCHER_QUERY_ADMISSION") or Admission.OPEN.value
    try:
        return Admission(raw)
    except ValueError:
        raise ConfigError(
            f"AIWATCHER_QUERY_ADMISSION is '{raw}'; it takes "
            + ", ".join(admission.value for admission in Admission)
        ) from None
