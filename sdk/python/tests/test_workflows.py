"""What the workflow API puts on the wire.

A recording transport rather than a mock: what is worth asserting is the
envelope the Rust side folds into a graph, and a mock that counted `emit` calls
would pass while sending a shape nothing can draw.
"""

from __future__ import annotations

import threading
import time
from typing import Any

import pytest

from aiwatcher_sdk import AiwatcherClient, Correlation


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


def test_edges_sharing_a_bound_are_declared_with_it_and_move_the_version(
    client: AiwatcherClient, transport: RecordingTransport
) -> None:
    edges = [("write", "review"), ("review", "write"), ("review", "fix"), ("fix", "write")]
    with client.workflow("revise", nodes=["write", "review", "fix"], edges=edges):
        pass
    bound = [{"edges": [("review", "write"), ("fix", "write")], "at_most": 3}]
    with client.workflow("revise", nodes=["write", "review", "fix"], edges=edges, bounds=bound):
        pass

    plain, bounded = transport.of_type("workflow.declared")
    assert "bounds" not in plain["data"], "a declaration without one sends what it always did"
    assert bounded["data"]["bounds"] == [
        {"edges": [["review", "write"], ["fix", "write"]], "at_most": 3}
    ]
    assert plain["data"]["version"] != bounded["data"]["version"]


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


def test_every_event_of_a_run_names_the_variant_its_client_is(
    transport: RecordingTransport,
) -> None:
    # A deployment is one variant: said once, on the client, and on every
    # event a run opened by it sends, agent and model call included.
    client = AiwatcherClient(service="support-bot", transport=transport, variant_id="v-prod")
    with client.run("run-1") as run, run.agent("answer") as agent, agent.llm(model="m"):
        pass

    assert transport.events
    assert {event.get("variant_id") for event in transport.events} == {"v-prod"}
    assert "evaluation_id" not in transport.of_type("run.started")[0]["data"]


def test_a_run_answering_a_measurement_names_it_on_its_start_and_its_own_variant(
    client: AiwatcherClient, transport: RecordingTransport
) -> None:
    # What keeps a benchmark out of what the variant was observed doing.
    with client.run("case-1", variant_id="v-candidate", evaluation_id="answers-candidate"):
        pass

    started = transport.of_type("run.started")[0]
    assert started["variant_id"] == "v-candidate"
    assert started["data"] == {"evaluation_id": "answers-candidate"}
    assert "evaluation_id" not in transport.of_type("run.completed")[0]["data"]


def test_a_run_of_a_client_that_names_no_variant_sends_none(
    client: AiwatcherClient, transport: RecordingTransport
) -> None:
    with client.run("run-1"):
        pass

    assert all("variant_id" not in event for event in transport.events)


def test_a_model_call_hands_its_server_the_run_it_is_and_the_server_names_it(
    client: AiwatcherClient, transport: RecordingTransport
) -> None:
    # A second witness: the server's own run names the call it served.
    from aiwatcher_sdk import CALLER_RUN_HEADER

    with client.run("app-run") as run, run.agent("bot") as agent, agent.llm(model="m") as call:
        headers = call.caller_headers()
    with client.run("serve-1", caller_run_id=headers[CALLER_RUN_HEADER]):
        pass

    assert headers == {CALLER_RUN_HEADER: "app-run"}
    served = [event for event in transport.of_type("run.started") if event["run_id"] == "serve-1"]
    assert served[0]["data"] == {"caller_run_id": "app-run"}


def test_what_a_call_was_rendered_with_goes_to_its_gateway_and_never_on_the_log(
    client: AiwatcherClient, transport: RecordingTransport
) -> None:
    from aiwatcher_sdk import GATEWAY_FIELD

    with client.run("app-run") as run, run.agent("bot") as agent, agent.llm(model="m") as call:
        body = call.caller_body(country="Peru")

    assert body == {GATEWAY_FIELD: {"variables": {"country": "Peru"}}}
    assert "Peru" not in str(transport.events)


def test_a_workflow_run_can_name_the_variant_and_the_measurement_it_answers(
    client: AiwatcherClient, transport: RecordingTransport
) -> None:
    with client.workflow(
        "capitals-app", nodes=["answer"], variant_id="v-candidate", evaluation_id="answers-1"
    ):
        pass

    started = transport.of_type("run.started")[0]
    assert started["variant_id"] == "v-candidate"
    assert started["data"] == {"evaluation_id": "answers-1"}
    assert transport.of_type("workflow.declared")[0]["variant_id"] == "v-candidate"


def test_each_client_numbers_the_events_it_sends_into_a_run_from_nought(
    client: AiwatcherClient, transport: RecordingTransport
) -> None:
    tracer_transport = RecordingTransport()
    tracer = AiwatcherClient(service="planner-import-service", transport=tracer_transport)
    with client.workflow("house-import", nodes=NODES, edges=EDGES) as flow:
        with flow.node("acquire"):
            tracer.emit("llm.started", flow.correlation, {"model": "m"})
            tracer.emit("llm.completed", flow.correlation, {"model": "m"})
        with client.run("another"):
            pass

    own = [event for event in transport.events if event["run_id"] == flow.correlation.run_id]
    assert [event["sequence"] for event in own] == list(range(len(own)))
    assert [event["sequence"] for event in tracer_transport.events] == [0, 1]
    assert own[0]["source"]["client"] != tracer_transport.events[0]["source"]["client"]
    assert {event["source"]["client"] for event in transport.events} == {
        own[0]["source"]["client"]
    }, "one client, one name"
    other = [event for event in transport.events if event["run_id"] == "another"]
    assert [event["sequence"] for event in other] == [0, 1]


def test_each_client_numbers_the_runs_it_opens_for_a_variant_and_no_measurement_among_them(
    transport: RecordingTransport,
) -> None:
    client = AiwatcherClient(service="capitals", transport=transport, variant_id="v1")
    with client.run("run-1"):
        pass
    with client.run("measured", evaluation_id="e1"):
        pass
    with client.workflow("house-import", nodes=NODES, edges=EDGES, run_id="run-2"):
        pass
    with client.run("elsewhere", variant_id="v2"):
        pass

    starts = {
        event["run_id"]: event.get("run_sequence") for event in transport.of_type("run.started")
    }
    assert starts == {"run-1": 0, "measured": None, "run-2": 1, "elsewhere": 0}
    assert all(
        "run_sequence" not in event and "run_counted_from" not in event
        for event in transport.events
        if event["event_type"] != "run.started"
    )
    began = {
        event["run_id"]: (event.get("run_counted_from"), event["occurred_at"])
        for event in transport.of_type("run.started")
    }
    assert began["run-2"][0] == began["run-1"][0] == began["run-1"][1], (
        "a count began when its first run started, and says so on every start after"
    )
    assert began["elsewhere"][0] == began["elsewhere"][1]
    assert began["measured"][0] is None


def test_events_from_many_threads_reach_the_transport_in_the_order_they_were_numbered() -> None:
    received: list[dict[str, Any]] = []

    class Slow:
        def send(self, batch: list[dict[str, Any]]) -> None:
            time.sleep(0.0005)
            received.extend(batch)

        def flush(self) -> None: ...

        def close(self) -> None: ...

    client = AiwatcherClient(service="capitals", transport=Slow())
    context = Correlation(run_id="parallel")
    threads = [
        threading.Thread(target=lambda: [client.emit("llm.chunk", context, {}) for _ in range(20)])
        for _ in range(8)
    ]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()

    assert [event["sequence"] for event in received] == list(range(160))


def test_a_shared_bound_on_anything_but_one_loop_s_ways_back_is_refused_naming_it(
    client: AiwatcherClient, transport: RecordingTransport
) -> None:
    nodes = ["write", "review", "fix", "publish", "draft", "check"]
    edges = [
        ("write", "review"),
        ("review", "write"),
        ("review", "fix"),
        ("fix", "review"),
        ("review", "publish"),
        ("draft", "check"),
        ("check", "draft"),
    ]
    with (
        pytest.raises(ValueError, match="review to publish, which leads nowhere back"),
        client.workflow(
            "revise",
            nodes=nodes,
            edges=edges,
            bounds=[{"edges": [("review", "write"), ("review", "publish")], "at_most": 3}],
        ),
    ):
        pass
    for other_loop in (("check", "draft"), ("fix", "review")):
        with (
            pytest.raises(ValueError, match="the heads of different loops"),
            client.workflow(
                "revise",
                nodes=nodes,
                edges=edges,
                bounds=[{"edges": [("review", "write"), other_loop], "at_most": 2}],
            ),
        ):
            pass
    assert not transport.of_type("workflow.declared"), "nothing declared a shape it cannot keep"


def test_an_edge_bound_that_holds_nothing_is_refused_naming_both_numbers(
    client: AiwatcherClient, transport: RecordingTransport
) -> None:
    with (
        pytest.raises(
            ValueError,
            match="the bound of at most 1 on rank to answer holds nothing, since rank completes "
            "at most once on this shape",
        ),
        client.workflow(
            "rank",
            nodes=["retrieve", "rank", "answer"],
            edges=[("retrieve", "rank"), {"from": "rank", "to": "answer", "at_most": 1}],
        ),
    ):
        pass
    joined = [("plan", "search"), ("plan", "browse"), ("search", "merge"), ("browse", "merge")]
    with (
        pytest.raises(ValueError, match="since merge completes at most 2 times"),
        client.workflow(
            "join",
            nodes=["plan", "search", "browse", "merge", "answer"],
            edges=[*joined, {"from": "merge", "to": "answer", "at_most": 2}],
        ),
    ):
        pass
    loop = ["write", "review", "fix"]
    ways_back = [("write", "review"), ("review", "fix"), ("fix", "write")]
    with (
        pytest.raises(
            ValueError,
            match="the bound of at most 3 on review to write holds nothing the bound of at most 2 "
            "that fix to write and review to write share does not",
        ),
        client.workflow(
            "revise",
            nodes=loop,
            edges=[*ways_back, {"from": "review", "to": "write", "at_most": 3}],
            bounds=[{"edges": [("review", "write"), ("fix", "write")], "at_most": 2}],
        ),
    ):
        pass
    assert not transport.of_type("workflow.declared"), "nothing declared a bound that holds nothing"

    with client.workflow(
        "join",
        nodes=["plan", "search", "browse", "merge", "answer"],
        edges=[*joined, {"from": "merge", "to": "answer", "at_most": 1}],
    ):
        pass
    with client.workflow(
        "items",
        nodes=["retrieve", {"id": "answer", "repeats": True}, "summarize"],
        edges=[("retrieve", "answer"), {"from": "answer", "to": "summarize", "at_most": 3}],
    ):
        pass
    with client.workflow(
        "revise",
        nodes=loop,
        edges=[*ways_back, {"from": "review", "to": "write", "at_most": 2}],
    ):
        pass
    assert len(transport.of_type("workflow.declared")) == 3, (
        "past a join, out of a repeating node and round a cycle a bound holds something"
    )
