"""Record conversion at the notebook transport boundary; transformations stay columnar."""

from typing import Any, cast

import pandas as pd


def frame_of(rows: list[dict[str, Any]]) -> pd.DataFrame:
    # Preserve JSON scalar distinctions (1 versus 1.0, None versus "null") on
    # input. Cast only the numeric columns that an operation actually needs.
    return pd.DataFrame(rows, dtype=object)


def records_of(frame: pd.DataFrame) -> list[dict[str, Any]]:
    # JSON has no NaN/NA. This is serialization, never an intermediate UDF step.
    return cast(
        list[dict[str, Any]],
        frame.astype(object).where(frame.notna(), None).to_dict(orient="records"),
    )
