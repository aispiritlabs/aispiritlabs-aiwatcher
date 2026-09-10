"""Composable capability system with lifecycle hooks.

Capabilities are cross-cutting concerns that can intercept and modify
the agent execution pipeline. They compose via ``CombinedCapability``.

Example::

    class LoggingCapability(AbstractCapability):
        def before_model_request(self, prompt: Any, context: HookContext) -> Any:
            print(f"Sending prompt: {prompt!r:.100}")
            return prompt

        def after_model_request(self, response: str, context: HookContext) -> str:
            print(f"Got response: {response!r:.100}")
            return response

    agent = Agent(
        model_provider=provider,
        prompt_builder=builder,
        capabilities=[LoggingCapability()],
    )
"""

from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass, field
from typing import Any


@dataclass(frozen=True, slots=True)
class HookContext:
    """Context passed to capability hooks."""

    agent_id: str = ""
    run_id: str = ""
    turn: int = 0
    metadata: dict[str, Any] = field(default_factory=dict)


class AbstractCapability:
    """Base class for composable capabilities.

    Override any hook method to intercept the agent execution pipeline.
    All hooks have pass-through defaults so you only override what you need.
    """

    def before_model_request(self, prompt: Any, context: HookContext) -> Any:
        """Called before sending a prompt to the model. Return modified prompt."""
        return prompt

    def after_model_request(self, response: str, context: HookContext) -> str:
        """Called after receiving a model response. Return modified response."""
        return response

    def before_tool_execute(
        self, tool_name: str, parameters: dict[str, Any], context: HookContext
    ) -> dict[str, Any]:
        """Called before executing a tool. Return modified parameters."""
        return parameters

    def after_tool_execute(self, tool_name: str, result: str, context: HookContext) -> str:
        """Called after tool execution. Return modified result."""
        return result

    def get_instructions(self, context: HookContext) -> str | None:
        """Return additional instructions to inject into the system prompt."""
        return None

    def on_error(self, error: Exception, context: HookContext) -> None:
        """Called when an error occurs during agent execution."""


class CombinedCapability(AbstractCapability):
    """Composes multiple capabilities into a single capability.

    Hooks are called in order for before_* hooks, and in reverse order
    for after_* hooks (middleware pattern).
    """

    def __init__(self, capabilities: Sequence[AbstractCapability]) -> None:
        self._capabilities = list(capabilities)

    @property
    def capabilities(self) -> list[AbstractCapability]:
        return list(self._capabilities)

    def before_model_request(self, prompt: Any, context: HookContext) -> Any:
        for cap in self._capabilities:
            prompt = cap.before_model_request(prompt, context)
        return prompt

    def after_model_request(self, response: str, context: HookContext) -> str:
        for cap in reversed(self._capabilities):
            response = cap.after_model_request(response, context)
        return response

    def before_tool_execute(
        self, tool_name: str, parameters: dict[str, Any], context: HookContext
    ) -> dict[str, Any]:
        for cap in self._capabilities:
            parameters = cap.before_tool_execute(tool_name, parameters, context)
        return parameters

    def after_tool_execute(self, tool_name: str, result: str, context: HookContext) -> str:
        for cap in reversed(self._capabilities):
            result = cap.after_tool_execute(tool_name, result, context)
        return result

    def get_instructions(self, context: HookContext) -> str | None:
        parts = []
        for cap in self._capabilities:
            instruction = cap.get_instructions(context)
            if instruction:
                parts.append(instruction)
        return "\n".join(parts) if parts else None

    def on_error(self, error: Exception, context: HookContext) -> None:
        for cap in self._capabilities:
            cap.on_error(error, context)
