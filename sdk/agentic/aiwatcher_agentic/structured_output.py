"""Structured output: parse and validate LLM responses into typed shapes.

Two implementations, one contract. :class:`StructuredOutput` handles frozen
dataclasses and puts its schema in the *prompt*. :class:`PydanticOutput` handles
pydantic models — nesting, lists of models, field validators — and puts its
schema in the *request*, as ``response_format``, which is the only form a
hosted router will enforce.
"""

from __future__ import annotations

import dataclasses
import json
from collections.abc import Callable
from typing import TYPE_CHECKING, Any, Protocol, get_type_hints, runtime_checkable

import structlog

from aiwatcher_agentic.exceptions import ModelRetry

if TYPE_CHECKING:
    from pydantic import BaseModel

logger = structlog.get_logger(__name__)


type OutputValidator = Callable[[Any], Any]


@runtime_checkable
class StructuredOutputProto(Protocol):
    """What an :class:`~aiwatcher_agentic.agent.Agent` asks of an output type."""

    def is_structured(self, response: str) -> bool: ...

    def parse(self, response: str) -> Any: ...

    def response_format(self) -> dict[str, Any] | None:
        """The ``response_format`` to send, or ``None`` to send none."""
        ...


def _extract_json(text: str) -> str:
    """Extract JSON from model response, handling markdown code blocks."""
    stripped = text.strip()
    if stripped.startswith("```json"):
        stripped = stripped.removeprefix("```json").strip()
    elif stripped.startswith("```"):
        stripped = stripped.removeprefix("```").strip()
    if stripped.endswith("```"):
        stripped = stripped.removesuffix("```").strip()
    return stripped


def _coerce_value(value: Any, expected_type: Any) -> Any:
    """Best-effort coercion of a parsed JSON value to the expected type."""
    if value is None:
        return value
    if isinstance(expected_type, type):
        if expected_type is str and not isinstance(value, str):
            return str(value)
        if expected_type is int and isinstance(value, (str, float)):
            return int(value)
        if expected_type is float and isinstance(value, (str, int)):
            return float(value)
        if expected_type is bool and isinstance(value, (str, int)):
            if isinstance(value, str):
                return value.lower() in ("true", "1", "yes")
            return bool(value)
    return value


class StructuredOutput:
    """Parse LLM text responses into typed dataclass instances.

    Supports:
      - Frozen dataclasses as output types
      - JSON extraction from markdown-wrapped responses
      - Custom output validators
      - Retry signal on validation failure (raises ``ModelRetry``)
    """

    def __init__(
        self,
        output_type: type | None = None,
        *,
        validators: list[OutputValidator] | None = None,
    ) -> None:
        self._output_type = output_type
        self._validators = validators or []
        self._is_dataclass = output_type is not None and dataclasses.is_dataclass(output_type)

    @property
    def output_type(self) -> type | None:
        return self._output_type

    def is_structured(self, response: str) -> bool:
        """Return True if we should attempt structured parsing."""
        if self._output_type is None:
            return False
        if self._is_dataclass:
            return True
        # Check if response looks like JSON
        stripped = _extract_json(response)
        return stripped.startswith(("{", "["))

    def parse(self, response: str) -> Any:
        """Parse and validate the response.

        Raises ``ModelRetry`` if parsing or validation fails, so the
        framework can feed the error back to the model.
        """
        if self._output_type is None:
            return response

        json_text = _extract_json(response)

        try:
            raw = json.loads(json_text)
        except json.JSONDecodeError as e:
            raise ModelRetry(
                f"Invalid JSON in response: {e}. "
                f"Please respond with valid JSON matching the schema."
            ) from e

        result = self._build_dataclass(self._output_type, raw) if self._is_dataclass else raw

        for validator in self._validators:
            result = validator(result)

        return result

    def _build_dataclass(self, output_type: type[Any], raw: Any) -> Any:
        """Build a dataclass instance from parsed JSON dict."""
        if not isinstance(raw, dict):
            raise ModelRetry(
                f"Expected a JSON object for {output_type.__name__}, "
                f"got {type(raw).__name__}. Please respond with a JSON object."
            )

        hints = get_type_hints(output_type)
        fields = {f.name for f in dataclasses.fields(output_type)}

        kwargs: dict[str, Any] = {}
        errors: list[str] = []

        for field_name in fields:
            if field_name in raw:
                expected = hints.get(field_name)
                kwargs[field_name] = _coerce_value(raw[field_name], expected)
            elif field_name not in {
                f.name
                for f in dataclasses.fields(output_type)
                if f.default is not dataclasses.MISSING
                or f.default_factory is not dataclasses.MISSING
            }:
                errors.append(f"missing required field '{field_name}'")

        extra = set(raw.keys()) - fields
        if extra:
            errors.append(f"unexpected fields: {', '.join(sorted(extra))}")

        if errors:
            detail = "; ".join(errors)
            field_list = ", ".join(sorted(fields))
            raise ModelRetry(
                f"Output validation failed for {output_type.__name__}: {detail}. "
                f"Expected fields: {field_list}"
            )

        try:
            return output_type(**kwargs)
        except (TypeError, ValueError) as e:
            raise ModelRetry(f"Failed to construct {output_type.__name__}: {e}") from e

    def json_schema(self) -> dict[str, Any] | None:
        """Generate a JSON schema for the output type (for prompt injection)."""
        if not self._is_dataclass or self._output_type is None:
            return None

        hints = get_type_hints(self._output_type)
        properties: dict[str, Any] = {}
        required: list[str] = []

        type_map = {
            str: "string",
            int: "integer",
            float: "number",
            bool: "boolean",
        }

        for f in dataclasses.fields(self._output_type):
            hint = hints.get(f.name, str)
            json_type = type_map.get(hint, "string") if isinstance(hint, type) else "string"
            prop: dict[str, Any] = {"type": json_type}
            if f.metadata and "description" in f.metadata:
                prop["description"] = f.metadata["description"]
            properties[f.name] = prop
            if f.default is dataclasses.MISSING and f.default_factory is dataclasses.MISSING:
                required.append(f.name)

        schema: dict[str, Any] = {
            "type": "object",
            "properties": properties,
        }
        if required:
            schema["required"] = required
        return schema

    def response_format(self) -> dict[str, Any] | None:
        """None: this schema is injected into the prompt, not into the request.

        The dataclass schema above is a flat, best-effort description meant for a
        model to read. Sending it as a strict ``response_format`` would promise
        an enforcement it cannot deliver for anything nested.
        """
        return None


class PydanticOutput[T: BaseModel]:
    """A pydantic model as the shape of an answer.

    The schema goes into the request as ``response_format``, so a provider that
    supports structured outputs enforces it; the answer is then validated again
    here, because a provider that does *not* support it silently ignores the
    field. A validation failure becomes :class:`~aiwatcher_agentic.exceptions.ModelRetry`
    carrying the field paths that were wrong, which is what a model needs to fix
    its own output.

    `validator` runs after validation and may raise ``ModelRetry`` of its own —
    it is where a caller puts the checks that are about the domain rather than
    the schema.

    `strict` asks the provider to enforce the schema exactly. The schema is sent
    as pydantic writes it, with ``additionalProperties: false`` added to every
    object, since every strict implementation requires it. Pass ``strict=False``
    for a model that rejects strict schemas; the prompt-side instructions and the
    validation here still stand.
    """

    def __init__(
        self,
        model: type[T],
        *,
        validator: Callable[[T], T] | None = None,
        strict: bool = True,
    ) -> None:
        self._model = model
        self._validator = validator
        self._strict = strict

    @property
    def output_type(self) -> type[T]:
        return self._model

    def json_schema(self) -> dict[str, Any]:
        schema = self._model.model_json_schema()
        return _forbid_extra_properties(schema) if self._strict else schema

    def response_format(self) -> dict[str, Any]:
        return {
            "type": "json_schema",
            "json_schema": {
                "name": self._model.__name__,
                "strict": self._strict,
                "schema": self.json_schema(),
            },
        }

    def is_structured(self, response: str) -> bool:
        del response
        return True

    def parse(self, response: str) -> T:
        from pydantic import ValidationError

        try:
            value = self._model.model_validate_json(_extract_json(response))
        except ValidationError as error:
            raise ModelRetry(_validation_message(self._model.__name__, error)) from error
        return self._validator(value) if self._validator else value


def _validation_message(model_name: str, error: Any) -> str:
    """Name the fields that were wrong — a model cannot fix "invalid JSON"."""
    problems = error.errors()
    detail = "; ".join(
        f"{'.'.join(str(part) for part in item['loc']) or '<root>'}: {item['msg']}"
        for item in problems[:4]
    )
    remaining = len(problems) - 4
    suffix = f" (and {remaining} more)" if remaining > 0 else ""
    return f"Output validation failed for {model_name}: {detail}{suffix}"


def _forbid_extra_properties(schema: dict[str, Any]) -> dict[str, Any]:
    """Set ``additionalProperties: false`` on every object in the schema."""

    def walk(node: Any) -> Any:
        if isinstance(node, dict):
            walked = {key: walk(value) for key, value in node.items()}
            if walked.get("type") == "object" or "properties" in walked:
                walked.setdefault("additionalProperties", False)
            return walked
        if isinstance(node, list):
            return [walk(item) for item in node]
        return node

    walked: dict[str, Any] = walk(schema)
    return walked
