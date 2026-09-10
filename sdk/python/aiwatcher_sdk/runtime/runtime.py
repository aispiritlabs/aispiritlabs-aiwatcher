"""The composition root: workflows, shared services, capacity and lifecycle."""

from __future__ import annotations

import threading
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from types import TracebackType
from typing import Any, Self

import httpx

from aiwatcher_sdk import AiwatcherClient
from aiwatcher_sdk.runtime.executions import ExecutionClient, ExecutionHandle, RegisteredWorkflow
from aiwatcher_sdk.runtime.pool import ExecutionPool, PoolStatus
from aiwatcher_sdk.worker import AttemptRef, Task, Worker
from aiwatcher_sdk.workflow import Workflow


@dataclass(frozen=True)
class RuntimeServices:
    telemetry: AiwatcherClient


@dataclass
class WorkerSlot:
    pool: str
    worker: Worker
    thread: threading.Thread | None = None
    retiring: bool = False
    succeeded: bool = False


class Runtime:
    """Host workflow implementations through aiwatcher's managed execution API.

    ``placement`` maps workflow refs to pool names. Capacity is local to this
    runtime process; it is not a global workflow limit or a Kubernetes replica
    count. Workflow factories receive shared services once during setup.

    ``on_close`` is what the application's own composition root releases: the
    agent runtime whose stores flush on close, say. The runtime owns it from the
    moment it is handed over — it is called after the workers have stopped, so
    no task is still inside it, and before telemetry closes, so what it reports
    on the way out is still sent. It is called once, and also when this
    constructor refuses, because the caller has already handed it over.
    """

    def __init__(
        self,
        *,
        name: str,
        url: str,
        workflows: Sequence[Workflow] | Callable[[RuntimeServices], Sequence[Workflow]],
        pools: Sequence[ExecutionPool],
        placement: Mapping[str, str],
        token: str | None = None,
        telemetry: AiwatcherClient | None = None,
        client: httpx.Client | None = None,
        poll_interval: float = 1.0,
        on_close: Callable[[], None] | None = None,
    ) -> None:
        self._on_close = on_close
        if not name.strip():
            raise ValueError("a runtime needs a name")
        if poll_interval <= 0:
            raise ValueError("poll_interval must be positive")
        self.name = name
        self._url, self._token = url, token
        self._client, self._poll_interval = client, poll_interval
        self._pools = {pool.name: pool for pool in pools}
        if not pools or len(self._pools) != len(pools):
            raise ValueError("runtime pools must be nonempty and uniquely named")
        if len({pool.queue for pool in pools}) != len(pools):
            raise ValueError("local pools need distinct queues to keep their capacity independent")
        self._owns_telemetry = telemetry is None
        self.services = RuntimeServices(
            telemetry or AiwatcherClient(service=name, base_url=url, token=token)
        )
        try:
            definitions = tuple(workflows(self.services) if callable(workflows) else workflows)
            self.workflows = {workflow.ref: workflow for workflow in definitions}
            if not definitions or len(self.workflows) != len(definitions):
                raise ValueError("a runtime needs uniquely versioned workflows")
            if set(placement) != set(self.workflows):
                raise ValueError("placement must name every workflow reference exactly once")
            if unknown := set(placement.values()) - set(self._pools):
                raise ValueError(f"unknown execution pools: {sorted(unknown)}")
            self._tasks: dict[str, dict[str, Task[Any, Any]]] = {pool.name: {} for pool in pools}
            for workflow in definitions:
                tasks = self._tasks[placement[workflow.ref]]
                for task in workflow.get_tasks():
                    if task.ref in tasks and tasks[task.ref] is not task:
                        raise ValueError(f"conflicting implementations of task {task.ref}")
                    tasks[task.ref] = task
            if any(not tasks for tasks in self._tasks.values()):
                raise ValueError("every execution pool must host at least one workflow")
        except BaseException:
            self._release()
            if self._owns_telemetry:
                self.services.telemetry.close()
            raise
        self._placement = dict(placement)
        self._control: ExecutionClient | None = None
        self._desired = {pool.name: pool.concurrency for pool in pools}
        self._slots: list[WorkerSlot] = []
        self._lock = threading.RLock()
        self._stopped = threading.Event()
        self._errors: list[BaseException] = []
        self._started = False
        self._single_attempt = False
        self._closed = False

    def register(self) -> dict[str, RegisteredWorkflow]:
        """Publish the runtime's workflows and placement, idempotently by content."""
        client = self._execution_client()
        return {
            ref: client.register(workflow, queue=self._pools[self._placement[ref]].queue)
            for ref, workflow in self.workflows.items()
        }

    def run(
        self,
        workflow: str | Workflow,
        *,
        parameters: Mapping[str, object] | None = None,
        idempotency_key: str | None = None,
    ) -> ExecutionHandle:
        """Register and start a pinned workflow; local capacity starts independently."""
        ref = workflow.ref if isinstance(workflow, Workflow) else workflow
        if ref not in self.workflows:
            raise ValueError(f"workflow {ref!r} is not hosted by this runtime")
        client = self._execution_client()
        registered = client.register(
            self.workflows[ref], queue=self._pools[self._placement[ref]].queue
        )
        return client.start(registered, parameters=parameters, idempotency_key=idempotency_key)

    def _execution_client(self) -> ExecutionClient:
        with self._lock:
            if self._closed:
                raise RuntimeError("runtime is closed")
            if self._control is None:
                self._control = ExecutionClient(self._url, self._token, client=self._client)
            return self._control

    def start(self) -> None:
        try:
            with self._lock:
                if self._stopped.is_set() or self._closed:
                    raise RuntimeError("a stopped runtime cannot be restarted; construct a new one")
                if self._started:
                    return
                self._started = True
                for pool, desired in self._desired.items():
                    self._resize(pool, desired)
        except BaseException as error:  # noqa: BLE001 — aggregate after resource cleanup
            self._abort_startup(error)

    def run_attempt(self, reference: AttemptRef | str, *, pool: str) -> bool:
        """Run one pinned attempt using this factory's services, then close.

        This is a fresh process's entry point, independent of configured pool
        concurrency. It never starts queue pollers or registers a new head.
        """
        target = AttemptRef.parse(reference) if isinstance(reference, str) else reference
        with self._lock:
            if self._started or self._closed or self._stopped.is_set():
                raise RuntimeError("run_attempt requires a fresh runtime")
            if pool not in self._pools:
                raise ValueError(f"unknown execution pool: {pool}")
        try:
            with self._lock:
                if self._started or self._closed or self._stopped.is_set():
                    raise RuntimeError("run_attempt requires a fresh runtime")
                self._started = self._single_attempt = True
                self._desired = {name: int(name == pool) for name in self._pools}
                self._resize(pool, 1, target)
                slot = self._slots[-1]
        except BaseException as error:  # noqa: BLE001 — roll back partial startup
            self._abort_startup(error)
        self.close()  # Stops admission, joins the attempt, then closes shared services.
        return slot.succeeded

    def scale(self, pool: str, *, concurrency: int) -> None:
        """Change local slots; a failed expansion stops and drains the runtime."""
        with self._lock:
            if self._single_attempt:
                raise RuntimeError("a single-attempt runtime cannot scale")
            if self._stopped.is_set() or self._closed:
                raise RuntimeError("cannot scale a stopped runtime")
            if pool not in self._pools:
                raise ValueError(f"unknown execution pool: {pool}")
            if concurrency < 0:
                raise ValueError("pool concurrency cannot be negative")
        try:
            with self._lock:
                if self._stopped.is_set():
                    raise RuntimeError("cannot scale a stopped runtime")
                self._desired[pool] = concurrency
                if self._started:
                    self._resize(pool, concurrency)
        except BaseException as error:  # noqa: BLE001 — aggregate after resource cleanup
            self._abort_startup(error)

    def _abort_startup(self, error: BaseException) -> None:
        # Never join while holding the supervisor lock: workers need it to exit.
        try:
            self.close()
        except BaseException as cleanup:  # noqa: BLE001 — preserve both lifecycle failures
            raise BaseExceptionGroup(
                "runtime startup and cleanup failed", [error, cleanup]
            ) from None
        raise error

    def _resize(self, pool: str, desired: int, reference: AttemptRef | None = None) -> None:
        self._slots = [
            slot
            for slot in self._slots
            if not slot.retiring or (slot.thread is not None and slot.thread.is_alive())
        ]
        active = [slot for slot in self._slots if slot.pool == pool and not slot.retiring]
        for slot in active[desired:]:
            slot.retiring = True
            slot.worker.stop()
        for _ in range(max(0, desired - len(active))):
            worker = Worker(
                self._url,
                self._token,
                queues=[self._pools[pool].queue],
                tasks=list(self._tasks[pool].values()),
                client=self._client,
                telemetry=self.services.telemetry,
                poll_interval=self._poll_interval,
            )
            slot = WorkerSlot(pool, worker)
            try:
                slot.thread = threading.Thread(
                    target=self._run_slot,
                    args=(slot, reference),
                    name=f"aiwatcher-runtime-{self.name}-{pool}",
                )
                slot.thread.start()
            except BaseException as error:
                try:
                    worker.close()
                except BaseException as cleanup:  # noqa: BLE001 — preserve both lifecycle failures
                    raise BaseExceptionGroup(
                        "worker startup and cleanup failed", [error, cleanup]
                    ) from None
                raise
            # The supervisor lock prevents an exiting thread from being processed
            # until its successfully started slot has been published.
            self._slots.append(slot)

    def _record_failure(self, error: BaseException) -> None:
        with self._lock:
            self._errors.append(error)
            self.stop()

    def _run_slot(self, slot: WorkerSlot, reference: AttemptRef | None = None) -> None:
        try:
            if reference is not None:
                slot.succeeded = slot.worker.run_attempt(reference)
            else:
                slot.worker.run()
                with self._lock:
                    if not slot.retiring and not self._stopped.is_set():
                        self._record_failure(RuntimeError("worker exited without a stop request"))
        except BaseException as error:  # noqa: BLE001 — supervise thread exits including SystemExit
            self._record_failure(error)
        finally:
            try:
                slot.worker.close()
            except BaseException as error:  # noqa: BLE001 — aggregate after resource cleanup
                self._record_failure(error)

    def get_status(self) -> tuple[PoolStatus, ...]:
        with self._lock:
            return tuple(
                PoolStatus(
                    pool.name,
                    pool.queue,
                    self._desired[pool.name],
                    sum(
                        slot.thread is not None and slot.thread.is_alive() and not slot.retiring
                        for slot in self._slots
                        if slot.pool == pool.name
                    ),
                    sum(
                        slot.thread is not None and slot.thread.is_alive() and slot.retiring
                        for slot in self._slots
                        if slot.pool == pool.name
                    ),
                )
                for pool in self._pools.values()
            )

    def serve(self) -> None:
        try:
            self.start()
            self._stopped.wait()
        finally:
            self.close()

    def stop(self) -> None:
        with self._lock:
            self._stopped.set()
            for slot in self._slots:
                slot.retiring = True
                slot.worker.stop()

    def close(self) -> None:
        self.stop()
        for slot in self._slots:
            if slot.thread is not None and slot.thread is not threading.current_thread():
                try:
                    slot.thread.join()
                except BaseException as error:  # noqa: BLE001 — aggregate after resource cleanup
                    self._record_failure(error)
        with self._lock:
            if self._closed:
                return
            self._closed = True
            try:
                self._release()
            except BaseException as error:  # noqa: BLE001 — cleanup every owned resource
                self._errors.append(error)
            if self._control is not None:
                try:
                    self._control.close()
                except BaseException as error:  # noqa: BLE001 — cleanup every owned resource
                    self._errors.append(error)
            if self._owns_telemetry:
                try:
                    self.services.telemetry.close()
                except BaseException as error:  # noqa: BLE001 — aggregate after resource cleanup
                    self._errors.append(error)
            if self._errors:
                raise BaseExceptionGroup("runtime workers failed", self._errors)

    def _release(self) -> None:
        release, self._on_close = self._on_close, None
        if release is not None:
            release()

    def __enter__(self) -> Self:
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        self.close()
