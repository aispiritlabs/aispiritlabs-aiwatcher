from __future__ import annotations

from collections.abc import Callable, Sequence
from dataclasses import asdict, dataclass
from enum import StrEnum
from typing import Any, overload

import structlog

from aiwatcher_agentic.capabilities import AbstractCapability, Allow, Deny, HookContext
from aiwatcher_agentic.exceptions import ModelRetry, ToolValidationError
from aiwatcher_agentic.tracer import LLMTracer, NoopLLMTracer
from aiwatcher_agentic.workflow.trace import TraceSnapshot
from aiwatcher_agentic.workflow.tracer import SpanHandle

from ._tools import (
    Command,
    JsonRepairer,
    Tool,
    ToolCall,
    ToolCallCommand,
    ToolContext,
    ToolFailure,
)

logger = structlog.get_logger(__name__)

#: How a failed tool result used to be recognised, and still is for a tool that
#: returns a plain string. A tool that knows it failed says so with
#: `ToolFailure`; these prefixes only read back strings this SDK did not author,
#: which is also why `runtime.fine_tuning` still selects records by them.
TOOL_ERROR_PREFIXES: tuple[str, ...] = ("Błąd:", "Error:")


class ToolRunStatus(StrEnum):
    SUCCESS = "success"
    ERROR = "error"
    RETRY = "retry"
    #: A capability refused the call. The tool never ran.
    DENIED = "denied"


#: How each status annotates the span: the level it reports, and the key its
#: message rides under. RETRY is a warning because the attempt can still succeed.
_SPAN_ANNOTATION: dict[ToolRunStatus, tuple[str | None, str]] = {
    ToolRunStatus.SUCCESS: (None, "output"),
    ToolRunStatus.ERROR: ("ERROR", "error"),
    ToolRunStatus.DENIED: ("ERROR", "error"),
    ToolRunStatus.RETRY: ("WARNING", "retry"),
}


@dataclass(frozen=True)
class ToolRunResult:
    tool_call: ToolCall
    output: str
    trace: TraceSnapshot | None = None
    retry: bool = False
    status: ToolRunStatus = ToolRunStatus.SUCCESS

    @property
    def success(self) -> bool:
        return self.status == ToolRunStatus.SUCCESS and not self.retry


class Toolset:
    def __init__(self, tools: Sequence[Callable[..., Any] | Tool]):
        self._tools: list[Tool] = []
        for tool in tools:
            if isinstance(tool, Tool):
                self._tools.append(tool)
            elif callable(tool):
                self._tools.append(Tool(tool))
            else:
                raise ValueError(f"Expected Tool or callable, got {type(tool)}")

    def execute(
        self,
        function_name: str,
        parameters: dict[str, Any],
        *,
        tool_context: ToolContext | None = None,
    ) -> Any:
        for tool in self._tools:
            if tool.name == function_name:
                self._validate_parameters(tool, function_name, parameters)
                return tool.call(parameters, tool_context=tool_context)
        raise ValueError(f"Tool '{function_name}' not found in toolset")

    @staticmethod
    def _validate_parameters(tool: Tool, function_name: str, parameters: dict[str, Any]) -> None:
        provided = set(parameters.keys())
        missing = sorted(tool.required_parameters - provided)
        unexpected = sorted(provided - tool.all_parameters)

        errors: list[str] = []

        if missing:
            errors.append(f"missing required parameters: {', '.join(missing)}")

        if unexpected:
            errors.append(f"unexpected parameters: {', '.join(unexpected)}")

        type_errors = tool.validate_types(parameters)
        if type_errors:
            errors.extend(type_errors)

        if errors:
            detail = "; ".join(errors)
            raise ToolValidationError(f"Tool '{function_name}' validation failed: {detail}")

    def has_tool(self, function_name: str) -> bool:
        return any(tool.name == function_name for tool in self._tools)

    @property
    def tools(self) -> tuple[Tool, ...]:
        return tuple(self._tools)


class Toolsets(Sequence[Toolset]):
    def __init__(
        self,
        toolsets: Sequence[Toolset] | None = None,
        *,
        json_repairer: JsonRepairer | None = None,
    ):
        self._toolsets = list(toolsets or [])
        self._json_repairer = json_repairer
        self._validate_unique_tool_names()
        self._tool_names = tuple(tool.name for toolset in self._toolsets for tool in toolset.tools)
        self._command_to_tool: dict[type[Command], str] = {}
        for toolset in self._toolsets:
            for tool_def in toolset.tools:
                if tool_def.command_class is not None:
                    self._command_to_tool[tool_def.command_class] = tool_def.name

    @classmethod
    def from_sources(
        cls,
        *,
        tools: Sequence[Callable[..., Any] | Tool] | None = None,
        toolsets: Sequence[Toolset] | Toolsets | None = None,
        json_repairer: JsonRepairer | None = None,
    ) -> Toolsets:
        merged_toolsets: list[Toolset] = []
        if tools:
            merged_toolsets.append(Toolset(tools))

        if isinstance(toolsets, Toolsets):
            merged_toolsets.extend(toolsets._toolsets)
            if json_repairer is None:
                json_repairer = toolsets._json_repairer
        elif toolsets:
            merged_toolsets.extend(toolsets)

        return cls(merged_toolsets, json_repairer=json_repairer)

    def __len__(self) -> int:
        return len(self._toolsets)

    @overload
    def __getitem__(self, index: int) -> Toolset: ...
    @overload
    def __getitem__(self, index: slice) -> Sequence[Toolset]: ...
    def __getitem__(self, index: int | slice) -> Toolset | Sequence[Toolset]:
        return self._toolsets[index]

    def _validate_unique_tool_names(self) -> None:
        seen: set[str] = set()
        duplicates: set[str] = set()
        for toolset in self._toolsets:
            for tool in toolset.tools:
                if tool.name in seen:
                    duplicates.add(tool.name)
                seen.add(tool.name)

        if duplicates:
            duplicate_names = ", ".join(sorted(duplicates))
            raise ValueError(f"Duplicate tool names found: {duplicate_names}")

    def tool_exists(self, function_name: str) -> bool:
        return any(toolset.has_tool(function_name) for toolset in self._toolsets)

    def detect_tool(self, payload: Any) -> bool:
        if not self._tool_names:
            return False

        if isinstance(payload, tuple) and len(payload) == 2 and isinstance(payload[0], str):
            return payload[0] in self._tool_names

        if isinstance(payload, dict):
            for key in ("name", "tool", "function"):
                value = payload.get(key)
                if isinstance(value, str) and value in self._tool_names:
                    return True
            return False

        if not isinstance(payload, str):
            return False

        return any(tool_name in payload for tool_name in self._tool_names)

    @staticmethod
    def is_tool_error(result: str) -> bool:
        """Read a failure out of a plain string, for tools that return one.

        This is the fallback, not the contract: it cannot tell a failed call
        from a tool whose honest answer happens to start with "Error:". A tool
        that knows returns `ToolFailure`, and that answer is believed over this.
        """
        return any(result.startswith(prefix) for prefix in TOOL_ERROR_PREFIXES)

    @classmethod
    def _as_tool_error(cls, message: str) -> str:
        """What the model reads for a failure: one prefix, never two."""
        return message if cls.is_tool_error(message) else f"Error: {message}"

    @staticmethod
    def _settle(
        span: SpanHandle,
        tool_call: ToolCall,
        output: str,
        status: ToolRunStatus,
        tracer: LLMTracer,
        *,
        reason: str | None = None,
    ) -> ToolRunResult:
        """Close one tool call: annotate the span, then hand back the result.

        `output` is what the model reads; `reason` is what the span records,
        and they differ wherever the model needs the "Error: " prefix and the
        panel needs the cause without it.
        """
        level, key = _SPAN_ANNOTATION[status]
        span.update(
            level=level,
            output={key: (reason if reason is not None else output)[:500]},
            metadata={"agentic.tool_status": status.value},
        )
        return ToolRunResult(
            tool_call=tool_call,
            output=output,
            trace=tracer.current_trace,
            retry=status is ToolRunStatus.RETRY,
            status=status,
        )

    @staticmethod
    def _coerce_tool_call(payload: Any, repairer: JsonRepairer | None = None) -> ToolCall | None:
        return Tool.parse_tool_definition(payload, repairer=repairer)

    def parse_tool(self, payload: Any) -> Command | None:
        """Parse model response into a typed Command without executing."""
        tool_call = self._coerce_tool_call(payload, repairer=self._json_repairer)
        if tool_call is None:
            return None
        function_name, parameters = tool_call
        for toolset in self._toolsets:
            for tool_def in toolset.tools:
                if tool_def.name == function_name:
                    return tool_def.create_command(parameters)
        return None

    def execute(
        self,
        command: Command,
        *,
        tool_context: ToolContext | None = None,
        tracer: LLMTracer | None = None,
        capability: AbstractCapability | None = None,
        hook_context: HookContext | None = None,
    ) -> ToolRunResult:
        """Execute an already-parsed Command."""
        resolved_tracer = tracer or NoopLLMTracer()
        if isinstance(command, ToolCallCommand):
            function_name = command.function_name
            params = dict(command.parameters)
        else:
            cmd_type = type(command)
            if cmd_type not in self._command_to_tool:
                raise ValueError(f"No tool registered for command type {cmd_type.__name__}")
            function_name = self._command_to_tool[cmd_type]
            params = asdict(command)

        tool_call_tuple: ToolCall = (function_name, params)
        # The hook runs inside the span, so a call a capability refuses — or one
        # whose hook itself raised — is a tool call the panel can see. Outside
        # it, a refusal was indistinguishable from a call nobody made.
        with resolved_tracer.step(
            name=f"tool.{function_name}",
            span_type="TOOL",
            input=params,
            attributes={"tool_name": function_name},
        ) as span:
            if capability is not None and hook_context is not None:
                try:
                    decision = capability.before_tool_execute(function_name, params, hook_context)
                except Exception as error:
                    capability.on_error(error, hook_context)
                    raise
                if isinstance(decision, Deny):
                    return self._settle(
                        span,
                        tool_call_tuple,
                        self._as_tool_error(decision.reason),
                        ToolRunStatus.DENIED,
                        resolved_tracer,
                        reason=decision.reason,
                    )
                params = decision.parameters if isinstance(decision, Allow) else decision
                tool_call_tuple = (function_name, params)

            for toolset in self._toolsets:
                if toolset.has_tool(function_name):
                    try:
                        result = toolset.execute(function_name, params, tool_context=tool_context)
                    except (ModelRetry, ToolValidationError) as retry_error:
                        error_text = str(retry_error)
                        if capability is not None and hook_context is not None:
                            capability.on_error(retry_error, hook_context)
                        return self._settle(
                            span,
                            tool_call_tuple,
                            f"Error: {error_text}",
                            ToolRunStatus.RETRY,
                            resolved_tracer,
                            reason=error_text,
                        )
                    except Exception as error:  # noqa: BLE001 - a tool's failure is its result
                        error_text = str(error)
                        if capability is not None and hook_context is not None:
                            capability.on_error(error, hook_context)
                        return self._settle(
                            span,
                            tool_call_tuple,
                            self._as_tool_error(error_text),
                            ToolRunStatus.ERROR,
                            resolved_tracer,
                            reason=error_text,
                        )
                    declared = result if isinstance(result, ToolFailure) else None
                    output = (
                        declared.message
                        if declared is not None
                        else ("" if result is None else str(result))
                    )
                    if capability is not None and hook_context is not None:
                        try:
                            output = capability.after_tool_execute(
                                function_name, output, hook_context
                            )
                        except Exception as error:
                            capability.on_error(error, hook_context)
                            raise
                    if declared is None:
                        status = (
                            ToolRunStatus.ERROR
                            if self.is_tool_error(output)
                            else ToolRunStatus.SUCCESS
                        )
                        return self._settle(span, tool_call_tuple, output, status, resolved_tracer)
                    # A declared failure still reaches the model the way every
                    # other failure does; what the tool said is the reason.
                    status = ToolRunStatus.RETRY if declared.retryable else ToolRunStatus.ERROR
                    return self._settle(
                        span,
                        tool_call_tuple,
                        self._as_tool_error(output),
                        status,
                        resolved_tracer,
                        reason=output,
                    )

            output = f"Error: tool '{function_name}' does not exist."
            span.update(
                level="ERROR",
                output={"error": output},
                metadata={"agentic.tool_status": ToolRunStatus.ERROR.value},
            )
            return ToolRunResult(
                tool_call=tool_call_tuple,
                output=output,
                trace=resolved_tracer.current_trace,
                status=ToolRunStatus.ERROR,
            )

    def run_tool(
        self,
        payload: Any,
        *,
        tool_context: ToolContext | None = None,
        tracer: LLMTracer | None = None,
        capability: AbstractCapability | None = None,
        hook_context: HookContext | None = None,
    ) -> ToolRunResult | None:
        """Backward compat: parse + execute in one step."""
        command = self.parse_tool(payload)
        logger.info("run_tool", command=command)
        if command is None:
            tool_call = self._coerce_tool_call(payload, repairer=self._json_repairer)
            if tool_call is not None:
                function_name = tool_call[0]
                return ToolRunResult(
                    tool_call=tool_call,
                    output=f"Error: tool '{function_name}' does not exist.",
                    status=ToolRunStatus.ERROR,
                )
            return None
        return self.execute(
            command,
            tool_context=tool_context,
            tracer=tracer,
            capability=capability,
            hook_context=hook_context,
        )
