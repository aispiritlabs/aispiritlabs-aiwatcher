"""The ML pipeline runtime for aiwatcher's curation blocks.

Two halves that meet at one directory of staged rows.

`Block` is what a **notebook** imports — the fallback its first cell uses when
nothing is being injected, which is every time somebody opens it in the panel.
The rest of this package is the **service** that stages those rows, serves the
notebook as a live app, and runs it as a step through marimo's own `App.run`.
"""

from ml_pipeline.block import OUTPUT, Block
from ml_pipeline.staging import Row, StagedInput, Staging

__all__ = ["OUTPUT", "Block", "Row", "StagedInput", "Staging"]
