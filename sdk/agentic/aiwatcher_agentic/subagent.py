"""One sub-task, declared once: its own prompt, model, tools, context and result.

:class:`~aiwatcher_agentic.agent.Agent` deliberately does not loop. ``run()``
answers or asks for a tool and hands both back, because the decision of whether
a tool may run at all belongs to whoever owns the run. That is the right default
and it leaves a gap: an application that wants the ordinary "ask, call the tool,
answer from what came back" cycle has to write the cycle itself, and every one
that did wrote it the same way — two agents, one holding the tools and one
holding the output schema, with the tool's result **pasted into the second
one's prompt as text**.

Pasting is where it goes wrong. A tool result is a turn with a role, and the
model has been trained on it as one; flattened into a user message it arrives as
something the user supposedly said, next to instructions about how to read it.
The two agents then have to be kept in step by hand — same task, same rules,
two prompts — and the pair is invisible to anything reading the trace, which
sees two unrelated runs.

A :class:`Subagent` is that cycle as a declaration. It owns one agent, feeds
tool results back as :class:`~aiwatcher_agentic.message.ToolMessage` turns, and
returns one result to its caller.

**The last step withholds the tools.** A request carries either a tool list or a
response format — a grammar the answer must match — and a model given both can
only emit the schema, so it could never ask for the tool it was offered. Rather
than leave that as a trap, the final step is defined as the answering one: tools
come off, the schema goes on, and a subagent therefore always terminates with an
answer instead of a dangling tool call. ``max_steps`` counts model requests, so
the Scout case — search once, then extract — is ``max_steps=2``.

A tool that fails is reported back to the model while a tool-capable step
remains, because an error it can read is an error it can retry with different
arguments. On the last such step there is nothing left to retry *with* — the
next step withholds the tools — so the failure is raised as
:class:`SubagentToolError` rather than handed to a model that could only answer
around it, which is the one thing a sourced answer must never do.

What stays with the caller is what a *successful* tool result means: whether an
empty search counts as an answer, how the result is validated, what an
unsearched question is worth. :attr:`SubagentResult.tool_runs` is there to be
checked, and this module never decides that on the application's behalf.
"""

from __future__ import annotations

from collections.abc import Callable, Sequence
from dataclasses import dataclass, field, replace
from typing import Any

from aiwatcher_agentic.agent import Agent, Context
from aiwatcher_agentic.capabilities import AbstractCapability
from aiwatcher_agentic.exceptions import DEFAULT_RETRY, ModelResponseError, RetryPolicy
from aiwatcher_agentic.message import AssistantMessage, Message, ToolMessage, UserMessage
from aiwatcher_agentic.model import ModelSource
from aiwatcher_agentic.prompts import PromptBuilder
from aiwatcher_agentic.structured_output import StructuredOutputProto
from aiwatcher_agentic.tools import Tool, ToolCall, Toolset, Toolsets
from aiwatcher_agentic.tracer import LLMTracer
from aiwatcher_agentic.usage import RunUsage, UsageLimits

__all__ = ["Subagent", "SubagentResult", "SubagentToolError", "ToolRun"]


@dataclass(frozen=True, slots=True)
class ToolRun:
    """One tool the subagent asked for, and what came back from it."""

    name: str
    output: str
    succeeded: bool


class SubagentToolError(RuntimeError):
    """A tool failed on the last step that could still have called one."""

    def __init__(self, subagent: str, run: ToolRun) -> None:
        self.run = run
        super().__init__(f"subagent {subagent!r} could not use {run.name!r}: {run.output}")


@dataclass(frozen=True, slots=True)
class SubagentResult[T]:
    """What a subagent hands back to whoever ran it.

    `content` is the answer from the final step — parsed by the structured
    output when there is one, and the response text when there is not.
    """

    content: T
    tool_runs: tuple[ToolRun, ...] = ()
    usage: RunUsage = field(default_factory=RunUsage)
    #: Model requests actually made, which is at most ``max_steps`` and fewer
    #: when the subagent stopped asking for tools early.
    steps: int = 0


class Subagent[T]:
    """A sub-task with its own prompt, model, toolset and context."""

    def __init__(
        self,
        name: str,
        model_provider: ModelSource,
        *,
        prompt_builder: PromptBuilder,
        tools: Sequence[Callable[..., Any] | Tool] | None = None,
        toolsets: Toolsets | Sequence[Toolset] | None = None,
        structured_output: StructuredOutputProto | None = None,
        max_steps: int = 2,
        context: Context | None = None,
        tracer: LLMTracer | None = None,
        retry_policy: RetryPolicy = DEFAULT_RETRY,
        usage_limits: UsageLimits | None = None,
        capabilities: Sequence[AbstractCapability] | None = None,
    ) -> None:
        if max_steps < 1:
            raise ValueError(f"subagent {name!r} needs at least one step to answer in.")
        if max_steps < 2 and (tools or toolsets):
            raise ValueError(
                f"subagent {name!r} has tools and max_steps={max_steps}. The last step is the "
                "answering one and withholds them, so there is no step left that could call a "
                "tool. Give it at least 2, or declare it without tools."
            )
        self._name = name
        self._max_steps = max_steps
        self._tracer = tracer
        # History off by default: a subagent is one sub-task, and what it needs
        # to see is the task and its own tool results, not a conversation it was
        # never part of. Isolation is the point of running one.
        self._context = context or Context(add_history_to_context=False)
        self._agent = Agent(
            model_provider,
            prompt_builder=prompt_builder,
            context=self._context,
            tools=tools,
            toolsets=toolsets,
            structured_output=structured_output,
            tracer=tracer,
            usage_limits=usage_limits,
            retry_policy=retry_policy,
            capabilities=capabilities,
        )

    @property
    def name(self) -> str:
        return self._name

    @property
    def agent(self) -> Agent:
        """The agent underneath, for a caller that needs its toolset or history."""
        return self._agent

    def run(self, task: str | Message) -> SubagentResult[T]:
        """Run the sub-task to an answer.

        A capability that answered `Ask` suspends this the same way it suspends
        a bare agent: `ToolApprovalRequired` leaves through here, with the turn
        lock released. Catching it to keep going would mean answering from a
        tool result nobody produced.
        """
        usage = RunUsage()
        turns: list[Message] = []
        tool_runs: list[ToolRun] = []
        message: str | Message = task
        steps = 0
        # One turn of this agent is the whole sub-task, not one model request:
        # a second caller landing between the tool step and the answering step
        # would answer from somebody else's tool result.
        with self._agent.turn_lock:
            # Counts down the steps that may still call a tool: what a failure
            # means depends on whether another attempt could follow it.
            for retries_left in reversed(range(self._max_steps - 1)):
                steps += 1
                result = self._agent.run(message, self._context, history=turns)
                self._absorb(usage, self._agent.run_usage)
                if not result.tool_calls:
                    # It answered instead of reaching for a tool. Nothing to feed
                    # back, so the answering step below asks the same question
                    # again — this time with the schema on.
                    break
                turns.append(message if isinstance(message, Message) else UserMessage(str(message)))
                turns.append(AssistantMessage(result.response_text))
                output, failed = self._call_tools(result.tool_calls, tool_runs)
                if failed is not None and not retries_left:
                    raise SubagentToolError(self._name, failed)
                message = ToolMessage(output)
            steps += 1
            answer = self._agent.run(
                message, replace(self._context, offer_tools=False), history=turns
            )
            self._absorb(usage, self._agent.run_usage)
        return SubagentResult(
            content=answer.content,
            tool_runs=tuple(tool_runs),
            usage=usage,
            steps=steps,
        )

    def _call_tools(
        self, calls: Sequence[ToolCall], recorded: list[ToolRun]
    ) -> tuple[str, ToolRun | None]:
        outputs: list[str] = []
        failed: ToolRun | None = None
        for call in calls:
            ran = self._agent.run_tool(call, tracer=self._tracer)
            if ran is None:
                raise ModelResponseError(
                    f"subagent {self._name!r} asked for {call[0]!r}, which its toolset could "
                    "neither run nor name as missing."
                )
            run = ToolRun(name=ran.tool_call[0], output=ran.output, succeeded=ran.success)
            recorded.append(run)
            outputs.append(ran.output)
            failed = failed or (None if run.succeeded else run)
        return "\n".join(outputs), failed

    @staticmethod
    def _absorb(total: RunUsage, step: RunUsage) -> None:
        """Roll one step's usage into the run's.

        `Agent.run_usage` is reset at the start of every run, so a subagent that
        made three requests would otherwise report the last one's cost as if it
        were the whole sub-task's.
        """
        for request in step.request_usages:
            total.add(request)
        total.add_tool_calls(step.tool_calls)
