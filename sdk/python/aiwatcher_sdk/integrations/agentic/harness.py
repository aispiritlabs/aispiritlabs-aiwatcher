"""A harness's steps: the shape it has, and one execution of it.

A harness is the other way an application runs a sequence of agents. A graph
composed in `agentic_graph` has a topology object to declare — that is what
`declare_graph` reads — while a harness is a function that calls five stages in
order, and its shape exists only as two constants beside the code.

What both need from aiwatcher is the same three levels, and the reason there
are three is that they are three different facts:

``flow.node``
    the stage of a declared graph. **Only this** sets a node's status in the
    Workflows view; a `step.*` from the tracer carries `name` and not `node`,
    so it draws a waterfall and nothing else.
``node.agent``
    who ran the stage. Little on its own — the agent already rides in the
    step's correlation — but it is the parent the SDK needs in order to hang a
    tool call under the stage.
``agent.tool``
    the one call that leaves the process. This is why the first two are not
    enough: a stage measures the call to the provider together with the sifting
    of what came back, and those are two numbers whose sum answers neither.

Writing those three by hand is ninety lines, and an application with two
harnesses had them twice, differing only in which constants they named. Hence
this: the constants are a :class:`Harness`, the three levels are here.

**The declaration is unconditional and idempotent** — its version is a hash of
the topology — so the graph stays in the catalog even after retention has eaten
every execution of it. Five nodes nobody started is a sentence about a run that
died on its first step; a missing graph is not a sentence at all.

What this does not decide is **who publishes**. A harness whose steps an engine
executes already has a publisher, and a second one on the same step is two
accounts of one fact — so an application that hands its work to an engine hands
its harness a silent tracer instead of this one, and that choice stays with the
application, which is the only side that knows which path it is on.
"""

from __future__ import annotations

import contextlib
from collections.abc import Callable, Generator, Iterable
from contextlib import AbstractContextManager
from dataclasses import dataclass
from typing import TYPE_CHECKING, Any

from .graph import GraphTraversal

if TYPE_CHECKING:
    from aiwatcher_sdk import AiwatcherClient

__all__ = ["Harness", "HarnessStep", "HarnessTool", "harness_steps"]

#: One call out of the process, inside a step.
type HarnessTool = Callable[..., AbstractContextManager[None]]

#: One step of the harness, yielding the scope of its outward call.
type HarnessStep = Callable[..., AbstractContextManager[HarnessTool]]


@dataclass(frozen=True, slots=True)
class Harness:
    """The steps a harness runs, and which step may follow which.

    The shape, with no execution in it: the same value declares every run.
    """

    workflow: str
    steps: tuple[str, ...]
    edges: tuple[tuple[str, str], ...]

    @classmethod
    def of(
        cls,
        workflow: object,
        steps: Iterable[object],
        edges: Iterable[tuple[object, object]],
    ) -> Harness:
        """Build one from whatever an application names its stages with.

        Usually a `StrEnum`, because a stage id is a closed set and a harness
        that can name a stage it does not have is a harness whose graph the
        panel cannot match. Stringified here so the caller keeps its enum and
        this module keeps knowing nothing about it.
        """
        return cls(
            workflow=str(workflow),
            steps=tuple(str(step) for step in steps),
            edges=tuple((str(head), str(tail)) for head, tail in edges),
        )


@contextlib.contextmanager
def harness_steps(
    client: AiwatcherClient, harness: Harness, *, run_id: str
) -> Generator[HarnessStep, None, None]:
    """Declare the harness and open one execution, yielding its step scope.

    `run_id` is the execution as the application knows it — the id something is
    read back by — because a trace under one id and a saved run under another
    are two identifiers for one piece of work.

        with harness_steps(client, OFFER_SCOUT, run_id=str(cycle_id)) as step:
            with step(ScoutAgentId.CATALOG_SCOUT, tool="search", input_count=0) as call:
                with call("search"):
                    ...
    """
    with client.workflow(
        harness.workflow,
        nodes=list(harness.steps),
        edges=[(head, tail) for head, tail in harness.edges],
        run_id=run_id,
    ) as flow:
        traversal = GraphTraversal(client, flow)

        @contextlib.contextmanager
        def step(step_id: object, **payload: Any) -> Generator[HarnessTool, None, None]:
            name = str(step_id)
            with (
                traversal.node(name, agent_id=name, **payload) as node,
                node.agent(name) as agent,
            ):
                # `AgentContext.tool` already has the port's shape, so the bound
                # method travels rather than a copy of its signature.
                yield agent.tool

        yield step
