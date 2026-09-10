"""Technical routing — maps commands to Reactors.

Decides HOW a command is executed (which Reactor handles it).
"""

from __future__ import annotations

from aiwatcher_agentic.workflow.messages import Message, UserMessage
from aiwatcher_agentic.workflow.reactor import Reactor, TechnicalRoutingFn


def make_llm_routing(reactor: Reactor) -> TechnicalRoutingFn:
    """Simple routing: UserMessage goes to the LLM reactor, everything else is ignored.

    Used by all workflows — the MessageRouter decides WHAT, routing decides HOW.
    """

    def routing(command: Message) -> Reactor | None:
        if isinstance(command, UserMessage):
            return reactor
        return None

    return routing
