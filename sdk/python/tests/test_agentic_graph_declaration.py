"""An `agentic_graph` composition, declared as a workflow.

Against stand-ins for `AgentGraph`, `AgentNode` and `Connection` rather than the
real ones, which is the point: this SDK does not depend on the agent's packages,
and a test that imported them would quietly make it do so.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from aiwatcher_sdk import AiwatcherClient, NullTransport
from aiwatcher_sdk.integrations.agentic import as_topology, declare_graph


@dataclass(frozen=True, slots=True)
class Node:
    node_id: str
    display_name: str = ""
    node_type: str = "agent"


@dataclass(frozen=True, slots=True)
class Edge:
    source_node_id: str
    target_node_id: str


@dataclass(frozen=True, slots=True)
class Graph:
    graph_id: str = "g-1"
    name: str = "searcher and summarizer"
    nodes: tuple[Node, ...] = ()
    connections: tuple[Edge, ...] = ()


def fan_out() -> Graph:
    return Graph(
        nodes=(
            Node("planner", "Planner", "planner"),
            Node("search-a", "Search A"),
            Node("search-b", "Search B"),
            Node("summarizer", "Summarizer"),
        ),
        connections=(
            Edge("planner", "search-a"),
            Edge("planner", "search-b"),
            Edge("search-a", "summarizer"),
            Edge("search-b", "summarizer"),
        ),
    )


class Recorder:
    """A transport that keeps what was emitted rather than sending it.

    `send` takes a *batch*, as the protocol does — the client buffers, and a
    double that took one envelope would pass while the real transport never
    saw a second one.
    """

    def __init__(self) -> None:
        self.sent: list[dict[str, Any]] = []

    def send(self, batch: list[dict[str, Any]]) -> None:
        self.sent.extend(batch)

    def flush(self) -> None: ...

    def close(self) -> None: ...


def test_a_graph_is_declared_by_node_id_and_never_by_display_name() -> None:
    # A rename is a display change. A graph keyed by what somebody typed is one
    # where renaming an agent silently starts a second workflow with no history.
    nodes, edges = as_topology(fan_out())
    assert [node["id"] for node in nodes] == [
        "planner",
        "search-a",
        "search-b",
        "summarizer",
    ]
    assert nodes[0]["name"] == "Planner"
    assert nodes[0]["kind"] == "planner"
    assert edges == [
        ("planner", "search-a"),
        ("planner", "search-b"),
        ("search-a", "summarizer"),
        ("search-b", "summarizer"),
    ]


def test_a_node_with_no_display_name_is_known_by_its_id_rather_than_by_nothing() -> None:
    nodes, _ = as_topology(Graph(nodes=(Node("lonely"),)))
    assert nodes[0]["name"] == "lonely"


def test_a_graph_this_module_has_never_imported_is_still_readable() -> None:
    # The whole rule: aiwatcher does not depend on the agent's packages. Any
    # object with these attribute names declares.
    class Bare:
        nodes = (Node("only"),)
        connections = ()

    nodes, edges = as_topology(Bare())
    assert nodes == [{"id": "only", "name": "only", "kind": "agent"}]
    assert edges == []


def test_declaring_opens_one_execution_and_publishes_the_shape_before_any_node() -> None:
    # Declaring the shape is what lets the panel draw a node that has not been
    # reached. Published before anything runs, because after the fact it would
    # only ever describe what already happened.
    recorder = Recorder()
    client = AiwatcherClient(service="test", transport=recorder)
    with declare_graph(client, fan_out(), turn_id="turn-7") as flow, flow.node("planner"):
        pass

    kinds = [envelope["event_type"] for envelope in recorder.sent]
    assert kinds[0] == "run.started"
    assert kinds[1] == "workflow.declared"
    assert "step.started" in kinds

    declared = recorder.sent[1]
    assert declared["data"]["name"] == "searcher and summarizer"
    assert len(declared["data"]["nodes"]) == 4
    assert len(declared["data"]["edges"]) == 4
    # One turn is one traversal: several agents running in it are stages of one
    # execution rather than separate runs.
    assert declared["workflow_run_id"] == "turn-7"
    assert declared["workflow_id"] == "g-1"


def test_a_preview_that_runs_once_needs_no_execution_id() -> None:
    # Omitted, the run *is* the execution — which is right for a preview that
    # runs once and is thrown away.
    recorder = Recorder()
    client = AiwatcherClient(service="test", transport=recorder)
    with declare_graph(client, fan_out()):
        pass
    # Absent rather than null: an envelope that carried an empty execution id
    # would join every other run that also carried one.
    assert "workflow_run_id" not in recorder.sent[1]


def test_declaring_the_same_topology_twice_reaches_the_same_version() -> None:
    # Idempotent, so it is published unconditionally rather than once — which is
    # what keeps the catalog alive across retention eviction.
    def version_of(graph: Graph) -> str:
        recorder = Recorder()
        client = AiwatcherClient(service="test", transport=recorder)
        with declare_graph(client, graph):
            pass
        return str(recorder.sent[1]["data"]["version"])

    assert version_of(fan_out()) == version_of(fan_out())
    moved = Graph(nodes=fan_out().nodes, connections=fan_out().connections[:3])
    assert version_of(fan_out()) != version_of(moved)


def test_a_client_with_no_transport_declares_nothing_and_raises_nothing() -> None:
    # Telemetry must never be the reason a graph will not run.
    client = AiwatcherClient(service="test", transport=NullTransport())
    with declare_graph(client, fan_out(), turn_id="turn-1") as flow, flow.node("planner"):
        pass
