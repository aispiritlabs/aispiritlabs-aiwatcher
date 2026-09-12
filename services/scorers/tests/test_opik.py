"""Opik's adapter against Opik itself: heuristics run for real, graded metrics
are checked for what they are built with."""

from __future__ import annotations

from typing import Any

import pytest

from aiwatcher_scorers.adapter import scored
from aiwatcher_scorers.adapters import load
from aiwatcher_scorers.contract import Case
from aiwatcher_scorers.model import ModelSettings

pytest.importorskip("opik")


def adapter(model: ModelSettings | None = None) -> Any:
    loaded = load(["opik"], model)
    assert len(loaded) == 1
    return loaded[0]


def test_heuristics_score_through_opik_with_its_tracing_off() -> None:
    metrics = adapter().metrics()
    assert set(metrics) == {"equals", "contains", "regex_match", "is_json", "levenshtein_ratio"}
    expected = {"has_expected": True}
    assert (
        scored(metrics["equals"], {}, Case(answer="warsaw", expected="Warsaw", **expected)).value
        == 1.0
    )
    assert (
        scored(
            metrics["equals"],
            {"case_sensitive": True},
            Case(answer="warsaw", expected="Warsaw", **expected),
        ).value
        == 0.0
    )
    assert (
        scored(
            metrics["contains"], {}, Case(answer="in Warsaw", expected="warsaw", **expected)
        ).value
        == 1.0
    )
    assert scored(metrics["regex_match"], {"regex": "^\\d+$"}, Case(answer="42")).value == 1.0
    assert scored(metrics["is_json"], {}, Case(answer="not json")).value == 0.0
    ratio = scored(
        metrics["levenshtein_ratio"], {}, Case(answer="Warszawa", expected="Warsaw", **expected)
    )
    assert ratio.value is not None and 0.0 < ratio.value < 1.0


def test_a_graded_metric_is_built_on_the_configured_model_and_untracked(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    held = adapter(ModelSettings(url="http://127.0.0.1:1/v1", name="gemma-4-e2b", revision="q4"))
    built: dict[str, Any] = {}

    class Result:
        value = 0.25
        scoring_failed = False

    class Hallucination:
        def __init__(self, **kwargs: Any) -> None:
            built.update(kwargs)

        def score(self, **kwargs: Any) -> Result:
            built["asked"] = kwargs
            return Result()

    import opik.evaluation.metrics

    monkeypatch.setattr(opik.evaluation.metrics, "Hallucination", Hallucination)
    metric = held.metrics()["hallucination"]
    assert metric.metric.direction == "lower"
    reply = scored(metric, {}, Case(answer="Paris", input="Capital of Poland?", has_input=True))
    assert reply.value == 0.25
    assert built["track"] is False and built["temperature"] == 0.0
    assert built["asked"] == {"input": "Capital of Poland?", "output": "Paris"}
    assert built["model"].model_name == "openai/gemma-4-e2b"


def test_a_score_opik_marks_failed_is_a_failure_and_never_its_number(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    class Failed:
        value = 0.0
        scoring_failed = True

    class Equals:
        def __init__(self, **_: Any) -> None: ...

        def score(self, **_: Any) -> Failed:
            return Failed()

    import opik.evaluation.metrics

    monkeypatch.setattr(opik.evaluation.metrics, "Equals", Equals)
    reply = scored(
        adapter().metrics()["equals"], {}, Case(answer="a", expected="a", has_expected=True)
    )
    assert reply.value is None
    assert reply.failed == "ScoringFailedError: the metric raised while scoring this case"
