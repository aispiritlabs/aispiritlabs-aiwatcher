"""A harness declared and traced without a composition object to read.

The shape here is two constants and a function that calls its stages in order —
what an application has when its agents are not a graph somebody composed — and
what is being tested is that the three levels still come out: the node that
sets a status, the agent that ran it, and the one call that left the process.
"""

from __future__ import annotations

from enum import StrEnum
from typing import Any

import pytest

from aiwatcher_sdk import AiwatcherClient
from aiwatcher_sdk.integrations.agentic import AiwatcherTracer, Harness, harness_steps
from aiwatcher_sdk.integrations.agentic.tracer import current_attempt


class Stage(StrEnum):
    SCOUT = "catalog_scout"
    VERIFIER = "offer_verifier"
    ANALYST = "market_analyst"


SCOUTING = Harness.of(
    "offer-scout",
    Stage,
    ((Stage.SCOUT, Stage.VERIFIER), (Stage.VERIFIER, Stage.ANALYST)),
)


class Recorder:
    def __init__(self) -> None:
        self.sent: list[dict[str, Any]] = []

    def send(self, batch: list[dict[str, Any]]) -> None:
        self.sent.extend(batch)

    def flush(self) -> None: ...

    def close(self) -> None: ...


def of_type(events: list[dict[str, Any]], event_type: str) -> list[dict[str, Any]]:
    return [event for event in events if event["event_type"] == event_type]


def test_a_harness_names_its_stages_with_its_own_enum() -> None:
    """The caller keeps its enum; this module never learns what it is."""
    assert SCOUTING.steps == ("catalog_scout", "offer_verifier", "market_analyst")
    assert SCOUTING.edges == (
        ("catalog_scout", "offer_verifier"),
        ("offer_verifier", "market_analyst"),
    )
    assert SCOUTING.workflow == "offer-scout"


def test_the_shape_is_declared_before_any_stage_runs() -> None:
    """A node nobody reached can only be drawn against a shape published first."""
    recorder = Recorder()
    client = AiwatcherClient(service="test", transport=recorder)

    with harness_steps(client, SCOUTING, run_id="cycle-1") as step, step(Stage.SCOUT):
        pass
    client.close()

    kinds = [event["event_type"] for event in recorder.sent]
    assert kinds[:2] == ["run.started", "workflow.declared"]
    declared = recorder.sent[1]
    assert declared["workflow_id"] == "offer-scout"
    assert [node["id"] for node in declared["data"]["nodes"]] == list(SCOUTING.steps)
    assert len(declared["data"]["edges"]) == 2
    # The trace and the saved run are one piece of work, so one identifier.
    assert declared["run_id"] == "cycle-1"
    # A stage nobody opened publishes nothing, which is what leaves it Pending.
    started = {event["data"]["node"] for event in of_type(recorder.sent, "step.started")}
    assert started == {"catalog_scout"}


def test_a_stage_is_a_node_an_agent_and_the_one_call_that_left_the_process() -> None:
    recorder = Recorder()
    client = AiwatcherClient(service="test", transport=recorder)

    with harness_steps(client, SCOUTING, run_id="cycle-1") as step:
        with step(Stage.SCOUT, tool="search", input_count=0) as call, call("search"):
            pass
        with step(Stage.ANALYST, tool="rank", input_count=4):
            pass
    client.close()

    [node] = of_type(recorder.sent, "step.completed")[:1]
    assert node["data"]["node"] == "catalog_scout"
    assert node["agent_id"] == "catalog_scout"
    # Metadata, never content: what the stage was given, counted.
    assert node["data"]["tool"] == "search" and node["data"]["input_count"] == 0
    agents = of_type(recorder.sent, "agent.started")
    tools = of_type(recorder.sent, "tool.completed")
    assert [event["agent_id"] for event in agents] == ["catalog_scout", "market_analyst"]
    # The second stage made no outward call, so it has no tool span. A stage
    # that never left the process and one that did must not look the same.
    assert [event["data"]["tool_name"] for event in tools] == ["search"]
    # The agent names the stage's own span as its parent, which is the whole
    # reason the stage mints one: without it the call would hang off the run.
    [stage] = of_type(recorder.sent, "step.started")[:1]
    assert agents[0]["parent_span_id"] == stage["span_id"]
    assert tools[0]["agent_id"] == "catalog_scout"


def test_a_stage_that_raises_fails_its_node_and_leaves_the_rest_pending() -> None:
    recorder = Recorder()
    client = AiwatcherClient(service="test", transport=recorder)

    with (
        pytest.raises(RuntimeError, match="no confirmed price"),
        harness_steps(client, SCOUTING, run_id="cycle-1") as step,
    ):
        with step(Stage.SCOUT):
            pass
        with step(Stage.VERIFIER):
            raise RuntimeError("no confirmed price")
    client.close()

    assert [event["data"]["node"] for event in of_type(recorder.sent, "step.completed")] == [
        "catalog_scout"
    ]
    failed = of_type(recorder.sent, "step.failed")
    assert [event["agent_id"] for event in failed] == ["offer_verifier"]
    assert "market_analyst" not in {
        event["data"]["node"] for event in of_type(recorder.sent, "step.started")
    }
    assert of_type(recorder.sent, "run.completed") == []
    assert len(of_type(recorder.sent, "run.failed")) == 1


def test_a_tracer_running_inside_a_stage_nests_under_it_rather_than_starting_a_run() -> None:
    """The reason a stage mints its own span id: the tracer is a second client."""
    recorder = Recorder()
    client = AiwatcherClient(service="test", transport=recorder)
    tracer_recorder = Recorder()
    tracer_client = AiwatcherClient(service="agent", transport=tracer_recorder)

    with harness_steps(client, SCOUTING, run_id="cycle-1") as step, step(Stage.SCOUT):
        assert current_attempt.get() is not None
        with AiwatcherTracer(client=tracer_client).step(name="parse"):
            pass
    assert current_attempt.get() is None
    client.close()
    tracer_client.close()

    [stage] = of_type(recorder.sent, "step.started")
    [nested] = of_type(tracer_recorder.sent, "step.started")
    assert nested["parent_span_id"] == stage["span_id"]
    assert nested["run_id"] == stage["run_id"]
    # The tracer opened no root of its own.
    assert of_type(tracer_recorder.sent, "run.started") == []
