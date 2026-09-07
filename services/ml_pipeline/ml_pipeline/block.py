"""What a notebook implements to be a step in a curation chain.

Two names, and they are the whole contract::

    rows, params    what the previous block produced, and this block's settings
    output          the rows the next block reads

Nothing in a notebook opens a file, and nothing in it calls this service. When
the chain runs, `ml_pipeline.step` hands the values straight to marimo's own
[`App.run(defs=…)`][marimo.App.run], which uses them *instead of* executing the
cell that would define them, and reads `output` back out of the definitions it
returns. The notebook is a marimo notebook, not a marimo notebook with a
harness bolted to it.

`Block` is what that same cell falls back to when nobody is injecting anything
— which is the whole interactive half. Open the notebook in the panel and it
reads the rows the last run staged for it, so the widgets are being moved against
the rows the block will actually run on::

    @app.cell
    def _(Block):
        _block = Block.for_notebook(__file__)
        rows = _block.get_rows()
        params = _block.get_params()
        return params, rows

The underscore on `_block` is load-bearing. `App.run` skips the whole cell that
defines an injected name, so the cell must define `rows` and `params` and
nothing else that anything downstream needs.
"""

from __future__ import annotations

import os
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from ml_pipeline.config import SERVICE_ROOT
from ml_pipeline.staging import Row, StagedInput, Staging

DATA_VARIABLE = "AIWATCHER_ML_PIPELINE_DATA"

#: The definition a step reads back out of a notebook. One conventional name,
#: because a block that had to declare which variable was its output would be a
#: notebook with configuration attached to it.
OUTPUT = "output"

#: How a notebook says it does not answer the same thing twice.
#:
#: A managed chain caches a notebook step by the rows it read, the parameters
#: it was given and the revision it pinned — which is right for a transform and
#: wrong for a notebook that reads the clock, draws a random sample or asks a
#: model. Nothing can detect the difference from outside: this is arbitrary
#: Python, and the only party that knows is the notebook.
#:
#: So it declares it, the same way it declares its output::
#:
#:     deterministic = False
#:
#: Absent means `True`, because a curation block normally is one and a default
#: that turned caching off would make every chain pay for the exceptions.
DETERMINISTIC = "deterministic"


@dataclass(frozen=True)
class Block:
    """One notebook's end of the chain, when nothing is being injected."""

    notebook: str
    staging: Staging

    @classmethod
    def for_notebook(cls, file: str | Path) -> Block:
        """The block this notebook file is, wherever it is being opened from.

        `__file__` rather than a name typed into the notebook: a copy saved
        under a new name gets its own staged rows by being renamed, and a name
        that disagreed with the file it sits in would read an empty staging
        directory and report, quite calmly, that there is no data.
        """
        data = os.environ.get(DATA_VARIABLE)
        return cls(
            notebook=Path(file).stem,
            staging=Staging(root=Path(data) if data else SERVICE_ROOT / ".data"),
        )

    def get_input(self) -> StagedInput:
        """Everything the last run staged: rows, columns and parameters."""
        return self.staging.get_input(self.notebook)

    def get_rows(self) -> list[Row]:
        """The rows the previous block handed on. Empty when nothing is staged."""
        return self.get_input().rows

    def get_params(self) -> dict[str, Any]:
        """The settings the block was configured with."""
        return self.get_input().params
