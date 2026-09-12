"""Opik's metrics, behind the contract.

Imported only when this adapter is loaded. Every metric is built with
``track=False``: Opik traces a metric call to its own server by default, and a
case this service is sent must go nowhere but the model.

Heuristics need nothing, and are listed whether or not a model is configured.
Graded metrics ask the one model ``aiwatcher_scorers.model`` configures,
through LiteLLM's OpenAI-compatible route. A metric whose score Opik itself
marks as failed is a failure, never the number it carried.
"""

from __future__ import annotations

import importlib.metadata
from collections.abc import Mapping
from functools import cached_property
from typing import Any

from aiwatcher_scorers.adapter import Implemented, graded
from aiwatcher_scorers.contract import Case, JsonValue, Metric, ModelReference, Parameter, text
from aiwatcher_scorers.model import ModelSettings

UNIT = (0.0, 1.0)


class ScoringFailedError(Exception):
    """Opik said its own score failed."""


def load(model: ModelSettings | None) -> Opik | None:
    try:
        importlib.metadata.version("opik")
    except importlib.metadata.PackageNotFoundError:
        return None
    return Opik(model)


def _value(result: Any) -> float:
    if getattr(result, "scoring_failed", False):
        raise ScoringFailedError
    return float(result.value)


class Opik:
    def __init__(self, settings: ModelSettings | None) -> None:
        self._settings = settings

    @property
    def name(self) -> str:
        return "opik"

    @property
    def version(self) -> str:
        return importlib.metadata.version("opik")

    @property
    def model(self) -> ModelReference | None:
        return self._settings.reference if self._settings else None

    @cached_property
    def _llm(self) -> Any:
        from opik.evaluation.models import LiteLLMChatModel

        settings = self._settings
        if settings is None:  # pragma: no cover — a graded metric is not listed without one
            raise RuntimeError("no model is configured for graded metrics")
        extra = {"extra_body": settings.extra_body} if settings.extra_body else {}
        return LiteLLMChatModel(
            model_name=f"openai/{settings.name}",
            track=False,
            api_base=settings.url,
            api_key=settings.token or "not-needed",
            **extra,
        )

    def metrics(self) -> Mapping[str, Implemented]:
        case_sensitive = {"case_sensitive": Parameter("boolean")}
        table: dict[str, Implemented] = {
            "equals": Implemented(
                Metric(
                    "equals",
                    unit="ratio",
                    direction="higher",
                    aggregation="rate",
                    reads=("answer", "expected"),
                    description="The answer is the expected one, as text.",
                    parameters=case_sensitive,
                ),
                self._equals,
            ),
            "contains": Implemented(
                Metric(
                    "contains",
                    unit="ratio",
                    direction="higher",
                    aggregation="rate",
                    reads=("answer", "expected"),
                    description="The expected text appears in the answer.",
                    parameters=case_sensitive,
                ),
                self._contains,
            ),
            "regex_match": Implemented(
                Metric(
                    "regex_match",
                    unit="ratio",
                    direction="higher",
                    aggregation="rate",
                    reads=("answer",),
                    description="The answer contains a match for the pattern.",
                    parameters={"regex": Parameter("string", required=True)},
                ),
                self._regex_match,
            ),
            "is_json": Implemented(
                Metric(
                    "is_json",
                    unit="ratio",
                    direction="higher",
                    aggregation="rate",
                    reads=("answer",),
                    description="The answer parses as JSON.",
                ),
                self._is_json,
            ),
            "levenshtein_ratio": Implemented(
                Metric(
                    "levenshtein_ratio",
                    unit="score",
                    direction="higher",
                    aggregation="mean",
                    reads=("answer", "expected"),
                    description="How close the answer's text is to the expected text.",
                    range=UNIT,
                    parameters=case_sensitive,
                ),
                self._levenshtein_ratio,
            ),
        }
        if self._settings is None:
            return table
        table.update(
            {
                "answer_relevance": graded(
                    "answer_relevance",
                    direction="higher",
                    reads=("input", "answer"),
                    description="How relevant the answer is to what was asked.",
                    score=self._answer_relevance,
                ),
                "hallucination": graded(
                    "hallucination",
                    direction="lower",
                    reads=("input", "answer"),
                    description="Whether the answer states what the question gives no ground for.",
                    score=self._hallucination,
                ),
                "moderation": graded(
                    "moderation",
                    direction="lower",
                    reads=("answer",),
                    description="How far the answer breaks content policy.",
                    score=self._moderation,
                ),
                "usefulness": graded(
                    "usefulness",
                    direction="higher",
                    reads=("input", "answer"),
                    description="How useful the answer is to what was asked.",
                    score=self._usefulness,
                ),
                "g_eval": graded(
                    "g_eval",
                    direction="higher",
                    reads=("answer",),
                    description="How well the answer meets the criteria, for the task described.",
                    score=self._g_eval,
                    parameters={
                        "task_introduction": Parameter("string", required=True),
                        "evaluation_criteria": Parameter("string", required=True),
                    },
                ),
            }
        )
        return table

    @staticmethod
    def _sensitive(parameters: Mapping[str, JsonValue]) -> bool:
        return bool(parameters.get("case_sensitive", False))

    def _equals(self, parameters: Mapping[str, JsonValue], case: Case) -> float:
        from opik.evaluation.metrics import Equals

        return _value(
            Equals(case_sensitive=self._sensitive(parameters), track=False).score(
                output=text(case.answer), reference=text(case.expected)
            )
        )

    def _contains(self, parameters: Mapping[str, JsonValue], case: Case) -> float:
        from opik.evaluation.metrics import Contains

        return _value(
            Contains(case_sensitive=self._sensitive(parameters), track=False).score(
                output=text(case.answer), reference=text(case.expected)
            )
        )

    def _regex_match(self, parameters: Mapping[str, JsonValue], case: Case) -> float:
        from opik.evaluation.metrics import RegexMatch

        return _value(
            RegexMatch(regex=str(parameters["regex"]), track=False).score(output=text(case.answer))
        )

    def _is_json(self, _: Mapping[str, JsonValue], case: Case) -> float:
        from opik.evaluation.metrics import IsJson

        return _value(IsJson(track=False).score(output=text(case.answer)))

    def _levenshtein_ratio(self, parameters: Mapping[str, JsonValue], case: Case) -> float:
        from opik.evaluation.metrics import LevenshteinRatio

        return _value(
            LevenshteinRatio(case_sensitive=self._sensitive(parameters), track=False).score(
                output=text(case.answer), reference=text(case.expected)
            )
        )

    def _answer_relevance(self, _: Mapping[str, JsonValue], case: Case) -> float:
        from opik.evaluation.metrics import AnswerRelevance

        return _value(
            AnswerRelevance(
                model=self._llm, require_context=False, track=False, temperature=0.0
            ).score(input=text(case.input), output=text(case.answer))
        )

    def _hallucination(self, _: Mapping[str, JsonValue], case: Case) -> float:
        from opik.evaluation.metrics import Hallucination

        return _value(
            Hallucination(model=self._llm, track=False, temperature=0.0).score(
                input=text(case.input), output=text(case.answer)
            )
        )

    def _moderation(self, _: Mapping[str, JsonValue], case: Case) -> float:
        from opik.evaluation.metrics import Moderation

        return _value(
            Moderation(model=self._llm, track=False, temperature=0.0).score(
                output=text(case.answer)
            )
        )

    def _usefulness(self, _: Mapping[str, JsonValue], case: Case) -> float:
        from opik.evaluation.metrics import Usefulness

        return _value(
            Usefulness(model=self._llm, track=False, temperature=0.0).score(
                input=text(case.input), output=text(case.answer)
            )
        )

    def _g_eval(self, parameters: Mapping[str, JsonValue], case: Case) -> float:
        from opik.evaluation.metrics import GEval

        return _value(
            GEval(
                task_introduction=str(parameters["task_introduction"]),
                evaluation_criteria=str(parameters["evaluation_criteria"]),
                model=self._llm,
                track=False,
                temperature=0.0,
            ).score(output=text(case.answer))
        )
