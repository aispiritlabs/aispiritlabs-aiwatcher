"""What a framework has to be to stand behind the contract.

An adapter is four members: its name, the release installed, the model its
graded metrics ask, and a table of metrics each of which scores one case. The
table is the adapter — a metric reaches a function through a dictionary and
never a callable named by a request — and the service does everything else:
the catalog, holding a request to what the card pinned, the parameters, the
order of the replies.

**A failure is the adapter's own sentence.** An exception a framework raises
can carry the text it was scoring, so a case that fails is reported by the
exception's class and a fixed phrase — never by its message.
"""

from __future__ import annotations

from collections.abc import Callable, Mapping
from dataclasses import dataclass
from typing import Protocol

from aiwatcher_scorers.contract import (
    Case,
    Direction,
    JsonValue,
    Metric,
    ModelReference,
    Parameter,
    Scored,
    Side,
)

type ScoreCase = Callable[[Mapping[str, JsonValue], Case], float]


@dataclass(frozen=True)
class Implemented:
    """One metric: how it is described, and how it scores a case."""

    metric: Metric
    score: ScoreCase


class Adapter(Protocol):
    @property
    def name(self) -> str: ...

    @property
    def version(self) -> str: ...

    @property
    def model(self) -> ModelReference | None: ...

    def metrics(self) -> Mapping[str, Implemented]: ...


def graded(
    name: str,
    *,
    direction: Direction,
    reads: tuple[Side, ...],
    description: str,
    score: ScoreCase,
    parameters: Mapping[str, Parameter] | None = None,
) -> Implemented:
    """A metric a model grades on nought to one: the shape both frameworks share."""
    return Implemented(
        Metric(
            name,
            unit="score",
            direction=direction,
            aggregation="mean",
            reads=reads,
            description=description,
            model_graded=True,
            range=(0.0, 1.0),
            parameters=parameters or {},
        ),
        score,
    )


def scored(implemented: Implemented, parameters: Mapping[str, JsonValue], case: Case) -> Scored:
    """One case through one metric, as a number or the reason there is none."""
    try:
        value = implemented.score(parameters, case)
    except Exception as error:  # noqa: BLE001 — a framework raises anything, and one case must not end a run
        return Scored(failed=f"{type(error).__name__}: the metric raised while scoring this case")
    return Scored(value=value)
