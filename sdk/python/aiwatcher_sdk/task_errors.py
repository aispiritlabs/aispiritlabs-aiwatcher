"""Task failures independent of any execution transport."""

from typing import Literal

FailureClass = Literal[
    "validation", "user_code", "transient", "timeout", "infrastructure", "policy"
]


class TaskError(Exception):
    """A task classifies a failure; the server owns the retry decision."""

    def __init__(self, message: str, *, classification: FailureClass = "user_code") -> None:
        super().__init__(message)
        self.classification = classification
