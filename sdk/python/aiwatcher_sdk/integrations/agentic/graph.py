"""An `agentic_graph` composition, as a workflow aiwatcher can draw.

The graph a person composed is the shape; the turns that ran against it are the
execution. Without the shape the panel can only ever show the nodes that have
already run, and "which node has this not reached" is the question somebody
watching a fan-out is asking — which is exactly the question a serial fan-out
answers badly, and the reason drawing one is worth anything.

## Read structurally, as everything in this package is

An `AgentGraph` is a frozen dataclass with `nodes` and `connections`, and this
module never imports it: it reads `node_id`, `display_name`, `agent_name` and
`node_type` off a node and `source_node_id`/`target_node_id` off a connection.
The same rule the tracer keeps, and for the same reason — aiwatcher does not
depend on the agent, and the agent does not depend on aiwatcher's types.

## The id a node is known by

`node_id`, never the alias. A rename is a display change, and a graph keyed by
its display names is one where renaming an agent silently starts a second
workflow with no history.
"""

from __future__ import annotations

import contextlib
from collections.abc import Generator
from typing import Any

__all__ = ["as_topology", "declare_graph"]


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


@contextlib.contextmanager
def declare_graph(
    client: Any, graph: Any, *, turn_id: str | None = None
) -> Generator[Any, None, None]:
    """Declare the graph, and open the execution that runs against it.

    `turn_id` is the execution: one turn of a graph is one traversal, and
    several agents running in one turn are stages of it rather than separate
    runs. Omit it and the run *is* the execution, which is right for a preview
    that runs once and is thrown away.

    Yields whatever `client.workflow` yields, so a caller opens its nodes with
    `flow.node(node_id)` exactly as any other producer does.

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
        yield flow
