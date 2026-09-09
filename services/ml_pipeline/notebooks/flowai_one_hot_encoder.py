"""OneHotEncoder — an editable FlowAI block. See examples/titanic/README.md."""

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
    from flowai import OneHotEncoder
    from flowai.frame import frame_of, records_of

    _frame = frame_of(rows)
    _columns = params.get(
        "columns", ["Sex", "Pclass", "EmbarkedFilled", "Title", "Deck", "AgeBand", "FareBand"]
    )
    _encoder = OneHotEncoder(
        _columns, handle_unknown=params.get("handle_unknown", "ignore")
    ).fit_frame(_frame.loc[_frame["_split"].eq("train")])
    encoder_state = _encoder.to_state()
    _features = _encoder.transform_frame(_frame)
    # Nested model_features is the notebook wire format. Pack it only at output.
    output = records_of(_frame.assign(model_features=_features.to_dict(orient="records")))
    return encoder_state, output


if __name__ == "__main__":
    app.run()
