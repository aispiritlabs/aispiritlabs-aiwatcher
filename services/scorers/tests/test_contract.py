"""The wire, as the fixtures aiwatcher's Rust half is tested against write it."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from aiwatcher_scorers.adapter import Implemented
from aiwatcher_scorers.contract import (
    Case,
    ContractError,
    JsonValue,
    Metric,
    ModelReference,
    Parameter,
    parse_score_request,
    text,
)
from aiwatcher_scorers.service import catalog

FIXTURES = Path(__file__).resolve().parents[3] / "contracts" / "fixtures" / "scorers-v1"


def fixture(name: str) -> JsonValue:
    loaded: JsonValue = json.loads((FIXTURES / name).read_text())
    return loaded


class Pinned:
    """An adapter describing exactly what the catalog fixture describes."""

    name = "deepeval"
    version = "4.2.2"
    model: ModelReference | None = ModelReference("gemma-4-e2b", "ud-q4-k-xl")

    def metrics(self) -> dict[str, Implemented]:
        return {
            "answer_relevancy": Implemented(
                Metric(
                    "answer_relevancy",
                    unit="score",
                    direction="higher",
                    aggregation="mean",
                    reads=("input", "answer"),
                    description="How much of the answer is relevant to what was asked.",
                    model_graded=True,
                    range=(0.0, 1.0),
                ),
                lambda _parameters, _case: 0.75,
            ),
            "pattern_match": Implemented(
                Metric(
                    "pattern_match",
                    unit="ratio",
                    direction="higher",
                    aggregation="rate",
                    reads=("answer",),
                    description="The whole answer matches the pattern.",
                    parameters={
                        "pattern": Parameter("string", required=True),
                        "ignore_case": Parameter("boolean"),
                    },
                ),
                lambda _parameters, _case: 1.0,
            ),
        }


def test_the_catalog_this_service_serves_is_the_one_the_rust_half_reads() -> None:
    assert catalog([Pinned()]) == fixture("catalog.json")


def test_a_score_request_as_aiwatcher_sends_it_parses() -> None:
    request = parse_score_request(fixture("score-request.json"))
    assert request.declared.version == "4.2.2"
    assert request.declared.model == ModelReference("gemma-4-e2b", "ud-q4-k-xl")
    assert request.cases[0].has_input and not request.cases[0].has_expected


@pytest.mark.parametrize(
    ("body", "says"),
    [
        ([], "JSON object"),
        ({"metric": "m", "declared": {"version": "1"}, "cases": [{"answer": 1}]}, "`adapter`"),
        ({"adapter": "a", "metric": "m", "cases": [{"answer": 1}]}, "`declared`"),
        ({"adapter": "a", "metric": "m", "declared": {"version": "1"}, "cases": []}, "`cases`"),
        (
            {"adapter": "a", "metric": "m", "declared": {"version": "1"}, "cases": [{"input": 1}]},
            "`answer`",
        ),
    ],
)
def test_a_malformed_request_is_a_400_naming_what_is_wrong(body: JsonValue, says: str) -> None:
    with pytest.raises(ContractError) as refused:
        parse_score_request(body)
    assert refused.value.status == 400
    assert says in refused.value.message


def test_parameters_are_refused_with_every_problem_at_once() -> None:
    metric = Pinned().metrics()["pattern_match"].metric
    assert metric.refusals({"pattern": "^a$"}) == []
    assert metric.refusals({"ignore_case": "yes", "flags": 2}) == [
        "takes no parameter `flags`",
        "requires `pattern`",
        "`ignore_case` is a boolean",
    ]
    assert not Parameter("number").admits(float("nan"))
    assert not Parameter("integer").admits(True)


def test_a_side_that_is_not_text_is_read_as_its_json_the_same_way_every_time() -> None:
    assert text("Warsaw") == "Warsaw"
    assert text({"b": 1, "a": "ł"}) == '{"a": "ł", "b": 1}'
    assert Case(answer=None).has_input is False
