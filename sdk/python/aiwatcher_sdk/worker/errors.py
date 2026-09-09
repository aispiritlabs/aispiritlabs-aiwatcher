"""A task's failure is distinct from losing contact with its owner."""

from aiwatcher_sdk.api import ApiError
from aiwatcher_sdk.task_errors import FailureClass, TaskError

__all__ = ["FailureClass", "LeaseLostError", "TaskError", "WorkerError"]


class WorkerError(ApiError):
    """The worker API refused or could not be reached."""


class LeaseLostError(WorkerError):
    """Stop cooperating with this attempt; its result can no longer be trusted."""
