"""Columnar scikit-learn adapters with explicit, portable fitted state."""

from __future__ import annotations

import json
from collections.abc import Sequence
from typing import Any, Literal, Self, cast

import pandas as pd
from sklearn.preprocessing import LabelEncoder as SklearnLabelEncoder
from sklearn.preprocessing import OneHotEncoder as SklearnOneHotEncoder

type Row = dict[str, Any]


def _key(value: Any) -> str:
    if value is not None and not isinstance(value, str | int | float | bool):
        raise ValueError("categories must be JSON scalars")
    return json.dumps(value, ensure_ascii=False, allow_nan=False)


class OneHotEncoder:
    """Encode named columns; unknown categories become zeros or raise explicitly.

    `transform` returns only model features, never IDs or target columns. Keep
    the original rows beside these features when provenance is needed.
    """

    def __init__(
        self, columns: Sequence[str], *, handle_unknown: Literal["ignore", "error"] = "ignore"
    ) -> None:
        if not columns or len(set(columns)) != len(columns):
            raise ValueError("columns must be nonempty and unique")
        if handle_unknown not in ("ignore", "error"):
            raise ValueError("handle_unknown must be ignore or error")
        self.columns = list(columns)
        self.handle_unknown = handle_unknown
        self._encoder: Any = None

    def _matrix(self, frame: pd.DataFrame) -> pd.DataFrame:
        # Portable-state boundary: canonical JSON distinguishes scalar types.
        # This named scalar conversion is intentionally not a row-wise feature UDF.
        return frame.loc[:, self.columns].map(_key)

    def fit(self, rows: Sequence[Row]) -> Self:
        return self.fit_frame(pd.DataFrame(rows, dtype=object))

    def fit_frame(self, frame: pd.DataFrame) -> Self:
        if frame.empty:
            raise ValueError("fit needs training rows")
        self._encoder = SklearnOneHotEncoder(
            sparse_output=False, handle_unknown=self.handle_unknown
        ).fit(self._matrix(frame))
        return self

    def _fitted(self) -> Any:
        if self._encoder is None:
            raise ValueError("fit the encoder on training data before transforming")
        return self._encoder

    @property
    def feature_names(self) -> list[str]:
        return [
            f"{column}={category}"
            for column, categories in zip(self.columns, self._fitted().categories_, strict=True)
            for category in categories
        ]

    def transform(self, rows: Sequence[Row]) -> list[dict[str, float]]:
        return cast(
            list[dict[str, float]],
            self.transform_frame(pd.DataFrame(rows, dtype=object)).to_dict(orient="records"),
        )

    def transform_frame(self, frame: pd.DataFrame) -> pd.DataFrame:
        encoder = self._fitted()
        if frame.empty:
            return pd.DataFrame(index=frame.index, columns=self.feature_names, dtype=float)
        return pd.DataFrame(
            encoder.transform(self._matrix(frame)), index=frame.index, columns=self.feature_names
        )

    def fit_transform(self, rows: Sequence[Row]) -> list[dict[str, float]]:
        return self.fit(rows).transform(rows)

    def to_state(self) -> dict[str, Any]:
        return {
            "version": 1,
            "kind": "one_hot",
            "columns": self.columns.copy(),
            "handle_unknown": self.handle_unknown,
            "categories": [values.tolist() for values in self._fitted().categories_],
        }

    @classmethod
    def from_state(cls, state: dict[str, Any]) -> Self:
        if state.get("version") != 1 or state.get("kind") != "one_hot":
            raise ValueError("unsupported OneHotEncoder state")
        instance = cls(state["columns"], handle_unknown=state["handle_unknown"])
        categories = state["categories"]
        if len(categories) != len(instance.columns) or any(
            not isinstance(values, list)
            or not values
            or not all(isinstance(value, str) for value in values)
            or len(set(values)) != len(values)
            for values in categories
        ):
            raise ValueError("invalid OneHotEncoder categories")
        instance._encoder = SklearnOneHotEncoder(
            categories=categories, sparse_output=False, handle_unknown=instance.handle_unknown
        ).fit(pd.DataFrame([[values[0] for values in categories]], columns=instance.columns))
        return instance


class LabelEncoder:
    """Encode target labels. Unseen targets raise; they never become a new class."""

    def __init__(self) -> None:
        self._encoder: Any = None

    @staticmethod
    def _keys(values: Sequence[Any]) -> list[str]:
        if any(value is None for value in values):
            raise ValueError("target labels cannot be missing")
        return [_key(value) for value in values]

    def fit(self, values: Sequence[Any]) -> Self:
        if not values:
            raise ValueError("fit needs training labels")
        self._encoder = SklearnLabelEncoder().fit(self._keys(values))
        return self

    def _fitted(self) -> Any:
        if self._encoder is None:
            raise ValueError("fit the encoder on training labels before transforming")
        return self._encoder

    def transform(self, values: Sequence[Any]) -> list[int]:
        return [int(value) for value in self._fitted().transform(self._keys(values))]

    def fit_transform(self, values: Sequence[Any]) -> list[int]:
        return self.fit(values).transform(values)

    def inverse_transform(self, values: Sequence[int]) -> list[Any]:
        return [json.loads(value) for value in self._fitted().inverse_transform(values)]

    def to_state(self) -> dict[str, Any]:
        return {"version": 1, "kind": "label", "classes": self._fitted().classes_.tolist()}

    @classmethod
    def from_state(cls, state: dict[str, Any]) -> Self:
        if state.get("version") != 1 or state.get("kind") != "label":
            raise ValueError("unsupported LabelEncoder state")
        values = state["classes"]
        if (
            not isinstance(values, list)
            or not values
            or not all(isinstance(value, str) for value in values)
            or values != sorted(set(values))
        ):
            raise ValueError("invalid LabelEncoder classes")
        instance = cls().fit([json.loads(value) for value in values])
        if instance.to_state() != state:
            raise ValueError("noncanonical LabelEncoder state")
        return instance
