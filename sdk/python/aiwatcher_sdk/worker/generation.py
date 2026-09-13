"""The worker task that generates a scoring run's answers.

A scoring run may name a task rather than a recording for its answers
(``{"generated_by": {"task": "name@version", "queue": …}}``). aiwatcher then
hands a worker each case's input — a row per case in ``cases``, and never what
the case expected, which a generator could otherwise copy — and scores the rows
the task writes to ``answers``, one per case. What makes an answer is the
application's, so it is a task a worker registers rather than code a
declaration carries; the task is told the variant the result will be published
as, so one worker can serve a baseline and a candidate — and says what it
generates with, which is held to the variant's pins.

    @generation_task("support-bot.answer", version="3", generated_with=holding)
    def answer(case: Case, run: Generation) -> JsonValue:
        return support_bot(case.input, prompt=run.variant["prompt"])
"""

from __future__ import annotations

import hashlib
from collections.abc import Callable
from dataclasses import dataclass
from typing import cast

from aiwatcher_sdk.task import Task
from aiwatcher_sdk.task_errors import TaskError
from aiwatcher_sdk.worker.context import get_task_context
from aiwatcher_sdk.worker.contract import JsonObject, JsonValue

#: What the step before reads the cohort into, and what this task writes.
CASES = "cases"
ANSWERS = "answers"
GENERATED_WITH = "generated_with"


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


@dataclass(frozen=True)
class GeneratedWith:
    """What this worker answers a variant with: the sha256 of each artifact's bytes.

    The variant pins its code and generation config by digest, and only the
    worker holds either — one built from another commit, or handed another
    config, would answer under the variant's name with something else. So the
    task says what it holds before it answers anything, is refused before a
    model is asked when that is not what the variant pins, and writes it beside
    the answers for aiwatcher's score step to hold to the same pins. It is the
    worker's word, checked for agreement rather than proved.
    """

    code: str
    generation_config: str
    #: Required exactly when the variant pins one.
    response_schema: str | None = None
    tools: str | None = None

    @classmethod
    def of(
        cls,
        *,
        code: bytes,
        generation_config: bytes,
        response_schema: bytes | None = None,
        tools: bytes | None = None,
    ) -> GeneratedWith:
        """From the bytes themselves, digested the way a variant pins them."""

        def digest(content: bytes | None) -> str | None:
            return None if content is None else hashlib.sha256(content).hexdigest()

        return cls(
            code=hashlib.sha256(code).hexdigest(),
            generation_config=hashlib.sha256(generation_config).hexdigest(),
            response_schema=digest(response_schema),
            tools=digest(tools),
        )

    def disagreements(self, variant: JsonObject) -> list[str]:
        """Every artifact this is not what ``variant`` pins, naming both digests."""

        def pinned(field: str) -> str | None:
            artifact = variant.get(field)
            return str(artifact["digest"]) if isinstance(artifact, dict) else None

        return [
            f"{field}: the variant pins {pins or 'nothing'} "
            f"and the task generated with {held or 'nothing'}"
            for field, pins, held in (
                ("code", pinned("code"), self.code),
                ("generation_config", pinned("generation_config"), self.generation_config),
                ("response_schema", pinned("response_schema"), self.response_schema),
                ("tools", pinned("tools"), self.tools),
            )
            if pins != held
        ]

    def row(self) -> JsonObject:
        written: JsonObject = {"code": self.code, "generation_config": self.generation_config}
        if self.response_schema is not None:
            written["response_schema"] = self.response_schema
        if self.tools is not None:
            written["tools"] = self.tools
        return written


type Answering = Callable[[Case, Generation], JsonValue | Generated | Declined]


def generation_task(
    name: str, *, version: str, generated_with: Callable[[Generation], GeneratedWith]
) -> Callable[[Answering], Task[[str, str, str, JsonObject, JsonObject], JsonObject]]:
    """Declare a task that answers every case of a scoring run, one call per case.

    ``generated_with`` says what this worker would answer the run's variant with;
    when that is not what the variant pins, the attempt fails before a case is
    read. Otherwise the task reads ``cases``, calls the function once per case in
    the cohort's order, looks at the run's cancellation between cases, and writes
    one row per case it answered to ``answers`` — a case the function ``Declined``
    is left out, and counts as unscored — and what it generated with to
    ``generated_with``. A function that raises fails the attempt, which the run
    retries from the first case: the rows are written once, at the end, so a
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
            held = generated_with(run)
            disagreements = held.disagreements(variant)
            if disagreements:
                raise TaskError(
                    "this worker does not generate with what the variant pins, so its answers "
                    "would not be the variant's — " + "; ".join(disagreements)
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
            context.write_artifact(GENERATED_WITH, [held.row()])
            return {"answered": len(rows)}

        return Task(generate, name, version)

    return declare
