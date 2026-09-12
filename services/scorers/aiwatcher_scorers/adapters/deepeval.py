"""DeepEval's metrics, behind the contract.

Imported only when this adapter is loaded, so a service without the
``deepeval`` extra never sees the package. A metric is built per case, with no
reason asked for and no async loop of its own: the service already runs each
case in a worker thread, and a reason is a model's words the contract carries
none of.

Every graded metric asks the one model ``aiwatcher_scorers.model`` configures,
through DeepEval's ``LocalModel`` — an OpenAI-compatible endpoint — with the
temperature at nought.
"""

from __future__ import annotations

import importlib.metadata
from collections.abc import Mapping
from functools import cached_property
from typing import Any

from aiwatcher_scorers.adapter import Implemented, graded
from aiwatcher_scorers.contract import (
    Case,
    JsonValue,
    Metric,
    ModelReference,
    Parameter,
    text,
)
from aiwatcher_scorers.model import ModelSettings


def load(model: ModelSettings | None) -> DeepEval | None:
    try:
        importlib.metadata.version("deepeval")
    except importlib.metadata.PackageNotFoundError:
        return None
    return DeepEval(model)


class DeepEval:
    def __init__(self, settings: ModelSettings | None) -> None:
        self._settings = settings

    @property
    def name(self) -> str:
        return "deepeval"

    @property
    def version(self) -> str:
        return importlib.metadata.version("deepeval")

    @property
    def model(self) -> ModelReference | None:
        return self._settings.reference if self._settings else None

    @cached_property
    def _llm(self) -> Any:
        from deepeval.models import LocalModel

        settings = self._settings
        if settings is None:  # pragma: no cover — a graded metric is not listed without one
            raise RuntimeError("no model is configured for graded metrics")
        extra = {"extra_body": settings.extra_body} if settings.extra_body else {}
        return LocalModel(
            model=settings.name,
            base_url=settings.url,
            api_key=settings.token or "not-needed",
            temperature=0.0,
            generation_kwargs=extra,
        )

    def metrics(self) -> Mapping[str, Implemented]:
        table: dict[str, Implemented] = {
            "exact_match": Implemented(
                Metric(
                    "exact_match",
                    unit="ratio",
                    direction="higher",
                    aggregation="rate",
                    reads=("answer", "expected"),
                    description="The answer is the expected one, as text.",
                ),
                self._exact_match,
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
                self._pattern_match,
            ),
        }
        if self._settings is None:
            return table
        table.update(
            {
                "answer_relevancy": graded(
                    "answer_relevancy",
                    direction="higher",
                    reads=("input", "answer"),
                    description="How much of the answer is relevant to what was asked.",
                    score=self._answer_relevancy,
                ),
                "bias": graded(
                    "bias",
                    direction="lower",
                    reads=("answer",),
                    description="The share of the answer's opinions that are biased.",
                    score=self._bias,
                ),
                "toxicity": graded(
                    "toxicity",
                    direction="lower",
                    reads=("answer",),
                    description="The share of the answer's opinions that are toxic.",
                    score=self._toxicity,
                ),
                "g_eval": graded(
                    "g_eval",
                    direction="higher",
                    reads=("input", "answer"),
                    description="How well the answer meets the criteria, for what was asked.",
                    score=self._g_eval,
                    parameters={"criteria": Parameter("string", required=True)},
                ),
                "g_eval_with_expected": graded(
                    "g_eval_with_expected",
                    direction="higher",
                    reads=("input", "answer", "expected"),
                    description="How well the answer meets the criteria, beside the expectation.",
                    score=self._g_eval_with_expected,
                    parameters={"criteria": Parameter("string", required=True)},
                ),
            }
        )
        return table

    @staticmethod
    def _case(case: Case) -> Any:
        from deepeval.test_case import LLMTestCase

        return LLMTestCase(
            input=text(case.input) if case.has_input else "",
            actual_output=text(case.answer),
            expected_output=text(case.expected) if case.has_expected else None,
        )

    @staticmethod
    def _measured(metric: Any, case: Case) -> float:
        metric.measure(DeepEval._case(case))
        return float(metric.score)

    def _exact_match(self, _: Mapping[str, JsonValue], case: Case) -> float:
        from deepeval.metrics import ExactMatchMetric

        return self._measured(ExactMatchMetric(), case)

    def _pattern_match(self, parameters: Mapping[str, JsonValue], case: Case) -> float:
        from deepeval.metrics import PatternMatchMetric

        return self._measured(
            PatternMatchMetric(
                pattern=str(parameters["pattern"]),
                ignore_case=bool(parameters.get("ignore_case", False)),
            ),
            case,
        )

    def _graded(self) -> dict[str, Any]:
        return {"model": self._llm, "include_reason": False, "async_mode": False}

    def _answer_relevancy(self, _: Mapping[str, JsonValue], case: Case) -> float:
        from deepeval.metrics import AnswerRelevancyMetric

        return self._measured(AnswerRelevancyMetric(**self._graded()), case)

    def _bias(self, _: Mapping[str, JsonValue], case: Case) -> float:
        from deepeval.metrics import BiasMetric

        return self._measured(BiasMetric(**self._graded()), case)

    def _toxicity(self, _: Mapping[str, JsonValue], case: Case) -> float:
        from deepeval.metrics import ToxicityMetric

        return self._measured(ToxicityMetric(**self._graded()), case)

    def _geval(self, parameters: Mapping[str, JsonValue], case: Case, expected: bool) -> float:
        from deepeval.metrics import GEval
        from deepeval.test_case import SingleTurnParams

        reads = [SingleTurnParams.INPUT, SingleTurnParams.ACTUAL_OUTPUT]
        if expected:
            reads.append(SingleTurnParams.EXPECTED_OUTPUT)
        return self._measured(
            GEval(
                name="aiwatcher",
                criteria=str(parameters["criteria"]),
                evaluation_params=reads,
                model=self._llm,
                async_mode=False,
            ),
            case,
        )

    def _g_eval(self, parameters: Mapping[str, JsonValue], case: Case) -> float:
        return self._geval(parameters, case, expected=False)

    def _g_eval_with_expected(self, parameters: Mapping[str, JsonValue], case: Case) -> float:
        return self._geval(parameters, case, expected=True)
