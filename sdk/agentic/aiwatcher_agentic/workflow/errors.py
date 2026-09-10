from __future__ import annotations


class AgenticError(RuntimeError):
    """Base error for all agentic workflow errors."""

    error_code: int = 500

    def __init__(self, message: str = "") -> None:
        super().__init__(message)


class ValidationError(AgenticError):
    """Invalid input or parameters (400)."""

    error_code: int = 400


class IllegalStateError(AgenticError):
    """Command rejected because aggregate is in wrong state (403)."""

    error_code: int = 403


class NotFoundError(AgenticError):
    """Requested resource does not exist (404)."""

    error_code: int = 404


class ConcurrencyConflictError(AgenticError):
    """Optimistic concurrency check failed (412)."""

    error_code: int = 412

    def __init__(
        self,
        stream_name: str,
        expected: object,
        current: object,
    ) -> None:
        self.stream_name = stream_name
        self.expected = expected
        self.current = current
        super().__init__(
            f"Concurrency conflict on stream '{stream_name}': "
            f"expected version {expected}, but current is {current}"
        )


class DuplicateMessageError(AgenticError):
    """A message or event identity was already recorded (409)."""

    error_code: int = 409

    def __init__(self, stream_name: str) -> None:
        self.stream_name = stream_name
        super().__init__(f"Duplicate message identity on stream '{stream_name}'")


class StepLimitExceeded(AgenticError):  # noqa: N818 - a public name callers already catch
    """Consumer reached maximum processing steps (429)."""

    error_code: int = 429

    def __init__(self, max_steps: int) -> None:
        self.max_steps = max_steps
        super().__init__(f"Consumer reached max_steps={max_steps}.")
