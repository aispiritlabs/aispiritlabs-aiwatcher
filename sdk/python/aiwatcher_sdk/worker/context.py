"""The lease and artifact boundary a running task cooperates with."""

from __future__ import annotations

import threading
import time
from collections.abc import Callable, Generator, Sequence
from contextlib import contextmanager
from contextvars import ContextVar
from datetime import UTC, datetime

from aiwatcher_sdk import AiwatcherClient, Correlation, RunContext
from aiwatcher_sdk.integrations.agentic import AiwatcherTracer
from aiwatcher_sdk.worker.assignment import Assignment
from aiwatcher_sdk.worker.attempt import AttemptAPI
from aiwatcher_sdk.worker.contract import ArtifactRef, JsonObject
from aiwatcher_sdk.worker.errors import LeaseLostError, TaskError, WorkerError

current_context: ContextVar[TaskContext | None] = ContextVar("aiwatcher_task_context", default=None)


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
        self.run = RunContext(client, correlation)
        self.tracer = AiwatcherTracer(client=client, context=correlation)
        self._monotonic = monotonic
        self._api = api
        self._outputs: dict[str, ArtifactRef] = {}
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
    def outputs(self) -> list[ArtifactRef]:
        return list(self._outputs.values())

    @contextmanager
    def activate(self) -> Generator[TaskContext, None, None]:
        token = current_context.set(self)
        try:
            yield self
        finally:
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
