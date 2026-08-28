"""The DeepEval bridge, against a report shaped like DeepEval's.

Built by hand rather than by running an optimiser: the bridge reads the report
structurally and never imports deepeval, so the thing worth pinning is exactly
which attributes it depends on. A test that ran a real optimiser would need an
API key and would prove less.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field
from typing import Any

import pytest

from aiwatcher_sdk.integrations.deepeval import (
    best_prompt_text,
    record_optimization,
    serialize_report,
)
from aiwatcher_sdk.prompts import OptimizationRecord, Score, scores, version_id_of

BASELINE = "Describe the floor plan on {{ page }} in {{ language }}."
CANDIDATE = "Read {{ page }} closely; describe every room in {{ language }}."


@dataclass
class FakePrompt:
    alias: str | None = None
    text_template: str | None = None
    messages_template: list[Any] | None = None


@dataclass
class FakeConfiguration:
    prompts: dict[str, FakePrompt]
    parent: str | None = None


@dataclass
class FakeIteration:
    before: float
    after: float


@dataclass
class FakeReport:
    optimization_id: str = "5f2b9c1e-0e33-4a3e-9c0f-1d2b3c4d5e6f"
    best_id: str = "cfg-3"
    accepted_iterations: list[FakeIteration] = field(default_factory=list)
    pareto_scores: dict[str, list[float]] = field(default_factory=dict)
    parents: dict[str, str] = field(default_factory=dict)
    prompt_configurations: dict[str, FakeConfiguration] = field(default_factory=dict)


def a_report(**overrides: Any) -> FakeReport:
    report = FakeReport(
        accepted_iterations=[FakeIteration(before=0.61, after=0.70), FakeIteration(0.70, 0.79)],
        pareto_scores={"cfg-3": [0.8, 0.78]},
        parents={"cfg-3": "cfg-1"},
        prompt_configurations={
            "cfg-1": FakeConfiguration(prompts={"system": FakePrompt("system", BASELINE)}),
            "cfg-3": FakeConfiguration(
                prompts={"system": FakePrompt("system", CANDIDATE)}, parent="cfg-1"
            ),
        },
    )
    for key, value in overrides.items():
        setattr(report, key, value)
    return report


class RecordingRegistry:
    """Stands in for `PromptRegistry`, capturing what the bridge sends.

    Satisfies the `Recorder` protocol structurally — which is the point of the
    protocol: the bridge's coupling to the registry is one method, and this is
    what proves it.
    """

    def __init__(self) -> None:
        self.calls: list[dict[str, Any]] = []

    def record_optimization(
        self,
        name: str,
        *,
        algorithm: str,
        baseline: str,
        candidate_text: str,
        primary_metric: str,
        dev: Sequence[Score] = (),
        test: Sequence[Score] = (),
        dataset: str | None = None,
        evaluation_id: str | None = None,
        optimization_id: str | None = None,
        started_at: str | None = None,
        duration_ms: float | None = None,
        iterations: int | None = None,
        report: Mapping[str, Any] | None = None,
        promote: bool = False,
    ) -> OptimizationRecord:
        self.calls.append(
            {
                "prompt": name,
                "algorithm": algorithm,
                "baseline": baseline,
                "candidate_text": candidate_text,
                "primary_metric": primary_metric,
                "dev": dev,
                "test": test,
                "dataset": dataset,
                "evaluation_id": evaluation_id,
                "optimization_id": optimization_id,
                "started_at": started_at,
                "duration_ms": duration_ms,
                "iterations": iterations,
                "report": report,
                "promote": promote,
            }
        )
        return OptimizationRecord(
            optimization_id=optimization_id or "derived",
            prompt=name,
            algorithm=algorithm,
            baseline=baseline,
            candidate=version_id_of(candidate_text),
            primary_metric=primary_metric,
            outcome="admitted",
            dev=tuple(dev),
            test=tuple(test),
        )


def test_the_candidate_is_the_winning_configurations_prompt() -> None:
    assert best_prompt_text(a_report()) == CANDIDATE


def test_a_report_whose_best_configuration_is_missing_says_so() -> None:
    # Rather than publishing an empty prompt, which the registry would store
    # and a service would then run on.
    with pytest.raises(ValueError, match="best id"):
        best_prompt_text(a_report(best_id="cfg-nowhere"))


def test_a_message_template_is_flattened_rather_than_dropped() -> None:
    @dataclass
    class Message:
        role: str
        content: str

    report = a_report(
        prompt_configurations={
            "cfg-3": FakeConfiguration(
                prompts={
                    "system": FakePrompt(
                        alias="system",
                        messages_template=[
                            Message("system", "Be precise."),
                            Message("user", "Go."),
                        ],
                    )
                }
            )
        }
    )
    assert best_prompt_text(report) == "Be precise.\n\nGo."


def test_the_serialised_report_carries_the_search_and_no_live_objects() -> None:
    document = serialize_report(a_report())

    assert document["best_id"] == "cfg-3"
    assert document["pareto_scores"] == {"cfg-3": [0.8, 0.78]}
    assert document["accepted_iterations"] == [
        {"before": 0.61, "after": 0.70},
        {"before": 0.70, "after": 0.79},
    ]
    assert document["prompt_configurations"]["cfg-3"]["prompts"]["system"]["text"] == CANDIDATE
    # It has to survive `json.dumps`, because that is what happens to it next.
    import json

    json.dumps(document)


def test_recording_sends_both_splits_and_the_optimisers_own_id() -> None:
    registry = RecordingRegistry()

    record = record_optimization(
        registry,
        "planner.floor-plan",
        report=a_report(),
        baseline=version_id_of(BASELINE),
        dev=scores({"mean_score": 0.61}, {"mean_score": 0.79}),
        test=scores({"mean_score": 0.60}, {"mean_score": 0.67}),
        dataset="house-catalog@3",
        promote=True,
    )

    sent = registry.calls[-1]
    assert sent["candidate_text"] == CANDIDATE
    assert sent["baseline"] == version_id_of(BASELINE)
    assert sent["dev"] == [Score("mean_score", 0.61, 0.79)]
    assert sent["test"] == [Score("mean_score", 0.60, 0.67)]
    # Passed through, so a retried CI step writes one record rather than two.
    assert sent["optimization_id"] == "5f2b9c1e-0e33-4a3e-9c0f-1d2b3c4d5e6f"
    assert sent["iterations"] == 2
    assert sent["report"]["best_id"] == "cfg-3"
    assert record.overfit_gap == pytest.approx(0.11)


def test_an_id_the_registry_would_refuse_is_dropped_rather_than_sent() -> None:
    # The derived id the server falls back to is just as stable, and sending a
    # bad one turns a recorded experiment into a 400.
    registry = RecordingRegistry()
    for hostile in ["../escape", "a/b", "", ".hidden", "x" * 200]:
        record_optimization(
            registry,
            "planner.floor-plan",
            report=a_report(optimization_id=hostile),
            baseline=version_id_of(BASELINE),
        )
        assert registry.calls[-1]["optimization_id"] is None, hostile


def test_the_algorithm_name_comes_from_the_report_when_it_has_one() -> None:
    registry = RecordingRegistry()
    record_optimization(
        registry,
        "planner.floor-plan",
        report=a_report(algorithm="SIMBA"),
        baseline=version_id_of(BASELINE),
    )
    assert registry.calls[-1]["algorithm"] == "deepeval/SIMBA"

    record_optimization(
        registry,
        "planner.floor-plan",
        report=a_report(),
        baseline=version_id_of(BASELINE),
    )
    assert registry.calls[-1]["algorithm"] == "deepeval/PromptOptimizer"


def test_recording_without_a_held_out_split_still_records() -> None:
    # And the server refuses it a promotion. The bridge does not second-guess
    # that: an experiment nobody kept is an experiment somebody re-runs.
    registry = RecordingRegistry()
    record_optimization(
        registry,
        "planner.floor-plan",
        report=a_report(),
        baseline=version_id_of(BASELINE),
        dev=scores({"mean_score": 0.61}, {"mean_score": 0.96}),
        promote=True,
    )
    sent = registry.calls[-1]
    assert sent["test"] == ()
    assert sent["promote"] is True, "asking is fine; the server is what refuses"
