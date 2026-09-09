"""One server-owned attempt, never a callable supplied over the wire."""

from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime
from urllib.parse import quote

from aiwatcher_sdk.worker.contract import (
    ArtifactRef,
    JsonObject,
    artifact_ref,
    boolean,
    json_object,
    string,
    unsigned,
)


@dataclass(frozen=True)
class Assignment:
    execution_id: str
    step_id: str
    attempt: int
    task_ref: str
    queue: str
    context_id: str
    timeout_seconds: int
    lease_expires_at: datetime
    params: JsonObject
    parameters: JsonObject
    inputs: tuple[ArtifactRef, ...]
    is_retake: bool
    workflow_id: str = ""
    trace_id: str = ""
    parent_span_id: str = ""
    outputs: tuple[str, ...] = ()
    report_idempotent: bool = False

    @classmethod
    def from_dict(cls, value: object) -> Assignment:
        body = json_object(value)
        required = {
            "execution_id",
            "step_id",
            "attempt",
            "task_ref",
            "queue",
            "context_id",
            "timeout_seconds",
            "lease_expires_at",
            "params",
            "parameters",
            "inputs",
            "is_retake",
        }
        if missing := required - body.keys():
            raise ValueError(f"assignment is missing fields: {sorted(missing)}")
        deadline = datetime.fromisoformat(
            string(body["lease_expires_at"], "lease_expires_at").replace("Z", "+00:00")
        )
        if deadline.tzinfo is None:
            raise ValueError("lease_expires_at must include its timezone")
        inputs = body["inputs"]
        if not isinstance(inputs, list):
            raise ValueError("inputs must be an array of artifact references")
        outputs = body.get("outputs", [])
        if not isinstance(outputs, list) or any(
            not isinstance(name, str) or not name for name in outputs
        ):
            raise ValueError("outputs must be an array of nonempty names")
        names = tuple(string(name, "output name") for name in outputs)
        if len(set(names)) != len(names):
            raise ValueError("output names must be unique")
        return cls(
            execution_id=string(body["execution_id"], "execution_id"),
            step_id=string(body["step_id"], "step_id"),
            attempt=unsigned(body["attempt"], "attempt", bits=32),
            task_ref=string(body["task_ref"], "task_ref"),
            queue=string(body["queue"], "queue"),
            context_id=string(body["context_id"], "context_id"),
            timeout_seconds=unsigned(body["timeout_seconds"], "timeout_seconds"),
            lease_expires_at=deadline,
            params=json_object(body["params"]),
            parameters=json_object(body["parameters"]),
            inputs=tuple(artifact_ref(ref) for ref in inputs),
            is_retake=boolean(body["is_retake"], "is_retake"),
            workflow_id=string(body.get("workflow_id", ""), "workflow_id"),
            trace_id=string(body.get("trace_id", ""), "trace_id"),
            parent_span_id=string(body.get("parent_span_id", ""), "parent_span_id"),
            outputs=names,
            report_idempotent=boolean(body.get("report_idempotent", False), "report_idempotent"),
        )

    @property
    def path(self) -> str:
        return "/api/v1/worker/claims/" + "/".join(
            quote(str(part), safe="") for part in (self.execution_id, self.step_id, self.attempt)
        )
