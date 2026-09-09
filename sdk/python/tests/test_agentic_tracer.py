"""The tracer's keyword arguments describe spans, not arguments to the model."""

from dataclasses import dataclass
from typing import Any

import pytest

from aiwatcher_sdk import NullTransport
from aiwatcher_sdk.integrations.agentic import aiwatcher_tracer, tee


@dataclass
class ModelResponse:
    text: str = "{}"
    model: str = "m"


@pytest.mark.parametrize("nested", [False, True])
def test_span_attributes_never_reach_the_zero_argument_model_callback(nested: bool) -> None:
    tracer: Any = aiwatcher_tracer(service="test", transport=NullTransport())
    if nested:
        tracer = tee(aiwatcher_tracer(service="other", transport=NullTransport()), tracer)
    calls: list[bool] = []
    response = ModelResponse()

    def invoke() -> ModelResponse:
        calls.append(True)
        return response

    assert (
        tracer.llm(
            name="llm-call",
            model="m",
            messages=[],
            tools=[{"type": "function"}],
            extra_attributes={"k": 1},
            invoke=invoke,
        )
        is response
    )
    assert calls == [True]


def test_managed_task_tracer_attaches_child_spans_without_a_second_run_lifecycle() -> None:
    from test_worker import WorkerApi, assignment, worker

    from aiwatcher_sdk import AiwatcherClient
    from aiwatcher_sdk.worker import TaskContext

    events: list[dict[str, Any]] = []

    class RecordingTransport:
        def send(self, batch: list[dict[str, Any]]) -> None:
            events.extend(batch)

        def close(self) -> None:
            pass

    client = AiwatcherClient(service="test", transport=RecordingTransport())
    api = WorkerApi(assignment())

    def stage(inputs: dict[str, Any], ctx: TaskContext) -> None:
        tracer = ctx.tracer
        with (
            tracer.workflow(name="inner-agent-flow", session_id="session"),
            tracer.agent(name="agent"),
        ):
            tracer.llm(name="model", model="test", messages=[], invoke=lambda: ModelResponse())
        tracer.shutdown()

    with worker(api, stage, telemetry=client) as process:
        process.run_once()
    assert events
    assert not any(event["event_type"].startswith(("run.", "step.")) for event in events)
    assert all(event["workflow_run_id"] == "import-1" for event in events)
    agent = next(event for event in events if event["event_type"] == "agent.started")
    llm = next(event for event in events if event["event_type"] == "llm.started")
    assert agent["parent_span_id"] == "cd" * 8
    assert llm["parent_span_id"] == agent["span_id"]
