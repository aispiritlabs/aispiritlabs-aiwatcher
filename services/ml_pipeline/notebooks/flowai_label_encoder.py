"""LabelEncoder — an editable FlowAI block. See examples/titanic/README.md."""

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
    import pandas as pd
    from flowai import LabelEncoder
    from flowai.frame import frame_of, records_of

    _frame = frame_of(rows)
    _target = params.get("target", "Survived")
    _encoder = LabelEncoder().fit(_frame.loc[_frame["_split"].eq("train"), _target].tolist())
    encoder_state = _encoder.to_state()
    _present = _frame[_target].notna()
    # One sklearn batch call, not one transform call per record.
    _encoded = pd.Series(
        _encoder.transform(_frame.loc[_present, _target].tolist()),
        index=_frame.index[_present],
        dtype=object,
    )
    output = records_of(_frame.assign(target_encoded=_encoded))
    return encoder_state, output


if __name__ == "__main__":
    app.run()
