"""Running one notebook as a step, in a process of its own.

Marimo runs the notebook — [`ml_pipeline.step`][] calls `App.run(defs=…)`, which
is the API for exactly this. What this module owns is the process that call
happens in, and that boundary is the decision worth explaining.

The service also **hosts** notebooks: the live app the panel embeds executes
cells in this process, because that is what the app host is. A run is a
different animal. It is somebody's half-written detector over somebody's data,
it may allocate a model, it may import something that calls `sys.exit`, and it
may not stop. In a subprocess it gets a timeout, its own memory and a traceback
that arrives as text — instead of a service that is no longer answering, taking
every other block's live app down with it.

What comes back is bounded on purpose. A block that hands on more rows than the
dataset registry will store has produced something that cannot be saved, and
the honest place to say so is here, not at the end of the chain.
"""

from __future__ import annotations

import os
import subprocess
import sys
import time
from dataclasses import dataclass, field
from typing import Any

from ml_pipeline.block import DATA_VARIABLE
from ml_pipeline.config import SERVICE_ROOT, Config
from ml_pipeline.log import logger
from ml_pipeline.notebooks import Notebook
from ml_pipeline.staging import Row, Staging, columns_of, read_json

log = logger(__name__)

# How much of a failing notebook's output is worth carrying back to a browser.
# The end is what says what went wrong; the middle of a stack trace is not.
MAX_CAPTURED_CHARACTERS = 8_000


class NotebookFailedError(RuntimeError):
    """A run that did not produce rows, with what the notebook printed."""

    def __init__(self, message: str, stdout: str = "", stderr: str = "") -> None:
        super().__init__(message)
        self.stdout = stdout
        self.stderr = stderr


@dataclass(frozen=True)
class RunResult:
    """What one notebook block produced."""

    notebook: str
    revision: str
    rows: list[Row]
    columns: list[str] = field(default_factory=list)
    truncated: bool = False
    stdout: str = ""
    took_ms: int = 0
    #: What the notebook said about itself. See `block.DETERMINISTIC`: a
    #: managed chain caches this step, and only the notebook knows whether it
    #: may be.
    deterministic: bool = True

    @property
    def row_count(self) -> int:
        return len(self.rows)


def run_notebook(
    notebook: Notebook,
    rows: list[Row],
    params: dict[str, Any],
    staging: Staging,
    config: Config,
    context: str | None = None,
) -> RunResult:
    """Stage the rows, run the notebook over them, and read what it handed on.

    `context` is the managed run's `<execution>/<step>/<attempt>`; the panel's
    own runs have none and share the ad-hoc directory, which is the same thing
    they shared before contexts existed.
    """
    staging.stage(notebook.name, rows, params=params, context=context)
    output_path = staging.output_path(notebook.name, context)
    output_path.unlink(missing_ok=True)

    environment = dict(os.environ)
    environment.update(
        {
            DATA_VARIABLE: str(staging.root),
            # marimo's own runtime chatter is not this run's output.
            "MARIMO_SKIP_UPDATE_CHECK": "1",
            "PYTHONPATH": os.pathsep.join(
                [str(SERVICE_ROOT), *filter(None, [environment.get("PYTHONPATH")])]
            ),
        }
    )

    started = time.monotonic()
    try:
        # S603: the command is this interpreter and a path this service resolved
        # inside its own notebook directory — never a string from a request. No
        # shell is involved.
        completed = subprocess.run(  # noqa: S603
            [
                sys.executable,
                "-m",
                "ml_pipeline.step",
                str(notebook.path),
                str(staging.input_path(notebook.name, context)),
                str(output_path),
            ],
            cwd=str(SERVICE_ROOT),
            env=environment,
            capture_output=True,
            text=True,
            timeout=config.timeout_seconds,
            check=False,
        )
    except subprocess.TimeoutExpired as expired:
        log.warning(
            "notebook.timeout", notebook=notebook.name, timeout_seconds=config.timeout_seconds
        )
        raise NotebookFailedError(
            f"{notebook.name} did not finish within {config.timeout_seconds:.0f}s",
            stdout=_tail(expired.stdout),
            stderr=_tail(expired.stderr),
        ) from expired
    took_ms = int((time.monotonic() - started) * 1000)

    if completed.returncode != 0:
        log.warning(
            "notebook.failed",
            notebook=notebook.name,
            status=completed.returncode,
            took_ms=took_ms,
        )
        raise NotebookFailedError(
            f"{notebook.name} exited with status {completed.returncode}",
            stdout=_tail(completed.stdout),
            stderr=_tail(completed.stderr),
        )

    if not output_path.is_file():
        raise NotebookFailedError(
            f"{notebook.name} ran but handed nothing on: a block's last cell names the "
            "rows it produces, as `output = …`",
            stdout=_tail(completed.stdout),
            stderr=_tail(completed.stderr),
        )

    body = read_json(output_path)
    produced = body.get("rows") if isinstance(body, dict) else None
    if not isinstance(produced, list):
        raise NotebookFailedError(
            f"{notebook.name} wrote an output that is not a list of rows",
            stdout=_tail(completed.stdout),
            stderr=_tail(completed.stderr),
        )

    kept = [row for row in produced if isinstance(row, dict)][: config.max_rows]
    log.info(
        "notebook.run",
        notebook=notebook.name,
        revision=notebook.revision[:12],
        rows_in=len(rows),
        rows_out=len(kept),
        truncated=len(produced) > len(kept),
        took_ms=took_ms,
    )
    declared = body.get("deterministic") if isinstance(body, dict) else True
    return RunResult(
        notebook=notebook.name,
        revision=notebook.revision,
        rows=kept,
        columns=columns_of(kept),
        truncated=len(produced) > len(kept),
        stdout=_tail(completed.stdout),
        took_ms=took_ms,
        deterministic=declared if isinstance(declared, bool) else True,
    )


def _tail(text: str | bytes | None) -> str:
    if text is None:
        return ""
    decoded = text.decode("utf-8", "replace") if isinstance(text, bytes) else text
    if len(decoded) <= MAX_CAPTURED_CHARACTERS:
        return decoded
    return "… " + decoded[-MAX_CAPTURED_CHARACTERS:]
