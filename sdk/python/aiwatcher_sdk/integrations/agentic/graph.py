"""An `agentic_graph` composition, as a workflow aiwatcher can draw.

The graph a person composed is the shape; the turns that ran against it are the
execution. Without the shape the panel can only ever show the nodes that have
already run, and "which node has this not reached" is the question somebody
watching a fan-out is asking — which is exactly the question a serial fan-out
answers badly, and the reason drawing one is worth anything.

## Read structurally, as everything in this package is

An `AgentGraph` is a frozen dataclass with `nodes` and `connections`, and this
module never imports it: it reads `node_id`, `display_name` and `node_type` off
a node and `source_node_id`/`target_node_id` off a connection. The same rule
the tracer keeps, and for the same reason — aiwatcher does not depend on the
agent, and the agent does not depend on aiwatcher's types.

## The id a node is known by

`node_id`, never the alias. A rename is a display change, and a graph keyed by
its display names is one where renaming an agent silently starts a second
workflow with no history.

## A traversal, while it runs

`declare_graph` yields a `GraphTraversal`. Its nodes are steps and its hand-offs
are messages — the declared shape on one side, what was said on the other, and
the Workflows view keeps the two apart. A node also binds `current_attempt`, the
contextvar a worker's attempt publishes, so a tracer running the node's agent
opens no root of its own and nests what it opens under the node.
"""

from __future__ import annotations

import contextlib
import uuid
from collections.abc import Generator
from dataclasses import replace
from typing import TYPE_CHECKING, Any

from .tracer import current_attempt

if TYPE_CHECKING:
    from aiwatcher_sdk import AiwatcherClient, NodeContext, WorkflowContext

__all__ = ["GraphTraversal", "as_topology", "declare_graph"]


def as_topology(graph: Any) -> tuple[list[dict[str, Any]], list[tuple[str, str]]]:
    """One graph as the `nodes` and `edges` a workflow is declared with.

    The node carries its display name and its kind so the panel can label it
    without a second read; the edge carries nothing, because what an edge means
    here is only "may follow". What was actually *said* between two agents is an
    `agent.message`, published when it happens — a declared edge is what the
    composition promised, and merging the two would make sequence look like
    communication.
    """
    nodes = [
        {
            "id": node.node_id,
            "name": getattr(node, "display_name", "") or node.node_id,
            "kind": getattr(node, "node_type", "agent"),
        }
        for node in getattr(graph, "nodes", ())
    ]
    edges = [
        (connection.source_node_id, connection.target_node_id)
        for connection in getattr(graph, "connections", ())
    ]
    return nodes, edges


class GraphTraversal:
    """One turn of a declared graph, while it runs.

    Its nodes are steps, which the panel draws against the declared shape — a
    node nobody reached stays `Pending`. Its hand-offs are messages, which the
    panel draws between agents and never as edges of the shape: an edge is what
    the composition allowed, a message is what one agent actually handed
    another.
    """

    def __init__(self, client: AiwatcherClient, flow: WorkflowContext) -> None:
        self._client = client
        #: The traversal as the telemetry client knows it, for what this does
        #: not wrap — an artifact a node handed on.
        self.flow = flow

    @contextlib.contextmanager
    def node(
        self,
        node_id: str,
        *,
        agent_id: str | None = None,
        kind: str = "agent",
        **payload: Any,
    ) -> Generator[NodeContext, None, None]:
        """One node running, and every span a tracer opens in it nested under it.

        The tracer is bound through `current_attempt`, the contextvar a worker's
        attempt publishes: its own `workflow` scope then opens no second root,
        and what it opens is attributed to this traversal. The node's span id is
        minted here rather than derived by the server, because that tracer
        publishes through a client of its own — two clients are two queues, and
        a parent inferred from "what is still open" would depend on which of
        them drained first. Minted once and carried by every event of the node,
        so a redelivered envelope still lands on the span it opened.

        `payload` rides along on the node's events — what this stage was given
        to work on, counted rather than quoted. Metadata, never content: a
        query a stage sent belongs in what the stage saved, not in the log of
        the fact that it ran.
        """
        span_id = uuid.uuid4().hex[:16]
        with self.flow.node(
            node_id, agent_id=agent_id, kind=kind, span_id=span_id, **payload
        ) as node:
            bound = current_attempt.set(node.correlation)
            try:
                yield node
            finally:
                current_attempt.reset(bound)

    def message(self, source: str, target: str, *, kind: str = "dispatch", **extra: Any) -> None:
        """`source` handing work to `target`, both named as the agents that run them.

        A point rather than a scope — a hand-off has a moment, not a duration —
        and published from the traversal rather than from an agent scope,
        because what hands off here is the graph, between two turns, when no
        agent is open.
        """
        self._client.emit(
            "agent.message",
            replace(self.flow.correlation, agent_id=source),
            {"from": source, "to": target, "kind": kind, **extra},
        )


@contextlib.contextmanager
def declare_graph(
    client: AiwatcherClient, graph: Any, *, turn_id: str | None = None
) -> Generator[GraphTraversal, None, None]:
    """Declare the graph, and open the traversal that runs against it.

    `turn_id` is the execution: one turn of a graph is one traversal, and
    several agents running in one turn are stages of it rather than separate
    runs. Omit it and the run *is* the execution, which is right for a preview
    that runs once and is thrown away.

    Yields a `GraphTraversal`: a caller opens its nodes with
    `traversal.node(node_id)` and records a hand-off with
    `traversal.message(source, target)`.

    Declaring is idempotent — the version is a hash of the topology — so this is
    called unconditionally rather than once, which is what keeps the catalog
    alive across retention eviction.
    """
    nodes, edges = as_topology(graph)
    with client.workflow(
        # The graph's own id, not its name: a name is what somebody typed and
        # may be typed again differently, and a workflow that changed id on a
        # rename would lose every earlier execution.
        getattr(graph, "graph_id", "") or getattr(graph, "name", "graph"),
        nodes=nodes,
        edges=edges,
        name=getattr(graph, "name", None),
        execution_id=turn_id,
    ) as flow:
        yield GraphTraversal(client, flow)
