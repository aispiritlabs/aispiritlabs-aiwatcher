"""Model training & evaluation — an editable FlowAI block. See examples/titanic/README.md."""

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
    import json
    import pandas as pd
    import marimo as mo
    from sklearn.ensemble import RandomForestClassifier
    from sklearn.metrics import accuracy_score, roc_auc_score
    from flowai.frame import frame_of, records_of

    _frame = frame_of(rows)
    _numeric = params.get(
        "numeric_columns",
        [
            "AgeFilled",
            "FareFilled",
            "FamilySize",
            "IsAlone",
            "FarePerPerson",
            "TicketFrequency",
            "AgeMissing",
            "FareMissing",
            "CabinMissing",
        ],
    )
    if set(_numeric) & {"PassengerId", "Survived", "target_encoded", "_split"}:
        raise ValueError("IDs, split and target columns are not model features.")
    _train = _frame["_split"].eq("train")
    _validation = _frame["_split"].eq("validation")
    if not _train.any() or not _validation.any():
        raise ValueError("Training and validation rows are required.")
    # Unpack the nested wire field once at the model input boundary.
    _features = pd.DataFrame(_frame["model_features"].tolist(), index=_frame.index)
    _categorical = sorted(_frame.loc[_train, "model_features"].iloc[0])
    _matrix = pd.concat([_frame[_numeric].astype(float), _features[_categorical]], axis=1)
    _target = _frame["target_encoded"].astype(int)
    model = RandomForestClassifier(
        n_estimators=int(params.get("n_estimators", 120)),
        max_depth=6,
        min_samples_leaf=2,
        random_state=int(params.get("seed", 42)),
        n_jobs=1,
    )
    model.fit(_matrix.loc[_train], _target.loc[_train])
    if list(model.classes_) != [0, 1]:
        raise ValueError("This Titanic baseline requires both target classes 0 and 1.")
    _predicted = model.predict(_matrix.loc[_validation])
    _probability = model.predict_proba(_matrix.loc[_validation])[:, 1]
    _truth = _target.loc[_validation]
    metrics = {
        "train_rows": int(_train.sum()),
        "validation_rows": int(_validation.sum()),
        "accuracy": float(accuracy_score(_truth, _predicted)),
        "roc_auc": float(roc_auc_score(_truth, _probability)) if _truth.nunique() > 1 else None,
        "seed": int(params.get("seed", 42)),
    }
    print(json.dumps(metrics, sort_keys=True))
    output = records_of(
        _frame.assign(
            prediction=model.predict(_matrix),
            survival_probability=model.predict_proba(_matrix)[:, 1],
        )
    )
    mo.md(
        f"## Model evaluation\nHeld-out validation accuracy: **{metrics['accuracy']:.3f}** · {metrics['validation_rows']} passengers.\n\nA 25-row preview is a wiring check, not an estimate of model quality."
    )
    return metrics, model, output


if __name__ == "__main__":
    app.run()
