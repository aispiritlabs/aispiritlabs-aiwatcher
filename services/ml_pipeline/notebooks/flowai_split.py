"""Train / validation split — an editable FlowAI block. See examples/titanic/README.md."""

import marimo

app = marimo.App(width="medium")


@app.cell
def _():
    from ml_pipeline import Block

    return (Block,)


@app.cell
def _(Block):
    _block = Block.for_notebook(__file__)
    rows = _block.get_rows()
    params = _block.get_params()
    return params, rows


@app.cell
def _(params, rows):
    import numpy as np
    from sklearn.model_selection import train_test_split
    from flowai.frame import frame_of, records_of

    _frame = frame_of(rows)
    if len(_frame) < 8:
        raise ValueError("Use at least 8 labelled rows for a stratified smoke test.")
    if _frame["PassengerId"].isna().any() or _frame["PassengerId"].duplicated().any():
        raise ValueError("PassengerId must be present and unique.")
    _train, _validation = train_test_split(
        _frame["PassengerId"],
        test_size=float(params.get("validation_fraction", 0.2)),
        random_state=int(params.get("seed", 42)),
        stratify=_frame["Survived"],
    )
    output = records_of(
        _frame.assign(_split=np.where(_frame["PassengerId"].isin(_train), "train", "validation"))
    )
    return (output,)


if __name__ == "__main__":
    app.run()
