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
    def answer(case: Case, run: Generation) -> Generated:
        with run.traced(telemetry, case) as traced:
            said = support_bot(case.input, prompt=run.variant["prompt"], telemetry=traced)
        return Generated(said, run_id=traced.correlation.run_id)

What the answer names — the run it was made in — is how aiwatcher holds it to
the variant's prompt, model and workflow: the traces step reads that run off
the log and refuses answers whose model calls rendered another prompt version
or were served by another model version, or whose run declared the pinned
workflow in another shape (``run.traced_workflow``). A model server that sends
its own run for the call (``llm.caller_headers()``) is a second witness.
"""

from __future__ import annotations

import contextlib
import hashlib
import time
import uuid
from collections.abc import Callable, Generator
from dataclasses import dataclass
from typing import TYPE_CHECKING, Any, cast

from aiwatcher_sdk.task import Task
from aiwatcher_sdk.task_errors import TaskError
from aiwatcher_sdk.worker.context import get_task_context
from aiwatcher_sdk.worker.contract import JsonObject, JsonValue

if TYPE_CHECKING:
    from aiwatcher_sdk import AiwatcherClient, RunContext, WorkflowContext

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
    #: The variant's content address. The application's runs name it, with
    #: ``evaluation_id`` beside it on their start
    #: (``client.run(…, variant_id=run.variant_id, evaluation_id=run.evaluation_id)``),
    #: so a trace says which variant made it and that a measurement did.
    variant_id: str = ""

    @contextlib.contextmanager
    def traced(self, client: AiwatcherClient, case: Case) -> Generator[RunContext, None, None]:
        """The application's run for one case, named as this variant's and this measurement's.

        Its id is new for every call, so a retried attempt's runs are not folded
        into the first one's; name it on the answer
        (``Generated(…, run_id=traced.correlation.run_id)``) and every model call
        made inside it is what the answer is held to.
        """
        with client.run(
            f"generate-{self.evaluation_id}-{case.case_id}-{uuid.uuid4().hex[:12]}",
            variant_id=self.variant_id or None,
            evaluation_id=self.evaluation_id,
        ) as run:
            yield run

    @contextlib.contextmanager
    def traced_workflow(
        self,
        client: AiwatcherClient,
        case: Case,
        workflow_id: str,
        *,
        nodes: list[str] | list[dict[str, Any]],
        edges: list[tuple[str, str]] | list[dict[str, Any]] | None = None,
    ) -> Generator[WorkflowContext, None, None]:
        """The same run, as an execution of the workflow the variant pins.

        It declares ``nodes`` and ``edges``, and each stage opened with
        ``flow.node(…)`` is a step of that run — which is what the answer is
        held to beside the prompt and the model: a run declaring the pinned
        workflow in another shape, or stepping through a node the pinned
        declaration does not have, is not the variant's. So is one stepping in
        an order the edges do not lead: a node starts once per completion of a
        node leading into it (a retry after a failure needs none), so a
        declared loop goes round as often as it completes and a node started
        twice for one completion is refused — declare a node that runs once per
        item as ``{"id": …, "repeats": True}``, and bound how many times a node
        may start, retries included, with ``"at_most": n``. Name the run on the
        answer as with :meth:`traced` (``run_id=flow.correlation.run_id``).
        """
        with client.workflow(
            workflow_id,
            nodes=nodes,
            edges=edges,
            run_id=f"generate-{self.evaluation_id}-{case.case_id}-{uuid.uuid4().hex[:12]}",
            variant_id=self.variant_id or None,
            evaluation_id=self.evaluation_id,
        ) as flow:
            yield flow


@dataclass(frozen=True)
class Generated:
    """An answer, the run and trace of making it, and the tokens it cost.

    ``run_id`` is the application's run on aiwatcher's log — what the answer is
    held to the variant's prompt and model through; ``Generation.traced`` opens
    one. The tokens are the application's count, since only it saw its model's
    reply; left ``None`` they are not counted, which is never zero. How long
    the answer took is measured here, around the call that made it.
    """

    answer: JsonValue
    run_id: str | None = None
    trace_id: str | None = None
    span_id: str | None = None
    input_tokens: int | None = None
    output_tokens: int | None = None


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
    worker's word, checked for agreement rather than proved. The prompt and model
    are held through the traces of the runs the answers name instead.
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
) -> Callable[[Answering], Task[[str, str, str, JsonObject, JsonObject, str], JsonObject]]:
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
    ) -> Task[[str, str, str, JsonObject, JsonObject, str], JsonObject]:
        def generate(
            declaration: str,
            evaluation_id: str,
            repetition_id: str,
            variant: JsonObject,
            params: JsonObject,
            variant_id: str,
        ) -> JsonObject:
            context = get_task_context()
            run = Generation(
                declaration=declaration,
                evaluation_id=evaluation_id,
                repetition_id=repetition_id,
                variant=variant,
                params=params,
                variant_id=variant_id,
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
                started = time.monotonic()
                produced = answering(case, run)
                took = (time.monotonic() - started) * 1000
                if isinstance(produced, Declined):
                    continue
                answered = produced if isinstance(produced, Generated) else Generated(produced)
                written: JsonObject = {"case_id": case.case_id, "answer": answered.answer}
                if answered.run_id is not None:
                    written["run_id"] = answered.run_id
                if answered.trace_id is not None:
                    written["trace_id"] = answered.trace_id
                if answered.span_id is not None:
                    written["span_id"] = answered.span_id
                usage: JsonObject = {"latency_ms": round(took, 3)}
                if answered.input_tokens is not None:
                    usage["input_tokens"] = answered.input_tokens
                if answered.output_tokens is not None:
                    usage["output_tokens"] = answered.output_tokens
                written["usage"] = usage
                rows.append(written)
            context.write_artifact(ANSWERS, rows)
            context.write_artifact(GENERATED_WITH, [held.row()])
            return {"answered": len(rows)}

        return Task(generate, name, version)

    return declare
