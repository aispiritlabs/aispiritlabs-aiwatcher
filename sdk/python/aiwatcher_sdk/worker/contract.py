"""Validated values at the worker API boundary, independent of HTTP."""

from __future__ import annotations

import math
from dataclasses import dataclass
from typing import Literal, NotRequired, TypeAlias, TypedDict, cast

from aiwatcher_sdk.task_errors import FailureClass

JsonValue: TypeAlias = "bool | int | float | str | list[JsonValue] | dict[str, JsonValue] | None"
JsonObject: TypeAlias = dict[str, JsonValue]
ArtifactKind = Literal["rows", "code", "preview", "log", "model", "report", "blob"]


class ArtifactRef(TypedDict):
    name: str
    uri: str
    digest: str
    kind: NotRequired[ArtifactKind]
    content_type: NotRequired[str]
    schema_ref: NotRequired[str | None]
    size_bytes: NotRequired[int | None]


def json_value(value: object) -> JsonValue:
    if value is None or isinstance(value, (str, bool, int)):
        return value
    if isinstance(value, float) and math.isfinite(value):
        return value
    if isinstance(value, list):
        return [json_value(item) for item in value]
    if isinstance(value, dict) and all(isinstance(key, str) for key in value):
        return {key: json_value(item) for key, item in value.items()}
    raise ValueError("expected a finite JSON value with string object keys")


def json_object(value: object) -> JsonObject:
    result = json_value(value)
    if not isinstance(result, dict):
        raise ValueError("expected a JSON object")
    return result


def string(value: object, field: str) -> str:
    if not isinstance(value, str):
        raise ValueError(f"{field} must be a string")
    return value


def unsigned(value: object, field: str, *, bits: int = 64) -> int:
    if type(value) is not int or not 0 <= value < 2**bits:
        raise ValueError(f"{field} must be an unsigned {bits}-bit integer")
    return value


def boolean(value: object, field: str) -> bool:
    if not isinstance(value, bool):
        raise ValueError(f"{field} must be a boolean")
    return value


def artifact_ref(value: object) -> ArtifactRef:
    body = json_object(value)
    allowed = {"name", "uri", "digest", "kind", "content_type", "schema_ref", "size_bytes"}
    if set(body) - allowed:
        raise ValueError("artifact reference contains unknown fields")
    ref = ArtifactRef(
        name=string(body.get("name"), "name"),
        uri=string(body.get("uri"), "uri"),
        digest=string(body.get("digest"), "digest"),
    )
    if "kind" in body:
        kind = string(body["kind"], "kind")
        if kind not in ("rows", "code", "preview", "log", "model", "report", "blob"):
            raise ValueError("unknown artifact kind")
        ref["kind"] = cast(ArtifactKind, kind)
    if "content_type" in body:
        ref["content_type"] = string(body["content_type"], "content_type")
    if "schema_ref" in body:
        ref["schema_ref"] = (
            None if body["schema_ref"] is None else string(body["schema_ref"], "schema_ref")
        )
    if "size_bytes" in body:
        ref["size_bytes"] = (
            None if body["size_bytes"] is None else unsigned(body["size_bytes"], "size_bytes")
        )
    return ref


def artifact_rows(value: object) -> list[JsonObject]:
    if not isinstance(value, list):
        raise ValueError("artifact rows must be an array of objects")
    return [json_object(row) for row in value]


@dataclass(frozen=True)
class Completed:
    result: JsonValue
    outputs: list[ArtifactRef]
    cacheable: bool = False

    def to_dict(self) -> JsonObject:
        return {
            "outcome": "completed",
            "result": self.result,
            "outputs": [json_object(ref) for ref in self.outputs],
            "cacheable": self.cacheable,
        }


@dataclass(frozen=True)
class Failed:
    classification: FailureClass
    message: str

    def to_dict(self) -> JsonObject:
        return {"outcome": "failed", "class": self.classification, "message": self.message}


Report: TypeAlias = Completed | Failed
