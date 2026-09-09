"""Start and control durable workflows through the platform's command API."""

from __future__ import annotations

import time
import uuid
from collections.abc import Mapping
from dataclasses import dataclass
from types import TracebackType
from typing import Self
from urllib.parse import quote

import httpx

from aiwatcher_sdk.api import ApiError, Transport
from aiwatcher_sdk.worker.contract import JsonObject, json_object, string
from aiwatcher_sdk.workflow import Workflow


class ExecutionError(ApiError):
    """The platform refused a workflow command or returned an invalid response."""


@dataclass(frozen=True)
class RegisteredWorkflow:
    name: str
    revision: str


@dataclass(frozen=True)
class ExecutionHandle:
    execution_id: str
    client: ExecutionClient

    @property
    def path(self) -> str:
        return "/api/v1/executions/" + quote(self.execution_id, safe="")

    def status(self) -> JsonObject:
        return json_object(self.client._api.json("GET", self.path))

    def history(self, *, after: int = 0, limit: int = 100) -> JsonObject:
        return json_object(
            self.client._api.json(
                "GET", self.path + "/history", params={"after": after, "limit": limit}
            )
        )

    def retry(self, step: str) -> JsonObject:
        return self._command("/steps/" + quote(step, safe="") + "/commands/retry")

    def pause(self) -> JsonObject:
        return self._command("/commands/pause")

    def resume(self) -> JsonObject:
        return self._command("/commands/resume")

    def cancel(self, reason: str = "") -> JsonObject:
        return self._command("/commands/cancel", {"reason": reason})

    def _command(self, path: str, body: JsonObject | None = None) -> JsonObject:
        # Commands derive their inbox identity from current state. They cannot
        # safely be replayed after an ambiguous response without a receipt key.
        return json_object(self.client._api.json("POST", self.path + path, body or {}))

    def wait(self, *, timeout: float = 300, poll_interval: float = 0.5) -> JsonObject:
        if timeout < 0 or poll_interval <= 0:
            raise ValueError("timeout must be nonnegative and poll_interval positive")
        deadline = time.monotonic() + timeout
        while True:
            view = self.status()
            execution = json_object(view.get("execution"))
            state = json_object(execution.get("state"))
            if state.get("state_type") in ("completed", "failed", "cancelled", "crashed"):
                return view
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError(f"execution {self.execution_id} has not finished")
            time.sleep(min(poll_interval, remaining))


class ExecutionClient:
    def __init__(
        self, url: str, token: str | None = None, *, client: httpx.Client | None = None
    ) -> None:
        self._api = Transport(
            url,
            token=token,
            client=client,
            attempts=1,
            error=ExecutionError,
            subject="the execution API",
        )

    def register(self, workflow: Workflow, *, queue: str) -> RegisteredWorkflow:
        body = self._api.json(
            "POST", "/api/v1/workflow-definitions", workflow.to_definition(queue), idempotent=True
        )
        return RegisteredWorkflow(workflow.name, string(body.get("revision"), "revision"))

    def start(
        self,
        workflow: RegisteredWorkflow,
        *,
        parameters: Mapping[str, object] | None = None,
        idempotency_key: str | None = None,
    ) -> ExecutionHandle:
        body = self._api.json(
            "POST",
            "/api/v1/executions",
            {
                "target": {
                    "kind": "workflow",
                    "name": workflow.name,
                    "revision": workflow.revision,
                },
                "parameters": dict(parameters or {}),
            },
            idempotent=True,
            idempotency_key=idempotency_key or uuid.uuid4().hex,
        )
        execution = json_object(body.get("execution"))
        return self.execution(string(execution.get("execution_id"), "execution_id"))

    def execution(self, execution_id: str) -> ExecutionHandle:
        return ExecutionHandle(execution_id, self)

    def close(self) -> None:
        self._api.close()

    def __enter__(self) -> Self:
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        self.close()
