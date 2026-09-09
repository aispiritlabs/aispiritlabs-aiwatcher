"""Pull one pinned callable at a time; leave every scheduling decision to Rust."""

from __future__ import annotations

import json
import logging
import os
import socket
import threading
import uuid
from collections.abc import Sequence
from types import TracebackType
from typing import Any, Self

import httpx

from aiwatcher_sdk import AiwatcherClient
from aiwatcher_sdk.api import Transport
from aiwatcher_sdk.worker.assignment import Assignment
from aiwatcher_sdk.worker.context import TaskContext
from aiwatcher_sdk.worker.contract import Completed, Failed, Report, json_value
from aiwatcher_sdk.worker.errors import InputRequired, LeaseLostError, TaskError, WorkerError
from aiwatcher_sdk.worker.http import HttpAttemptAPI, claim
from aiwatcher_sdk.worker.reference import AttemptRef
from aiwatcher_sdk.worker.task import Task

logger = logging.getLogger(__name__)


class Worker:
    """A synchronous worker. ``stop`` requests draining after the current task.

    Task inputs are run parameters overlaid by step parameters. Parent data is
    read explicitly through ``ctx.read_artifact(name)``; return values are bounded
    control metadata, not the data passed to the next step.
    """

    def __init__(
        self,
        url: str,
        token: str | None = None,
        *,
        queues: list[str],
        tasks: Sequence[Task[Any, Any]],
        name: str | None = None,
        poll_interval: float = 1.0,
        timeout: float = 10.0,
        client: httpx.Client | None = None,
        telemetry: AiwatcherClient | None = None,
    ) -> None:
        if not queues or any(not queue.strip() for queue in queues):
            raise ValueError("a worker needs at least one nonempty queue")
        if poll_interval <= 0 or timeout <= 0:
            raise ValueError("poll_interval and timeout must be positive")
        self._tasks = {entry.ref: entry for entry in tasks}
        if len(self._tasks) != len(tasks):
            raise ValueError("a worker cannot host duplicate task references")
        if not self._tasks:
            raise ValueError("register at least one pinned task before starting a worker")
        self.name = name or f"{socket.gethostname()}-{os.getpid()}-{uuid.uuid4().hex}"
        self._queues = list(queues)
        self._poll_interval = poll_interval
        self._stop = threading.Event()
        # Claims are not replayed. A report opts into retry only when the
        # assignment advertises acknowledgement from durable history.
        self._api = Transport(
            url,
            token=token,
            timeout=timeout,
            attempts=1,
            client=client,
            error=WorkerError,
            subject="the worker API",
        )
        self._owns_telemetry = telemetry is None
        self._telemetry = telemetry or AiwatcherClient(
            service="aiwatcher-worker", base_url=url, token=token, instance=self.name
        )

    def run_once(self) -> bool:
        """Perform at most one assignment; False means nothing was claimable."""
        assignment = claim(self._api, self.name, self._queues, sorted(self._tasks))
        if assignment is None:
            return False
        self._validate(assignment)
        try:
            self._perform(assignment)
        except WorkerError as error:
            if not isinstance(error, LeaseLostError) and not (
                error.status == 409 and error.code == "lease_lost"
            ):
                raise
            logger.warning(
                "lease lost for %s; discarding this attempt's result", assignment.context_id
            )
        return True

    def run_attempt(self, reference: AttemptRef | str) -> bool:
        """Execute exactly the referenced attempt; return whether its task succeeded.

        Unavailable work or a lost lease raises instead of claiming a different
        attempt. A failed task is reported durably and returns False.
        """
        target = AttemptRef.parse(reference) if isinstance(reference, str) else reference
        assignment = claim(self._api, self.name, self._queues, sorted(self._tasks), attempt=target)
        if assignment is None:
            raise WorkerError("the requested attempt is not claimable", code="attempt_unavailable")
        if (assignment.execution_id, assignment.step_id, assignment.attempt) != (
            target.execution_id,
            target.step_id,
            target.attempt,
        ):
            raise WorkerError("the server assigned a different attempt than requested")
        self._validate(assignment)
        return self._perform(assignment)

    def _validate(self, assignment: Assignment) -> None:
        if assignment.task_ref not in self._tasks or assignment.queue not in self._queues:
            raise WorkerError(
                "the server assigned work outside this worker's advertised capabilities"
            )

    def _perform(self, assignment: Assignment) -> bool:
        api = HttpAttemptAPI(self._api, assignment, self.name)
        ctx = TaskContext(assignment, api, self._telemetry)
        try:
            ctx.start()
            report = self._invoke(assignment, ctx)
            # Still beating while uploading/reporting. A loss already observed by
            # the heartbeat must not be followed by a second, contradictory report.
            ctx.check_lease()
            api.report(report)
            return isinstance(report, Completed)
        finally:
            ctx.stop()

    def _invoke(self, assignment: Assignment, ctx: TaskContext) -> Report:
        try:
            ctx.raise_if_cancelled()
            with ctx.activate():
                result = self._tasks[assignment.task_ref].invoke(
                    assignment.parameters | assignment.params
                )
            ctx.raise_if_cancelled()
            missing = set(assignment.outputs) - {output["name"] for output in ctx.outputs}
            if missing:
                raise TaskError(
                    f"missing declared outputs: {', '.join(sorted(missing))}",
                    classification="validation",
                )
            encoded = json.dumps(result, allow_nan=False, ensure_ascii=False).encode()
            if len(encoded) > 64 * 1024:
                raise TaskError(
                    "result exceeds 64 KiB; write an artifact instead", classification="validation"
                )
            return Completed(json_value(json.loads(encoded)), ctx.outputs)
        except InputRequired as asked:
            # Not a failure and not a result. The attempt parks: this worker
            # stops holding it, and the answer schedules a new attempt that runs
            # this task again with the answer in front of it.
            return asked.as_report()
        except WorkerError:
            # A lost response is not evidence the task failed. Let the caller see
            # the transport failure and let the server recover the expired lease.
            raise
        except TaskError as error:
            return Failed(error.classification, str(error))
        except Exception as error:  # noqa: BLE001 — the task is the user-code boundary
            return Failed("user_code", str(error))

    def run(self) -> None:
        while not self._stop.is_set():
            if not self.run_once():
                self._stop.wait(self._poll_interval)

    def stop(self) -> None:
        self._stop.set()

    def close(self) -> None:
        self.stop()
        errors: list[BaseException] = []
        try:
            self._api.close()
        except BaseException as error:  # noqa: BLE001 — aggregate after resource cleanup
            errors.append(error)
        if self._owns_telemetry:
            try:
                self._telemetry.close()
            except BaseException as error:  # noqa: BLE001 — aggregate after resource cleanup
                errors.append(error)
        if errors:
            raise BaseExceptionGroup("worker cleanup failed", errors)

    def __enter__(self) -> Self:
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        self.close()
