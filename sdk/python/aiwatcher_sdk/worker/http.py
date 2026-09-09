"""The HTTP adapter validates server data before it reaches task execution."""

from collections.abc import Sequence
from dataclasses import asdict
from urllib.parse import quote

from aiwatcher_sdk.api import Transport
from aiwatcher_sdk.worker.assignment import Assignment
from aiwatcher_sdk.worker.contract import (
    ArtifactRef,
    Completed,
    JsonObject,
    Report,
    artifact_ref,
    artifact_rows,
    boolean,
    string,
    unsigned,
)
from aiwatcher_sdk.worker.errors import WorkerError
from aiwatcher_sdk.worker.reference import AttemptRef


def claim(
    api: Transport,
    worker: str,
    queues: list[str],
    tasks: list[str],
    *,
    attempt: AttemptRef | None = None,
) -> Assignment | None:
    # An empty 200 is a broken assignment, not the 204 idle response.
    response = api.send(
        "POST",
        "/api/v1/worker/claims",
        {
            "worker": worker,
            "queues": queues,
            "tasks": tasks,
            **({"attempt": asdict(attempt)} if attempt is not None else {}),
        },
    )
    if response.status_code == 204:
        return None
    try:
        return Assignment.from_dict(response.json())
    except ValueError as error:
        raise WorkerError(f"invalid worker assignment: {error}") from error


class HttpAttemptAPI:
    def __init__(self, api: Transport, assignment: Assignment, worker: str) -> None:
        self._api = api
        self._assignment = assignment
        self._worker = worker

    def heartbeat(self) -> None:
        self._api.json(
            "POST", self._assignment.path + "/heartbeat", {"worker": self._worker}, idempotent=True
        )

    def read_artifact(self, name: str) -> list[JsonObject]:
        body = self._api.json(
            "GET",
            self._assignment.path + "/inputs/" + quote(name, safe=""),
            params={"worker": self._worker},
        )
        try:
            return artifact_rows(body.get("rows"))
        except ValueError as error:
            raise WorkerError(f"invalid artifact response: {error}") from error

    def write_artifact(self, name: str, rows: Sequence[JsonObject]) -> ArtifactRef:
        body = self._api.json(
            "POST",
            self._assignment.path + "/outputs/" + quote(name, safe=""),
            {"rows": artifact_rows(list(rows))},
            params={"worker": self._worker},
            idempotent=True,
        )
        try:
            ref = artifact_ref(body)
            if ref["name"] != name:
                raise ValueError("artifact name differs from the requested output")
            return ref
        except ValueError as error:
            raise WorkerError(f"invalid artifact reference: {error}") from error

    def report(self, report: Report) -> None:
        body = self._api.json(
            "POST",
            self._assignment.path + "/result",
            {"worker": self._worker, **report.to_dict()},
            idempotent=self._assignment.report_idempotent,
            attempts=3 if self._assignment.report_idempotent else 1,
        )
        try:
            step = string(body.get("step_id"), "step_id")
            attempt = unsigned(body.get("attempt"), "attempt", bits=32)
            succeeded = boolean(body.get("succeeded"), "succeeded")
            if succeeded != isinstance(report, Completed):
                raise ValueError("settlement outcome differs from the reported result")
            if (step, attempt) != (self._assignment.step_id, self._assignment.attempt):
                raise ValueError("settlement does not identify the reported attempt")
        except ValueError as error:
            # The server may have committed the report. Never replay it or emit
            # a contradictory task-failure report on this validation error.
            raise WorkerError(f"invalid settlement: {error}") from error
