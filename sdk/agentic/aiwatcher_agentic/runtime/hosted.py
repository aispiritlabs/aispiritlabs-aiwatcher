"""Host an application's agents on aiwatcher, each one a workflow of its own.

`AgenticRuntime` is where an application composes its agents — the router,
the workflows, the stores, the tracer — and `aiwatcher_sdk.runtime.Runtime` is
where a process hosts the work aiwatcher hands it. They were two composition
roots for one application. This is the seam that makes them one: the
application builds its agentic runtime exactly as it does for chat, and
:func:`hosted_runtime` hands it to the SDK's runtime, which registers every
agent in it as a workflow (`aiwatcher_sdk.integrations.agentic.agent_workflow`)
that the panel starts, a schedule fires and a worker in this process answers —
and which closes it when it closes.

    aiwatcher-runtime --factory my_app.hosted:build

## One agent, no graph

Each agent is registered on its own, as a workflow of one step. A graph is how
agents are composed; an agent that is not composed with anything needs no graph
document to run, and has none. `agentic_graph` is not imported here.

## Training data, at least once

Handed a `ConversationArchive`, every turn is recorded there as an exchange a
reviewer can approve into a fine-tuning corpus — the reason a hosted agent
exists as often as not. A turn is delivered at least once: the server retries an
attempt that may not have finished. What keeps that from doing a tool's write
twice is the message's idempotency key, which is the attempt's *step* key — the
same on every attempt of the turn — so a tool that keys its writes by it does
them once. ``retry`` sets the budget per agent, down to one attempt for an agent
whose tools cannot be keyed.

## What a turn is

The turn goes through `AgenticRuntime.handle` as a message *targeted* at the
agent, so it skips the router and runs through the same turn executor chat
does — the same lifecycle records, the same tracer, the same stores. Its turn id
is the attempt's context id and its correlation id the execution's, which is
what joins the agent's own records to the run that asked for them; its
idempotency key is the step's, which is what a retried turn shares with the
attempt before it.

## Capacity

One slot by default. `AgenticRuntime` keeps SQLite stores and an in-process bus
written for one conversation at a time, and nothing has shown them safe under
concurrent turns; raising `concurrency` is a claim about them that this module
does not make.
"""

from __future__ import annotations

import os
from collections.abc import Callable, Mapping, Sequence
from pathlib import Path
from typing import TYPE_CHECKING

from aiwatcher_agentic.workflow.messages import (
    ConversationData,
    RecordedMessageMetadata,
    UserMessage,
)

if TYPE_CHECKING:
    import httpx
    from aiwatcher_sdk import AiwatcherClient
    from aiwatcher_sdk.conversations import ConversationArchive
    from aiwatcher_sdk.integrations.agentic import PayloadStore
    from aiwatcher_sdk.runtime import Runtime
    from aiwatcher_sdk.workflow import RetryPolicy

    from aiwatcher_agentic.runtime.runtime import AgenticRuntime

__all__ = ["DEFAULT_PAYLOAD_ROOT", "POOL", "QUEUE", "hosted_runtime", "payload_root"]

#: Where a payload lands when the environment names no directory. Beside the
#: agent's own data rather than in a temporary directory, because a payload the
#: operating system may delete is a reply nobody can read back on a Tuesday.
DEFAULT_PAYLOAD_ROOT = Path(".data") / "aiwatcher-payloads"

#: The queue every hosted agent is claimed from, unless the caller names one.
QUEUE = "agents"

#: The one local pool. Placement is not a choice here: every agent is a turn,
#: and one runtime's turns share one set of stores.
POOL = "agents"


def payload_root() -> Path:
    """The directory content is written to under the `external` policy."""
    named = os.environ.get("AIWATCHER_PAYLOAD_ROOT", "").strip()
    return Path(named) if named else DEFAULT_PAYLOAD_ROOT


def hosted_runtime(
    runtime: AgenticRuntime,
    *,
    version: str,
    agents: Sequence[str] | None = None,
    default_messages: Mapping[str, str] | None = None,
    name: str = "ai-spirit-agent",
    url: str | None = None,
    token: str | None = None,
    queue: str = QUEUE,
    concurrency: int = 1,
    payloads: PayloadStore | None = None,
    archive: ConversationArchive | None = None,
    retry: RetryPolicy | Mapping[str, RetryPolicy] | None = None,
    client: httpx.Client | None = None,
    telemetry: AiwatcherClient | None = None,
) -> Runtime:
    """The SDK runtime that hosts ``runtime``'s agents, and owns it from now on.

    ``agents`` narrows which are registered — every one by default.
    ``default_messages`` is what a scheduled run of each answers, since a
    schedule starts a run with no parameters. ``version`` is the release label
    each definition carries; the revision aiwatcher pins is its content.

    ``archive`` records every turn for review and export as training data.
    ``retry`` is one policy for every agent or a policy per agent; an agent it
    does not name keeps the server's default. The hosted runtime owns both
    ``runtime`` and ``archive`` from here and closes them when it closes.

    ``url`` and ``token`` default to ``AIWATCHER_URL`` and ``AIWATCHER_TOKEN``,
    and a missing URL is refused: hosting an agent on nothing is a worker that
    polls forever. Replies go to ``payloads``, a directory under
    ``AIWATCHER_PAYLOAD_ROOT`` unless one is given.
    """
    from aiwatcher_sdk.integrations.agentic import FilePayloadStore, agent_workflow
    from aiwatcher_sdk.runtime import ExecutionPool, Runtime

    try:
        chosen = list(runtime.workflows) if agents is None else list(agents)
        if unknown := sorted(set(chosen) - set(runtime.workflows)):
            raise ValueError(
                f"no such agent in this runtime: {', '.join(unknown)} "
                f"(it has {', '.join(sorted(runtime.workflows)) or 'none'})"
            )
        defaults = dict(default_messages or {})
        if stray := sorted(set(defaults) - set(chosen)):
            raise ValueError(f"a default message for an agent not hosted: {', '.join(stray)}")
        budgets = retry if isinstance(retry, Mapping) else dict.fromkeys(chosen, retry)
        if stray := sorted(set(budgets) - set(chosen)):
            raise ValueError(f"a retry policy for an agent not hosted: {', '.join(stray)}")
        address = (url or os.environ.get("AIWATCHER_URL", "")).strip()
        if not address:
            raise ValueError("hosting agents on aiwatcher needs AIWATCHER_URL, or url=")
    except BaseException:
        # Handed over and refused: nobody else is going to close them.
        _close(runtime, archive)
        raise

    store = payloads if payloads is not None else FilePayloadStore(payload_root() / "turns")
    workflows = [
        agent_workflow(
            agent,
            _responder(runtime, agent),
            version=version,
            payloads=store,
            archive=archive,
            default_message=defaults.get(agent),
            retry=budgets.get(agent),
        )
        for agent in chosen
    ]
    return Runtime(
        name=name,
        url=address,
        token=token if token is not None else (os.environ.get("AIWATCHER_TOKEN") or None),
        workflows=workflows,
        pools=[ExecutionPool(POOL, queue, concurrency)],
        placement={workflow.ref: POOL for workflow in workflows},
        client=client,
        telemetry=telemetry,
        on_close=lambda: _close(runtime, archive),
    )


def _responder(runtime: AgenticRuntime, agent: str) -> Callable[[str], str]:
    def respond(text: str) -> str:
        turn_id, correlation_id, idempotency_key = _attempt()
        return runtime.handle(
            UserMessage(
                data=ConversationData(role="user", text=text),
                metadata=RecordedMessageMetadata(
                    runtime_id=runtime.runtime_id,
                    session_id=runtime.session_id,
                    turn_id=turn_id,
                    correlation_id=correlation_id,
                    idempotency_key=idempotency_key,
                    domain=agent,
                    source="aiwatcher",
                    target=agent,
                ),
            )
        )

    return respond


def _attempt() -> tuple[str, str, str]:
    """The attempt's context id, its execution and its step key.

    Outside a worker there is none of the three, and the runtime mints a turn id
    as it does for chat — a turn is still a turn when a test or a shell calls it
    directly.
    """
    from aiwatcher_sdk.worker import get_task_context

    try:
        context = get_task_context()
    except RuntimeError:
        return "", "", ""
    return context.context_id, context.run_id, context.step_key


def _close(runtime: AgenticRuntime, archive: ConversationArchive | None) -> None:
    """Close both, and lose neither failure to the other."""
    errors: list[Exception] = []
    for close in (runtime.close, archive.close if archive is not None else None):
        if close is None:
            continue
        try:
            close()
        except Exception as error:  # noqa: BLE001 - collected, so the other still closes
            errors.append(error)
    if errors:
        raise ExceptionGroup("closing the hosted agents failed", errors)
