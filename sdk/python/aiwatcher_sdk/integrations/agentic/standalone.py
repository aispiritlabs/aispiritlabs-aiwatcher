"""One agent, registered as a workflow of its own — no graph around it.

A graph is how several agents are composed. Some agents are not composed with
anything: a digest that runs at nine, a triage agent somebody starts from the
panel. For those, a graph is a document written only to say "this one node",
and ADR_0025 already has the thing they need — a registered
`WorkflowDefinition` that `POST /api/v1/executions` starts and a schedule
fires, compiled by the same `executions::compile_head` either way. So an agent
becomes a workflow of one step, of the kind `workflow`, and there is no third
definition kind for it: `DefinitionKind::Workflow` already reads "a declared
agent, search or ML workflow", and a third kind would be a `match` arm on each
side of the schedule guardrail.

## What an agent is, here

A turn: text in, text out. The SDK imports nothing of the agent's packages —
the rule every integration here keeps — so it takes a callable rather than an
`AgenticWorkflow`, and the application adapts its own agent to it. That
adapter is where a user message is built with the metadata the agent's own
runtime expects, and it lives with that runtime.

## Where the words go

The reply is a completion, and a completion is conversation content: it goes
to a `PayloadStore` under §40.4's `external` policy, and the step's result —
which lands on the execution's stream and in its projection — carries the
reference, a digest and a size. The same rule the durable join keeps for a
hop, for the same reason.

The *message* arrives as a run parameter, because that is what a panel form and
a schedule can send. A turn a person typed is therefore in the stream the way
any run parameter is; an agent that answers people rather than instructions
should be started with a reference, which is the next thing this module would
grow.

## Why a turn is tried once

An agent's turn calls tools, and a tool writes — a note added, a message sent.
The server re-runs a failed attempt from the beginning, so a retry would do
those writes again. `max_attempts=1` is the default for that reason, and a
caller whose agent only reads opts into more. A worker that disappears is
still recovered: that is the lease expiring, not a retry of a failure.
"""

from __future__ import annotations

from collections.abc import Callable

from aiwatcher_sdk.task import Task
from aiwatcher_sdk.task_errors import TaskError
from aiwatcher_sdk.workflow import RetryPolicy, Workflow, WorkflowStep

from .payloads import PayloadStore, digest_of, encode_payload

__all__ = ["TURN", "Respond", "agent_workflow"]

#: The one step's id. A name rather than the agent's, so every standalone
#: agent's run reads the same in the Workflows view: one node, called a turn.
TURN = "turn"

#: One turn of an agent: the message in, the reply out.
Respond = Callable[[str], str]


def agent_workflow(
    name: str,
    respond: Respond,
    *,
    version: str,
    payloads: PayloadStore,
    default_message: str | None = None,
    timeout_seconds: int = 600,
    retry: RetryPolicy | None = None,
) -> Workflow:
    """One agent as a `Workflow` a `Runtime` registers, hosts and starts.

    ``default_message`` is what a run with no ``message`` parameter answers —
    a schedule sends none, so an agent that runs at nine is registered with the
    instruction it runs on. It is part of the definition, so changing it is a
    new revision, and a run's own ``message`` always wins over it.
    """

    def turn(message: str | None = None, default_message: str | None = None) -> dict[str, object]:
        text = message if message is not None else default_message
        if not isinstance(text, str) or not text.strip():
            raise TaskError(
                f"`{name}` was started with no message: pass `message` in the run's "
                "parameters, or register the agent with a `default_message` for its schedule",
                classification="validation",
            )
        reply = respond(text)
        if not isinstance(reply, str):
            raise TaskError(
                f"`{name}` answered with {type(reply).__name__}, not text",
                classification="user_code",
            )
        digest = digest_of(reply)
        return {
            "agent": name,
            "reply": {
                "reference": payloads.store_payload(digest, reply),
                "digest": digest,
                "size": len(encode_payload(reply)),
            },
        }

    step = WorkflowStep(
        TURN,
        Task(turn, name, version),
        params={} if default_message is None else {"default_message": default_message},
        retry=retry or RetryPolicy(max_attempts=1),
        timeout_seconds=timeout_seconds,
    )
    return Workflow(name, version, (step,))
