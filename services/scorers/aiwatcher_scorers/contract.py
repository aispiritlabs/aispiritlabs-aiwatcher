"""The wire contract aiwatcher's work role speaks to this service.

Two routes and no framework's name in either. ``GET /scorers/catalog`` is what
this service measures — every adapter at the release installed, the model its
graded metrics ask, and each metric with its unit, which way is better, what it
reads and the parameters it takes. ``POST /scorers/score`` is one metric over a
list of cases, answered in the same order with a number or the adapter's own
sentence about why there is none.

The Rust half is ``aiwatcher_evaluation::external``; the shapes here are its
shapes, and ``CONTRACT`` is its ``SCORER_CONTRACT``.

**A reply never carries a model's words.** A framework's ``reason`` is a
model's text and can repeat the answer it was shown; aiwatcher keeps replies
beside evidence that is sealed and retained, and a reason there would be the
one copy of that answer that is neither.
"""

from __future__ import annotations

import json
import math
from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field
from typing import Literal, cast

type JsonValue = bool | int | float | str | list[JsonValue] | dict[str, JsonValue] | None

CONTRACT = 1

Direction = Literal["higher", "lower", "none"]
Aggregation = Literal["mean", "rate"]
Side = Literal["input", "answer", "expected"]
ParameterKind = Literal["string", "number", "integer", "boolean", "string_list"]

SIDES: tuple[Side, ...] = ("input", "answer", "expected")


class ContractError(Exception):
    """A request this service will not act on, and the status that says why."""

    def __init__(self, status: int, message: str) -> None:
        super().__init__(message)
        self.status = status
        self.message = message


@dataclass(frozen=True)
class ModelReference:
    name: str
    version: str

    def to_json(self) -> dict[str, JsonValue]:
        return {"name": self.name, "version": self.version}


@dataclass(frozen=True)
class Parameter:
    kind: ParameterKind
    required: bool = False
    description: str = ""

    def admits(self, value: JsonValue) -> bool:
        match self.kind:
            case "string":
                return isinstance(value, str)
            case "boolean":
                return isinstance(value, bool)
            case "integer":
                return isinstance(value, int) and not isinstance(value, bool)
            case "number":
                return (
                    isinstance(value, int | float)
                    and not isinstance(value, bool)
                    and math.isfinite(value)
                )
            case "string_list":
                return isinstance(value, list) and all(isinstance(item, str) for item in value)

    def to_json(self) -> dict[str, JsonValue]:
        described: dict[str, JsonValue] = {"kind": self.kind, "required": self.required}
        if self.description:
            described["description"] = self.description
        return described


@dataclass(frozen=True)
class Metric:
    """One metric, as its adapter describes it — and as a card pins it."""

    metric: str
    unit: str
    direction: Direction
    aggregation: Aggregation
    reads: tuple[Side, ...]
    description: str = ""
    model_graded: bool = False
    range: tuple[float, float] | None = None
    parameters: Mapping[str, Parameter] = field(default_factory=dict)

    def to_json(self) -> dict[str, JsonValue]:
        described: dict[str, JsonValue] = {
            "metric": self.metric,
            "unit": self.unit,
            "direction": self.direction,
            "aggregation": self.aggregation,
            "reads": list(self.reads),
            "model_graded": self.model_graded,
        }
        if self.description:
            described["description"] = self.description
        if self.range is not None:
            described["range"] = [self.range[0], self.range[1]]
        if self.parameters:
            described["parameters"] = {
                name: parameter.to_json() for name, parameter in sorted(self.parameters.items())
            }
        return described

    def refusals(self, parameters: Mapping[str, JsonValue]) -> list[str]:
        """Every problem with these parameters, at once."""
        problems = [
            f"takes no parameter `{name}`" for name in parameters if name not in self.parameters
        ]
        for name, parameter in self.parameters.items():
            if name not in parameters:
                if parameter.required:
                    problems.append(f"requires `{name}`")
            elif not parameter.admits(parameters[name]):
                problems.append(f"`{name}` is a {parameter.kind}")
        return problems


@dataclass(frozen=True)
class Case:
    """One case, with only the sides the metric reads."""

    answer: JsonValue
    input: JsonValue = None
    expected: JsonValue = None
    has_input: bool = False
    has_expected: bool = False


@dataclass(frozen=True)
class Scored:
    """A number, or the adapter's own sentence about why there is none."""

    value: float | None = None
    failed: str | None = None

    def to_json(self) -> dict[str, JsonValue]:
        if self.failed is not None:
            return {"failed": self.failed}
        return {"value": self.value}


@dataclass(frozen=True)
class Declared:
    """What the card being measured pinned about its metric."""

    version: str
    model: ModelReference | None


@dataclass(frozen=True)
class ScoreRequest:
    adapter: str
    metric: str
    declared: Declared
    parameters: Mapping[str, JsonValue]
    cases: Sequence[Case]


def text(value: JsonValue) -> str:
    """A side of a case as the text a framework reads.

    Text is itself; anything else is its JSON, keys sorted, so the same value
    reads the same way every time it is asked about.
    """
    if isinstance(value, str):
        return value
    return json.dumps(value, ensure_ascii=False, sort_keys=True)


MAX_CASES = 64


def parse_score_request(body: JsonValue) -> ScoreRequest:
    """The request, or a 400 naming what is wrong with it."""
    if not isinstance(body, dict):
        raise ContractError(400, "the request is a JSON object")
    adapter = body.get("adapter")
    metric = body.get("metric")
    if not isinstance(adapter, str) or not adapter:
        raise ContractError(400, "`adapter` names an adapter in the catalog")
    if not isinstance(metric, str) or not metric:
        raise ContractError(400, "`metric` names a metric of that adapter")
    declared = body.get("declared")
    if not isinstance(declared, dict) or not isinstance(declared.get("version"), str):
        raise ContractError(400, "`declared` carries the version the card pinned")
    model = declared.get("model")
    pinned_model: ModelReference | None = None
    if isinstance(model, dict):
        name, version = model.get("name"), model.get("version")
        if not isinstance(name, str) or not isinstance(version, str):
            raise ContractError(400, "`declared.model` is a name and a version")
        pinned_model = ModelReference(name, version)
    elif model is not None:
        raise ContractError(400, "`declared.model` is a name and a version")
    parameters = body.get("parameters", {})
    if not isinstance(parameters, dict):
        raise ContractError(400, "`parameters` is an object")
    cases = body.get("cases")
    if not isinstance(cases, list) or not cases:
        raise ContractError(400, "`cases` is a non-empty list")
    if len(cases) > MAX_CASES:
        raise ContractError(400, f"`cases` holds at most {MAX_CASES} cases per request")
    parsed: list[Case] = []
    for case in cases:
        if not isinstance(case, dict) or "answer" not in case:
            raise ContractError(400, "every case carries an `answer`")
        parsed.append(
            Case(
                answer=case["answer"],
                input=case.get("input"),
                expected=case.get("expected"),
                has_input="input" in case,
                has_expected="expected" in case,
            )
        )
    return ScoreRequest(
        adapter=adapter,
        metric=metric,
        declared=Declared(cast(str, declared["version"]), pinned_model),
        parameters=parameters,
        cases=parsed,
    )
