"""A task's failure, losing contact with its owner, and stopping to ask.

Three outcomes, and only the first is an error. The third is here because it
travels the same way — raised out of the task rather than returned — and for the
same reason: something a task can carry on past is something it will.
"""

from __future__ import annotations

from collections.abc import Sequence

from aiwatcher_sdk.api import ApiError
from aiwatcher_sdk.task_errors import FailureClass, TaskError
from aiwatcher_sdk.worker.contract import OnTimeout, Parked

__all__ = [
    "FailureClass",
    "InputRequired",
    "LeaseLostError",
    "TaskError",
    "WorkerError",
]


class WorkerError(ApiError):
    """The worker API refused or could not be reached."""


class LeaseLostError(WorkerError):
    """Stop cooperating with this attempt; its result can no longer be trusted."""


class InputRequired(Exception):  # noqa: N818 — this is not an error; the attempt parks
    """The task needs somebody to answer something before it can go on.

    Raised by ``ctx.ask`` rather than returned, which is the telemetry client's
    rule the other way round: a worker that cannot ask must not silently carry
    on, so this unwinds the task instead of being a value somebody can ignore.

    The worker turns it into a ``Parked`` report. The attempt keeps its row and
    loses its lease, and the answer schedules a new attempt of the same step —
    so the task runs again from the beginning and ``ctx.ask`` returns the answer
    at the point it raised.
    """

    def __init__(
        self,
        prompt: str,
        *,
        role: str = "editor",
        choices: Sequence[str] = (),
        timeout_seconds: int | None = None,
        on_timeout: OnTimeout | None = None,
    ) -> None:
        super().__init__(prompt)
        self.prompt = prompt
        self.role = role
        self.choices = tuple(choices)
        self.timeout_seconds = timeout_seconds
        self.on_timeout = on_timeout

    def as_report(self) -> Parked:
        return Parked(
            prompt=self.prompt,
            role=self.role,
            choices=self.choices,
            timeout_seconds=self.timeout_seconds,
            on_timeout=self.on_timeout,
        )
