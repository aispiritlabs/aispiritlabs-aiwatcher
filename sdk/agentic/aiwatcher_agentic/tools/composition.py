"""Composable toolset wrappers for filtering, prefixing, and approval.

Example::

    from aiwatcher_agentic.tools import Toolset, tool
    from aiwatcher_agentic.tools.composition import FilteredToolset, PrefixedToolset

    @tool
    def search(query: str) -> str:
        ...

    @tool
    def delete_note(note_id: str) -> str:
        ...

    base = Toolset([search, delete_note])

    # Only expose search tool
    filtered = FilteredToolset(base, allow={"search"})

    # Prefix all tool names with "notes_"
    prefixed = PrefixedToolset(base, prefix="notes_")

    # Compose fluently
    composed = FilteredToolset(
        PrefixedToolset(base, prefix="notes_"),
        allow={"notes_search"},
    )
"""

from __future__ import annotations

from collections.abc import Callable
from typing import Any

from ._tools import Tool, ToolContext


class WrapperToolset:
    """Base class for toolset wrappers that delegate to an inner toolset."""

    def __init__(self, inner: Any) -> None:
        self._inner = inner

    @property
    def tools(self) -> tuple[Tool, ...]:
        tools: tuple[Tool, ...] = self._inner.tools
        return tools

    def has_tool(self, function_name: str) -> bool:
        return bool(self._inner.has_tool(function_name))

    def execute(
        self,
        function_name: str,
        parameters: dict[str, Any],
        *,
        tool_context: ToolContext | None = None,
    ) -> Any:
        return self._inner.execute(function_name, parameters, tool_context=tool_context)


class FilteredToolset(WrapperToolset):
    """Exposes only a subset of tools from the inner toolset.

    Use ``allow`` to whitelist tool names, or ``deny`` to blacklist.
    """

    def __init__(
        self,
        inner: Any,
        *,
        allow: set[str] | None = None,
        deny: set[str] | None = None,
        filter_fn: Callable[[str], bool] | None = None,
    ) -> None:
        super().__init__(inner)
        self._allow = allow
        self._deny = deny or set()
        self._filter_fn = filter_fn

    def _is_allowed(self, name: str) -> bool:
        if self._filter_fn is not None:
            return self._filter_fn(name)
        if self._allow is not None:
            return name in self._allow
        return name not in self._deny

    @property
    def tools(self) -> tuple[Tool, ...]:
        return tuple(t for t in self._inner.tools if self._is_allowed(t.name))

    def has_tool(self, function_name: str) -> bool:
        return self._is_allowed(function_name) and self._inner.has_tool(function_name)

    def execute(
        self,
        function_name: str,
        parameters: dict[str, Any],
        *,
        tool_context: ToolContext | None = None,
    ) -> Any:
        if not self._is_allowed(function_name):
            raise ValueError(f"Tool '{function_name}' is not allowed by filter")
        return self._inner.execute(function_name, parameters, tool_context=tool_context)


class PrefixedToolset(WrapperToolset):
    """Adds a prefix to all tool names from the inner toolset.

    Useful for namespacing tools from different agents.
    """

    def __init__(self, inner: Any, prefix: str) -> None:
        super().__init__(inner)
        self._prefix = prefix
        self._prefixed_tools = tuple(_PrefixedTool(t, prefix) for t in inner.tools)

    @property
    def tools(self) -> tuple[Tool, ...]:
        return self._prefixed_tools  # type: ignore[return-value]

    def has_tool(self, function_name: str) -> bool:
        if function_name.startswith(self._prefix):
            original = function_name[len(self._prefix) :]
            return bool(self._inner.has_tool(original))
        return False

    def execute(
        self,
        function_name: str,
        parameters: dict[str, Any],
        *,
        tool_context: ToolContext | None = None,
    ) -> Any:
        if function_name.startswith(self._prefix):
            original = function_name[len(self._prefix) :]
            return self._inner.execute(original, parameters, tool_context=tool_context)
        raise ValueError(f"Tool '{function_name}' does not match prefix '{self._prefix}'")


class _PrefixedTool:
    """Wrapper that presents a Tool with a prefixed name."""

    def __init__(self, tool: Tool, prefix: str) -> None:
        self._tool = tool
        self._prefix = prefix

    @property
    def name(self) -> str:
        return f"{self._prefix}{self._tool.name}"

    @property
    def doc(self) -> str:
        return self._tool.doc

    @property
    def args(self) -> list[dict[str, Any]]:
        return self._tool.args

    @property
    def required_parameters(self) -> set[str]:
        return self._tool.required_parameters

    @property
    def all_parameters(self) -> set[str]:
        return self._tool.all_parameters

    @property
    def command_class(self) -> Any:
        return self._tool.command_class

    def validate_types(self, parameters: dict[str, Any]) -> list[str]:
        return self._tool.validate_types(parameters)

    def call(self, parameters: dict[str, Any], *, tool_context: ToolContext | None = None) -> Any:
        return self._tool.call(parameters, tool_context=tool_context)

    async def acall(
        self, parameters: dict[str, Any], *, tool_context: ToolContext | None = None
    ) -> Any:
        return await self._tool.acall(parameters, tool_context=tool_context)

    def create_command(self, parameters: dict[str, Any]) -> Any:
        return self._tool.create_command(parameters)
