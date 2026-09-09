"""Worker behavior at the HTTP seam; no scheduler is implemented by this stand-in."""

from __future__ import annotations

import hashlib
import json
import subprocess
import sys
import threading
from collections.abc import Callable
from concurrent.futures import ThreadPoolExecutor
from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import Any

import httpx
import pytest
from jsonschema import Draft202012Validator

from aiwatcher_sdk import AiwatcherClient, NullTransport
from aiwatcher_sdk.worker import (
    Fail,
    InputRequired,
    TaskContext,
    TaskError,
    Worker,
    WorkerError,
    get_task_context,
    task,
)

CONTRACT = json.loads((Path(__file__).parents[3] / "contracts/openapi.json").read_text())


def validate_contract(schema: str, value: object) -> None:
    Draft202012Validator(
        {
            "$ref": f"#/components/schemas/{schema}",
            "components": CONTRACT["components"],
        }
    ).validate(value)


def reference(name: str, rows: object = None) -> dict[str, Any]:
    return {
        "name": name,
        "uri": "artifact://" + name,
        "kind": "rows",
        "digest": hashlib.sha256(json.dumps(rows).encode()).hexdigest(),
    }


def assignment(**changes: Any) -> dict[str, Any]:
    return {
        "execution_id": "import-1",
        "workflow_id": "test-import",
        "trace_id": "ab" * 16,
        "parent_span_id": "cd" * 8,
        "step_id": "acquire",
        "attempt": 1,
        "task_ref": "planner.acquire@sha256:abc",
        "queue": "planner-import",
        "context_id": "import-1/acquire/1",
        "timeout_seconds": 30,
        "lease_expires_at": (datetime.now(UTC) + timedelta(seconds=300)).isoformat(),
        "params": {"stage": "acquire"},
        "parameters": {"job": "house-1", "stage": "default"},
        "inputs": [reference("source", [{"house": 1}])],
        "is_retake": False,
        "outputs": [],
        "answers": [],
        "report_idempotent": False,
        **changes,
    }


class WorkerApi:
    """Answers the checked-in WorkAssignment/WorkerReport protocol."""

    def __init__(self, *claims: dict[str, Any]) -> None:
        self.claims = list(claims)
        self.requests: list[httpx.Request] = []
        self.reports: list[dict[str, Any]] = []
        self.artifacts: dict[str, list[Any]] = {"source": [{"house": 1}]}
        self.heartbeat_status = 204
        self.report_error: Exception | None = None
        self.beaten = threading.Event()

    def handle(self, request: httpx.Request) -> httpx.Response:
        self.requests.append(request)
        path = request.url.path
        if path == "/api/v1/worker/claims":
            validate_contract("ClaimRequest", json.loads(request.content))
            if not self.claims:
                return httpx.Response(204)
            body = self.claims.pop(0)
            validate_contract("WorkAssignment", body)
            return httpx.Response(200, json=body)
        if path.endswith("/heartbeat"):
            validate_contract("WorkerBody", json.loads(request.content))
            self.beaten.set()
            return httpx.Response(self.heartbeat_status)
        if path.endswith("/result"):
            report = json.loads(request.content)
            validate_contract("WorkerReport", report)
            self.reports.append(report)
            if self.report_error:
                raise self.report_error
            settled = {
                "step_id": path.split("/")[-3],
                "attempt": int(path.split("/")[-2]),
                "succeeded": report["outcome"] == "completed",
                "outcome": report["outcome"],
            }
            validate_contract("Settled", settled)
            return httpx.Response(200, json=settled)
        name = path.rsplit("/", 1)[-1]
        if "/inputs/" in path:
            body = {"rows": self.artifacts[name]}
            validate_contract("RowsBody", body)
            return httpx.Response(200, json=body)
        if "/outputs/" in path:
            body = json.loads(request.content)
            validate_contract("RowsBody", body)
            self.artifacts[name] = body["rows"]
            ref = reference(name, body["rows"])
            validate_contract("ArtifactRef", ref)
            return httpx.Response(200, json=ref)
        raise AssertionError(f"unexpected request: {request.method} {request.url}")


def worker(
    api: WorkerApi,
    function: Callable[[dict[str, Any], TaskContext], Any],
    *,
    telemetry: AiwatcherClient | None = None,
) -> Worker:
    @task("planner.acquire", version="sha256:abc")
    def entry(**parameters: Any) -> Any:
        return function(parameters, get_task_context())

    return Worker(
        "http://aiwatcher.invalid",
        "queue-token",
        queues=["planner-import"],
        tasks=[entry],
        name="worker-1",
        client=httpx.Client(transport=httpx.MockTransport(api.handle)),
        telemetry=telemetry or AiwatcherClient(service="test", transport=NullTransport()),
    )


@pytest.mark.parametrize("succeeds", [True, False])
def test_lost_report_response_retries_only_the_report_when_server_supports_receipts(
    succeeds: bool,
) -> None:
    api = WorkerApi(assignment(report_idempotent=True))
    invocations = []
    original = api.handle

    def lose_first_reply(request: httpx.Request) -> httpx.Response:
        response = original(request)
        if request.url.path.endswith("/result") and len(api.reports) == 1:
            raise httpx.ReadTimeout("committed but reply was lost")
        return response

    api.handle = lose_first_reply  # type: ignore[method-assign]

    def execute(inputs: dict[str, Any], ctx: TaskContext) -> None:
        invocations.append(ctx.context_id)
        if not succeeds:
            raise TaskError("temporary outage", classification="transient")

    with worker(api, execute) as instance:
        assert instance.run_once()
    assert invocations == ["import-1/acquire/1"]
    assert len(api.reports) == 2
    assert api.reports[0] == api.reports[1]


def test_a_task_that_omits_a_declared_artifact_reports_validation_failure() -> None:
    api = WorkerApi(assignment(outputs=["required"]))
    with worker(api, lambda inputs, ctx: {"count": 1}) as instance:
        assert instance.run_once()
    assert api.reports[0]["outcome"] == "failed"
    assert api.reports[0]["class"] == "validation"
    assert "required" in api.reports[0]["message"]


def test_run_attempt_sends_the_exact_key_and_executes_once() -> None:
    api = WorkerApi(assignment())
    with worker(api, lambda inputs, ctx: {"count": 1}) as instance:
        assert instance.run_attempt("import-1/acquire/1")
    body = json.loads(api.requests[0].content)
    assert body["attempt"] == {"execution_id": "import-1", "step_id": "acquire", "attempt": 1}
    assert len(api.reports) == 1


def test_run_attempt_does_not_execute_a_neighbour_or_poll_when_target_is_unavailable() -> None:
    called = []
    api = WorkerApi(assignment(step_id="other"))
    with worker(api, lambda inputs, ctx: called.append(1)) as instance:
        with pytest.raises(WorkerError, match="different attempt"):
            instance.run_attempt("import-1/acquire/1")
        with pytest.raises(WorkerError, match="not claimable"):
            instance.run_attempt("import-1/acquire/1")
    assert not called
    assert not api.reports
    assert len(api.requests) == 2


def test_two_stages_exchange_artifacts_and_keep_the_execution_identity() -> None:
    api = WorkerApi(
        assignment(),
        assignment(
            step_id="persist",
            context_id="import-1/persist/1",
            params={"stage": "persist"},
            inputs=[reference("normalized", [{"house": 1}])],
        ),
    )
    contexts: list[str] = []

    def stage(inputs: dict[str, Any], ctx: TaskContext) -> dict[str, str]:
        assert inputs["job"] == "house-1"
        assert ctx.run_id == ctx.workflow_run_id == "import-1"
        contexts.append(ctx.context_id)
        if inputs["stage"] == "acquire":
            ctx.write_artifact("normalized", ctx.read_artifact("source"))
        else:
            assert ctx.read_artifact("normalized") == [{"house": 1}]
        return {"stage": inputs["stage"]}

    with worker(api, stage) as process:
        assert process.run_once()
        assert process.run_once()
        assert not process.run_once()
    assert contexts == ["import-1/acquire/1", "import-1/persist/1"]
    assert [report["outcome"] for report in api.reports] == ["completed", "completed"]
    assert api.reports[0]["outputs"][0]["name"] == "normalized"
    assert api.reports[1]["outputs"] == []
    claim = json.loads(api.requests[0].content)
    assert claim == {
        "worker": "worker-1",
        "queues": ["planner-import"],
        "tasks": ["planner.acquire@sha256:abc"],
    }
    assert all(request.headers["authorization"] == "Bearer queue-token" for request in api.requests)
    artifact_requests = [
        r for r in api.requests if "/inputs/" in r.url.path or "/outputs/" in r.url.path
    ]
    assert all(r.url.params["worker"] == "worker-1" for r in artifact_requests)


def test_a_heartbeat_runs_while_user_code_is_busy() -> None:
    from aiwatcher_sdk.api import Transport
    from aiwatcher_sdk.worker.assignment import Assignment
    from aiwatcher_sdk.worker.http import HttpAttemptAPI

    api = WorkerApi()
    claim = Assignment.from_dict(assignment())
    # A supplied wall clock determines the interval without a lease expiring
    # while CI is scheduling the worker thread.
    now = claim.lease_expires_at - timedelta(seconds=0.02)
    with httpx.Client(transport=httpx.MockTransport(api.handle)) as client:
        transport = Transport(
            "http://aiwatcher.invalid", client=client, attempts=1, error=WorkerError
        )
        ctx = TaskContext(
            claim,
            HttpAttemptAPI(transport, claim, "worker-1"),
            AiwatcherClient(service="test", transport=NullTransport()),
            now=lambda: now,
        )
        try:
            ctx.start()
            with ctx.activate():
                assert api.beaten.wait(3), "heartbeat did not run independently of task code"
        finally:
            ctx.stop()
    assert not any(t.name.startswith("aiwatcher-heartbeat-") for t in threading.enumerate())


def test_a_lost_lease_discards_the_result_even_when_task_code_ignores_it() -> None:
    api = WorkerApi(assignment())
    api.heartbeat_status = 409

    def stage(inputs: dict[str, Any], ctx: TaskContext) -> str:
        with pytest.raises(WorkerError):
            ctx.heartbeat()
        assert ctx.cancelled
        return "do not report this"

    with worker(api, stage) as process:
        assert process.run_once()
    assert api.reports == []


def test_a_completion_whose_reply_was_lost_is_not_repeated_or_reported_as_failure() -> None:
    api = WorkerApi(assignment())
    api.report_error = httpx.ReadTimeout("reply lost after commit")
    with worker(api, lambda inputs, ctx: {"done": True}) as process, pytest.raises(WorkerError):
        process.run_once()
    assert len(api.reports) == 1
    assert api.reports[0]["outcome"] == "completed"


def test_a_claim_whose_reply_was_lost_is_not_repeated() -> None:
    api = WorkerApi()

    def fail(request: httpx.Request) -> httpx.Response:
        api.requests.append(request)
        raise httpx.ReadTimeout("the server may already have leased a task")

    api.handle = fail  # type: ignore[method-assign]
    with worker(api, lambda inputs, ctx: None) as process, pytest.raises(WorkerError):
        process.run_once()
    assert len(api.requests) == 1


@pytest.mark.parametrize(
    ("error", "classification"),
    [
        (ValueError("bad code"), "user_code"),
        (TaskError("model unavailable", classification="transient"), "transient"),
        (TaskError("invalid input", classification="validation"), "validation"),
    ],
)
def test_task_failure_is_classified_and_the_server_decides_the_retry(
    error: Exception,
    classification: str,
) -> None:
    api = WorkerApi(assignment())

    def stage(inputs: dict[str, Any], ctx: TaskContext) -> None:
        raise error

    with worker(api, stage) as process:
        assert process.run_once()
    assert api.reports == [
        {"worker": "worker-1", "outcome": "failed", "class": classification, "message": str(error)}
    ]


def test_shutdown_does_not_turn_a_keyboard_interrupt_into_a_user_code_failure() -> None:
    api = WorkerApi(assignment())

    def stage(inputs: dict[str, Any], ctx: TaskContext) -> None:
        raise KeyboardInterrupt

    with worker(api, stage) as process, pytest.raises(KeyboardInterrupt):
        process.run_once()
    assert api.reports == []
    assert not any(t.name.startswith("aiwatcher-heartbeat-") for t in threading.enumerate())


def test_an_unadvertised_task_is_never_executed() -> None:
    api = WorkerApi(assignment(task_ref="planner.acquire@different-version"))
    calls: list[bool] = []
    with (
        worker(api, lambda inputs, ctx: calls.append(True)) as process,
        pytest.raises(WorkerError, match="capabilities"),
    ):
        process.run_once()
    assert calls == []
    assert api.reports == []


def test_the_inline_limit_counts_encoded_bytes_and_directs_data_into_artifacts() -> None:
    api = WorkerApi(assignment())
    with worker(api, lambda inputs, ctx: "ą" * 32768) as process:
        process.run_once()
    assert api.reports[0]["class"] == "validation"
    assert "artifact" in api.reports[0]["message"]


def test_an_expired_assignment_never_enters_user_code() -> None:
    api = WorkerApi(
        assignment(lease_expires_at=(datetime.now(UTC) - timedelta(seconds=1)).isoformat())
    )
    calls: list[bool] = []
    with worker(api, lambda inputs, ctx: calls.append(True)) as process:
        assert process.run_once()
    assert calls == []
    assert api.reports == []


def test_a_task_deadline_is_checked_before_running_the_function() -> None:
    api = WorkerApi(assignment(timeout_seconds=0))
    calls: list[bool] = []
    with worker(api, lambda inputs, ctx: calls.append(True)) as process:
        process.run_once()
    assert calls == []
    assert api.reports[0]["class"] == "timeout"


def test_a_retake_keeps_the_servers_idempotency_key() -> None:
    api = WorkerApi(assignment(is_retake=True))

    def stage(inputs: dict[str, Any], ctx: TaskContext) -> None:
        assert ctx.assignment.is_retake
        assert ctx.context_id == "import-1/acquire/1"

    with worker(api, stage) as process:
        process.run_once()
    assert api.reports[0]["outcome"] == "completed"


def test_a_task_keeps_its_signature_and_can_be_called_without_a_server() -> None:
    @task("add", version="1")
    def add(left: int, right: int = 2) -> int:
        return left + right

    answer: int = add(3, right=4)
    assert answer == 7
    assert add(3) == 5
    assert add.ref == "add@1"
    assert str(add.signature) == "(left: 'int', right: 'int' = 2) -> 'int'"
    with pytest.raises(ValueError, match="nonempty"):
        task("add", version="")(add.fn)
    with pytest.raises(RuntimeError, match="no managed task"):
        get_task_context()


def test_a_worker_binds_normal_arguments_and_resets_the_context_afterwards() -> None:
    api = WorkerApi(assignment(parameters={"job": "house-1"}, params={}))

    @task("planner.acquire", version="sha256:abc")
    def acquire(job: str, floor: int = 2) -> str:
        assert get_task_context().context_id == "import-1/acquire/1"
        return f"{job}:{floor}"

    with Worker(
        "http://aiwatcher.invalid",
        queues=["planner-import"],
        tasks=[acquire],
        client=httpx.Client(transport=httpx.MockTransport(api.handle)),
        telemetry=AiwatcherClient(service="test", transport=NullTransport()),
    ) as process:
        process.run_once()
    assert api.reports[0]["result"] == "house-1:2"
    with pytest.raises(RuntimeError, match="no managed task"):
        get_task_context()


def test_a_bad_parameter_binding_is_validation_before_user_code_runs() -> None:
    api = WorkerApi(assignment(parameters={"unknown": True}, params={}))
    calls: list[bool] = []

    @task("planner.acquire", version="sha256:abc")
    def acquire(job: str) -> None:
        calls.append(True)

    with Worker(
        "http://aiwatcher.invalid",
        queues=["planner-import"],
        tasks=[acquire],
        client=httpx.Client(transport=httpx.MockTransport(api.handle)),
        telemetry=AiwatcherClient(service="test", transport=NullTransport()),
    ) as process:
        process.run_once()
    assert calls == []
    assert api.reports[0]["class"] == "validation"


def test_decorating_an_unselected_task_does_not_advertise_it_to_the_server() -> None:
    @task("not-selected", version="1")
    def unrelated() -> None:
        pass

    api = WorkerApi()
    with worker(api, lambda inputs, ctx: None) as process:
        process.run_once()
    assert json.loads(api.requests[0].content)["tasks"] == ["planner.acquire@sha256:abc"]


def test_context_is_reset_after_user_code_fails() -> None:
    api = WorkerApi(assignment())

    def stage(inputs: dict[str, Any], ctx: TaskContext) -> None:
        assert get_task_context() is ctx
        raise ValueError("failure inside context")

    with worker(api, stage) as process:
        process.run_once()
    with pytest.raises(RuntimeError, match="no managed task"):
        get_task_context()


def test_two_workers_in_one_process_do_not_share_the_task_context() -> None:
    ready = threading.Barrier(2)

    def stage(inputs: dict[str, Any], ctx: TaskContext) -> str:
        ready.wait(timeout=3)
        assert get_task_context() is ctx
        return get_task_context().run_id

    first, second = WorkerApi(assignment()), WorkerApi(assignment(execution_id="import-2"))
    with worker(first, stage) as one, worker(second, stage) as two, ThreadPoolExecutor(2) as pool:
        futures = [pool.submit(one.run_once), pool.submit(two.run_once)]
        assert all(future.result(timeout=5) for future in futures)
    assert first.reports[0]["result"] == "import-1"
    assert second.reports[0]["result"] == "import-2"


def test_a_worker_refuses_two_functions_with_the_same_pinned_reference() -> None:
    @task("duplicate", version="1")
    def first() -> None:
        pass

    @task("duplicate", version="1")
    def second() -> None:
        pass

    with pytest.raises(ValueError, match="duplicate"):
        Worker("http://aiwatcher.invalid", queues=["planner-import"], tasks=[first, second])


def test_importing_telemetry_does_not_import_the_worker_or_httpx() -> None:
    result = subprocess.run(
        [
            sys.executable,
            "-c",
            "import sys, aiwatcher_sdk; "
            "assert 'aiwatcher_sdk.worker' not in sys.modules; assert 'httpx' not in sys.modules",
        ],
        check=False,
        capture_output=True,
        text=True,
    )
    assert result.returncode == 0, result.stderr


@pytest.mark.parametrize(
    "changes",
    [
        {"attempt": True},
        {"attempt": -1},
        {"attempt": 2**32},
        {"timeout_seconds": "30"},
        {"execution_id": 10},
        {"is_retake": "false"},
        {"parameters": []},
        {"params": {"bad": float("nan")}},
        {"inputs": [{"name": "source"}]},
        {"inputs": {}},
        {"lease_expires_at": "2026-09-08T12:00:00"},
    ],
)
def test_invalid_assignments_are_rejected_before_entering_user_code(
    changes: dict[str, Any],
) -> None:
    from aiwatcher_sdk.worker.assignment import Assignment

    with pytest.raises(ValueError):
        Assignment.from_dict(assignment(**changes))


@pytest.mark.parametrize("body", [{}, [], {"attempt": 1}, assignment(inputs=[{"name": "source"}])])
def test_malformed_claim_responses_are_protocol_errors(body: object) -> None:
    calls: list[bool] = []

    @task("planner.acquire", version="sha256:abc")
    def stage() -> None:
        calls.append(True)

    with (
        httpx.Client(
            transport=httpx.MockTransport(lambda _: httpx.Response(200, json=body))
        ) as client,
        Worker(
            "http://aiwatcher.invalid",
            queues=["planner-import"],
            tasks=[stage],
            client=client,
            telemetry=AiwatcherClient(service="test", transport=NullTransport()),
        ) as process,
        pytest.raises(WorkerError, match="invalid worker assignment"),
    ):
        process.run_once()
    assert calls == []


def test_contract_rejects_the_incomplete_artifact_used_before_review() -> None:
    from jsonschema import ValidationError

    with pytest.raises(ValidationError, match="digest"):
        validate_contract("ArtifactRef", {"name": "rows", "uri": "artifact://rows", "kind": "rows"})
    with pytest.raises(ValidationError):
        validate_contract(
            "WorkerReport",
            {"worker": "test", "outcome": "completed", "outputs": [{"name": "rows"}]},
        )


@pytest.mark.parametrize(
    "reply",
    [
        {"name": "normalized", "uri": "artifact://normalized"},
        reference("another-output"),
        {"rows": [1]},
    ],
)
def test_invalid_artifact_response_is_not_reported_as_a_user_failure(reply: dict[str, Any]) -> None:
    api = WorkerApi(assignment())

    def handle(request: httpx.Request) -> httpx.Response:
        if "/outputs/" in request.url.path:
            return httpx.Response(200, json=reply)
        return api.handle(request)

    @task("planner.acquire", version="sha256:abc")
    def stage(**kwargs: Any) -> None:
        get_task_context().write_artifact("normalized", [{"house": 1}])

    with (
        httpx.Client(transport=httpx.MockTransport(handle)) as client,
        Worker(
            "http://aiwatcher.invalid",
            queues=["planner-import"],
            tasks=[stage],
            client=client,
            telemetry=AiwatcherClient(service="test", transport=NullTransport()),
        ) as process,
        pytest.raises(WorkerError, match="invalid artifact reference"),
    ):
        process.run_once()
    assert api.reports == []


@pytest.mark.parametrize(
    "reply",
    [
        {},
        {"step_id": "wrong", "attempt": 1, "succeeded": True, "outcome": "completed"},
        {"step_id": "acquire", "attempt": True, "succeeded": True, "outcome": "completed"},
        # A settlement that says the attempt parked, answering a report that
        # said it finished. `succeeded` alone could not tell these apart.
        {"step_id": "acquire", "attempt": 1, "succeeded": False, "outcome": "parked"},
    ],
)
def test_invalid_settlement_never_repeats_or_contradicts_a_report(reply: dict[str, Any]) -> None:
    api = WorkerApi(assignment())

    def handle(request: httpx.Request) -> httpx.Response:
        response = api.handle(request)
        if request.url.path.endswith("/result"):
            return httpx.Response(200, json=reply)
        return response

    @task("planner.acquire", version="sha256:abc")
    def stage(**kwargs: Any) -> None:
        pass

    with (
        httpx.Client(transport=httpx.MockTransport(handle)) as client,
        Worker(
            "http://aiwatcher.invalid",
            queues=["planner-import"],
            tasks=[stage],
            client=client,
            telemetry=AiwatcherClient(service="test", transport=NullTransport()),
        ) as process,
        pytest.raises(WorkerError, match="invalid settlement"),
    ):
        process.run_once()
    assert len(api.reports) == 1
    assert api.reports[0]["outcome"] == "completed"


# ── Stopping to ask, mid-attempt ─────────────────────────────────────────────


def test_a_task_that_asks_parks_the_attempt_instead_of_failing_it() -> None:
    """The whole of §41's protocol change, from the worker's end.

    A capability hook that wants a tool call approved raises out of the task.
    That is neither a result nor a failure, and reporting it as either would be
    wrong in a way nothing downstream could see: a failure spends the retry
    budget on a question, and a completion publishes rows nobody approved.
    """
    api = WorkerApi(assignment())

    def execute(inputs: dict[str, Any], ctx: TaskContext) -> None:
        ctx.ask(
            "Send this email?",
            choices=["approve", "reject"],
            timeout_seconds=3600,
            on_timeout=Fail(),
        )
        raise AssertionError("the task carried on past a question nobody answered")

    with worker(api, execute) as instance:
        assert instance.run_once()

    assert api.reports[0]["outcome"] == "parked"
    assert api.reports[0]["prompt"] == "Send this email?"
    assert api.reports[0]["choices"] == ["approve", "reject"]
    assert api.reports[0]["on_timeout"] == {"on": "fail"}
    # A gate only ever raises the editor floor; nobody named a role here.
    assert api.reports[0]["role"] == "editor"


def test_the_resumed_attempt_reads_the_answer_where_it_stopped() -> None:
    """The other half: attempt two runs the same task from the beginning.

    So the work before the question happens twice — the rule every retry already
    lives under. What must not happen twice is the *question*: an ``ask`` that
    parked again with the answer already in hand would be a run that never moves.
    """
    api = WorkerApi(
        assignment(
            attempt=2,
            context_id="import-1/acquire/2",
            answers=[{"attempt": 1, "answered_by": "mkubasz@gmail.com", "response": "approve"}],
        )
    )
    seen: list[Any] = []

    def execute(inputs: dict[str, Any], ctx: TaskContext) -> dict[str, Any]:
        seen.append(ctx.ask("Send this email?", choices=["approve", "reject"]))
        return {"sent": True}

    with worker(api, execute) as instance:
        assert instance.run_once()

    assert seen == ["approve"]
    assert api.reports[0]["outcome"] == "completed"
    assert api.reports[0]["result"] == {"sent": True}


def test_a_task_that_asks_twice_consumes_its_answers_in_order() -> None:
    """Why the assignment carries a list rather than the last answer.

    Attempt three replays past *both* earlier questions. Given only the most
    recent, the first ``ask`` would park again and the run would never reach the
    second — which is the failure that looks like a worker doing its job.
    """
    api = WorkerApi(
        assignment(
            attempt=3,
            context_id="import-1/acquire/3",
            answers=[
                {"attempt": 1, "answered_by": "a@example.com", "response": "approve"},
                {"attempt": 2, "answered_by": "b@example.com", "response": "second"},
            ],
        )
    )
    seen: list[Any] = []

    def execute(inputs: dict[str, Any], ctx: TaskContext) -> None:
        seen.append(ctx.ask("First?"))
        seen.append(ctx.ask("Second?"))

    with worker(api, execute) as instance:
        assert instance.run_once()

    assert seen == ["approve", "second"]
    assert api.reports[0]["outcome"] == "completed"


def test_a_third_question_parks_again_rather_than_running_out_of_answers() -> None:
    """The answers run out exactly where the work has not been approved yet."""
    api = WorkerApi(
        assignment(
            attempt=2,
            context_id="import-1/acquire/2",
            answers=[{"attempt": 1, "answered_by": "a@example.com", "response": "approve"}],
        )
    )

    def execute(inputs: dict[str, Any], ctx: TaskContext) -> None:
        ctx.ask("First?")
        ctx.ask("Second?")

    with worker(api, execute) as instance:
        assert instance.run_once()

    assert api.reports[0]["outcome"] == "parked"
    assert api.reports[0]["prompt"] == "Second?"


def test_asking_is_raised_rather_than_returned_so_a_task_cannot_carry_on() -> None:
    """The telemetry client's rule, the other way round.

    Telemetry swallows because it must never take an agent down. This must: a
    task that carried on past an unapproved tool call would perform exactly the
    side effect the question exists to hold back, and a returned sentinel is
    something a caller can ignore by accident.
    """
    api = WorkerApi(assignment())
    reached: list[str] = []

    def execute(inputs: dict[str, Any], ctx: TaskContext) -> None:
        try:
            ctx.ask("May I?")
        except InputRequired:
            reached.append("unwound")
            raise

    with worker(api, execute) as instance:
        assert instance.run_once()

    assert reached == ["unwound"]
    assert api.reports[0]["outcome"] == "parked"
