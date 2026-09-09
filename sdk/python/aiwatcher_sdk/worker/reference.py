"""A concrete attempt to execute in one process, with no queue fallback."""

from dataclasses import dataclass


@dataclass(frozen=True)
class AttemptRef:
    execution_id: str
    step_id: str
    attempt: int

    def __post_init__(self) -> None:
        if not self.execution_id or "/" in self.execution_id or not self.step_id:
            raise ValueError("an attempt needs an execution id and a step id")
        if type(self.attempt) is not int or not 1 <= self.attempt <= 2**32 - 1:
            raise ValueError("attempt must be a positive uint32")

    @classmethod
    def parse(cls, reference: str) -> "AttemptRef":
        execution, _, rest = reference.partition("/")
        step, separator, attempt = rest.rpartition("/")
        if not separator or not attempt.isascii() or not attempt.isdecimal():
            raise ValueError("attempt reference must be execution/step/positive-attempt")
        return cls(execution, step, int(attempt))
