"""Durable Evaluation client. Failures raise; telemetry remains best effort.

Retry publication with the same evaluation ID and body after an ambiguous
transport failure. The server returns the first receipt or an explicit conflict.
Use ``evaluation`` for types without importing the HTTP transport.
"""

from __future__ import annotations

from types import TracebackType
from typing import Any, Literal, Self, TypedDict
from urllib.parse import quote

import httpx

from aiwatcher_sdk.api import ApiError, Transport
from aiwatcher_sdk.evaluation import EvaluationManifest


class EvaluationRegistryError(ApiError):
    """The durable registry refused or could not complete an operation."""


class CaseMeasurement(TypedDict):
    case_id: str
    repetition_id: str
    actual: Any
    metrics: dict[str, float]
    error: str | None
    trace_id: str | None
    span_id: str | None


class EvaluationRegistry:
    def __init__(
        self,
        base_url: str,
        *,
        token: str | None = None,
        timeout: float = 30.0,
        attempts: int = 3,
        client: httpx.Client | None = None,
    ) -> None:
        self._transport = Transport(
            base_url,
            token=token,
            timeout=timeout,
            attempts=attempts,
            client=client,
            error=EvaluationRegistryError,
            subject="the evaluation registry",
        )

    def close(self) -> None:
        self._transport.close()

    def __enter__(self) -> Self:
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc_value: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        self.close()

    def publish(
        self,
        manifest: EvaluationManifest,
        cases: list[CaseMeasurement],
        *,
        status: Literal["succeeded", "failed", "partial"] = "succeeded",
    ) -> dict[str, Any]:
        response = self._transport.send(
            "POST",
            "/api/v1/evaluation-results",
            {"manifest": manifest, "cases": cases, "status": status},
            idempotent=True,
        )
        return self._object(response)

    def get(self, evaluation_id: str) -> dict[str, Any]:
        return self._object(self._transport.send("GET", self._path(evaluation_id)))

    def list(self, *, cursor: str | None = None, limit: int = 200) -> dict[str, Any]:
        return self._object(
            self._transport.send(
                "GET", "/api/v1/evaluation-results", params={"cursor": cursor, "limit": limit}
            )
        )

    def cases(
        self,
        evaluation_id: str,
        version: str,
        *,
        cursor: str | None = None,
        limit: int = 200,
    ) -> dict[str, Any]:
        return self._object(
            self._transport.send(
                "GET",
                self._path(evaluation_id) + "/cases",
                params={"version": version, "cursor": cursor, "limit": limit},
            )
        )

    @staticmethod
    def _path(evaluation_id: str) -> str:
        return "/api/v1/evaluation-results/" + quote(evaluation_id, safe="")

    @staticmethod
    def _object(response: httpx.Response) -> dict[str, Any]:
        try:
            value: Any = response.json()
        except ValueError as error:
            raise EvaluationRegistryError(
                "the evaluation registry returned invalid JSON"
            ) from error
        if not isinstance(value, dict):
            raise EvaluationRegistryError("the evaluation registry returned no object")
        return value
