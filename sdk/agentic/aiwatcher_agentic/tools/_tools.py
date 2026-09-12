from __future__ import annotations

import asyncio
import inspect
import re
import types
from collections.abc import Callable
from dataclasses import dataclass
from typing import Any, Union, get_args, get_origin, get_type_hints

import orjson
import structlog

ToolCall = tuple[str, dict[str, Any]]
JsonRepairer = Callable[[str, str], Any | None]
logger = structlog.get_logger(__name__)


@dataclass(frozen=True)
class Command:
    """Base class for tool commands. Each tool registers its own subclass."""


@dataclass(frozen=True)
class ToolCallCommand(Command):
    """Fallback command for tools without a registered command class."""

    function_name: str
    parameters: dict[str, Any]


@dataclass(frozen=True)
class ToolContext:
    agent_id: str
    track_id: str


@dataclass(frozen=True, slots=True)
class ToolFailure:
    """A tool result that is a failure because the tool said so.

    A tool that handles its own failure has always had to encode that in the
    string it returns, and the toolset read it back by prefix — so a tool whose
    honest answer began with "Error:" was a failed call, and a failure phrased
    any other way was a successful one. Return this instead: the status is then
    declared by the code that knows, in any language, and the message is only
    a message.

    `retryable` is the same distinction `ModelRetry` makes — worth another
    attempt — without raising for a failure the tool already understood.
    """

    message: str
    retryable: bool = False


class JsonParser:
    @staticmethod
    def extract_json_block(text: str) -> str:
        stripped = text.strip()

        stripped = re.sub(r"<think>.*?(?:</think>|$)\s*", "", stripped, flags=re.DOTALL)

        tool_call_match = re.search(
            r"<tool_call>\s*(.*?)\s*</tool_call>",
            stripped,
            flags=re.DOTALL,
        )
        if tool_call_match:
            stripped = tool_call_match.group(1).strip()

        if stripped.startswith("```json"):
            stripped = stripped.removeprefix("```json").strip()
        elif stripped.startswith("```"):
            stripped = stripped.removeprefix("```").strip()
        if stripped.endswith("```"):
            stripped = stripped.removesuffix("```").strip()

        if stripped.startswith("<tool_call>") and stripped.endswith("</tool_call>"):
            stripped = stripped.removeprefix("<tool_call>").removesuffix("</tool_call>").strip()

        return stripped

    @staticmethod
    def parse_json(text: str, repairer: JsonRepairer | None = None) -> Any | None:
        try:
            return orjson.loads(text)
        except orjson.JSONDecodeError as e:
            logger.warning("json_parse_failed", error=str(e))
            logger.debug("json_parse_input", text=text)
            if repairer is not None:
                repaired = repairer(text, str(e))
                if repaired is not None:
                    return repaired
            return JsonParser.repair_json(text, str(e))

    @staticmethod
    def repair_json(text: str, error: str) -> Any | None:
        if "unexpected end of data" in error:
            text += "}"
        try:
            return orjson.loads(text)
        except orjson.JSONDecodeError:
            return None

    @staticmethod
    def parse_tool_call(text: str, repairer: JsonRepairer | None = None) -> dict[str, Any] | None:
        payload = JsonParser.extract_json_block(text)
        if repairer is None:
            return JsonParser.parse_json(payload)
        return JsonParser.parse_json(payload, repairer=repairer)


def build_hf_json_repairer(
    *,
    model_id: str = "HuggingFaceTB/SmolLM2-135M-Instruct",
    max_new_tokens: int = 80,
) -> JsonRepairer:
    pipe: Any | None = None
    extract_code = re.compile(r"<code>(.*?)</code>", re.DOTALL)

    def repair(text: str, _: str) -> Any | None:
        nonlocal pipe
        try:
            if pipe is None:
                from transformers import pipeline

                pipe = pipeline(
                    "text-generation",
                    model=model_id,
                    max_new_tokens=max_new_tokens,
                    do_sample=False,
                    temperature=0.0,
                )
            prompt = (
                "Task: Fix the JSON input.\n"
                "Rules:\n"
                "- Output ONLY a single minified JSON object.\n"
                "- Output exactly one valid JSON object.\n"
                f"Input:\n{text}\n\nOutput:"
            )
            response = pipe(prompt)[0]["generated_text"]
            if match := extract_code.search(response):
                return orjson.loads(match.group(1).strip())
            return None
        except Exception as error:  # noqa: BLE001 - a failed repair is a parse that failed
            logger.warning("json_repair_failed", error=str(error))
            return None

    return repair


def json_schema_type(value: Any) -> str:
    mapping = {
        "str": "string",
        "int": "integer",
        "float": "number",
        "bool": "boolean",
        "dict": "object",
        "list": "array",
        "tuple": "array",
    }
    return mapping.get(str(value), "string")


def build_chat_tools(tool_schema: list[dict[str, Any]]) -> list[dict[str, Any]]:
    tools: list[dict[str, Any]] = []
    for tool in tool_schema:
        args = tool.get("arguments", [])
        if not isinstance(args, list):
            continue
        properties: dict[str, Any] = {}
        required: list[str] = []
        for arg in args:
            if not isinstance(arg, dict):
                continue
            arg_name = arg.get("name")
            if not isinstance(arg_name, str) or not arg_name:
                continue
            properties[arg_name] = {"type": json_schema_type(arg.get("type", "string"))}
            if arg.get("required"):
                required.append(arg_name)
        function_def: dict[str, Any] = {
            "name": tool.get("name", ""),
            "parameters": {
                "type": "object",
                "properties": properties,
            },
        }
        description = tool.get("description")
        if description:
            function_def["description"] = description
        if required:
            function_def["parameters"]["required"] = required
        tools.append({"type": "function", "function": function_def})
    return tools


def _check_type(value: Any, expected: Any) -> bool:
    """Check if *value* matches the *expected* type hint (best-effort)."""
    if expected is inspect.Parameter.empty or expected is Any:
        return True

    origin = get_origin(expected)

    # Handle Union / X | Y (including Optional)
    if origin is Union or origin is types.UnionType:
        return any(_check_type(value, arg) for arg in get_args(expected))

    # Handle None / NoneType
    if expected is type(None):
        return value is None

    # Handle generic containers (list[X], dict[X,Y], etc.) — only check outer type
    if origin is not None:
        if origin is list:
            return isinstance(value, list)
        if origin is dict:
            return isinstance(value, dict)
        if origin is tuple:
            return isinstance(value, tuple)
        if origin is set:
            return isinstance(value, set)
        return isinstance(value, origin)

    # Plain type
    if isinstance(expected, type):
        # Allow int where float is expected
        if expected is float and isinstance(value, int):
            return True
        return isinstance(value, expected)

    return True


class Tool:
    def __init__(self, func: Callable[..., Any], *, command: type[Command] | None = None):
        self._func = func
        self._func_name = func.__name__
        self._doc = (inspect.getdoc(func) or "").strip()
        self._args = self._extract_args(func)
        self._command_class = command
        self._type_hints = get_type_hints(func)
        signature = inspect.signature(func)
        self._accepts_tool_context = "tool_context" in signature.parameters

    @staticmethod
    def parse_tool_definition(
        payload: Any,
        *,
        repairer: JsonRepairer | None = None,
    ) -> ToolCall | None:
        if isinstance(payload, tuple) and len(payload) == 2 and isinstance(payload[0], str):
            name, parameters = payload
            if isinstance(parameters, dict):
                return name, parameters
            return None

        definition: Any
        if isinstance(payload, (dict, list)):
            definition = payload
        elif isinstance(payload, str):
            definition = JsonParser.parse_tool_call(payload, repairer=repairer)
        else:
            return None

        if isinstance(definition, list):
            for item in definition:
                if isinstance(item, dict) and (
                    tool_call := Tool._extract_direct_function_call(item)
                ):
                    return tool_call
            return None

        if isinstance(definition, dict):
            return Tool._extract_direct_function_call(definition)

        return None

    @staticmethod
    def _extract_direct_function_call(payload: dict[str, Any]) -> ToolCall | None:
        name = payload.get("name")
        parameters = payload.get("parameters")
        if isinstance(name, str):
            if isinstance(parameters, dict):
                return name, parameters
            if parameters is None:
                return name, {}

        tool_name = payload.get("tool")
        if isinstance(tool_name, str):
            if isinstance(parameters, dict):
                return tool_name, parameters
            if parameters is None:
                return tool_name, {}

        function_name = payload.get("function")
        arguments = payload.get("arguments")
        if isinstance(function_name, str) and isinstance(arguments, dict):
            return function_name, arguments

        return None

    @staticmethod
    def _extract_args(func: Callable[..., Any]) -> list[dict[str, Any]]:
        sig = inspect.signature(func)
        hints = get_type_hints(func)
        args: list[dict[str, Any]] = []

        for name, p in sig.parameters.items():
            if p.kind in (inspect.Parameter.VAR_POSITIONAL, inspect.Parameter.VAR_KEYWORD):
                continue
            if name == "tool_context":
                continue

            ann = hints.get(name, p.annotation)
            ann_name = getattr(ann, "__name__", str(ann)) if ann is not inspect._empty else "Any"

            args.append(
                {
                    "name": name,
                    "type": ann_name,
                    "required": p.default is inspect._empty,
                    "default": None if p.default is inspect._empty else p.default,
                }
            )

        return args

    @property
    def name(self) -> str:
        return self._func_name

    @property
    def doc(self) -> str:
        return self._doc

    @property
    def args(self) -> list[dict[str, Any]]:
        return self._args

    def call(self, parameters: dict[str, Any], *, tool_context: ToolContext | None = None) -> Any:
        call_parameters = dict(parameters)
        if self._accepts_tool_context:
            call_parameters["tool_context"] = tool_context
        if inspect.iscoroutinefunction(self._func):
            return asyncio.run(self._func(**call_parameters))
        return self._func(**call_parameters)

    async def acall(
        self,
        parameters: dict[str, Any],
        *,
        tool_context: ToolContext | None = None,
    ) -> Any:
        call_parameters = dict(parameters)
        if self._accepts_tool_context:
            call_parameters["tool_context"] = tool_context
        if not inspect.iscoroutinefunction(self._func):
            return await asyncio.to_thread(self._func, **call_parameters)
        return await self._func(**call_parameters)

    def validate_types(self, parameters: dict[str, Any]) -> list[str]:
        """Validate parameter types against function type hints.

        Returns a list of error messages for each type mismatch.
        """
        errors: list[str] = []
        for name, value in parameters.items():
            if name == "tool_context":
                continue
            expected = self._type_hints.get(name)
            if expected is None:
                continue
            if not _check_type(value, expected):
                errors.append(
                    f"parameter '{name}' expected {expected}, got {type(value).__name__}: {value!r}"
                )
        return errors

    @property
    def required_parameters(self) -> set[str]:
        return {arg["name"] for arg in self._args if arg["required"]}

    @property
    def all_parameters(self) -> set[str]:
        return {arg["name"] for arg in self._args}

    @property
    def command_class(self) -> type[Command] | None:
        return self._command_class

    def create_command(self, parameters: dict[str, Any]) -> Command:
        if self._command_class is not None:
            return self._command_class(**parameters)
        return ToolCallCommand(function_name=self._func_name, parameters=parameters)


def tool(func: Callable[..., Any]) -> Tool:
    return Tool(func)
