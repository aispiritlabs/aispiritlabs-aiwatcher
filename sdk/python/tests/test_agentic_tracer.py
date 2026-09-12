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


class Recording:
    def __init__(self) -> None:
        self.events: list[dict[str, Any]] = []

    def send(self, batch: list[dict[str, Any]]) -> None:
        self.events.extend(batch)

    def close(self) -> None:
        pass


def test_a_tracer_built_before_any_attempt_nests_what_it_records_under_the_attempt() -> None:
    # An agent runtime builds its tracer once, long before a worker hands it an
    # attempt. It used to open a root run of its own for every turn.
    from aiwatcher_sdk import AiwatcherClient, Correlation
    from aiwatcher_sdk.integrations.agentic import AiwatcherTracer
    from aiwatcher_sdk.integrations.agentic.tracer import current_attempt

    transport = Recording()
    client = AiwatcherClient(service="agent", transport=transport)
    tracer = AiwatcherTracer(client=client)
    token = current_attempt.set(
        Correlation(
            run_id="exec-1",
            workflow_id="digest",
            workflow_run_id="exec-1",
            correlation_id="exec-1",
            parent_span_id="cd" * 8,
        )
    )
    try:
        with tracer.workflow(name="digest", session_id="session"), tracer.agent(name="digest"):
            tracer.llm(name="model", model="m", messages=[], invoke=lambda: ModelResponse())
    finally:
        current_attempt.reset(token)
    client.close()

    events = transport.events
    assert not any(event["event_type"].startswith("run.") for event in events)
    assert {event["run_id"] for event in events} == {"exec-1"}
    agent = next(event for event in events if event["event_type"] == "agent.started")
    llm = next(event for event in events if event["event_type"] == "llm.started")
    assert agent["parent_span_id"] == "cd" * 8
    assert llm["parent_span_id"] == agent["span_id"]


def test_outside_an_attempt_a_tracer_opens_a_run_of_its_own_as_it_always_did() -> None:
    from aiwatcher_sdk import AiwatcherClient
    from aiwatcher_sdk.integrations.agentic import AiwatcherTracer

    transport = Recording()
    client = AiwatcherClient(service="agent", transport=transport)
    tracer = AiwatcherTracer(client=client)
    with tracer.workflow(name="digest", session_id="session"), tracer.agent(name="digest"):
        pass
    client.close()

    [run] = [event for event in transport.events if event["event_type"] == "run.started"]
    assert run["run_id"].startswith("session-")


def _llm_events(extra_attributes: dict[str, Any] | None) -> list[dict[str, Any]]:
    from aiwatcher_sdk import AiwatcherClient
    from aiwatcher_sdk.integrations.agentic import AiwatcherTracer

    transport = Recording()
    client = AiwatcherClient(service="agent", transport=transport)
    tracer = AiwatcherTracer(client=client)
    kwargs: dict[str, Any] = (
        {} if extra_attributes is None else {"extra_attributes": extra_attributes}
    )
    with tracer.workflow(name="digest", session_id="session"):
        tracer.llm(name="llm-call", model="m", messages=[], invoke=ModelResponse, **kwargs)
    client.close()
    return [event for event in transport.events if event["event_type"].startswith("llm.")]


def test_the_prompt_an_agent_names_reaches_both_llm_events_as_a_reference() -> None:
    version = "ab" * 32

    events = _llm_events(
        {
            "agentic.prompt_name": "sage",
            "agentic.prompt_version": version,
            "agentic.prompt_hash": "of the rendered prompt",
        }
    )

    assert [event["event_type"] for event in events] == ["llm.started", "llm.completed"]
    for event in events:
        assert event["data"]["prompt_name"] == "sage"
        assert event["data"]["prompt_version"] == version
        assert "of the rendered prompt" not in event["data"].values()


@pytest.mark.parametrize("extra_attributes", [None, {"agentic.prompt_hash": "h"}])
def test_a_call_that_names_no_prompt_carries_no_reference(
    extra_attributes: dict[str, Any] | None,
) -> None:
    events = _llm_events(extra_attributes)

    assert events
    assert not any({"prompt_name", "prompt_version"} & event["data"].keys() for event in events)


@pytest.mark.parametrize(
    ("updates", "terminal", "reason", "retryable"),
    [
        ([{"level": "WARNING"}], "completed", None, None),
        ([{"output": {"error": "data, not status"}}], "completed", None, None),
        (
            [{"level": "ERROR", "output": {"error": "legacy failure"}}],
            "failed",
            "legacy failure",
            False,
        ),
        (
            [{"level": "WARNING", "output": {"retry": "legacy retry"}}],
            "failed",
            "legacy retry",
            True,
        ),
        ([{"metadata": {"agentic.tool_status": "error"}}], "failed", "Tool reported error", False),
        ([{"metadata": {"agentic.tool_status": "retry"}}], "failed", "Tool reported retry", True),
        (
            [
                {"level": "ERROR", "output": {"error": "first"}},
                {"level": "WARNING"},
                {"output": {"output": "private result"}},
            ],
            "failed",
            "first",
            False,
        ),
        ([{"level": "ERROR", "output": {"error": "x" * 1000}}], "failed", "x" * 500, False),
    ],
)
def test_tool_updates_are_outcomes_not_payload_capture(
    updates: list[dict[str, Any]], terminal: str, reason: str | None, retryable: bool | None
) -> None:
    from aiwatcher_sdk import AiwatcherClient
    from aiwatcher_sdk.integrations.agentic import AiwatcherTracer

    recording = Recording()
    client = AiwatcherClient(service="test", transport=recording)
    tracer = AiwatcherTracer(client=client)
    with tracer.workflow(name="scout", session_id="session"):
        with tracer.step(name="search", span_type="TOOL", input={"secret": "argument"}) as span:
            for update in updates:
                span.update(**update)
        with tracer.step(name="next", span_type="TOOL"):
            pass
    client.close()
    events = [e for e in recording.events if e["event_type"].startswith("tool.")]
    assert [e["event_type"] for e in events] == [
        "tool.started",
        f"tool.{terminal}",
        "tool.started",
        "tool.completed",
    ]
    assert events[1]["data"].get("error") == reason
    assert events[1]["data"].get("retryable") is retryable
    assert events[0]["span_id"] == events[1]["span_id"]
    assert events[0]["span_id"] != events[2]["span_id"]
    assert "argument" not in str(events)
    assert "private result" not in str(events)


def test_exception_after_error_update_emits_one_failure_with_escaping_cause() -> None:
    from aiwatcher_sdk import AiwatcherClient
    from aiwatcher_sdk.integrations.agentic import AiwatcherTracer

    recording = Recording()
    client = AiwatcherClient(service="test", transport=recording)
    tracer = AiwatcherTracer(client=client)
    error = ValueError("escaping failure")
    with tracer.workflow(name="scout", session_id="session"):
        with (
            pytest.raises(ValueError) as raised,
            tracer.step(name="search", span_type="TOOL") as span,
        ):
            span.update(level="ERROR", output={"error": "earlier failure"})
            raise error
        assert raised.value is error
    client.close()
    events = [e for e in recording.events if e["event_type"].startswith("tool.")]
    assert [e["event_type"] for e in events] == ["tool.started", "tool.failed"]
    assert events[1]["data"]["error"] == "escaping failure"


def test_tee_updates_other_handles_when_one_annotation_fails() -> None:
    from aiwatcher_sdk.integrations.agentic.tracer import TeeSpan, ToolSpan

    class BrokenSpan:
        def update(self, **kwargs: Any) -> None:
            raise RuntimeError("backend unavailable")

    watcher = ToolSpan()
    TeeSpan([BrokenSpan(), watcher]).update(level="ERROR", output={"error": "tool failure"})
    assert watcher.outcome()["error"] == "tool failure"


def test_a_step_that_reports_an_error_without_raising_fails_like_a_tool() -> None:
    """A step whose work returned a failure must not close as `step.completed`.

    Without this, the only way to make a failed step visible was to raise an
    exception the caller did not have, or to wrap already-scoped work in a
    second, tool-shaped span — which also doubles the tool calls the panel
    shows for one search.
    """
    from aiwatcher_sdk import AiwatcherClient
    from aiwatcher_sdk.integrations.agentic import AiwatcherTracer

    recording = Recording()
    client = AiwatcherClient(service="test", transport=recording)
    tracer = AiwatcherTracer(client=client)
    with tracer.workflow(name="scout", session_id="session"):
        with tracer.step(name="discover", span_type="CHAIN") as span:
            span.update(level="ERROR", output={"error": "the search returned no sources"})
        with tracer.step(name="compose", span_type="CHAIN"):
            pass
    client.close()

    events = [e for e in recording.events if e["event_type"].startswith("step.")]
    assert [e["event_type"] for e in events] == [
        "step.started",
        "step.failed",
        "step.started",
        "step.completed",
    ]
    assert events[1]["data"]["error"] == "the search returned no sources"
    assert events[1]["data"]["step_type"] == "chain"
    # A step is not a tool: the tool-shaped keys stay on tool spans.
    assert "tool_status" not in events[1]["data"]
    assert "retryable" not in events[1]["data"]
