"""What this service was told, and the defaults it uses when it was told nothing.

Every value is read once, at start-up, and passed down. Nothing below reads the
environment again: a notebook that could change where its own rows come from by
setting a variable would be a notebook that can read another block's data.
"""

from __future__ import annotations

import os
from dataclasses import dataclass
from pathlib import Path

SERVICE_ROOT = Path(__file__).resolve().parent.parent

DEFAULT_HOST = "127.0.0.1"
DEFAULT_PORT = 8082
DEFAULT_TIMEOUT_SECONDS = 120.0
# The same ceiling the dataset registry applies to a published version. A block
# that hands on more than this is a block whose output cannot be saved, and
# finding that out at the end of a chain is worse than finding it out here.
DEFAULT_MAX_ROWS = 1_000


@dataclass(frozen=True)
class Config:
    """Where the notebooks are, where their rows go, and what a run may cost."""

    host: str = DEFAULT_HOST
    port: int = DEFAULT_PORT
    notebooks: Path = SERVICE_ROOT / "notebooks"
    #: One exact source per digest, kept forever. Separate from `data`, which
    #: is staging and is safe to delete: this is what an old run resolves, so
    #: deleting it is deleting the provenance of every execution that pinned
    #: one of these. Separate from `notebooks` because marimo serves every file
    #: under that root as a live app.
    revisions: Path = SERVICE_ROOT / ".revisions"
    data: Path = SERVICE_ROOT / ".data"
    timeout_seconds: float = DEFAULT_TIMEOUT_SECONDS
    max_rows: int = DEFAULT_MAX_ROWS

    @classmethod
    def from_env(cls) -> Config:
        return cls(
            host=os.environ.get("AIWATCHER_ML_PIPELINE_HOST", DEFAULT_HOST),
            port=_int("AIWATCHER_ML_PIPELINE_PORT", DEFAULT_PORT),
            notebooks=_path("AIWATCHER_ML_PIPELINE_NOTEBOOKS", SERVICE_ROOT / "notebooks"),
            revisions=_path("AIWATCHER_ML_PIPELINE_REVISIONS", SERVICE_ROOT / ".revisions"),
            data=_path("AIWATCHER_ML_PIPELINE_DATA", SERVICE_ROOT / ".data"),
            timeout_seconds=float(
                _int("AIWATCHER_ML_PIPELINE_TIMEOUT", int(DEFAULT_TIMEOUT_SECONDS))
            ),
            max_rows=_int("AIWATCHER_ML_PIPELINE_MAX_ROWS", DEFAULT_MAX_ROWS),
        )


def _int(name: str, fallback: int) -> int:
    raw = os.environ.get(name)
    if raw is None or not raw.strip():
        return fallback
    try:
        value = int(raw)
    except ValueError:
        return fallback
    return value if value > 0 else fallback


def _path(name: str, fallback: Path) -> Path:
    raw = os.environ.get(name)
    return Path(raw).expanduser().resolve() if raw and raw.strip() else fallback
