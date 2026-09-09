"""My data preparation block."""

import marimo

app = marimo.App(width="medium")


@app.cell
def _():
    import ml_pipeline

    return (ml_pipeline,)


@app.cell
def _(ml_pipeline):
    _block = ml_pipeline.Block.for_notebook(__file__)
    rows = _block.get_rows()
    params = _block.get_params()
    return rows, params


@app.cell
def _(rows, params):
    import marimo as mo

    # Replace this transformation with your own code.
    output = [dict(row) for row in rows]
    mo.ui.table(output[:25])
    return (output,)


if __name__ == "__main__":
    app.run()
