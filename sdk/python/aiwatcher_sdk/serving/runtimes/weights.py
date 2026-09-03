"""``Runtime::Weights`` — a JSON array of numbers and a declared shape.

The smallest thing that is still a runtime: no graph, no operators, no code.
It is here because this repository ships one — ``just e2e-train`` fits an
eight-weight classifier — and having a name for it is what keeps the first
example anybody reads from declaring itself as ONNX in order to be loadable,
which would make the field that decides which loader runs a lie in the demo.

It is also the control. Everything the hardened half does — verify, warm,
bound, validate, watch, roll back, report — is exercised by this loader with
no dependency at all, which is how a change to that half is known not to have
broken it before an ONNX graph is anywhere near the process.
"""

from __future__ import annotations

import json
import math
from collections.abc import Mapping, Sequence
from typing import Any

from aiwatcher_sdk.serving.artifact import ArtifactReader, LoadError, read_verified
from aiwatcher_sdk.serving.loader import Predictor, pick_artifact

__all__ = ["LinearPredictor", "WeightsLoader"]

#: Past this the sigmoid is 1.0 or 0.0 to every float this could return, and
#: `exp` of the negation overflows. Clamped rather than allowed to raise: a
#: request that produces a huge logit is a request about a row far outside
#: anything the model was fit on, which is a prediction of 1.0 and not an error.
_LOGIT_CLAMP = 30.0


class LinearPredictor:
    """A dot product and a sigmoid."""

    def __init__(self, weights: Sequence[float], classes: Sequence[str]) -> None:
        self._weights = list(weights)
        self._classes = tuple(classes)

    @property
    def features(self) -> int:
        return len(self._weights)

    @property
    def classes(self) -> tuple[str, ...]:
        return self._classes

    def predict(self, rows: Sequence[Sequence[float]]) -> list[list[float]]:
        scores: list[list[float]] = []
        for row in rows:
            total = sum(weight * value for weight, value in zip(self._weights, row, strict=True))
            clamped = max(-_LOGIT_CLAMP, min(_LOGIT_CLAMP, total))
            scores.append([1.0 / (1.0 + math.exp(-clamped))])
        return scores

    def describe(self) -> Mapping[str, Any]:
        return {"weights": len(self._weights), "link": "sigmoid"}


class WeightsLoader:
    """Reads the array, and checks it against the shape the package declared."""

    @property
    def runtime(self) -> str:
        return "weights"

    @property
    def executes_packaged_code(self) -> bool:
        return False

    def load(
        self,
        package: Mapping[str, Any],
        reader: ArtifactReader,
        *,
        version: str,
    ) -> Predictor:
        artifact = pick_artifact(package)
        uri = str(artifact.get("uri", ""))
        weights = _parse(read_verified(reader, artifact, version=version), uri)
        _check_declared_width(package, len(weights), uri)
        return LinearPredictor(weights, _classes(package))


def _parse(body: bytes, uri: str) -> list[float]:
    try:
        raw = json.loads(body)
    except json.JSONDecodeError as error:
        raise LoadError(f"{uri} is not a JSON weight vector: {error}") from error
    if not isinstance(raw, list) or not raw:
        raise LoadError(f"{uri} must be a non-empty JSON array of weights")
    try:
        weights = [float(value) for value in raw]
    except (TypeError, ValueError) as error:
        raise LoadError(f"{uri} contains a non-numeric weight") from error
    if not all(math.isfinite(value) for value in weights):
        raise LoadError(f"{uri} contains a non-finite weight")
    return weights


def _check_declared_width(package: Mapping[str, Any], found: int, uri: str) -> None:
    """The package's declared row width against the vector that arrived.

    The same class of check the ONNX loader makes against a graph's own
    metadata, and it is worth having here too: a package that says ``[null, 8]``
    over a seven-weight vector would otherwise refuse every request as
    malformed at run time, having loaded happily. The declaration and the
    artifact disagreeing is a fact at load, and a refusal then names both.
    """
    inputs: Sequence[Mapping[str, Any]] = package.get("inputs") or []
    if not inputs:
        return
    shape: Sequence[Any] = inputs[0].get("shape") or []
    trailing = [dimension for dimension in shape if dimension is not None]
    if not trailing:
        return
    declared = int(trailing[-1])
    if declared != found:
        raise LoadError(
            f"the package declares {declared} features and {uri} holds {found} weights. These do "
            "not describe the same model"
        )


def _classes(package: Mapping[str, Any]) -> tuple[str, ...]:
    outputs: Sequence[Mapping[str, Any]] = package.get("outputs") or []
    if not outputs:
        return ()
    declared: Sequence[Any] = outputs[0].get("classes") or []
    return tuple(str(name) for name in declared)
