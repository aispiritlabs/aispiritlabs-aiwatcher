"""Training runs, from Python.

Deliberately **not** part of the telemetry client. An earlier version of this
put `train.*` events on the same log as `llm.*` and `tool.*`; following that
through, an epoch turned out not to be a span, a step turned out not to belong
on the log at all, and a profiler session turned out not to be a trace. What
was left was one span with no children. A training run has a different grain, a
different lifetime and a different reader, so it has its own client, its own
API and its own store. See ADR_0018.

    from aiwatcher_sdk.annotations import AnnotationRegistry
    from aiwatcher_sdk.training import TrainingClient

    export = AnnotationRegistry(URL).build_export("floor-plans/dom-projekt")
    training = TrainingClient(URL)

    with training.run("effnetv2s-2026-09-01", model="effnetv2-s",
                      dataset=export.reference) as run:
        for index in range(epochs):
            with run.epoch(index) as epoch:
                for batch in loader:
                    epoch.step(loss=loss.item())     # arithmetic, not a request
                epoch.metrics(val_miou=score)        # measured once
        run.checkpoint(path, metric="val_miou", value=score, best=True)

    training.register_model("floor-plan/segmenter", run_id=run.run_id,
                            checkpoint_uri=path,
                            validation={"miou": 0.81}, test={"miou": 0.74})

**Two failure policies, on purpose.** Opening a run raises: if the server is
going to refuse it, a trainer should find out before six GPU-hours rather than
after. Progress never raises — losing an epoch record is a gap in a curve, and
killing a training run because an observability server restarted is exactly the
failure telemetry must not cause; unsent batches are kept and go out with the
next flush. Registering a model raises again, because a model nobody can find
is worse than one that was never trained.
"""

from __future__ import annotations

import contextlib
import sys
import time
from collections.abc import Iterator, Mapping
from dataclasses import dataclass, field
from types import TracebackType
from typing import Any, Literal, Self
from urllib.parse import quote

import httpx

from aiwatcher_sdk.api import ApiError, Transport

__all__ = [
    "EpochContext",
    "TrainingClient",
    "TrainingError",
    "TrainingRun",
]

Status = Literal["running", "succeeded", "failed", "cancelled"]

#: How long a sampled series has to wait between points, by default.
#:
#: Not a suggestion. A loop calling :meth:`TrainingRun.sample` every step would
#: publish hundreds of thousands of points for one run, and the server would
#: decimate them anyway — so the client drops them at the source, where it is
#: free.
DEFAULT_SAMPLE_INTERVAL = 10.0


class TrainingError(ApiError):
    """The training registry refused, or could not be reached."""


@dataclass
class _Buffer:
    """What has been measured and not yet sent."""

    epochs: list[dict[str, Any]] = field(default_factory=list)
    samples: list[dict[str, Any]] = field(default_factory=list)
    checkpoints: list[dict[str, Any]] = field(default_factory=list)
    profiles: list[dict[str, Any]] = field(default_factory=list)

    def is_empty(self) -> bool:
        return not (self.epochs or self.samples or self.checkpoints or self.profiles)

    def payload(self) -> dict[str, Any]:
        return {
            "epochs": self.epochs,
            "samples": self.samples,
            "checkpoints": self.checkpoints,
            "profiles": self.profiles,
        }

    def clear(self) -> None:
        self.epochs.clear()
        self.samples.clear()
        self.checkpoints.clear()
        self.profiles.clear()


class EpochContext:
    """One epoch's local aggregation. Nothing here reaches the network."""

    def __init__(self, index: int) -> None:
        self.index = index
        self.steps = 0
        self._sums: dict[str, float] = {}
        self._final: dict[str, float] = {}

    def step(self, **metrics: float) -> None:
        """One optimiser step. Counted and averaged; never sent.

        This is the method a training loop calls a hundred thousand times, and
        the reason it costs nothing is that it does arithmetic rather than IO.
        """
        self.steps += 1
        for key, value in metrics.items():
            self._sums[key] = self._sums.get(key, 0.0) + float(value)

    def metrics(self, **metrics: float) -> None:
        """A number known once per epoch — a validation score.

        Overrides an averaged one of the same name, because a validation mIoU
        is measured, not averaged over batches.
        """
        self._final.update({key: float(value) for key, value in metrics.items()})

    def summary(self) -> dict[str, float]:
        averaged = (
            {key: value / self.steps for key, value in self._sums.items()} if self.steps else {}
        )
        return {**averaged, **self._final}


class TrainingRun:
    """One open run. Buffers locally and flushes at epoch boundaries."""

    def __init__(
        self,
        client: TrainingClient,
        run_id: str,
        *,
        mirror: Any | None = None,
        sample_interval: float = DEFAULT_SAMPLE_INTERVAL,
    ) -> None:
        self.run_id = run_id
        self._client = client
        self._mirror = mirror
        self._sample_interval = sample_interval
        self._buffer = _Buffer()
        self._last_sample_at = 0.0
        self._epochs = 0
        self._best: dict[str, Any] | None = None
        self._warned = False

    @contextlib.contextmanager
    def epoch(self, index: int) -> Iterator[EpochContext]:
        """One epoch. Times itself and flushes one batch when it ends."""
        epoch = EpochContext(index)
        started = time.monotonic()
        try:
            yield epoch
        finally:
            metrics = epoch.summary()
            self._buffer.epochs.append(
                {
                    "epoch": index,
                    "duration_ms": (time.monotonic() - started) * 1000.0,
                    "steps": epoch.steps,
                    "metrics": metrics,
                }
            )
            self._epochs = max(self._epochs, index + 1)
            self._mirror_log({**metrics, "epoch": index})
            self.flush()

    def sample(self, **metrics: float) -> None:
        """A point on a finer series — a learning rate, a gradient norm.

        Rate-limited rather than trusted. Calls inside the interval are dropped,
        not queued: the point of a sampled series is that it is sampled, and a
        queue would deliver the whole thing at the end anyway.
        """
        now = time.monotonic()
        if now - self._last_sample_at < self._sample_interval:
            return
        self._last_sample_at = now
        self._buffer.samples.append(
            {"metrics": {key: float(value) for key, value in metrics.items()}}
        )
        self._mirror_log(metrics)

    def checkpoint(
        self,
        uri: str,
        *,
        epoch: int | None = None,
        step: int | None = None,
        metric: str | None = None,
        value: float | None = None,
        best: bool = False,
    ) -> None:
        """Where the weights went, and what selected them. Never the weights."""
        record: dict[str, Any] = {"uri": uri, "best": best}
        for key, entry in (
            ("epoch", epoch),
            ("step", step),
            ("metric", metric),
            ("value", value),
        ):
            if entry is not None:
                record[key] = entry
        self._buffer.checkpoints.append(record)
        if best and metric and value is not None:
            self._best = {"metric": metric, "value": float(value), "epoch": epoch}
        self.flush()

    def profile(self, summary: Mapping[str, Any], *, uri: str | None = None) -> None:
        """What dominated, and by how much.

        A summary and a link, never the trace itself. See
        :func:`aiwatcher_sdk.integrations.torch.profile_summary`.
        """
        self._buffer.profiles.append({"summary": dict(summary), "uri": uri})
        self.flush()

    def best(self, metric: str, value: float, *, epoch: int | None = None) -> None:
        """The number this run is judged on. Carried on the close."""
        self._best = {"metric": metric, "value": float(value), "epoch": epoch}

    def flush(self) -> bool:
        """Send what has been measured. Never raises.

        A failure keeps the buffer, so the next epoch's flush carries both. The
        warning is printed once per run rather than per attempt: a trainer that
        prints a stack trace every thirty seconds for six hours is a trainer
        whose real output nobody can read.
        """
        if self._buffer.is_empty():
            return True
        try:
            self._client.request(
                "POST",
                f"/api/v1/training-runs/{quote(self.run_id)}/progress",
                self._buffer.payload(),
            )
        except TrainingError as error:
            if not self._warned:
                self._warned = True
                print(
                    f"aiwatcher: training progress is not reaching the server ({error}); "
                    "the run continues and unsent batches go out with the next flush",
                    file=sys.stderr,
                )
            return False
        self._buffer.clear()
        return True

    def finish(self, status: Status, *, error: str | None = None) -> None:
        """Close the run. Never raises, for the same reason `flush` does not."""
        self.flush()
        body: dict[str, Any] = {"status": status}
        if error is not None:
            body["error"] = error
        if self._best is not None:
            body["best"] = self._best
        try:
            self._client.request(
                "POST",
                f"/api/v1/training-runs/{quote(self.run_id)}/finish",
                body,
            )
        except TrainingError as failure:
            print(
                f"aiwatcher: could not close the training run {self.run_id} ({failure}); "
                "it will read as running until somebody closes it",
                file=sys.stderr,
            )

    def _mirror_log(self, metrics: Mapping[str, Any]) -> None:
        """Tee to a Weights & Biases run, or anything shaped like one.

        Swallowed on purpose: a mirror is a convenience, and an exception from
        somebody else's client must not take a training run down.
        """
        if self._mirror is None or not metrics:
            return
        log = getattr(self._mirror, "log", None)
        if not callable(log):
            return
        try:
            log(dict(metrics))
        except Exception as error:  # noqa: BLE001 - a mirror never fails a run
            print(f"aiwatcher: the metric mirror refused a point: {error}", file=sys.stderr)


class TrainingClient:
    """A client for `/api/v1/training-runs` and `/api/v1/models`."""

    def __init__(
        self,
        base_url: str,
        *,
        token: str | None = None,
        timeout: float = 15.0,
        attempts: int = 3,
        client: httpx.Client | None = None,
    ) -> None:
        self._http = Transport(
            base_url,
            token=token,
            timeout=timeout,
            attempts=attempts,
            error=TrainingError,
            subject="the training registry",
            client=client,
        )

    @property
    def base_url(self) -> str:
        return self._http.base_url

    def close(self) -> None:
        self._http.close()

    def __enter__(self) -> Self:
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        self.close()

    @contextlib.contextmanager
    def run(
        self,
        run_id: str,
        *,
        model: str,
        dataset: str,
        schema_version: str | None = None,
        framework: str = "",
        device: str = "",
        code: str = "",
        params: Mapping[str, Any] | None = None,
        workflow_run_id: str | None = None,
        mirror: Any | None = None,
        sample_interval: float = DEFAULT_SAMPLE_INTERVAL,
    ) -> Iterator[TrainingRun]:
        """Open a run, and close it however the block ends.

        `dataset` should be `project@export-sha256` from the annotation
        registry. A bare project name is accepted, recorded, and marks the run
        irreproducible — refusing it would only teach people to lie about it —
        but it is said once on stderr, because that is the field that decides
        whether anybody can repeat this later.
        """
        if dataset and "@" not in dataset:
            print(
                f"aiwatcher: training on {dataset!r}, which is not an immutable export "
                "reference (project@sha256); this run will not be reproducible and its "
                "model version will not be promotable",
                file=sys.stderr,
            )
        body: dict[str, Any] = {
            "run_id": run_id,
            "model": model,
            "dataset": dataset,
            "framework": framework,
            "device": device,
            "code": code,
            "params": dict(params or {}),
        }
        if schema_version:
            body["schema_version"] = schema_version
        if workflow_run_id:
            body["workflow_run_id"] = workflow_run_id
        # Raises. If this instance is going to refuse the run, six GPU-hours
        # from now is the wrong moment to find out.
        self.request("POST", "/api/v1/training-runs", body)

        run = TrainingRun(self, run_id, mirror=mirror, sample_interval=sample_interval)
        try:
            yield run
        except BaseException as error:
            # A `KeyboardInterrupt` closes the run too, or a cancelled run looks
            # identical to a hung one.
            cancelled = isinstance(error, KeyboardInterrupt)
            run.finish(
                "cancelled" if cancelled else "failed", error=str(error) or type(error).__name__
            )
            raise
        else:
            run.finish("succeeded")

    # ── Reads ────────────────────────────────────────────────────────────

    def runs(
        self,
        *,
        model: str | None = None,
        status: Status | None = None,
        dataset: str | None = None,
        limit: int = 50,
    ) -> list[dict[str, Any]]:
        query: dict[str, Any] = {
            "limit": limit,
            "model": model,
            "status": status,
            "dataset": dataset,
        }
        page = self.request("GET", "/api/v1/training-runs", params=query)
        return list(page.get("runs", []))

    def get_run(self, run_id: str) -> dict[str, Any]:
        """One run, with its whole curve."""
        return self.request("GET", f"/api/v1/training-runs/{quote(run_id)}")

    def models(self) -> list[dict[str, Any]]:
        return list(self.request("GET", "/api/v1/models").get("models", []))

    def get_model(self, name: str, *, version: str | None = None) -> dict[str, Any]:
        """One model. With no version this resolves `production`, then newest."""
        return self.request("GET", f"/api/v1/models/{quote(name)}", params={"version": version})

    # ── Writes ───────────────────────────────────────────────────────────

    def register_model(
        self,
        name: str,
        *,
        run_id: str,
        checkpoint_uri: str,
        validation: Mapping[str, float] | None = None,
        test: Mapping[str, float] | None = None,
        package: Mapping[str, Any] | None = None,
        description: str = "",
        notes: str = "",
    ) -> dict[str, Any]:
        """Register what a run produced. Raises — this one is the work.

        `validation` and `test` are separate because the distinction is the
        point: validation is what training selected against, and test is what
        nothing was allowed to look at. A version with no `test` measurement is
        recorded and cannot be promoted, and the reason comes back in
        `promotion_blocked`.

        `package` is what a serving runtime will be handed: the runtime, the
        entry point, the input and output shapes, the dependencies, and every
        artifact with its `sha256`. Optional, because a version registered
        before packages existed has none — and validated when given, because a
        declared runtime whose weights carry no digest reads like provenance
        and is not. See `ModelPackage` in `aiwatcher-training`.
        """
        return self.request(
            "POST",
            "/api/v1/models",
            {
                "name": name,
                "run_id": run_id,
                "checkpoint_uri": checkpoint_uri,
                "description": description,
                "notes": notes,
                "metrics": {
                    "validation": {key: float(v) for key, v in (validation or {}).items()},
                    "test": {key: float(v) for key, v in (test or {}).items()},
                },
                **({"package": dict(package)} if package else {}),
            },
        )

    def promote(self, name: str, version: str, *, label: str = "production") -> dict[str, Any]:
        """Point a label at a version. Needs `admin`, and can be refused."""
        return self.request(
            "POST",
            f"/api/v1/models/{quote(name)}/labels",
            {"label": label, "version": version},
        )

    # ── Transport ────────────────────────────────────────────────────────

    def request(
        self,
        method: str,
        path: str,
        body: Mapping[str, Any] | None = None,
        *,
        params: Mapping[str, Any] | None = None,
    ) -> dict[str, Any]:
        """One request. Public, because `TrainingRun` posts through it.

        Every write here is safe to repeat: opening a run that is already open
        returns it, `progress` **replaces** the epoch index it carries rather
        than appending one, and finishing a finished run is the same finish. A
        retried epoch that appended would draw a curve with two points at one
        x, which reads as training that went backwards.
        """
        return self._http.json(method, path, body, params=params, idempotent=True)


def curve(run: Mapping[str, Any], metric: str) -> list[tuple[int, float]]:
    """The named metric across a run's epochs, from :meth:`TrainingClient.get_run`.

    A convenience rather than a computation: the record already holds the
    series, and the one thing worth not writing twice is the `.get` that
    silently drops an epoch which did not report this metric.
    """
    points: list[tuple[int, float]] = []
    for epoch in run.get("epochs", []):
        value = (epoch.get("metrics") or {}).get(metric)
        if isinstance(value, int | float):
            points.append((int(epoch["epoch"]), float(value)))
    return points


def held_out_gap(version: Mapping[str, Any], metric: str) -> float | None:
    """How far the held-out number fell short of the one selection watched.

    The number worth following across a series of model versions, and the same
    shape as a prompt optimisation's `overfit_gap`.
    """
    metrics = version.get("metrics") or {}
    validation = (metrics.get("validation") or {}).get(metric)
    test = (metrics.get("test") or {}).get(metric)
    if validation is None or test is None:
        return None
    return float(validation) - float(test)
