"""PyTorch and Lightning, without importing either.

The rule this module follows is the one :mod:`aiwatcher_sdk.integrations.deepeval`
already follows: read the other library's objects *structurally* and never
import it. This SDK has no runtime dependencies because it is imported into
agent and training processes that already have opinions about their own
versions, and a `import torch` here would make a PyTorch release an SDK
release.

Two things live here.

:class:`TrainingCallback` duck-types Lightning's ``Callback`` protocol. Attach
it to a ``Trainer`` and epochs, checkpoints and the final status reach
aiwatcher with no change to the training loop::

    from aiwatcher_sdk.integrations.torch import TrainingCallback
    from aiwatcher_sdk.training import TrainingClient

    trainer = Trainer(callbacks=[TrainingCallback(
        TrainingClient("http://aiwatcher:8080"),
        run_id="floorplan-effnetv2s-2026-09-01",
        model="efficientnetv2-s",
        dataset=export.reference,
    )])

:func:`profile_summary` turns a finished ``torch.profiler.profile`` into the
handful of numbers worth keeping. The full Chrome trace stays wherever the
profiler wrote it: sixty seconds of profiling emits more records than the
projector holds for a week, and a flame graph is something a profiler UI draws
better than a waterfall ever will.
"""

from __future__ import annotations

import contextlib
from collections.abc import Iterator, Mapping
from typing import Any

from aiwatcher_sdk.training import EpochContext, TrainingClient, TrainingRun

__all__ = ["TrainingCallback", "describe_device", "profile_summary", "scalars"]


def scalars(metrics: Mapping[str, Any]) -> dict[str, float]:
    """Whatever Lightning put in ``callback_metrics``, as plain floats.

    A metric there is usually a zero-dimensional tensor, sometimes a float, and
    occasionally something with neither ``item`` nor a numeric conversion. The
    third case is dropped rather than stringified: a metric that is not a number
    is not a point on a curve.
    """
    out: dict[str, float] = {}
    for key, value in metrics.items():
        item = getattr(value, "item", None)
        try:
            out[key] = float(item()) if callable(item) else float(value)
        except (TypeError, ValueError):
            continue
    return out


def describe_device(module: Any = None) -> str:
    """A best-effort device label, read off whatever was handed in.

    Structural, like everything else here: ``module.device`` is what Lightning
    exposes, and a plain string is accepted because a caller who knows is
    better than a guess.
    """
    if module is None:
        return ""
    if isinstance(module, str):
        return module
    device = getattr(module, "device", None)
    return str(device) if device is not None else ""


def profile_summary(
    profile: Any, *, top: int = 10, sort_by: str = "self_cpu_time_total"
) -> dict[str, Any]:
    """The part of a profiler session worth putting on the log.

    Reads ``profile.key_averages()`` structurally. Every field is optional,
    because the attributes on a profiler event have moved between PyTorch
    releases more than once and a summary that raises on an unknown build is
    worse than one that is missing a column.
    """
    try:
        averages = list(profile.key_averages())
    except (AttributeError, TypeError, RuntimeError):
        return {}

    def number(entry: Any, *names: str) -> float:
        for name in names:
            value = getattr(entry, name, None)
            if value is None:
                continue
            with contextlib.suppress(TypeError, ValueError):
                return float(value)
        return 0.0

    ranked = sorted(averages, key=lambda entry: number(entry, sort_by), reverse=True)[:top]
    operators: list[dict[str, Any]] = [
        {
            "name": str(getattr(entry, "key", "") or getattr(entry, "name", "")),
            "count": int(number(entry, "count")),
            "self_cpu_us": number(entry, "self_cpu_time_total"),
            "cpu_us": number(entry, "cpu_time_total"),
            "self_device_us": number(entry, "self_device_time_total", "self_cuda_time_total"),
            "device_us": number(entry, "device_time_total", "cuda_time_total"),
            "cpu_memory_bytes": number(entry, "self_cpu_memory_usage"),
            "device_memory_bytes": number(
                entry, "self_device_memory_usage", "self_cuda_memory_usage"
            ),
        }
        for entry in ranked
    ]
    total_self_cpu = sum(number(entry, "self_cpu_time_total") for entry in averages)
    total_self_device = sum(
        number(entry, "self_device_time_total", "self_cuda_time_total") for entry in averages
    )
    return {
        "sort_by": sort_by,
        "operators": operators,
        "total_self_cpu_us": total_self_cpu,
        "total_self_device_us": total_self_device,
        "peak_device_memory_bytes": max(
            (number(entry, "device_memory_usage", "cuda_memory_usage") for entry in averages),
            default=0.0,
        ),
        # The one number a reviewer reads first: how much of the time went to
        # the single hottest operator. A run where it is 4% has no hot spot and
        # a different problem.
        "top_share": (
            float(operators[0]["self_cpu_us"]) / total_self_cpu
            if operators and total_self_cpu
            else 0.0
        ),
    }


class TrainingCallback:
    """A Lightning-shaped callback that publishes a training run.

    Not a subclass of anything — Lightning calls hooks by name, so a plain
    object with the right methods works and this file stays importable in a
    process that has never heard of Lightning.

    The callback owns the run's lifetime: it opens ``train.started`` on
    ``on_train_start`` and closes it on ``on_train_end`` or ``on_exception``.
    A crash that never reaches either leaves the run ``Running`` until the
    span assembler's orphan timeout, which is the correct outcome — nothing in
    the log distinguishes a killed trainer from a thinking one, and the
    projector deciding otherwise would be a guess.
    """

    def __init__(
        self,
        client: TrainingClient,
        *,
        run_id: str,
        model: str,
        dataset: str,
        params: Mapping[str, Any] | None = None,
        mirror: Any | None = None,
        framework: str = "pytorch",
    ) -> None:
        self._client = client
        self._run_id = run_id
        self._model = model
        self._dataset = dataset
        self._params = dict(params or {})
        self._mirror = mirror
        self._framework = framework
        self._stack: contextlib.ExitStack | None = None
        self._run: TrainingRun | None = None
        self._epoch: EpochContext | None = None
        self._epoch_stack: contextlib.ExitStack | None = None

    # ── Lightning hooks ──────────────────────────────────────────────────

    def on_train_start(self, trainer: Any = None, pl_module: Any = None) -> None:
        if self._run is not None:
            return
        stack = contextlib.ExitStack()
        self._run = stack.enter_context(
            self._client.run(
                self._run_id,
                model=self._model,
                dataset=self._dataset,
                framework=self._framework,
                device=describe_device(pl_module),
                params=self._params,
                mirror=self._mirror,
            )
        )
        self._stack = stack

    def on_train_epoch_start(self, trainer: Any = None, pl_module: Any = None) -> None:
        if self._run is None:
            return
        self._close_epoch()
        stack = contextlib.ExitStack()
        index = int(getattr(trainer, "current_epoch", 0) or 0)
        self._epoch = stack.enter_context(self._run.epoch(index))
        self._epoch_stack = stack

    def on_train_batch_end(
        self,
        trainer: Any = None,
        pl_module: Any = None,
        outputs: Any = None,
        batch: Any = None,
        batch_idx: int = 0,
    ) -> None:
        """One step. Counted and averaged locally; nothing is published here.

        This is the hook that fires a hundred thousand times, and the whole
        reason it costs nothing is that it does arithmetic rather than IO.
        """
        if self._epoch is None:
            return
        loss = outputs.get("loss") if isinstance(outputs, Mapping) else outputs
        values = scalars({"loss": loss}) if loss is not None else {}
        self._epoch.step(**values)

    def on_validation_epoch_end(self, trainer: Any = None, pl_module: Any = None) -> None:
        """Validation numbers are measured once, not averaged over batches."""
        if self._epoch is None:
            return
        self._epoch.metrics(**scalars(getattr(trainer, "callback_metrics", {}) or {}))

    def on_train_epoch_end(self, trainer: Any = None, pl_module: Any = None) -> None:
        if self._epoch is not None:
            self._epoch.metrics(**scalars(getattr(trainer, "callback_metrics", {}) or {}))
        self._close_epoch()

    def on_train_end(self, trainer: Any = None, pl_module: Any = None) -> None:
        self._close_epoch()
        self._record_checkpoint(trainer)
        self._close_run()

    def on_exception(
        self, trainer: Any = None, pl_module: Any = None, exception: BaseException | None = None
    ) -> None:
        self._close_epoch()
        self._close_run(exception)

    # ── Direct use, without Lightning ────────────────────────────────────

    @property
    def run(self) -> TrainingRun | None:
        """The underlying context, for a loop that wants to log a checkpoint or
        a profile itself."""
        return self._run

    @contextlib.contextmanager
    def epoch(self, index: int) -> Iterator[EpochContext]:
        """For a plain PyTorch loop with no Trainer to call the hooks."""
        if self._run is None:
            self.on_train_start()
        assert self._run is not None  # noqa: S101 - established one line above
        with self._run.epoch(index) as epoch:
            yield epoch

    # ── Internals ────────────────────────────────────────────────────────

    def _record_checkpoint(self, trainer: Any) -> None:
        """Whatever ``ModelCheckpoint`` selected, as a pointer.

        Read structurally off ``trainer.checkpoint_callback``, and skipped
        entirely when there is none. Weights never enter the log.
        """
        callback = getattr(trainer, "checkpoint_callback", None)
        path = getattr(callback, "best_model_path", None)
        if not path or self._run is None:
            return
        monitor = getattr(callback, "monitor", None)
        score = scalars({"score": getattr(callback, "best_model_score", None)}).get("score")
        self._run.checkpoint(
            str(path),
            metric=str(monitor) if monitor else None,
            value=score,
            best=True,
        )

    def _close_epoch(self) -> None:
        if self._epoch_stack is not None:
            self._epoch_stack.close()
        self._epoch_stack = None
        self._epoch = None

    def _close_run(self, exception: BaseException | None = None) -> None:
        stack, self._stack, self._run = self._stack, None, None
        if stack is None:
            return
        if exception is None:
            stack.close()
        else:
            # Hand the exception to the context manager so it publishes
            # `train.failed` rather than `train.completed`. A crashed run that
            # reported success is the one outcome worth going out of the way
            # for.
            stack.__exit__(type(exception), exception, exception.__traceback__)
