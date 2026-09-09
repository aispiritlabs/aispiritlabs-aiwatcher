"""The same attempt scenarios exercise the HTTP adapter and an in-memory port."""

from __future__ import annotations

from collections.abc import Iterator, Sequence
from datetime import UTC, datetime, timedelta

import httpx
import pytest
from test_worker import WorkerApi, assignment, reference, validate_contract

from aiwatcher_sdk import AiwatcherClient, NullTransport
from aiwatcher_sdk.api import Transport
from aiwatcher_sdk.worker import TaskContext, TaskError, WorkerError
from aiwatcher_sdk.worker.assignment import Assignment
from aiwatcher_sdk.worker.attempt import AttemptAPI
from aiwatcher_sdk.worker.contract import ArtifactRef, Completed, JsonObject, Report, artifact_ref
from aiwatcher_sdk.worker.errors import LeaseLostError
from aiwatcher_sdk.worker.http import HttpAttemptAPI


class MemoryAttempt:
    def __init__(self) -> None:
        self.rows: dict[str, list[JsonObject]] = {"source": [{"house": 1}]}
        self.reports: list[Report] = []
        self.beats = 0
        self.lease_lost = False

    def heartbeat(self) -> None:
        if self.lease_lost:
            raise WorkerError("lease lost", status=409)
        self.beats += 1

    def read_artifact(self, name: str) -> list[JsonObject]:
        return self.rows[name]

    def write_artifact(self, name: str, rows: Sequence[JsonObject]) -> ArtifactRef:
        validate_contract("RowsBody", {"rows": list(rows)})
        self.rows[name] = list(rows)
        return artifact_ref(reference(name, rows))

    def report(self, report: Report) -> None:
        validate_contract("WorkReport", report.to_dict())
        self.reports.append(report)


@pytest.fixture(params=["memory", "http"])
def attempt_api(request: pytest.FixtureRequest) -> Iterator[AttemptAPI]:
    if request.param == "memory":
        yield MemoryAttempt()
    else:
        fake = WorkerApi()
        with (
            httpx.Client(transport=httpx.MockTransport(fake.handle)) as client,
            Transport(
                "http://aiwatcher.invalid", client=client, attempts=1, error=WorkerError
            ) as transport,
        ):
            yield HttpAttemptAPI(transport, Assignment.from_dict(assignment()), "worker-1")


def test_attempt_port_contract(attempt_api: AttemptAPI) -> None:
    attempt_api.heartbeat()
    rows = attempt_api.read_artifact("source")
    assert rows == [{"house": 1}]
    ref = attempt_api.write_artifact("normalized", rows)
    validate_contract("ArtifactRef", ref)
    assert ref["name"] == "normalized"
    assert ref["digest"]
    attempt_api.report(Completed({"count": len(rows)}, [ref]))


def test_task_deadline_and_lease_loss_without_sleeping() -> None:
    now = datetime(2026, 9, 8, tzinfo=UTC)
    elapsed = 100.0
    api = MemoryAttempt()
    claim = Assignment.from_dict(
        assignment(
            timeout_seconds=5,
            lease_expires_at=(now + timedelta(seconds=60)).isoformat(),
        )
    )
    ctx = TaskContext(
        claim,
        api,
        AiwatcherClient(service="test", transport=NullTransport()),
        monotonic=lambda: elapsed,
        now=lambda: now,
    )
    ctx.heartbeat()
    assert api.beats == 1
    elapsed += 5
    assert ctx.cancelled
    with pytest.raises(TaskError) as failure:
        ctx.read_artifact("source")
    assert failure.value.classification == "timeout"
    with pytest.raises(TaskError):
        ctx.heartbeat()
    assert api.beats == 1
    ctx.stop()  # safe even if the heartbeat thread was never started

    ctx = TaskContext(
        claim,
        api,
        AiwatcherClient(service="test", transport=NullTransport()),
        monotonic=lambda: elapsed,
        now=lambda: now,
    )
    api.lease_lost = True
    with pytest.raises(LeaseLostError):
        ctx.heartbeat()
    assert ctx.cancelled
    with pytest.raises(LeaseLostError):
        ctx.write_artifact("normalized", [{"house": 2}])
    assert "normalized" not in api.rows


def test_expired_lease_uses_the_supplied_wall_clock() -> None:
    now = datetime(2026, 9, 8, tzinfo=UTC)
    claim = Assignment.from_dict(assignment(lease_expires_at=now.isoformat()))
    with pytest.raises(LeaseLostError):
        TaskContext(
            claim,
            MemoryAttempt(),
            AiwatcherClient(service="test", transport=NullTransport()),
            now=lambda: now,
        )
