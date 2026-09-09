"""Visualization — an editable FlowAI block. See examples/titanic/README.md."""

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
    import matplotlib.pyplot as plt
    import marimo as mo
    from flowai.frame import frame_of

    _frame = frame_of(rows)
    _train = _frame.loc[_frame["_split"].eq("train")]
    _columns = params.get("columns", ["Age", "Fare", "Cabin", "Embarked"])
    _missing = (_train[_columns].isna() | _train[_columns].eq("")).sum()
    figure, _axes = plt.subplots(1, 2, figsize=(11, 4), layout="constrained")
    _axes[0].bar(_columns, _missing, color="#d97706")
    _axes[0].set_title("Missing measurements · training rows")
    _axes[0].set_ylabel("Passengers")
    _rates = (
        _train.assign(Survived=_train["Survived"].astype(float)).groupby("Sex")["Survived"].mean()
    )
    _axes[1].bar(_rates.index, _rates, color="#0891b2")
    _axes[1].set_ylim(0, 1)
    _axes[1].set_title("Survival rate · training rows")
    output = rows
    mo.vstack(
        [
            mo.md(
                "## Visualization\nTraining data only; the validation labels do not guide feature selection."
            ),
            figure,
        ]
    )
    return figure, output


if __name__ == "__main__":
    app.run()
