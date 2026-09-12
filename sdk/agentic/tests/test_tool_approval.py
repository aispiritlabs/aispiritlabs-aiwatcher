"""A tool call a capability sends out for a decision, and the run that resumes it.

The gate these tests describe is the one that lives today in three places at
once — a state machine in TypeScript, a claim in Go and a plan in Python — and
the reason it had to is that a capability could answer `Allow` or `Deny` and
nothing else. Neither fits a human approval: `Allow` runs the call now and
`Deny` tells the model no, while an approval is a question whose answer arrives
after this process has gone.

`Ask` is that third answer, and the shape of the whole thing follows from one
fact: a suspended run has no result. So execution raises, the caller persists
what the exception carries, and resuming is an ordinary run in which the same
capability — now able to read the answer — says `Allow`.
"""

from collections.abc import Generator
from contextlib import contextmanager
from typing import Any

import pytest

from aiwatcher_agentic.agent import Agent
from aiwatcher_agentic.capabilities import (
    AbstractCapability,
    Allow,
    Ask,
    CombinedCapability,
    Deny,
    HookContext,
    ToolApprovalRequired,
    ToolDecision,
)
from aiwatcher_agentic.model import TextModel
from aiwatcher_agentic.prompts import GemmaPromptBuilder
from aiwatcher_agentic.tools import ToolRunStatus


class NoModels:
    """Nothing here reaches a model; a tool call is parsed and run on its own."""

    @contextmanager
    def session(self, name: str = "model") -> Generator[TextModel, None, None]:
        raise AssertionError("these tests must not call a model")
        yield  # pragma: no cover


class ApprovalLedger(AbstractCapability):
    """The application's side of the gate: a store, and one lookup per call.

    Deliberately this small. Everything durable stays here — which row, who
    approved it, when it expires — and nothing about *how a run suspends* does.
    """

    def __init__(self) -> None:
        self.decisions: dict[str, bool] = {}
        self.asked: list[tuple[str, dict[str, Any]]] = []

    def before_tool_execute(
        self, tool_name: str, parameters: dict[str, Any], context: HookContext
    ) -> ToolDecision:
        handle = f"{tool_name}:{parameters['to']}"
        settled = self.decisions.get(handle)
        if settled is None:
            self.asked.append((tool_name, dict(parameters)))
            return Ask(handle, detail={"preview": f"send to {parameters['to']}"})
        return Allow(parameters) if settled else Deny("the operator refused this message")


def _agent(capability: AbstractCapability, sent: list[str]) -> Agent:
    def send_mail(to: str) -> str:
        sent.append(to)
        return f"sent to {to}"

    return Agent(
        model_provider=NoModels(),
        prompt_builder=GemmaPromptBuilder(system_prompt="p"),
        tools=[send_mail],
        capabilities=[capability],
    )


def test_a_call_waiting_for_a_decision_suspends_the_run_instead_of_answering() -> None:
    """No result comes back, because there is nothing yet to tell the model.

    A `ToolRunResult` here would be read as a failed tool: the model would get
    a fabricated "waiting" string to reason from and a subagent would retry the
    call. The exception is what stops every caller in the loop at once.
    """
    sent: list[str] = []
    ledger = ApprovalLedger()

    with pytest.raises(ToolApprovalRequired) as raised:
        _agent(ledger, sent).run_tool(("send_mail", {"to": "buyer@example.com"}))

    assert sent == [], "the tool must not run while the decision is open"
    assert raised.value.tool_name == "send_mail"
    assert raised.value.parameters == {"to": "buyer@example.com"}
    assert raised.value.handle == "send_mail:buyer@example.com"
    assert raised.value.detail == {"preview": "send to buyer@example.com"}


def test_the_approved_call_runs_on_an_ordinary_second_run() -> None:
    """Resuming is not its own entry point — a second one is a second state machine."""
    sent: list[str] = []
    ledger = ApprovalLedger()
    agent = _agent(ledger, sent)

    with pytest.raises(ToolApprovalRequired) as raised:
        agent.run_tool(("send_mail", {"to": "buyer@example.com"}))

    # What an operator does in the panel, hours later and in another process.
    ledger.decisions[raised.value.handle] = True

    result = agent.run_tool(("send_mail", {"to": "buyer@example.com"}))
    assert result is not None
    assert result.success
    assert result.status == ToolRunStatus.SUCCESS
    assert result.output == "sent to buyer@example.com"
    assert sent == ["buyer@example.com"]


def test_a_refused_call_comes_back_as_the_tool_result_the_model_reads() -> None:
    """A settled *no* is a `Deny`, and `Deny` already had somewhere to go."""
    sent: list[str] = []
    ledger = ApprovalLedger()
    agent = _agent(ledger, sent)

    with pytest.raises(ToolApprovalRequired) as raised:
        agent.run_tool(("send_mail", {"to": "buyer@example.com"}))
    ledger.decisions[raised.value.handle] = False

    result = agent.run_tool(("send_mail", {"to": "buyer@example.com"}))
    assert result is not None
    assert result.status == ToolRunStatus.DENIED
    assert result.output == "Error: the operator refused this message"
    assert sent == []


def test_a_refusal_in_front_of_the_gate_means_nobody_is_asked() -> None:
    """Order decides: no sense sending a person a call policy throws away anyway."""

    class NeverOnSundays(AbstractCapability):
        def before_tool_execute(
            self, tool_name: str, parameters: dict[str, Any], context: HookContext
        ) -> Deny:
            return Deny("outside the sending window")

    ledger = ApprovalLedger()
    combined = CombinedCapability([NeverOnSundays(), ledger])

    decision = combined.before_tool_execute("send_mail", {"to": "buyer@example.com"}, HookContext())
    assert isinstance(decision, Deny)
    assert ledger.asked == []


def test_the_gate_in_front_stops_the_capabilities_behind_it() -> None:
    """A capability after the gate would run its `before` half for a call on hold."""
    widened: list[str] = []

    class Widen(AbstractCapability):
        def before_tool_execute(
            self, tool_name: str, parameters: dict[str, Any], context: HookContext
        ) -> Allow:
            widened.append(tool_name)
            return Allow(parameters)

    ledger = ApprovalLedger()
    combined = CombinedCapability([ledger, Widen()])

    decision = combined.before_tool_execute("send_mail", {"to": "buyer@example.com"}, HookContext())
    assert isinstance(decision, Ask)
    assert widened == []
