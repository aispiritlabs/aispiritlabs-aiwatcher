"""Feature engineering — an editable FlowAI block. See examples/titanic/README.md."""

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
    from flowai.frame import frame_of, records_of

    _frame = frame_of(rows)
    _train = _frame.loc[_frame["_split"].eq("train")]
    if _train.empty:
        raise ValueError("Feature engineering needs training rows.")
    _tickets = _train["Ticket"].value_counts(dropna=False)
    _age_edges = np.unique(
        np.quantile(_train["AgeFilled"].to_numpy(dtype=float), [0.2, 0.4, 0.6, 0.8])
    )
    _fare_edges = np.unique(
        np.quantile(_train["FareFilled"].to_numpy(dtype=float), [0.2, 0.4, 0.6, 0.8])
    )
    feature_state = {
        "age_edges": _age_edges.tolist(),
        "fare_edges": _fare_edges.tolist(),
        "ticket_counts": _tickets.to_dict(),
    }
    _title = (
        _frame["Name"]
        .str.extract(r",\s*([^.]+)\.", expand=False)
        .str.strip()
        .fillna("Unknown")
        .replace({"Mme": "Mrs", "Mlle": "Miss", "Ms": "Miss"})
    )
    _title = _title.where(_title.isin(["Mr", "Mrs", "Miss", "Master"]), "Rare")
    _family = _frame["SibSp"].astype(int) + _frame["Parch"].astype(int) + 1
    _result = _frame.assign(
        Title=_title,
        Deck=_frame["Cabin"].fillna("Unknown").replace("", "Unknown").str[0],
        FamilySize=_family,
        IsAlone=_family.eq(1).astype(int),
        FarePerPerson=_frame["FareFilled"].astype(float) / _family,
        TicketFrequency=_frame["Ticket"].map(_tickets).fillna(1).astype(int),
        AgeBand=np.searchsorted(
            _age_edges, _frame["AgeFilled"].to_numpy(dtype=float), side="right"
        ).astype(str),
        FareBand=np.searchsorted(
            _fare_edges, _frame["FareFilled"].to_numpy(dtype=float), side="right"
        ).astype(str),
    )
    output = records_of(_result)
    return feature_state, output


if __name__ == "__main__":
    app.run()
