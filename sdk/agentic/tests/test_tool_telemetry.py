"""The real Agent.run_tool boundary, with both local distributions installed."""

from collections.abc import Generator
from contextlib import contextmanager
from types import SimpleNamespace
from typing import Any

import pytest
from aiwatcher_sdk import AiwatcherClient, Correlation
from aiwatcher_sdk.integrations.agentic import AiwatcherTracer, tee
from aiwatcher_sdk.integrations.agentic import tracer as tracer_module
from aiwatcher_sdk.integrations.agentic.tracer import current_attempt

from aiwatcher_agentic.agent import Agent
from aiwatcher_agentic.capabilities import AbstractCapability, Allow, Deny, HookContext
from aiwatcher_agentic.exceptions import ModelRetry, ToolValidationError
from aiwatcher_agentic.model import TextModel
from aiwatcher_agentic.prompts import GemmaPromptBuilder
from aiwatcher_agentic.tools import ToolCallCommand, ToolFailure, ToolRunStatus
from aiwatcher_agentic.tracer import LLMTracer, NoopLLMTracer


class NoModels:
    @contextmanager
    def session(self, name: str = "model") -> Generator[TextModel, None, None]:
        raise AssertionError("run_tool must not call a model")
        yield  # pragma: no cover


class RecordingTransport:
    def __init__(self, *, broken: bool = False) -> None:
        self.events: list[dict[str, Any]] = []
        self.broken = broken
        self.calls = 0

    def send(self, batch: list[dict[str, Any]]) -> None:
        self.calls += 1
        if self.broken:
            raise RuntimeError("telemetry unavailable")
        self.events.extend(batch)

    def close(self) -> None:
        pass


class ToolCancelled(BaseException):
    pass


@pytest.mark.parametrize("composed", [False, True])
@pytest.mark.parametrize("managed", [False, True])
@pytest.mark.parametrize(
    ("behavior", "status", "output", "reason"),
    [
        ("success", ToolRunStatus.SUCCESS, "private result", None),
        ("exception", ToolRunStatus.ERROR, "Error: search unavailable", "search unavailable"),
        ("error", ToolRunStatus.ERROR, "Error: search refused", "Error: search refused"),
        ("polish_error", ToolRunStatus.ERROR, "Błąd: brak dostępu", "Błąd: brak dostępu"),
        (
            "long_error",
            ToolRunStatus.ERROR,
            "Error: " + "x" * 600 + "private suffix",
            ("Error: " + "x" * 600)[:500],
        ),
        ("retry", ToolRunStatus.RETRY, "Error: try again", "try again"),
        ("validation", ToolRunStatus.RETRY, "Error: invalid query", "invalid query"),
        ("escaping", None, None, "cancelled"),
    ],
)
def test_agent_tool_outcome(
    composed: bool,
    managed: bool,
    behavior: str,
    status: ToolRunStatus | None,
    output: str | None,
    reason: str | None,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    calls: list[str] = []
    cancelled = ToolCancelled("cancelled")
    clock = [10.0]
    monkeypatch.setattr(tracer_module, "time", SimpleNamespace(monotonic=lambda: clock[0]))

    def search(query: str) -> str:
        calls.append(query)
        clock[0] = 10.025
        if behavior == "exception":
            raise ValueError("search unavailable")
        if behavior == "retry":
            raise ModelRetry("try again")
        if behavior == "validation":
            raise ToolValidationError("invalid query")
        if behavior == "escaping":
            raise cancelled
        assert output is not None
        return output

    agent = Agent(
        model_provider=NoModels(),
        prompt_builder=GemmaPromptBuilder(system_prompt="private prompt"),
        tools=[search],
    )
    transport = RecordingTransport()
    client = AiwatcherClient(service="test", transport=transport)
    watcher = AiwatcherTracer(client=client)
    tracer: LLMTracer = tee(NoopLLMTracer(), watcher) if composed else watcher
    attempt = Correlation(
        run_id="run-1",
        conversation_id="session",
        workflow_id="scout",
        workflow_run_id="workflow-run-1",
        correlation_id="correlation-1",
        causation_id="cause-1",
        parent_span_id="ab" * 8,
    )
    token = current_attempt.set(attempt if managed else None)
    try:
        with tracer.workflow(name="scout", session_id="session"), tracer.agent(name="scout"):
            if behavior == "escaping":
                with pytest.raises(ToolCancelled) as raised:
                    agent.run_tool(("search", {"query": "private argument"}), tracer=tracer)
                assert raised.value is cancelled
            else:
                result = agent.run_tool(("search", {"query": "private argument"}), tracer=tracer)
                # The same call without telemetry has the same business result.
                baseline = agent.run_tool(("search", {"query": "private argument"}))
                assert result == baseline
                assert result is not None
                assert result.status == status
                assert result.output == output
                assert result.success is (status == ToolRunStatus.SUCCESS)
                assert result.retry is (status == ToolRunStatus.RETRY)
            # The closed tool must not remain the parent of the next span.
            with tracer.step(name="next"):
                pass
    finally:
        current_attempt.reset(token)
        client.close()

    assert calls == ["private argument"] * (1 if behavior == "escaping" else 2)
    events = transport.events
    [parent] = [e for e in events if e["event_type"] == "agent.started"]
    tool_events = [e for e in events if e["event_type"].startswith("tool.")]
    terminal = "tool.completed" if status == ToolRunStatus.SUCCESS else "tool.failed"
    assert [e["event_type"] for e in tool_events] == ["tool.started", terminal]
    start, end = tool_events
    assert len(start["span_id"]) == 16
    assert end["span_id"] == start["span_id"]
    assert end["data"]["call_id"] == start["data"]["call_id"]
    for event in tool_events:
        assert event["parent_span_id"] == parent["span_id"]
        for key in ("run_id", "conversation_id", "workflow_id", "agent_id"):
            assert event[key] == parent[key]
        if managed:
            assert event["workflow_run_id"] == "workflow-run-1"
            assert event["correlation_id"] == "correlation-1"
            assert event["causation_id"] == parent["causation_id"]
    assert end["data"]["duration_ms"] == pytest.approx(25.0)
    if reason is None:
        assert "error" not in end["data"]
    else:
        assert end["data"]["error"] == reason
    if status in (ToolRunStatus.ERROR, ToolRunStatus.RETRY):
        assert end["data"]["tool_status"] == status.value
        assert end["data"]["retryable"] is (status == ToolRunStatus.RETRY)
    [following] = [e for e in events if e["event_type"] == "step.started"]
    assert following["parent_span_id"] == parent["span_id"]
    assert "private argument" not in str(events)
    assert "private result" not in str(events)
    assert "private prompt" not in str(events)
    assert "private suffix" not in str(events)


@pytest.mark.parametrize("behavior", ["success", "exception", "retry", "error", "escaping"])
@pytest.mark.parametrize("failure", ["transport", "emit"])
def test_telemetry_failure_does_not_change_tool_execution(
    behavior: str, failure: str, monkeypatch: pytest.MonkeyPatch
) -> None:
    def search() -> str:
        if behavior == "exception":
            raise ValueError("search unavailable")
        if behavior == "retry":
            raise ModelRetry("try again")
        if behavior == "escaping":
            raise cancelled
        return "Error: refused" if behavior == "error" else "found"

    cancelled = ToolCancelled("cancelled")
    agent = Agent(
        model_provider=NoModels(),
        prompt_builder=GemmaPromptBuilder(system_prompt="test"),
        tools=[search],
    )
    transport = RecordingTransport(broken=failure == "transport")
    client = AiwatcherClient(service="test", transport=transport)
    emit_calls: list[bool] = []

    def broken_emit(*args: Any, **kwargs: Any) -> None:
        emit_calls.append(True)
        raise RuntimeError("emitter unavailable")

    if failure == "emit":
        monkeypatch.setattr(client, "emit", broken_emit)
    tracer = AiwatcherTracer(client=client)
    with tracer.workflow(name="scout", session_id="session"):
        if behavior == "escaping":
            with pytest.raises(ToolCancelled) as raised:
                agent.run_tool(("search", {}), tracer=tracer)
            assert raised.value is cancelled
        else:
            assert agent.run_tool(("search", {}), tracer=tracer) == agent.run_tool(("search", {}))
    client.close()
    assert emit_calls if failure == "emit" else transport.calls > 0


def test_missing_command_reports_failure_before_closing_span() -> None:
    agent = Agent(
        model_provider=NoModels(),
        prompt_builder=GemmaPromptBuilder(system_prompt="test"),
    )
    transport = RecordingTransport()
    client = AiwatcherClient(service="test", transport=transport)
    tracer = AiwatcherTracer(client=client)
    with tracer.workflow(name="scout", session_id="session"):
        result = agent.toolsets.execute(ToolCallCommand("missing", {}), tracer=tracer)
    client.close()
    assert result.status == ToolRunStatus.ERROR
    events = [e for e in transport.events if e["event_type"].startswith("tool.")]
    assert [e["event_type"] for e in events] == ["tool.started", "tool.failed"]
    assert events[-1]["data"]["error"] == result.output


def test_retry_then_success_are_two_attempts_with_separate_outcomes() -> None:
    calls = 0

    def search() -> str:
        nonlocal calls
        calls += 1
        if calls == 1:
            raise ModelRetry("try again")
        return "found"

    agent = Agent(
        model_provider=NoModels(),
        prompt_builder=GemmaPromptBuilder(system_prompt="test"),
        tools=[search],
    )
    transport = RecordingTransport()
    client = AiwatcherClient(service="test", transport=transport)
    tracer = AiwatcherTracer(client=client)
    with tracer.workflow(name="scout", session_id="session"):
        retry = agent.run_tool(("search", {}), tracer=tracer)
        success = agent.run_tool(("search", {}), tracer=tracer)
    client.close()
    assert retry is not None and retry.retry and not retry.success
    assert success is not None and success.success and success.output == "found"
    assert calls == 2
    events = [e for e in transport.events if e["event_type"].startswith("tool.")]
    assert [e["event_type"] for e in events] == [
        "tool.started",
        "tool.failed",
        "tool.started",
        "tool.completed",
    ]
    assert events[0]["span_id"] == events[1]["span_id"]
    assert events[2]["span_id"] == events[3]["span_id"]
    assert events[0]["span_id"] != events[2]["span_id"]
    assert events[0]["parent_span_id"] == events[2]["parent_span_id"]


@pytest.mark.parametrize("raises", [False, True])
def test_after_tool_hook_result_or_exception_is_the_recorded_outcome(raises: bool) -> None:
    error = ValueError("hook failure")

    class ResultHook(AbstractCapability):
        def after_tool_execute(self, tool_name: str, result: str, context: HookContext) -> str:
            if raises:
                raise error
            return "Error: rejected by hook"

    def search() -> str:
        return "found"

    agent = Agent(
        model_provider=NoModels(),
        prompt_builder=GemmaPromptBuilder(system_prompt="test"),
        tools=[search],
        capabilities=[ResultHook()],
    )
    transport = RecordingTransport()
    client = AiwatcherClient(service="test", transport=transport)
    tracer = AiwatcherTracer(client=client)
    with tracer.workflow(name="scout", session_id="session"):
        if raises:
            with pytest.raises(ValueError) as raised:
                agent.run_tool(("search", {}), tracer=tracer)
            assert raised.value is error
        else:
            result = agent.run_tool(("search", {}), tracer=tracer)
            assert result is not None and result.status == ToolRunStatus.ERROR
            assert result.output == "Error: rejected by hook"
    client.close()
    events = [e for e in transport.events if e["event_type"].startswith("tool.")]
    assert [e["event_type"] for e in events] == ["tool.started", "tool.failed"]
    assert events[-1]["data"]["error"] == ("hook failure" if raises else "Error: rejected by hook")


@pytest.mark.parametrize(
    ("failure", "status", "output"),
    [
        (
            ToolFailure("no sources in the allowed domains"),
            ToolRunStatus.ERROR,
            "Error: no sources in the allowed domains",
        ),
        (ToolFailure("rate limited", retryable=True), ToolRunStatus.RETRY, "Error: rate limited"),
        (ToolFailure("Error: already prefixed"), ToolRunStatus.ERROR, "Error: already prefixed"),
    ],
)
def test_a_tool_that_declares_its_failure_is_not_read_by_prefix(
    failure: ToolFailure, status: ToolRunStatus, output: str
) -> None:
    """The status comes from the tool, not from how its message happens to start.

    Read by prefix, `no sources in the allowed domains` is a successful call —
    which is how a search that found nothing used to close as `tool.completed`.
    """
    transport = RecordingTransport()
    client = AiwatcherClient(service="test", transport=transport)
    tracer = AiwatcherTracer(client=client)

    def search(query: str) -> ToolFailure:
        return failure

    agent = Agent(
        model_provider=NoModels(),
        prompt_builder=GemmaPromptBuilder(system_prompt="p"),
        tools=[search],
    )
    with tracer.workflow(name="scout", session_id="session"), tracer.agent(name="scout"):
        result = agent.run_tool(("search", {"query": "panels"}), tracer=tracer)
    client.close()

    assert result is not None
    assert result.status == status
    assert result.output == output
    assert result.success is False
    assert result.retry is (status == ToolRunStatus.RETRY)

    tool_events = [e for e in transport.events if e["event_type"].startswith("tool.")]
    assert [e["event_type"] for e in tool_events] == ["tool.started", "tool.failed"]
    end = tool_events[1]
    assert end["data"]["error"] == failure.message
    assert end["data"]["tool_status"] == status.value
    assert end["data"]["retryable"] is (status == ToolRunStatus.RETRY)


def test_a_capability_refuses_a_call_and_the_refusal_is_a_tool_span() -> None:
    """A denied call never runs, and is still visible as a failed tool call.

    Held outside the agent, an approval gate has to be kept in step with the
    agent by hand; held here, the refusal is the tool's result and the model
    reads the reason like any other failure.
    """

    class RequireApproval(AbstractCapability):
        def before_tool_execute(
            self, tool_name: str, parameters: dict[str, Any], context: HookContext
        ) -> Deny:
            return Deny("publishing needs an approved action")

    class Widen(AbstractCapability):
        def before_tool_execute(
            self, tool_name: str, parameters: dict[str, Any], context: HookContext
        ) -> Allow:
            return Allow({**parameters, "query": parameters["query"] + " site:example.org"})

    calls: list[str] = []

    def search(query: str) -> str:
        calls.append(query)
        return "found"

    transport = RecordingTransport()
    client = AiwatcherClient(service="test", transport=transport)
    tracer = AiwatcherTracer(client=client)
    denied = Agent(
        model_provider=NoModels(),
        prompt_builder=GemmaPromptBuilder(system_prompt="p"),
        tools=[search],
        # The widening capability sits after the refusal and must not be asked.
        capabilities=[RequireApproval(), Widen()],
    )
    with tracer.workflow(name="scout", session_id="session"), tracer.agent(name="scout"):
        result = denied.run_tool(("search", {"query": "panels"}), tracer=tracer)
    client.close()

    assert calls == []
    assert result is not None
    assert result.status == ToolRunStatus.DENIED
    assert result.output == "Error: publishing needs an approved action"
    assert result.success is False
    tool_events = [e for e in transport.events if e["event_type"].startswith("tool.")]
    assert [e["event_type"] for e in tool_events] == ["tool.started", "tool.failed"]
    assert tool_events[1]["data"]["error"] == "publishing needs an approved action"

    allowed = Agent(
        model_provider=NoModels(),
        prompt_builder=GemmaPromptBuilder(system_prompt="p"),
        tools=[search],
        capabilities=[Widen()],
    )
    granted = allowed.run_tool(("search", {"query": "panels"}))
    assert granted is not None and granted.success
    assert calls == ["panels site:example.org"]
    assert granted.tool_call == ("search", {"query": "panels site:example.org"})
