"""Durable Evaluation client. Failures raise; telemetry remains best effort.

Retry publication with the same evaluation ID and body after an ambiguous
transport failure. The server returns the first receipt or an explicit conflict.
Use ``evaluation`` for types without importing the HTTP transport.

The same client carries rubrics and assessments, because they have the same
owner: a judgement about a case is meaningless beside a result somebody else
holds. And it carries scoring runs — a scorecard, a staged recording and a
declaration that measures one against the other — because what they publish is
evidence in this registry, admitted at the same gate as everything else.
"""

from __future__ import annotations

import json
from collections.abc import Sequence
from types import TracebackType
from typing import Any, Literal, NotRequired, Self, TypedDict
from urllib.parse import quote

import httpx

from aiwatcher_sdk.api import ApiError, Transport
from aiwatcher_sdk.evaluation import (
    ArtifactReference,
    EvaluationManifest,
    VariantManifest,
    VersionReference,
)


class EvaluationRegistryError(ApiError):
    """The durable registry refused or could not complete an operation."""


class NumericScale(TypedDict):
    kind: Literal["numeric"]
    min: float
    max: float


class OrdinalScale(TypedDict):
    """Named levels, in the order declared. Order is meaning: the direction
    says which end is better, so reordering them is a different rubric."""

    kind: Literal["ordinal"]
    levels: list[str]


class FlagScale(TypedDict):
    kind: Literal["flag"]


Scale = NumericScale | OrdinalScale | FlagScale


class Rubric(TypedDict):
    """The form an assessment is given on. Versioned by its content, so
    publishing the same words twice lands on the version already there."""

    name: str
    question: str
    scale: Scale
    direction: Literal["higher", "lower", "none"]
    guidance: NotRequired[str]


class NumberValue(TypedDict):
    type: Literal["number"]
    value: float


class LevelValue(TypedDict):
    type: Literal["level"]
    value: str


class FlagValue(TypedDict):
    type: Literal["flag"]
    value: bool


AssessmentValue = NumberValue | LevelValue | FlagValue


class TraceTarget(TypedDict):
    kind: Literal["trace"]
    trace_id: str


class SpanTarget(TypedDict):
    kind: Literal["span"]
    trace_id: str
    span_id: str


class SessionTarget(TypedDict):
    """A session is still being added to, so judging one names the moment."""

    kind: Literal["session"]
    session_id: str
    as_of: int


class CaseTarget(TypedDict):
    kind: Literal["case"]
    evaluation_id: str
    case_id: str
    repetition_id: str


AssessmentTarget = TraceTarget | SpanTarget | SessionTarget | CaseTarget


class ExactMatch(TypedDict):
    kind: Literal["exact_match"]
    ignore_case: NotRequired[bool]
    trim: NotRequired[bool]


class Contains(TypedDict):
    kind: Literal["contains"]
    ignore_case: NotRequired[bool]


class RegexMatch(TypedDict):
    """The same question of every answer, so it reads no expected answer."""

    kind: Literal["regex_match"]
    pattern: str


class NumericWithin(TypedDict):
    kind: Literal["numeric_within"]
    tolerance: float


class Forbidden(TypedDict):
    """Counted rather than avoided: the server declares lower as better."""

    kind: Literal["forbidden"]
    text: str
    ignore_case: NotRequired[bool]


Scorer = ExactMatch | Contains | RegexMatch | NumericWithin | Forbidden


class ScorerSpec(TypedDict):
    """One measurement. Which way its metric is better is the server's to say."""

    metric: str
    scorer: Scorer
    answer_path: NotRequired[str]
    expected_path: NotRequired[str]


class Scorecard(TypedDict):
    name: str
    scorers: list[ScorerSpec]
    description: NotRequired[str]


class RecordedAnswer(TypedDict):
    case_id: str
    answer: Any
    trace_id: NotRequired[str]
    span_id: NotRequired[str]


class Cohort(TypedDict):
    case_manifest: ArtifactReference
    case_count: int
    split: str
    input_schema: ArtifactReference
    expectations_schema: ArtifactReference


class ScoringRun(TypedDict):
    evaluation_id: str
    repetition_id: str
    variant: VariantManifest
    cohort: Cohort
    scorecard: VersionReference
    answers: ArtifactReference


class CaseMeasurement(TypedDict):
    case_id: str
    repetition_id: str
    actual: Any
    metrics: dict[str, float]
    error: str | None
    trace_id: str | None
    span_id: str | None


class EvaluationRegistry:
    def __init__(
        self,
        base_url: str,
        *,
        token: str | None = None,
        timeout: float = 30.0,
        attempts: int = 3,
        client: httpx.Client | None = None,
    ) -> None:
        self._transport = Transport(
            base_url,
            token=token,
            timeout=timeout,
            attempts=attempts,
            client=client,
            error=EvaluationRegistryError,
            subject="the evaluation registry",
        )

    def close(self) -> None:
        self._transport.close()

    def __enter__(self) -> Self:
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc_value: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        self.close()

    def publish(
        self,
        manifest: EvaluationManifest,
        cases: list[CaseMeasurement],
        *,
        status: Literal["succeeded", "failed", "partial"] = "succeeded",
    ) -> dict[str, Any]:
        response = self._transport.send(
            "POST",
            "/api/v1/evaluation-results",
            {"manifest": manifest, "cases": cases, "status": status},
            idempotent=True,
        )
        return self._object(response)

    def get(self, evaluation_id: str) -> dict[str, Any]:
        return self._object(self._transport.send("GET", self._path(evaluation_id)))

    def list(self, *, cursor: str | None = None, limit: int = 200) -> dict[str, Any]:
        return self._object(
            self._transport.send(
                "GET", "/api/v1/evaluation-results", params={"cursor": cursor, "limit": limit}
            )
        )

    def cases(
        self,
        evaluation_id: str,
        version: str,
        *,
        cursor: str | None = None,
        limit: int = 200,
    ) -> dict[str, Any]:
        return self._object(
            self._transport.send(
                "GET",
                self._path(evaluation_id) + "/cases",
                params={"version": version, "cursor": cursor, "limit": limit},
            )
        )

    def publish_rubric(self, rubric: Rubric) -> dict[str, Any]:
        """Declare the form a judgement may be given on.

        Idempotent by content: the same words answer with the version that is
        already there, so a deploy step may publish its rubrics every time.
        """
        return self._object(
            self._transport.send(
                "POST", "/api/v1/evaluation-rubrics", dict(rubric), idempotent=True
            )
        )

    def list_rubrics(self) -> dict[str, Any]:
        return self._object(self._transport.send("GET", "/api/v1/evaluation-rubrics"))

    def get_rubric(self, name: str, *, version: str | None = None) -> dict[str, Any]:
        """One form, at ``version`` or at the head.

        Read the version before answering under it: the scale is what an answer
        has to fit, and a head moves.
        """
        return self._object(
            self._transport.send(
                "GET",
                "/api/v1/evaluation-rubrics/" + quote(name, safe=""),
                params={"version": version},
            )
        )

    def assess(
        self,
        target: AssessmentTarget,
        rubric: str,
        value: AssessmentValue,
        *,
        rubric_version: str | None = None,
        source: Literal["human", "judge"] = "human",
        author: str | None = None,
        rationale: str = "",
    ) -> dict[str, Any]:
        """Record one judgement about one thing.

        ``author`` names the judge that answered and is refused for a person:
        a person's judgement is attributed to the session that filed it, so a
        client that could name somebody else could file their judgement.

        Safe to send twice. Repeating what the current revision already says
        lands on that revision rather than recording a second one.
        """
        body: dict[str, Any] = {
            "target": dict(target),
            "rubric": rubric,
            "value": dict(value),
            "source": source,
            "rationale": rationale,
        }
        if rubric_version is not None:
            body["rubric_version"] = rubric_version
        if author is not None:
            body["author"] = author
        return self._object(
            self._transport.send("POST", "/api/v1/evaluation-assessments", body, idempotent=True)
        )

    def get_assessments(self, target: AssessmentTarget) -> dict[str, Any]:
        """Every standing judgement about one thing, at its current revision."""
        return self._object(
            self._transport.send("GET", "/api/v1/evaluation-assessments", params=dict(target))
        )

    def get_assessment_history(
        self,
        target_id: str,
        standing_id: str,
        *,
        before: int | None = None,
        limit: int | None = None,
    ) -> dict[str, Any]:
        """What one standing judgement said over time, newest first.

        Both IDs come from :meth:`get_assessments` or from a recorded
        judgement. Nothing here derives either.
        """
        return self._object(
            self._transport.send(
                "GET",
                "/api/v1/evaluation-assessments/"
                + quote(target_id, safe="")
                + "/"
                + quote(standing_id, safe=""),
                params={"before": before, "limit": limit},
            )
        )

    def publish_scorecard(self, scorecard: Scorecard) -> dict[str, Any]:
        """Declare what an evaluation measures.

        Idempotent by content, like a rubric. The metric each scorer writes
        comes back with its direction and unit, which the server derives: a
        scorecard carries no code and no opinion about which way is better.
        """
        return self._object(
            self._transport.send(
                "POST", "/api/v1/evaluation-scorecards", dict(scorecard), idempotent=True
            )
        )

    def list_scorecards(self) -> dict[str, Any]:
        return self._object(self._transport.send("GET", "/api/v1/evaluation-scorecards"))

    def get_scorecard(self, name: str, *, version: str | None = None) -> dict[str, Any]:
        return self._object(
            self._transport.send(
                "GET",
                "/api/v1/evaluation-scorecards/" + quote(name, safe=""),
                params={"version": version},
            )
        )

    def stage_recording(self, name: str, answers: Sequence[RecordedAnswer]) -> dict[str, Any]:
        """Keep the answers a scoring run will measure; return their reference.

        The server digests the bytes it received, so they are encoded here the
        same way every time — a retried upload is the same bytes, and lands on
        the same reference rather than beside it.
        """
        content = json.dumps(
            {"answers": list(answers)}, sort_keys=True, separators=(",", ":"), ensure_ascii=False
        ).encode()
        return self._object(
            self._transport.send(
                "PUT",
                "/api/v1/evaluation-recordings/" + quote(name, safe=""),
                content=content,
                content_type="application/json",
                idempotent=True,
            )
        )

    def declare_scoring_run(self, run: ScoringRun) -> dict[str, Any]:
        """Declare a measurement of a staged recording, without starting it.

        The answer carries the manifest a result will publish and the
        ``approval_id`` that admits it. Admit that manifest — it is derived, and
        writing it out by hand is a second answer to what the run measures —
        then call :meth:`start_scoring_run`. Idempotent by content.
        """
        return self._object(
            self._transport.send("POST", "/api/v1/evaluation-runs", dict(run), idempotent=True)
        )

    def get_scoring_run(self, declaration: str) -> dict[str, Any]:
        return self._object(
            self._transport.send("GET", "/api/v1/evaluation-runs/" + quote(declaration, safe=""))
        )

    def start_scoring_run(self, declaration: str) -> dict[str, Any]:
        """Start a declared measurement as a managed run.

        Safe to send twice: the declaration is the run's identity, so a repeat
        reaches the run already going. Raises while nothing admits the pair, and
        the message names the approval that would.
        """
        return self._object(
            self._transport.send(
                "POST",
                "/api/v1/evaluation-runs/" + quote(declaration, safe="") + "/start",
                {},
                idempotent=True,
            )
        )

    @staticmethod
    def _path(evaluation_id: str) -> str:
        return "/api/v1/evaluation-results/" + quote(evaluation_id, safe="")

    @staticmethod
    def _object(response: httpx.Response) -> dict[str, Any]:
        try:
            value: Any = response.json()
        except ValueError as error:
            raise EvaluationRegistryError(
                "the evaluation registry returned invalid JSON"
            ) from error
        if not isinstance(value, dict):
            raise EvaluationRegistryError("the evaluation registry returned no object")
        return value
