from __future__ import annotations

import hashlib
import json
from pathlib import Path
from typing import Any

import pytest
from starlette.testclient import TestClient

from flowai import LabelEncoder, OneHotEncoder
from flowai.frame import frame_of, records_of
from ml_pipeline.config import Config
from ml_pipeline.service import create_app
from ml_pipeline.step import inject, load_app, rows_of

ROOT = Path(__file__).resolve().parents[3]


def test_columnar_encoding_preserves_index_types_and_input_metadata() -> None:
    rows: list[dict[str, Any]] = [
        {"x": 1, "metadata": {"source": "one"}},
        {"x": "1", "metadata": {"source": "two"}},
        {"x": None, "metadata": {"source": "three"}},
    ]
    frame = frame_of(rows)
    frame.index = [9, 3, 8]
    encoder = OneHotEncoder(["x"]).fit_frame(frame.iloc[:2])
    state = encoder.to_state()
    encoded = encoder.transform_frame(frame)
    assert encoded.index.tolist() == [9, 3, 8]
    assert encoded.loc[9, "x=1"] == 1.0
    assert encoded.loc[3, 'x="1"'] == 1.0
    assert encoded.loc[8].sum() == 0.0
    assert OneHotEncoder.from_state(state).transform_frame(frame).equals(encoded)
    assert records_of(frame) == rows
    assert type(records_of(frame)[0]["x"]) is int
    assert records_of(frame)[2]["x"] is None


def passengers() -> list[dict[str, Any]]:
    return [
        {
            "PassengerId": index + 1,
            "Survived": index % 2,
            "Pclass": index % 3 + 1,
            "Name": f"Passenger{index}, Mr. Example",
            "Sex": "female" if index % 2 else "male",
            "Age": None if index % 5 == 0 else index + 15,
            "Fare": None if index % 7 == 0 else index + 3,
            "SibSp": index % 2,
            "Parch": 0,
            "Ticket": str(index // 2),
            "Cabin": None,
            "Embarked": None if index % 9 == 0 else "S",
        }
        for index in range(25)
    ]


def notebook(name: str, rows: list[dict[str, Any]]) -> dict[str, Any]:
    path = ROOT / "services/ml_pipeline/notebooks" / f"flowai_{name}.py"
    return inject(load_app(path), rows, {}, path.stem)


def test_one_hot_keeps_training_vocabulary_and_exports_fitted_state() -> None:
    encoder = OneHotEncoder(["port"]).fit([{"port": "S"}, {"port": "C"}])
    state = json.loads(json.dumps(encoder.to_state()))
    validation = [{"port": "Q"}, {"port": "C"}]
    encoded = encoder.transform(validation)
    assert encoded == [{'port="C"': 0.0, 'port="S"': 0.0}, {'port="C"': 1.0, 'port="S"': 0.0}]
    assert OneHotEncoder.from_state(state).transform(validation) == encoded
    assert encoder.to_state() == state
    assert encoder.transform([]) == []


def test_one_hot_supports_nulls_and_explicit_unknown_errors() -> None:
    encoder = OneHotEncoder(["x"], handle_unknown="error").fit([{"x": None}, {"x": "null"}])
    assert len(encoder.feature_names) == 2
    with pytest.raises(ValueError, match="unknown"):
        encoder.transform([{"x": "new"}])
    with pytest.raises(ValueError, match="fit"):
        OneHotEncoder(["x"]).transform([])
    with pytest.raises(ValueError, match="training"):
        OneHotEncoder(["x"]).fit([])


def test_labels_roundtrip_without_refitting_on_unseen_targets() -> None:
    encoder = LabelEncoder().fit(["survived", "died", "survived"])
    state = json.loads(json.dumps(encoder.to_state()))
    restored = LabelEncoder.from_state(state)
    encoded = restored.transform(["died", "survived"])
    assert restored.inverse_transform(encoded) == ["died", "survived"]
    with pytest.raises(ValueError, match="unseen"):
        restored.transform(["unknown"])
    with pytest.raises(ValueError, match="missing"):
        restored.transform([None])
    with pytest.raises(ValueError, match="fit"):
        LabelEncoder().transform([1])
    with pytest.raises(ValueError, match="unsupported"):
        LabelEncoder.from_state({**state, "version": 2})


def test_missing_values_and_features_do_not_learn_from_validation() -> None:
    split = notebook("split", passengers())["output"]
    changed = [
        dict(row, Age=10000, Fare=90000, Sex="unseen", Ticket="unseen", Survived=99)
        if row["_split"] == "validation"
        else row
        for row in split
    ]
    first = notebook("missing_values", split)
    second = notebook("missing_values", changed)
    assert first["imputation_state"] == second["imputation_state"]
    features = notebook("titanic_features", first["output"])
    changed_features = notebook("titanic_features", second["output"])
    assert features["feature_state"] == changed_features["feature_state"]
    encoded = notebook("one_hot_encoder", features["output"])
    changed_encoded = notebook("one_hot_encoder", changed_features["output"])
    assert encoded["encoder_state"] == changed_encoded["encoder_state"]
    original_training = [row for row in encoded["output"] if row["_split"] == "train"]
    changed_training = [row for row in changed_encoded["output"] if row["_split"] == "train"]
    assert original_training == changed_training
    assert any(row["Age"] is None and row["AgeMissing"] == 1 for row in original_training)


def test_portable_titanic_executes_its_embedded_code_and_preserves_rows(tmp_path: Path) -> None:
    bundle = json.loads((ROOT / "examples/titanic/titanic.flow.json").read_text())
    sources = {entry["name"]: entry for entry in bundle["notebooks"]}
    rows = passengers()
    metrics: dict[str, Any] = {}
    for block in bundle["pipeline"]["blocks"]:
        spec = block["spec"]
        if spec["kind"] != "notebook":
            continue
        entry = sources[spec["notebook"]]
        assert hashlib.sha256(entry["source"].encode()).hexdigest() == spec["revision"]
        canonical = ROOT / "services/ml_pipeline/notebooks" / f"{entry['name']}.py"
        assert entry["source"] == canonical.read_text(), "Rebuild examples/titanic/build_bundle.py"
        path = tmp_path / f"{entry['name']}.py"
        path.write_text(entry["source"])
        definitions = inject(load_app(path), rows, spec["params"], path.stem)
        before = rows
        rows = rows_of(definitions, path.stem)
        assert [row["PassengerId"] for row in rows] == list(range(1, 26))
        if block["id"] == "visualization":
            assert rows == before
            definitions["figure"].savefig(tmp_path / "visualization.png")
        metrics = definitions.get("metrics", metrics)
    assert metrics["train_rows"] == 20
    assert metrics["validation_rows"] == 5
    assert all(0 <= row["survival_probability"] <= 1 for row in rows)
    assert all(row["prediction"] in (0, 1) for row in rows)
    assert (tmp_path / "visualization.png").stat().st_size > 1000


def test_imported_code_runs_at_its_pinned_revision_through_service(scratch: Config) -> None:
    source = (ROOT / "services/ml_pipeline/notebooks/flowai_one_hot_encoder.py").read_text()
    revision = hashlib.sha256(source.encode()).hexdigest()
    name = f"flowai_one_hot_encoder_{revision[:20]}"
    with TestClient(create_app(scratch)) as client:
        saved = client.put(f"/ml-pipeline/notebooks/{name}", json={"source": source})
        assert saved.status_code == 200
        assert saved.json()["revision"] == revision
        changed = client.put(
            f"/ml-pipeline/notebooks/{name}", json={"source": source + "\n# edited\n"}
        )
        assert changed.status_code == 200
        result = client.post(
            "/ml-pipeline/run",
            json={
                "notebook": name,
                "code_revision": revision,
                "params": {"columns": ["Sex"]},
                "rows": [
                    {"Sex": "female", "_split": "train"},
                    {"Sex": "unknown", "_split": "validation"},
                ],
            },
        )
        assert result.status_code == 200, result.text
        assert result.json()["revision"] == revision
        assert result.json()["rows"][1]["model_features"] == {'Sex="female"': 0.0}
