"""Structured exception hierarchy and retry policy for the agentic framework.

Tools can raise ``ModelRetry`` to signal that the LLM should re-attempt its
response.  The framework automatically feeds the error message back to the
model as a tool-result error, governed by a ``RetryPolicy``.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Literal


class AgenticError(Exception):
    """Base class for all agentic framework exceptions."""


class ModelRetry(AgenticError):  # noqa: N818 - a public name before the move
    """Raise inside a tool to ask the model to try again.

    The *message* is forwarded to the LLM as a tool-error result so
    it can self-correct.

    Example::

        @tool
        def lookup_user(user_id: str) -> str:
            if not user_id.isdigit():
                raise ModelRetry("user_id must be numeric, got: " + user_id)
            ...
    """

    def __init__(self, message: str) -> None:
        super().__init__(message)
        self.message = message


class ToolValidationError(AgenticError):
    """Raised when tool arguments fail type validation.

    Automatically converted into a ``ModelRetry`` by the tool execution
    layer so the model receives the validation details.
    """

    def __init__(self, message: str) -> None:
        super().__init__(message)
        self.message = message


class ModelResponseError(AgenticError):
    """The endpoint answered, and the answer is not a completion.

    Separate from a transport failure on purpose: "no route to the model" is
    the application's problem and "the model stopped mid-JSON" is this one's,
    and a caller that maps both onto one message cannot tell a retry worth
    making from one that will fail the same way.
    """

    def __init__(self, message: str) -> None:
        super().__init__(message)
        self.message = message


class UsageLimitExceeded(AgenticError):  # noqa: N818 - likewise
    """Raised when a usage limit (tokens, requests, tool calls) is hit."""

    def __init__(self, message: str) -> None:
        super().__init__(message)
        self.message = message


type StopStrategy = Literal["after_attempt", "never"]
type WaitStrategy = Literal["none", "fixed", "exponential"]


@dataclass(frozen=True, slots=True)
class RetryPolicy:
    """Configurable retry policy inspired by tenacity.

    Parameters:
        max_retries: Maximum number of retry attempts. 0 means no retries.
        stop: When to stop retrying.
        wait: Wait strategy between retries (used by async callers).
        wait_seconds: Base wait time in seconds for fixed/exponential strategies.
        wait_max_seconds: Maximum wait time for exponential backoff.
        wait_multiplier: Multiplier for exponential backoff.
        retry_on: Exception types that trigger a retry. Defaults to
            (ModelRetry, ToolValidationError).
        keep_history: How the failed attempt is carried into the next one.
            ``False`` (the default) replaces the user message with one that
            quotes the failure — the model sees a single, self-contained turn.
            ``True`` keeps the conversation instead: the rejected answer stays
            in place as the assistant's turn and the correction follows it as
            the user's. A model repairing structured output needs the second
            form, because what it has to fix is its own previous answer.
    """

    max_retries: int = 3
    stop: StopStrategy = "after_attempt"
    wait: WaitStrategy = "none"
    wait_seconds: float = 0.0
    wait_max_seconds: float = 60.0
    wait_multiplier: float = 2.0
    retry_on: tuple[type[Exception], ...] = field(
        default=(ModelRetry, ToolValidationError),
    )
    keep_history: bool = False

    def should_retry(self, attempt: int, error: Exception) -> bool:
        """Return True if the error should be retried given the current attempt."""
        if self.stop == "never":
            return isinstance(error, self.retry_on)
        return attempt < self.max_retries and isinstance(error, self.retry_on)

    def wait_time(self, attempt: int) -> float:
        """Compute wait time in seconds for the given attempt number."""
        match self.wait:
            case "none":
                return 0.0
            case "fixed":
                return self.wait_seconds
            case "exponential":
                delay = self.wait_seconds * (self.wait_multiplier**attempt)
                return min(delay, self.wait_max_seconds)


NO_RETRY = RetryPolicy(max_retries=0)
DEFAULT_RETRY = RetryPolicy()
