"""One agent, registered as a workflow of its own — no graph around it.

A graph is how several agents are composed. Some agents are not composed with
anything: an agent that drafts answers over a list of questions at nine, a
triage agent somebody starts from the panel. For those, a graph is a document
written only to say "this one node", and ADR_0025 already has the thing they
need — a registered `WorkflowDefinition` that `POST /api/v1/executions` starts
and a schedule fires, compiled by the same `executions::compile_head` either
way. So an agent becomes a workflow of one step, of the kind `workflow`, and
there is no third definition kind for it: `DefinitionKind::Workflow` already
reads "a declared agent, search or ML workflow", and a third kind would be a
`match` arm on each side of the schedule guardrail.

## What an agent is, here

A turn: text in, text out. The SDK imports nothing of the agent's packages —
the rule every integration here keeps — so it takes a callable rather than an
`AgenticWorkflow`, and the application adapts its own agent to it. That
adapter is where a user message is built with the metadata the agent's own
runtime expects, and it lives with that runtime.

## Where the words go

The reply is a completion, and a completion is conversation content. It goes to
a `PayloadStore` under §40.4's `external` policy, and the step's result — which
lands on the execution's stream and in its projection — carries the reference,
a digest and a size. The same rule the durable join keeps for a hop.

## What a turn is for: training data

Hand the workflow a `ConversationArchive` and every turn is also recorded
there, as one exchange — the message and the reply, joined to the run by
`run_id`, `trace_id` and `span_id`. That is what makes a hosted agent a way of
*preparing fine-tuning data* rather than only of answering: the archive redacts
in this process before anything leaves it, holds the consent and retention that
permit keeping the exchange, puts it in front of a reviewer, and freezes what
was approved into a corpus in the shape a trainer reads — `sft`,
`prompt_response`, `dpo`. ADR_0021 is all of that; this module only feeds it.

The exchange is written **before** the step reports, so a completed turn is one
whose exchange is in the archive: the receipt after the data, as everywhere
else. An archive that cannot be reached fails the turn as `infrastructure`,
which the server retries. One that refuses it — no archive on this instance, or
content its policy rejects — fails it as `policy`, which it does not, because
the next attempt would be refused the same way and a person has to decide.

## At least once, and under control

The server retries an attempt that may not have finished — a timeout, a lost
worker, an unreachable dependency — and this step takes the default budget for
that, so a turn is delivered at least once. What keeps "at least once" from
becoming "twice":

* every record of the turn is filed under the **step**, not the attempt. The
  archive's message ids are the execution and the step, so a retried turn
  overwrites the exchange it wrote rather than adding a second one, and a reply
  that changed goes back to review — the archive's own rule;
* `TaskContext.step_key` is the same on every attempt, and a tool that writes
  keys its write by it (`agentic_runtime.hosted` hands it to the agent as the
  message's idempotency key);
* ``retry=`` sets the budget per agent — `RetryPolicy(max_attempts=1)` for an
  agent whose tools cannot be made idempotent;
* a failure in the agent's own code is `user_code`, which the server never
  retries by itself: the run stops on that step and a person retries it from
  the panel once the cause is fixed. A turn that knows its failure is worth
  another go raises `TaskError(..., classification="timeout")` and gets one.

## The message

The message arrives as a run parameter, because that is what a panel form and a
schedule can send. For an agent preparing data it is an instruction, not
somebody's turn in a conversation; the archive, not the stream, is where the
exchange is kept as content.
"""

from __future__ import annotations

import uuid
from collections.abc import Callable

from aiwatcher_sdk.api import ApiError
from aiwatcher_sdk.conversations import ConversationArchive, Turn
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
    archive: ConversationArchive | None = None,
    default_message: str | None = None,
    timeout_seconds: int = 600,
    retry: RetryPolicy | None = None,
) -> Workflow:
    """One agent as a `Workflow` a `Runtime` registers, hosts and starts.

    ``archive`` records every turn as an exchange a reviewer can approve into a
    fine-tuning corpus. ``retry`` is the budget for attempts that may not have
    finished — the server's default unless given.

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
        if archive is not None:
            _record(archive, name, text, reply)
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
        retry=retry or RetryPolicy(),
        timeout_seconds=timeout_seconds,
    )
    return Workflow(name, version, (step,))


def _record(archive: ConversationArchive, agent: str, message: str, reply: str) -> None:
    """File one turn in the archive, under ids every attempt of its step shares."""
    # Imported here: the worker imports this package for its tracer, so a
    # module-level import would be a cycle.
    from aiwatcher_sdk.worker.context import current_context

    attempt = current_context.get()
    if attempt is None:
        # Outside a worker — a test, a shell — a turn is still a turn. It has no
        # run to be joined to, and nothing will retry it.
        conversation, step = f"{agent}-{uuid.uuid4().hex}", TURN
        provenance = {"agent_id": agent}
    else:
        assignment = attempt.assignment
        conversation, step = assignment.execution_id, assignment.step_id
        provenance = {
            "run_id": assignment.execution_id,
            "agent_id": agent,
            "trace_id": assignment.trace_id,
            "span_id": assignment.parent_span_id,
        }
    provenance = {key: value for key, value in provenance.items() if value}
    # Dots rather than the step key's slash: an archive id is a segment of the
    # key its object is stored under.
    asked = f"{conversation}.{step}.user"
    try:
        archive.record_turns(
            [
                Turn(
                    conversation_id=conversation,
                    message_id=asked,
                    role="user",
                    parts=[{"kind": "text", "text": message}],
                    provenance=provenance,
                ),
                Turn(
                    conversation_id=conversation,
                    message_id=f"{conversation}.{step}.assistant",
                    role="assistant",
                    parts=[{"kind": "text", "text": reply}],
                    parent_message_id=asked,
                    ordinal=1,
                    provenance=provenance,
                ),
            ]
        )
    except ApiError as error:
        refused = error.code == "registry_disabled" or (
            error.status is not None and 400 <= error.status < 500
        )
        raise TaskError(
            f"`{agent}` answered and the conversation archive "
            f"{'refused the exchange' if refused else 'could not be reached'}: {error}",
            classification="policy" if refused else "infrastructure",
        ) from error
