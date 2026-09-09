"""Task declarations preserve Python signatures; workers explicitly select the code they host."""

from __future__ import annotations

import inspect
from collections.abc import Callable, Mapping
from functools import update_wrapper
from typing import Generic, ParamSpec, TypeVar, cast

from aiwatcher_sdk.task_errors import TaskError

P = ParamSpec("P")
R = TypeVar("R")


class Task(Generic[P, R]):
    """A named, versioned function. Calling it directly is an ordinary local call.

    Only a worker invocation adds execution context, persistence and heartbeats.
    Local calls never pretend to provide durable execution.
    """

    def __init__(self, function: Callable[P, R], name: str, version: str) -> None:
        if any(not part.strip() or part != part.strip() or "@" in part for part in (name, version)):
            raise ValueError("a task needs a nonempty name and version, neither containing @")
        if (
            inspect.iscoroutinefunction(function)
            or inspect.isgeneratorfunction(function)
            or inspect.isasyncgenfunction(function)
        ):
            raise TypeError(
                "this worker supports synchronous functions with a completed return value"
            )
        self.fn = function
        self.name = name
        self.version = version
        self.signature = inspect.signature(function)
        update_wrapper(self, function, updated=())

    @property
    def ref(self) -> str:
        return f"{self.name}@{self.version}"

    def __call__(self, *args: P.args, **kwargs: P.kwargs) -> R:
        return self.fn(*args, **kwargs)

    def invoke(self, parameters: Mapping[str, object]) -> R:
        """Bind the wire parameters before entering user code; do not silently drop keys."""
        try:
            self.signature.bind(**parameters)
        except TypeError as error:
            raise TaskError(str(error), classification="validation") from error
        return cast(Callable[..., R], self.fn)(**parameters)


def task(name: str | None = None, *, version: str) -> Callable[[Callable[P, R]], Task[P, R]]:
    """Declare a task without adding it to any process-global registry."""

    def declare(function: Callable[P, R]) -> Task[P, R]:
        return Task(function, name or f"{function.__module__}.{function.__qualname__}", version)

    return declare
