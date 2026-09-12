"""DeepEval's adapter against DeepEval itself — its heuristics really run, and a
graded metric is checked for what it is built with, since a model is not."""

from __future__ import annotations

from typing import Any

import pytest

from aiwatcher_scorers.adapter import scored
from aiwatcher_scorers.adapters import load, quiet
from aiwatcher_scorers.contract import Case
from aiwatcher_scorers.model import ModelSettings

pytest.importorskip("deepeval")

MODEL = ModelSettings(
    url="http://127.0.0.1:1/v1", name="gemma-4-e2b", revision="q4", profile="llamacpp"
)


def adapter(model: ModelSettings | None = None) -> Any:
    loaded = load(["deepeval"], model)
    assert len(loaded) == 1
    return loaded[0]


def test_heuristics_score_through_deepeval_and_need_no_model() -> None:
    held = adapter()
    assert held.model is None
    metrics = held.metrics()
    assert set(metrics) == {"exact_match", "pattern_match"}, "no model, no graded metric"
    exact = metrics["exact_match"]
    assert (
        scored(exact, {}, Case(answer="Warsaw", expected="Warsaw", has_expected=True)).value == 1.0
    )
    assert (
        scored(exact, {}, Case(answer="Kraków", expected="Warsaw", has_expected=True)).value == 0.0
    )
    pattern = metrics["pattern_match"]
    assert scored(pattern, {"pattern": "W.*"}, Case(answer="Warsaw")).value == 1.0
    assert (
        scored(pattern, {"pattern": "w.*", "ignore_case": True}, Case(answer="Warsaw")).value == 1.0
    )


def test_a_graded_metric_asks_the_configured_model_for_no_reason(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    held = adapter(MODEL)
    assert held.model is not None and held.model.name == "gemma-4-e2b"
    built: dict[str, Any] = {}

    class Relevancy:
        def __init__(self, **kwargs: Any) -> None:
            built.update(kwargs)
            self.score = 0.0

        def measure(self, test_case: Any) -> None:
            built["input"] = test_case.input
            built["output"] = test_case.actual_output
            self.score = 0.8

    import deepeval.metrics

    monkeypatch.setattr(deepeval.metrics, "AnswerRelevancyMetric", Relevancy)
    metric = held.metrics()["answer_relevancy"]
    assert metric.metric.model_graded and metric.metric.range == (0.0, 1.0)
    reply = scored(metric, {}, Case(answer={"text": "Warsaw"}, input="Capital?", has_input=True))
    assert reply.value == 0.8
    assert built["include_reason"] is False and built["async_mode"] is False
    assert built["input"] == "Capital?"
    assert built["output"] == '{"text": "Warsaw"}'
    model = built["model"]
    assert model.name == "gemma-4-e2b" and model.base_url == "http://127.0.0.1:1/v1"
    assert model.generation_kwargs == {
        "extra_body": {"chat_template_kwargs": {"enable_thinking": False}}
    }


def test_loading_turns_the_frameworks_phoning_home_off() -> None:
    import os

    quiet()
    assert os.environ["DEEPEVAL_TELEMETRY_OPT_OUT"] == "YES"
    assert os.environ["OPIK_TRACK_DISABLE"] == "true"
