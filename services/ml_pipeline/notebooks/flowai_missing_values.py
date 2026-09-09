"""Missing values — an editable FlowAI block. See examples/titanic/README.md."""

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
    from flowai.frame import frame_of, records_of

    _frame = frame_of(rows)
    _train = _frame.loc[_frame["_split"].eq("train")].copy()
    if _train.empty:
        raise ValueError("Missing values needs a split first; statistics are fitted on train only.")
    _train["Age"] = pd.to_numeric(_train["Age"])
    _train["Fare"] = pd.to_numeric(_train["Fare"])
    if _train["Age"].notna().sum() == 0 or _train["Fare"].notna().sum() == 0:
        raise ValueError("Training rows need at least one measured Age and Fare.")
    _age_medians = _train.groupby(["Sex", "Pclass"], dropna=False)["Age"].median().dropna()
    _fare_medians = _train.groupby("Pclass", dropna=False)["Fare"].median().dropna()
    _age_fallback = float(_train["Age"].median())
    _fare_fallback = float(_train["Fare"].median())
    _ports = _train["Embarked"].replace("", None).dropna().mode()
    _port = _ports.iloc[0] if not _ports.empty else "Unknown"
    imputation_state = {
        "age_fallback": _age_fallback,
        "fare_fallback": _fare_fallback,
        "embarked": _port,
        "age_groups": _age_medians.reset_index().to_numpy().tolist(),
        "fare_groups": _fare_medians.reset_index().to_numpy().tolist(),
    }
    _age_groups = pd.Series(
        _age_medians.reindex(pd.MultiIndex.from_frame(_frame[["Sex", "Pclass"]])).to_numpy(),
        index=_frame.index,
    ).fillna(_age_fallback)
    _fare_groups = pd.to_numeric(_frame["Pclass"].map(_fare_medians)).fillna(_fare_fallback)
    _result = _frame.assign(
        AgeFilled=pd.to_numeric(_frame["Age"]).fillna(_age_groups),
        FareFilled=pd.to_numeric(_frame["Fare"]).fillna(_fare_groups),
        EmbarkedFilled=_frame["Embarked"].replace("", None).fillna(_port),
        AgeMissing=_frame["Age"].isna().astype(int),
        FareMissing=_frame["Fare"].isna().astype(int),
        CabinMissing=_frame["Cabin"].fillna("").eq("").astype(int),
    )
    output = records_of(_result)
    return imputation_state, output


if __name__ == "__main__":
    app.run()
