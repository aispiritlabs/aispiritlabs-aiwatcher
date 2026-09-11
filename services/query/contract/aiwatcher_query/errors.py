"""The three ways a query does not answer, and the status each one is.

The split is the one a managed step's retry budget depends on, and it is Flow's:

    QueryRefusedError         422  the text was read and refused, or failed as written.
                              The same text fails the same way on every retry, so
                              the step reads it as the pipeline's fault.
    UpstreamFailedError       422  aiwatcher answered, and its answer is permanent — a
                         502  4xx or a 501 naming an unset variable — or may not be.
    AiwatcherUnreachableError 502  nothing answered, or what answered was not aiwatcher.

A ceiling the query reached is a `QueryRefusedError`: the same query reaches the same
ceiling on every retry, and a 5xx would spend ten attempts finding that out.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any


class QueryRefusedError(Exception):
    """The query was read and refused, with where — Python's own line and column, from 1."""

    def __init__(self, message: str, *, line: int = 0, column: int = 0) -> None:
        super().__init__(message)
        self.message = message
        self.line = line
        self.column = column

    def at(self, line: int, column: int) -> QueryRefusedError:
        """This refusal located, unless it already was."""
        if self.line:
            return self
        return QueryRefusedError(self.message, line=line, column=column)


class UpstreamFailedError(Exception):
    """aiwatcher answered, and the answer was not rows.

    Carries aiwatcher's own words and status: without them the failure would arrive as
    a missing key in an error body, and the status is what decides whether asking again
    could ever answer differently.
    """

    def __init__(self, status: int, detail: str, uri: str) -> None:
        super().__init__(f"aiwatcher answered {status} for {uri}: {detail}")
        self.status = status
        self.detail = detail
        self.uri = uri

    @property
    def permanent(self) -> bool:
        """A 4xx is this service asking wrongly and a 501 is the instance not doing that
        at all; neither changes because a reactor waited thirty seconds."""
        return 400 <= self.status < 500 or self.status == 501


class AiwatcherUnreachableError(Exception):
    """Nothing answered at aiwatcher's address, or what did was not aiwatcher's JSON."""


@dataclass(frozen=True)
class Failure:
    """A query that did not answer, as it crosses back from the child and onto the wire.

    A plain record rather than the exception that caused it, because an exception with
    extra constructor arguments does not survive pickling across a process boundary.
    """

    status: int
    message: str
    line: int = 0
    column: int = 0

    def body(self) -> dict[str, Any]:
        return {"error": {"message": self.message, "line": self.line, "column": self.column}}

    @classmethod
    def of(cls, error: Exception) -> Failure:
        match error:
            case QueryRefusedError():
                return cls(422, error.message, error.line, error.column)
            case UpstreamFailedError():
                return cls(422 if error.permanent else 502, f"The query could not be run: {error}")
            case _:
                return cls(502, f"The query could not be run: {error}")
