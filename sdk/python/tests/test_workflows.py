"""What the workflow API puts on the wire.

A recording transport rather than a mock: what is worth asserting is the
envelope the Rust side folds into a graph, and a mock that counted `emit` calls
would pass while sending a shape nothing can draw.
"""

from __future__ import annotations

from typing import Any

import pytest

from aiwatcher_sdk import AiwatcherClient


class RecordingTransport:
    def __init__(self) -> None:
        self.events: list[dict[str, Any]] = []

    def send(self, batch: list[dict[str, Any]]) -> None:
        self.events.extend(batch)

    def close(self) -> None:
        return None

    def of_type(self, event_type: str) -> list[dict[str, Any]]:
        return [event for event in self.events if event["event_type"] == event_type]


@pytest.fixture
def transport() -> RecordingTransport:
    return RecordingTransport()


@pytest.fixture
def client(transport: RecordingTransport) -> AiwatcherClient:
    return AiwatcherClient(service="planner-import-service", transport=transport)


NODES = ["acquire", "normalize", "analyze", "persist"]
EDGES = [("acquire", "normalize"), ("normalize", "analyze"), ("analyze", "persist")]


def test_a_declaration_accepts_bare_names_and_pairs(
    client: AiwatcherClient, transport: RecordingTransport
) -> None:
    # The shortest thing somebody writes first. Requiring the object form would
    # make the simplest declaration the one that silently does nothing.
    with client.workflow("house-import", nodes=NODES, edges=EDGES):
        pass

    declared = transport.of_type("workflow.declared")
    assert len(declared) == 1
    data = declared[0]["data"]
    assert [node["id"] for node in data["nodes"]] == NODES
    assert data["nodes"][0]["name"] == "acquire"
    assert data["edges"][0] == {"from": "acquire", "to": "normalize"}
    assert declared[0]["workflow_id"] == "house-import"


def test_the_version_is_a_hash_of_the_shape_not_of_the_call(
    client: AiwatcherClient, transport: RecordingTransport
) -> None:
    # Declaring on every execution has to be free, or producers will do it
    # conditionally and the catalog will go stale.
    with client.workflow("house-import", nodes=NODES, edges=EDGES):
        pass
    with client.workflow("house-import", nodes=list(NODES), edges=list(EDGES), name="Other"):
        pass

    versions = {event["data"]["version"] for event in transport.of_type("workflow.declared")}
    assert len(versions) == 1, "the same shape declared twice is one version"

    with client.workflow("house-import", nodes=[*NODES, "thumbnail"], edges=EDGES):
        pass
    assert len({event["data"]["version"] for event in transport.of_type("workflow.declared")}) == 2


def test_a_workflow_in_one_process_sends_no_execution_id(
    client: AiwatcherClient, transport: RecordingTransport
) -> None:
    # The backend defaults it to `run_id`. Sending it anyway would put a second
    # opaque id on every envelope for no gain.
    with client.workflow("house-import", nodes=NODES):
        pass

    assert all("workflow_run_id" not in event for event in transport.events)


def test_stages_in_several_processes_carry_the_same_execution_id(
    transport: RecordingTransport,
) -> None:
    # One pod per stage: each has its own run, and the execution id is the only
    # thing joining them back into one graph.
    for stage in ("acquire", "normalize"):
        client = AiwatcherClient(service=f"pod-{stage}", transport=transport)
        with (
            client.workflow("house-import", nodes=NODES, execution_id="exec-7") as flow,
            flow.node(stage),
        ):
            pass

    runs = {event["run_id"] for event in transport.events}
    executions = {event["workflow_run_id"] for event in transport.events}
    assert len(runs) == 2, "two processes, two runs"
    assert executions == {"exec-7"}, "one traversal"


def test_a_node_becomes_a_step_carrying_which_node_it_is(
    client: AiwatcherClient, transport: RecordingTransport
) -> None:
    with client.workflow("house-import", nodes=NODES) as flow, flow.node("acquire", kind="chain"):
        pass

    started = transport.of_type("step.started")[0]
    completed = transport.of_type("step.completed")[0]
    assert started["data"]["node"] == "acquire"
    assert started["data"]["step_type"] == "chain"
    assert started["data"]["call_id"] == completed["data"]["call_id"], (
        "a start and its end must share the key the span derives from"
    )
    assert completed["data"]["duration_ms"] >= 0


def test_two_attempts_of_one_node_are_distinguishable(
    client: AiwatcherClient, transport: RecordingTransport
) -> None:
    # The projection counts attempts by span key, which derives from `call_id`.
    # Two retries sharing one would fold into a single attempt.
    with client.workflow("house-import", nodes=NODES) as flow:
        with pytest.raises(RuntimeError), flow.node("analyze", attempt="try-1"):
            raise RuntimeError("vision provider timed out")
        with flow.node("analyze", attempt="try-2"):
            pass

    assert transport.of_type("step.failed")[0]["data"]["call_id"] == "try-1"
    assert transport.of_type("step.completed")[0]["data"]["call_id"] == "try-2"


def test_a_failing_stage_reports_why_and_reraises(
    client: AiwatcherClient, transport: RecordingTransport
) -> None:
    with (
        pytest.raises(RuntimeError, match="no walls"),
        client.workflow("house-import", nodes=NODES) as flow,
        flow.node("analyze"),
    ):
        raise RuntimeError("OpenCV found no walls")

    failed = transport.of_type("step.failed")[0]
    assert failed["data"]["node"] == "analyze"
    assert "no walls" in failed["data"]["error"]
    # And the traversal fails too, rather than reporting a workflow that ended
    # cleanly around a stage that did not.
    assert transport.of_type("run.failed"), "the run must fail with its stage"


def test_an_artifact_is_a_reference_on_its_node(
    client: AiwatcherClient, transport: RecordingTransport
) -> None:
    with client.workflow("house-import", nodes=NODES) as flow, flow.node("acquire") as stage:
        stage.artifact(
            "acquisition.json",
            uri="s3://planner-flyte/acquisition.json",
            media_type="application/json",
            size_bytes=41233,
        )

    artifact = transport.of_type("artifact.produced")[0]["data"]
    assert artifact["node"] == "acquire"
    assert artifact["uri"] == "s3://planner-flyte/acquisition.json"
    assert artifact["size_bytes"] == 41233
    # The bytes are never on the wire. Storing them would put a floor-plan PDF
    # in the durable log; the pointer is what is bounded.
    assert "content" not in artifact
    assert "data" not in artifact


def test_an_agent_message_names_both_ends(
    client: AiwatcherClient, transport: RecordingTransport
) -> None:
    # The edge nothing could infer from nesting: two agents exchanging work
    # through a queue nest inside nothing at all.
    with (
        client.workflow("house-import", nodes=NODES) as flow,
        flow.node("normalize") as stage,
        stage.agent("importer") as agent,
    ):
        agent.message("floor-plan", kind="handoff", channel="planner-import-data")

    message = transport.of_type("agent.message")[0]
    assert message["agent_id"] == "importer"
    assert message["data"]["from"] == "importer"
    assert message["data"]["to"] == "floor-plan"
    assert message["data"]["kind"] == "handoff"
    assert message["data"]["channel"] == "planner-import-data"


def test_a_workflow_with_no_declaration_still_publishes_its_run(
    client: AiwatcherClient, transport: RecordingTransport
) -> None:
    # A producer that knows its workflow's name but not its shape is a
    # legitimate first step, and it should light up the catalog.
    with client.workflow("house-import") as flow, flow.node("acquire"):
        pass

    assert not transport.of_type("workflow.declared"), "nothing to declare"
    assert transport.of_type("run.started")[0]["workflow_id"] == "house-import"
    assert transport.of_type("step.started")[0]["data"]["node"] == "acquire"


def test_every_event_of_one_traversal_shares_a_correlation(
    client: AiwatcherClient, transport: RecordingTransport
) -> None:
    with (
        client.workflow("house-import", nodes=NODES) as flow,
        flow.node("acquire") as stage,
        stage.agent("importer") as agent,
    ):
        agent.message("floor-plan")

    correlations = {event["correlation_id"] for event in transport.events}
    assert len(correlations) == 1
