"""The runtime owns composition and capacity; workflow code owns neither."""

from __future__ import annotations

import json
import threading
from datetime import UTC, datetime, timedelta
from typing import Any

import httpx
import pytest

from aiwatcher_sdk import AiwatcherClient, NullTransport
from aiwatcher_sdk.runtime import ExecutionPool, Runtime, RuntimeServices
from aiwatcher_sdk.worker import get_task_context, task
from aiwatcher_sdk.workflow import ApprovalStep, Workflow, WorkflowInput, WorkflowStep


def test_runtime_runs_one_attempt_from_its_factory_without_starting_pool_capacity() -> None:
    from test_worker import WorkerApi, assignment

    api = WorkerApi(assignment(params={}, parameters={}))
    calls = []

    def workflows(services: RuntimeServices) -> list[Workflow]:
        @task("planner.acquire", version="sha256:abc")
        def acquire() -> None:
            calls.append(get_task_context().context_id)

        return [Workflow("house", "1", (WorkflowStep("acquire", acquire),))]

    with httpx.Client(transport=httpx.MockTransport(api.handle)) as client:
        runtime = Runtime(
            name="single",
            url="http://aiwatcher.invalid",
            workflows=workflows,
            pools=[ExecutionPool("local", "planner-import", concurrency=5)],
            placement={"house@1": "local"},
            client=client,
            telemetry=AiwatcherClient(service="test", transport=NullTransport()),
        )
        assert runtime.run_attempt("import-1/acquire/1", pool="local")
        runtime.close()
        assert calls == ["import-1/acquire/1"]
        assert len(api.reports) == 1
        assert sum(request.url.path == "/api/v1/worker/claims" for request in api.requests) == 1
        assert not client.is_closed
        with pytest.raises(RuntimeError, match="fresh runtime"):
            runtime.run_attempt("import-1/acquire/1", pool="local")


def test_declaring_a_workflow_does_not_execute_its_steps() -> None:
    calls: list[bool] = []

    @task("stage", version="1")
    def stage() -> None:
        calls.append(True)

    workflow = Workflow(
        "house",
        "1",
        (
            WorkflowStep("acquire", stage),
            WorkflowStep("normalize", stage, after=("acquire",)),
        ),
    )
    assert calls == []
    assert workflow.ref == "house@1"
    assert workflow.get_tasks() == (stage,)


def test_an_approval_step_registers_no_task_and_sends_no_placement() -> None:
    # A gate waits for a person. Nothing claims it, so a worker registering a
    # task for it would advertise work it can never be handed — and a queue or
    # a timeout on the definition would sit there reading as something this
    # system does. Both are absent rather than empty.
    @task("stage", version="1")
    def stage() -> None:
        pass

    workflow = Workflow(
        "house",
        "1",
        (
            WorkflowStep("acquire", stage, outputs=("rows",)),
            ApprovalStep(
                "sign-off",
                "Import these houses?",
                choices=("approve", "reject"),
                after=("acquire",),
            ),
            WorkflowStep(
                "persist",
                stage,
                after=("sign-off",),
                inputs=(WorkflowInput("acquire", "rows"),),
            ),
        ),
    )

    assert workflow.get_tasks() == (stage,)
    steps = workflow.to_definition("local")["steps"]
    assert isinstance(steps, list)
    gate = steps[1]
    assert gate == {
        "id": "sign-off",
        "approval": {"prompt": "Import these houses?", "choices": ["approve", "reject"]},
        "after": ["acquire"],
        "inputs": [],
    }
    assert steps[0]["queue"] == "local"


def test_an_approval_step_is_ordered_and_depended_on_like_any_other() -> None:
    workflow_error = pytest.raises(ValueError, match=r"unknown")
    with workflow_error:
        Workflow("house", "1", (ApprovalStep("sign-off", "Go on?", after=("missing",)),))


@pytest.mark.parametrize("after", [("missing",), ("self",)])
def test_a_workflow_refuses_missing_dependencies_and_cycles(after: tuple[str, ...]) -> None:
    @task("stage", version="1")
    def stage() -> None:
        pass

    with pytest.raises(ValueError, match=r"unknown|cycle"):
        Workflow("house", "1", (WorkflowStep("self", stage, after=after),))


def test_runtime_factories_receive_shared_services_once_and_placement_is_required() -> None:
    telemetry = AiwatcherClient(service="test", transport=NullTransport())
    services_seen: list[RuntimeServices] = []

    @task("stage", version="1")
    def stage() -> None:
        pass

    def workflows(services: RuntimeServices) -> list[Workflow]:
        services_seen.append(services)
        return [Workflow("house", "1", (WorkflowStep("stage", stage),))]

    with Runtime(
        name="planner",
        url="http://aiwatcher.invalid",
        workflows=workflows,
        pools=[ExecutionPool("imports", "planner-import", concurrency=0)],
        placement={"house@1": "imports"},
        telemetry=telemetry,
    ) as runtime:
        runtime.start()
        runtime.start()
        assert runtime.workflows["house@1"].get_tasks() == (stage,)
        assert runtime.get_status()[0].running == 0
    assert len(services_seen) == 1
    assert services_seen[0].telemetry is telemetry
    with pytest.raises(ValueError, match="placement"):
        Runtime(
            name="planner",
            url="http://aiwatcher.invalid",
            workflows=workflows,
            pools=[ExecutionPool("imports", "planner-import")],
            placement={},
            telemetry=telemetry,
        )


def test_runtime_scales_local_capacity_and_drains_work_before_releasing_resources() -> None:
    lock = threading.Lock()
    two_running, release = threading.Event(), threading.Event()
    active = 0
    maximum = 0
    claimed = 0
    reports: list[dict[str, Any]] = []

    @task("stage", version="1")
    def stage() -> str:
        nonlocal active, maximum
        with lock:
            active += 1
            maximum = max(maximum, active)
            if active == 2:
                two_running.set()
        try:
            if not release.wait(3):
                raise TimeoutError("test did not release its tasks")
            return get_task_context().run_id
        finally:
            with lock:
                active -= 1

    def api(request: httpx.Request) -> httpx.Response:
        nonlocal claimed
        with lock:
            if request.url.path == "/api/v1/worker/claims":
                body = json.loads(request.content)
                assert body["tasks"] == ["stage@1"]
                assert body["queues"] == ["planner-import"]
                claimed += 1
                return httpx.Response(
                    200,
                    json={
                        "execution_id": f"run-{claimed}",
                        "step_id": "stage",
                        "attempt": 1,
                        "task_ref": "stage@1",
                        "queue": "planner-import",
                        "timeout_seconds": 30,
                        "context_id": f"run-{claimed}/stage/1",
                        "params": {},
                        "parameters": {},
                        "lease_expires_at": (
                            datetime.now(UTC) + timedelta(seconds=300)
                        ).isoformat(),
                        "inputs": [],
                        "is_retake": False,
                    },
                )
            if request.url.path.endswith("/result"):
                reports.append(json.loads(request.content))
                return httpx.Response(
                    200, json={"step_id": "stage", "attempt": 1, "succeeded": True}
                )
            raise AssertionError(f"unexpected request: {request.url}")

    client = httpx.Client(transport=httpx.MockTransport(api))
    with Runtime(
        name="planner",
        url="http://aiwatcher.invalid",
        workflows=[Workflow("house", "1", (WorkflowStep("stage", stage),))],
        pools=[ExecutionPool("imports", "planner-import", concurrency=0)],
        placement={"house@1": "imports"},
        client=client,
        telemetry=AiwatcherClient(service="test", transport=NullTransport()),
    ) as runtime:
        try:
            runtime.start()
            runtime.scale("imports", concurrency=2)
            assert two_running.wait(3)
            assert runtime.get_status()[0].running == 2
            runtime.scale("imports", concurrency=0)
            status = runtime.get_status()[0]
            assert (status.desired, status.running, status.draining) == (0, 0, 2)
        finally:
            release.set()
    assert maximum == 2
    assert claimed == len(reports) == 2
    assert all(report["outcome"] == "completed" for report in reports)
    assert not client.is_closed, "a caller-owned connection pool belongs to the caller"
    client.close()
    with pytest.raises(RuntimeError, match="stopped"):
        runtime.scale("imports", concurrency=1)
    assert not any(t.name.startswith("aiwatcher-runtime-") for t in threading.enumerate())


def test_runtime_surfaces_worker_failure_and_stops_all_its_pools() -> None:
    @task("stage", version="1")
    def stage() -> None:
        pass

    def api(request: httpx.Request) -> httpx.Response:
        return httpx.Response(403, json={"message": "queue token refused"})

    with httpx.Client(transport=httpx.MockTransport(api)) as client:
        runtime = Runtime(
            name="planner",
            url="http://aiwatcher.invalid",
            workflows=[Workflow("house", "1", (WorkflowStep("stage", stage),))],
            pools=[ExecutionPool("imports", "planner-import", concurrency=2)],
            placement={"house@1": "imports"},
            client=client,
            telemetry=AiwatcherClient(service="test", transport=NullTransport()),
        )
        with pytest.raises(ExceptionGroup, match="runtime workers failed") as failure:
            runtime.serve()
        assert "queue token refused" in str(failure.value.exceptions[0])
        assert runtime.get_status()[0].running == 0


@pytest.mark.parametrize("exit_type", [SystemExit, KeyboardInterrupt])
def test_runtime_supervises_base_exceptions_from_real_tasks(exit_type: type[BaseException]) -> None:
    from test_worker import WorkerApi, assignment

    api = WorkerApi(assignment(task_ref="stage@1", params={}, parameters={}))

    @task("stage", version="1")
    def stage() -> None:
        raise exit_type(7)

    with httpx.Client(transport=httpx.MockTransport(api.handle)) as client:
        runtime = Runtime(
            name="exit-test",
            url="http://aiwatcher.invalid",
            workflows=[Workflow("house", "1", (WorkflowStep("stage", stage),))],
            pools=[ExecutionPool("imports", "planner-import")],
            placement={"house@1": "imports"},
            client=client,
            telemetry=AiwatcherClient(service="test", transport=NullTransport()),
        )
        failures: list[BaseException] = []
        finished = threading.Event()

        def serve() -> None:
            try:
                runtime.serve()
            except BaseException as error:  # noqa: BLE001 — inspect supervisor outcome
                failures.append(error)
            finally:
                finished.set()

        supervisor = threading.Thread(target=serve)
        supervisor.start()
        try:
            assert finished.wait(3), "serve did not notice its worker exited"
        finally:
            runtime.stop()
            supervisor.join(3)
        assert not supervisor.is_alive()
        assert len(failures) == 1
        assert isinstance(failures[0], BaseExceptionGroup)
        assert isinstance(failures[0].exceptions[0], exit_type)
        assert api.reports == [], "process control must not become a user-code report"
        assert runtime.get_status()[0].running == 0
        with pytest.raises(RuntimeError, match="stopped"):
            runtime.scale("imports", concurrency=1)
        runtime.close()  # already closed; no repeated failure or resource release


@pytest.mark.parametrize("during_scale", [False, True])
@pytest.mark.parametrize("failed_slot", [1, 2])
def test_partial_start_failure_drains_started_slots_and_closes_owned_resources(
    monkeypatch: pytest.MonkeyPatch,
    during_scale: bool,
    failed_slot: int,
) -> None:
    @task("stage", version="1")
    def stage() -> None:
        pass

    baseline = set(threading.enumerate())
    real_start = threading.Thread.start
    starts = 0

    def start(thread: threading.Thread) -> None:
        nonlocal starts
        if thread.name.startswith("aiwatcher-runtime-"):
            starts += 1
            if starts == failed_slot:
                raise RuntimeError("can't start new thread")
        real_start(thread)

    owned_clients: list[httpx.Client] = []
    real_client = httpx.Client

    def client_factory(**kwargs: Any) -> httpx.Client:
        client = real_client(transport=httpx.MockTransport(lambda _: httpx.Response(204)), **kwargs)
        owned_clients.append(client)
        return client

    monkeypatch.setattr(httpx, "Client", client_factory)
    runtime = Runtime(
        name="start-test",
        url="http://aiwatcher.invalid",
        workflows=[Workflow("house", "1", (WorkflowStep("stage", stage),))],
        pools=[ExecutionPool("imports", "planner-import", concurrency=0 if during_scale else 2)],
        placement={"house@1": "imports"},
    )
    monkeypatch.setattr(threading.Thread, "start", start)
    try:
        with pytest.raises(RuntimeError, match="can't start new thread"):
            runtime.start()
            if during_scale:
                runtime.scale("imports", concurrency=2)
    finally:
        runtime.close()
    status = runtime.get_status()[0]
    assert (status.running, status.draining) == (0, 0)
    assert len(owned_clients) == failed_slot
    assert all(client.is_closed for client in owned_clients)
    assert not (set(threading.enumerate()) - baseline), (
        "runtime leaked a worker or telemetry thread"
    )


def test_worker_cleanup_failure_stops_runtime_and_still_closes_shared_telemetry(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    @task("stage", version="1")
    def stage() -> None:
        pass

    baseline = set(threading.enumerate())
    closed: list[bool] = []

    class FailingCloseClient(httpx.Client):
        def close(self) -> None:
            super().close()
            closed.append(self.is_closed)
            raise OSError("connection pool cleanup failed")

    def client_factory(**kwargs: Any) -> httpx.Client:
        # System adapter only; runtime and workers execute their actual lifecycle.
        return FailingCloseClient(
            transport=httpx.MockTransport(lambda _: httpx.Response(403)), **kwargs
        )

    monkeypatch.setattr(httpx, "Client", client_factory)
    runtime = Runtime(
        name="cleanup-test",
        url="http://aiwatcher.invalid",
        workflows=[Workflow("house", "1", (WorkflowStep("stage", stage),))],
        pools=[ExecutionPool("imports", "planner-import", concurrency=2)],
        placement={"house@1": "imports"},
    )
    with pytest.raises(ExceptionGroup) as failure:
        runtime.serve()
    assert closed == [True, True]
    assert any("worker cleanup failed" in str(error) for error in failure.value.exceptions)
    assert not (set(threading.enumerate()) - baseline)


def test_workflow_and_task_declarations_do_not_load_execution_infrastructure() -> None:
    import subprocess
    import sys

    result = subprocess.run(
        [
            sys.executable,
            "-c",
            "import sys; from aiwatcher_sdk.task import task; "
            "from aiwatcher_sdk.workflow import Workflow; "
            "assert not {'httpx', 'tenacity', 'aiwatcher_sdk.api', 'aiwatcher_sdk.worker'} "
            "& sys.modules.keys()",
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr


def test_runtime_registers_and_starts_a_pinned_definition_with_an_idempotency_key() -> None:
    from test_worker import validate_contract

    requests: list[httpx.Request] = []

    @task("stage", version="1")
    def stage() -> None:
        pass

    def api(request: httpx.Request) -> httpx.Response:
        requests.append(request)
        if request.url.path == "/api/v1/workflow-definitions":
            definition = json.loads(request.content)
            validate_contract("WorkflowSpec", definition)
            assert definition["steps"][0]["queue"] == "planner-import"
            return httpx.Response(200, json={"revision": "ab" * 32})
        if request.url.path == "/api/v1/executions":
            body = json.loads(request.content)
            validate_contract("StartExecutionBody", body)
            assert body["target"]["revision"] == "ab" * 32
            assert request.headers["idempotency-key"] == "same-request"
            return httpx.Response(202, json={"execution": {"execution_id": "run-1"}})
        if request.url.path.endswith("/commands/retry"):
            return httpx.Response(200, json={"execution": {"execution_id": "run-1"}})
        return httpx.Response(200, json={"execution": {"state": {"state_type": "completed"}}})

    with (
        httpx.Client(transport=httpx.MockTransport(api)) as client,
        Runtime(
            name="test",
            url="http://aiwatcher.invalid",
            workflows=[Workflow("house", "1", (WorkflowStep("stage", stage),))],
            pools=[ExecutionPool("imports", "planner-import", concurrency=0)],
            placement={"house@1": "imports"},
            client=client,
            telemetry=AiwatcherClient(service="test", transport=NullTransport()),
        ) as runtime,
    ):
        handle = runtime.run("house@1", parameters={"house": 1}, idempotency_key="same-request")
        assert handle.execution_id == "run-1"
        assert handle.wait(timeout=0)["execution"] == {"state": {"state_type": "completed"}}
        handle.retry("stage")
    assert requests[-1].url.path == "/api/v1/executions/run-1/steps/stage/commands/retry"
