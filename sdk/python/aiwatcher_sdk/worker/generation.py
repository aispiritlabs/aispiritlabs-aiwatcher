"""The worker task that generates a scoring run's answers.

A scoring run may name a task rather than a recording for its answers
(``{"generated_by": {"task": "name@version", "queue": …}}``). aiwatcher then
hands a worker each case's input — a row per case in ``cases``, and never what
the case expected, which a generator could otherwise copy — and scores the rows
the task writes to ``answers``, one per case. What makes an answer is the
application's, so it is a task a worker registers rather than code a
declaration carries; the task is told the variant the result will be published
as, so one worker can serve a baseline and a candidate.

    @generation_task("support-bot.answer", version="3")
    def answer(case: Case, run: Generation) -> JsonValue:
        return support_bot(case.input, prompt=run.variant["prompt"])
"""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass
from typing import cast

from aiwatcher_sdk.task import Task
from aiwatcher_sdk.worker.context import get_task_context
from aiwatcher_sdk.worker.contract import JsonObject, JsonValue

#: What the step before reads the cohort into, and what this task writes.
CASES = "cases"
ANSWERS = "answers"


@dataclass(frozen=True)
class Case:
    """One case, as a generator may see it: its id and what it asked."""

    case_id: str
    #: ``None`` when the cohort's owner keeps no input for this case.
    input: JsonValue


@dataclass(frozen=True)
class Generation:
    """The run a task is generating answers for."""

    declaration: str
    evaluation_id: str
    repetition_id: str
    #: The variant manifest the result is published as: its model, prompt and
    #: code references are what the task should answer with.
    variant: JsonObject
    #: What the declaration hands the task beside the variant.
    params: JsonObject


@dataclass(frozen=True)
class Generated:
    """An answer, and the trace of making it, for a result that links to it."""

    answer: JsonValue
    trace_id: str | None = None
    span_id: str | None = None


@dataclass(frozen=True)
class Declined:
    """No answer to this case. It is left out, and the result counts it unscored.

    Never an empty answer: a scorer would measure one and call it wrong, and a
    case the application could not answer would read as one it answered badly.
    """

    reason: str = ""


type Answering = Callable[[Case, Generation], JsonValue | Generated | Declined]


def generation_task(
    name: str, *, version: str
) -> Callable[[Answering], Task[[str, str, str, JsonObject, JsonObject], JsonObject]]:
    """Declare a task that answers every case of a scoring run, one call per case.

    The task reads ``cases``, calls the function once per case in the cohort's
    order, looks at the run's cancellation between cases, and writes one row
    per case it answered to ``answers`` — a case the function ``Declined`` is
    left out, and counts as unscored. A function that raises fails the attempt, which the
    run retries from the first case: the rows are written once, at the end, so a
    retry never scores half of one attempt beside half of another.
    """

    def declare(
        answering: Answering,
    ) -> Task[[str, str, str, JsonObject, JsonObject], JsonObject]:
        def generate(
            declaration: str,
            evaluation_id: str,
            repetition_id: str,
            variant: JsonObject,
            params: JsonObject,
        ) -> JsonObject:
            context = get_task_context()
            run = Generation(
                declaration=declaration,
                evaluation_id=evaluation_id,
                repetition_id=repetition_id,
                variant=variant,
                params=params,
            )
            rows: list[JsonObject] = []
            for row in context.read_artifact(CASES):
                context.raise_if_cancelled()
                case = Case(case_id=cast(str, row["case_id"]), input=row.get("input"))
                produced = answering(case, run)
                if isinstance(produced, Declined):
                    continue
                answered = produced if isinstance(produced, Generated) else Generated(produced)
                written: JsonObject = {"case_id": case.case_id, "answer": answered.answer}
                if answered.trace_id is not None:
                    written["trace_id"] = answered.trace_id
                if answered.span_id is not None:
                    written["span_id"] = answered.span_id
                rows.append(written)
            context.write_artifact(ANSWERS, rows)
            return {"answered": len(rows)}

        return Task(generate, name, version)

    return declare
