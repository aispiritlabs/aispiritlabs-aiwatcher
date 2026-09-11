"""The lease and artifact boundary a running task cooperates with."""

from __future__ import annotations

import threading
import time
import uuid
from collections.abc import Callable, Generator, Sequence
from contextlib import contextmanager
from contextvars import ContextVar
from datetime import UTC, datetime
from typing import Any

from aiwatcher_sdk import AiwatcherClient, Correlation, RunContext
from aiwatcher_sdk.integrations.agentic import AiwatcherTracer
from aiwatcher_sdk.integrations.agentic.tracer import current_attempt
from aiwatcher_sdk.worker.assignment import Assignment
from aiwatcher_sdk.worker.attempt import AttemptAPI
from aiwatcher_sdk.worker.contract import ArtifactRef, JsonObject, JsonValue, OnTimeout
from aiwatcher_sdk.worker.errors import InputRequired, LeaseLostError, TaskError, WorkerError

current_context: ContextVar[TaskContext | None] = ContextVar("aiwatcher_task_context", default=None)

_EVALUATIONS = uuid.uuid5(uuid.NAMESPACE_URL, "aiwatcher:evaluation")


def evaluation_id_for(step_key: str, suite: str, variant: str | None) -> str:
    """The id a step's report of ``suite`` on ``variant`` is filed under.

    A UUID, the shape a report recorded anywhere else gets, and the same on
    every attempt of the step — which is the whole point of it.
    """
    return str(uuid.uuid5(_EVALUATIONS, f"{step_key}/{suite}/{variant or ''}"))


def get_task_context() -> TaskContext:
    """The current managed task, or a clear error outside a worker invocation.

    A task-created thread must explicitly propagate context with copy_context().
    """
    context = current_context.get()
    if context is None:
        raise RuntimeError("no managed task is running in this context")
    return context


class TaskContext:
    """Use ``context_id`` to deduplicate side effects in the task's own store.

    Cancellation is cooperative: check ``raise_if_cancelled`` inside long loops.
    Artifact methods check it too. Arbitrary Python code cannot be forcibly stopped.
    """

    def __init__(
        self,
        assignment: Assignment,
        api: AttemptAPI,
        client: AiwatcherClient,
        *,
        monotonic: Callable[[], float] = time.monotonic,
        now: Callable[[], datetime] = lambda: datetime.now(UTC),
    ) -> None:
        self.assignment = assignment
        self.context_id = assignment.context_id
        self.run_id = assignment.execution_id
        self.workflow_run_id = assignment.execution_id
        self.client = client
        correlation = Correlation(
            run_id=assignment.execution_id,
            workflow_id=assignment.workflow_id or None,
            workflow_run_id=assignment.execution_id,
            correlation_id=assignment.execution_id,
            parent_span_id=assignment.parent_span_id or None,
        )
        self.correlation = correlation
        self.run = RunContext(client, correlation)
        self.tracer = AiwatcherTracer(client=client, context=correlation)
        self._monotonic = monotonic
        self._api = api
        self._outputs: dict[str, ArtifactRef] = {}
        self._answers = iter(assignment.answers)
        self._stopped = threading.Event()
        self._error: WorkerError | None = None
        self._heartbeat_lock = threading.Lock()
        self._deadline = self._monotonic() + assignment.timeout_seconds
        remaining = (assignment.lease_expires_at - now()).total_seconds()
        if remaining <= 0:
            raise LeaseLostError("the assignment arrived after its lease expired")
        self._interval = max(0.001, remaining / 2)
        self._thread = threading.Thread(
            target=self._keep_alive,
            daemon=True,
            name=f"aiwatcher-heartbeat-{assignment.context_id}",
        )

    @property
    def step_key(self) -> str:
        """The same on every attempt of this step, where ``context_id`` is not.

        What a write keyed by it does once however many attempts the step takes:
        the key a task hands its tools when a retry must not repeat them, and
        the one a record of the step's outcome is filed under so a retry lands
        on it rather than beside it.
        """
        return f"{self.assignment.execution_id}/{self.assignment.step_id}"

    @property
    def outputs(self) -> list[ArtifactRef]:
        return list(self._outputs.values())

    @contextmanager
    def activate(self) -> Generator[TaskContext, None, None]:
        token = current_context.set(self)
        attempt = current_attempt.set(self.correlation)
        try:
            yield self
        finally:
            current_attempt.reset(attempt)
            current_context.reset(token)

    @property
    def cancelled(self) -> bool:
        return self._error is not None or self._monotonic() >= self._deadline

    def raise_if_cancelled(self) -> None:
        self.check_lease()
        if self._monotonic() >= self._deadline:
            raise TaskError("the task exceeded its timeout", classification="timeout")

    def check_lease(self) -> None:
        """Surface heartbeat failure even if task code caught the original error."""
        if self._error is not None:
            raise self._error

    def heartbeat(self) -> None:
        with self._heartbeat_lock:
            self.raise_if_cancelled()
            try:
                self._api.heartbeat()
            except WorkerError as error:
                self._error = (
                    LeaseLostError(str(error), status=error.status, code=error.code)
                    if error.status == 409
                    else error
                )
                raise self._error from error

    def ask(
        self,
        prompt: str,
        *,
        role: str = "editor",
        choices: Sequence[str] = (),
        timeout_seconds: int | None = None,
        on_timeout: OnTimeout | None = None,
    ) -> JsonValue:
        """Put a question in front of somebody, and park this attempt until it is answered.

        What a capability hook calls when a tool call wants approving. It
        returns the answer — but not on this attempt: the first time it is
        reached it raises :class:`InputRequired`, the attempt parks, and the
        answer schedules a new attempt of the same step. That attempt runs this
        task again from the beginning and this call returns.

        So the work before the question happens twice. That is the rule every
        retry already lives under and not a special case: a task that wants to
        keep what it did hands it back with ``write_artifact`` and reads it
        again.

        Answers are consumed in the order they were given, which is the order a
        replay asks for them — so a task that asks two questions gets them back
        in the same two places.
        """
        self.raise_if_cancelled()
        answered = next(self._answers, None)
        if answered is not None:
            return answered.response
        raise InputRequired(
            prompt,
            role=role,
            choices=choices,
            timeout_seconds=timeout_seconds,
            on_timeout=on_timeout,
        )

    def read_artifact(self, name: str) -> list[JsonObject]:
        self.raise_if_cancelled()
        if not any(ref["name"] == name for ref in self.assignment.inputs):
            raise ValueError(f"{name!r} is not an input of this attempt")
        return self._api.read_artifact(name)

    def write_artifact(self, name: str, rows: Sequence[JsonObject]) -> ArtifactRef:
        self.raise_if_cancelled()
        if not name:
            raise ValueError("an output needs a name")
        ref = self._api.write_artifact(name, rows)
        self._outputs[name] = ref
        return ref

    def record_evaluation(
        self,
        *,
        suite: str,
        dataset: str | None = None,
        variant: str | None = None,
        params: dict[str, Any] | None = None,
        metrics: dict[str, float] | None = None,
        report: dict[str, Any] | None = None,
        cases_total: int | None = None,
        cases_passed: int | None = None,
        duration_ms: float | None = None,
        evaluation_id: str | None = None,
    ) -> str:
        """Publish an evaluation this step measured, listed under this run and this step.

        ``record_evaluation`` on the client, with the run and the step filled
        in — which is why it is here rather than a context the client picks up
        on its own: the client is shared across threads, and an ambient run
        would be stamped on a report some unrelated code in this process wrote.

        The id is derived from ``step_key``, the suite and the variant unless
        one is given, so a retried attempt lands on the report its predecessor
        wrote rather than beside it.
        """
        return self.client.record_evaluation(
            suite=suite,
            evaluation_id=evaluation_id or evaluation_id_for(self.step_key, suite, variant),
            dataset=dataset,
            variant=variant,
            params=params,
            metrics=metrics,
            report=report,
            cases_total=cases_total,
            cases_passed=cases_passed,
            duration_ms=duration_ms,
            workflow_id=self.correlation.workflow_id,
            workflow_run_id=self.workflow_run_id,
            step_id=self.assignment.step_id,
        )

    def start(self) -> None:
        self._thread.start()

    def stop(self) -> None:
        self._stopped.set()
        if self._thread.ident is not None:
            self._thread.join()

    def _keep_alive(self) -> None:
        while not self._stopped.wait(
            min(self._interval, max(0.001, self._deadline - self._monotonic()))
        ):
            try:
                self.heartbeat()
            except (WorkerError, TaskError):
                return
