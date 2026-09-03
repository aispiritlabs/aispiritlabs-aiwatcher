"""The training client, against a stub of the real API.

A stub rather than a mock of `urllib`, for the same reason the prompt and
annotation tests use one: what matters is the request the Rust side accepts.

Three properties here are worth defending and none of them is about happy
paths. A training loop calls `step` hundreds of thousands of times and must
produce **no** requests. A server that goes away must not take a six-hour run
with it. And a run that crashes must close as failed rather than stay open
forever.
"""

from __future__ import annotations

import json
import threading
from collections.abc import Iterator
from http.server import BaseHTTPRequestHandler, HTTPServer
from typing import Any, ClassVar

import pytest

from aiwatcher_sdk.training import TrainingClient, TrainingError, curve, held_out_gap

EXPORT = "floor-plans/dom-projekt@" + "9f" * 32

#: What the client is given below, and what a failing flush therefore costs
#: before it hands the batch back to the buffer.
ATTEMPTS = 2


class _Recorder(BaseHTTPRequestHandler):
    stubbed: ClassVar[dict[tuple[str, str], tuple[int, Any]]] = {}
    seen: ClassVar[list[dict[str, Any]]] = []
    fail_until: ClassVar[int] = 0

    def log_message(self, *args: Any) -> None:
        return

    def _handle(self, method: str) -> None:
        length = int(self.headers.get("content-length") or 0)
        raw = self.rfile.read(length) if length else b""
        path = self.path.split("?")[0]
        type(self).seen.append(
            {"method": method, "path": path, "body": json.loads(raw) if raw else None}
        )
        if type(self).fail_until > 0:
            type(self).fail_until -= 1
            self.send_response(503)
            self.send_header("content-length", "0")
            self.end_headers()
            return
        status, body = type(self).stubbed.get((method, path), (200, {}))
        payload = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def do_GET(self) -> None:
        self._handle("GET")

    def do_POST(self) -> None:
        self._handle("POST")


@pytest.fixture
def api() -> Iterator[tuple[TrainingClient, type[_Recorder]]]:
    _Recorder.stubbed = {}
    _Recorder.seen = []
    _Recorder.fail_until = 0
    server = HTTPServer(("127.0.0.1", 0), _Recorder)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield (
            TrainingClient(
                f"http://127.0.0.1:{server.server_port}", timeout=3.0, attempts=ATTEMPTS
            ),
            _Recorder,
        )
    finally:
        server.shutdown()
        server.server_close()


def posts(recorder: type[_Recorder], suffix: str) -> list[dict[str, Any]]:
    return [
        entry["body"]
        for entry in recorder.seen
        if entry["method"] == "POST" and entry["path"].endswith(suffix)
    ]


def test_a_step_never_becomes_a_request(
    api: tuple[TrainingClient, type[_Recorder]],
) -> None:
    # Ten thousand steps here is a small run; the same loop on a real corpus is
    # millions. A rule that only holds at the small size is not a rule.
    client, recorder = api
    with client.run("run-1", model="unet", dataset=EXPORT) as run, run.epoch(0) as epoch:
        for index in range(10_000):
            epoch.step(loss=1.0 / (index + 1))

    progress = posts(recorder, "/progress")
    assert len(progress) == 1
    assert progress[0]["epochs"][0]["steps"] == 10_000
    # What is sent is the epoch's mean, not the last batch's loss.
    assert progress[0]["epochs"][0]["metrics"]["loss"] == pytest.approx(0.00098, rel=0.05)
    assert progress[0]["samples"] == []


def test_a_validation_number_overrides_the_averaged_one_of_the_same_name(
    api: tuple[TrainingClient, type[_Recorder]],
) -> None:
    client, recorder = api
    with client.run("run-2", model="unet", dataset=EXPORT) as run, run.epoch(0) as epoch:
        epoch.step(miou=0.1)
        epoch.step(miou=0.3)
        epoch.metrics(miou=0.72)

    assert posts(recorder, "/progress")[0]["epochs"][0]["metrics"]["miou"] == 0.72


def test_a_sampled_series_is_rate_limited_at_the_source(
    api: tuple[TrainingClient, type[_Recorder]],
) -> None:
    client, recorder = api
    with client.run("run-3", model="unet", dataset=EXPORT) as run:
        for _ in range(1_000):
            run.sample(lr=0.001)
        run.flush()

    samples = [point for body in posts(recorder, "/progress") for point in body["samples"]]
    assert len(samples) == 1


def test_a_server_that_goes_away_does_not_take_the_run_with_it(
    api: tuple[TrainingClient, type[_Recorder]],
    capsys: pytest.CaptureFixture[str],
) -> None:
    # The whole reason progress never raises. Killing a six-hour training run
    # because an observability server restarted is the failure telemetry must
    # not cause.
    client, recorder = api
    with client.run("run-4", model="unet", dataset=EXPORT) as run:
        # Two flushes that fail *through* their retries. One 503 no longer
        # loses a batch — the transport comes back on its own — so the buffer
        # is what carries the ones that could not be delivered at all.
        recorder.fail_until = 2 * ATTEMPTS
        with run.epoch(0) as epoch:
            epoch.step(loss=1.0)
        with run.epoch(1) as epoch:
            epoch.step(loss=0.5)
        # The third flush succeeds and carries everything the first two held.
        with run.epoch(2) as epoch:
            epoch.step(loss=0.25)

    delivered = [body for body in posts(recorder, "/progress") if body["epochs"]]
    assert [epoch["epoch"] for epoch in delivered[-1]["epochs"]] == [0, 1, 2]
    # Said once, not once per flush.
    assert capsys.readouterr().err.count("not reaching the server") == 1


def test_opening_a_run_raises_because_six_gpu_hours_later_is_the_wrong_moment(
    api: tuple[TrainingClient, type[_Recorder]],
) -> None:
    client, recorder = api
    recorder.stubbed[("POST", "/api/v1/training-runs")] = (
        409,
        {"code": "run_closed", "message": "the run run-5 already finished as succeeded"},
    )
    with pytest.raises(TrainingError) as failure:  # noqa: SIM117
        with client.run("run-5", model="unet", dataset=EXPORT):
            pass

    assert failure.value.status == 409
    assert failure.value.is_retryable is False


def test_a_crash_closes_the_run_as_failed_and_keeps_the_epochs_it_reached(
    api: tuple[TrainingClient, type[_Recorder]],
) -> None:
    client, recorder = api
    with pytest.raises(RuntimeError, match="CUDA out of memory"):  # noqa: SIM117
        with client.run("run-6", model="unet", dataset=EXPORT) as run:
            with run.epoch(0) as epoch:
                epoch.step(loss=2.0)
            raise RuntimeError("CUDA out of memory")

    finish = posts(recorder, "/finish")[0]
    assert finish["status"] == "failed"
    assert "CUDA out of memory" in finish["error"]
    # A run that died in epoch forty is a different finding from one that died
    # in epoch one.
    assert posts(recorder, "/progress")[0]["epochs"][0]["epoch"] == 0


def test_an_interrupt_closes_the_run_as_cancelled_rather_than_failed(
    api: tuple[TrainingClient, type[_Recorder]],
) -> None:
    # Ctrl-C on a training run is a decision, not a crash, and a runs list that
    # showed them the same way would make every real failure harder to find.
    client, recorder = api
    with pytest.raises(KeyboardInterrupt):  # noqa: SIM117
        with client.run("run-7", model="unet", dataset=EXPORT):
            raise KeyboardInterrupt

    assert posts(recorder, "/finish")[0]["status"] == "cancelled"


def test_a_best_checkpoint_becomes_the_number_the_run_closes_with(
    api: tuple[TrainingClient, type[_Recorder]],
) -> None:
    client, recorder = api
    with client.run("run-8", model="unet", dataset=EXPORT) as run:
        run.checkpoint(
            "s3://models/unet/e12.pt", epoch=12, metric="val_miou", value=0.81, best=True
        )

    checkpoint = posts(recorder, "/progress")[0]["checkpoints"][0]
    assert checkpoint["uri"] == "s3://models/unet/e12.pt"
    assert "weights" not in checkpoint
    assert posts(recorder, "/finish")[0]["best"] == {
        "metric": "val_miou",
        "value": 0.81,
        "epoch": 12,
    }


def test_training_on_a_mutable_dataset_name_warns_and_still_runs(
    api: tuple[TrainingClient, type[_Recorder]],
    capsys: pytest.CaptureFixture[str],
) -> None:
    # A smoke test on an unversioned dataset is legitimate. Taking the process
    # down for it is not; saying so once, and refusing the *promotion* later,
    # is.
    client, recorder = api
    with client.run("run-9", model="unet", dataset="floor-plans/dom-projekt"):
        pass

    assert "not an immutable export reference" in capsys.readouterr().err
    assert posts(recorder, "/training-runs")[0]["dataset"] == "floor-plans/dom-projekt"


def test_a_mirror_receives_the_same_points_and_cannot_fail_the_run(
    api: tuple[TrainingClient, type[_Recorder]],
) -> None:
    class AngryWandb:
        def __init__(self) -> None:
            self.logged: list[dict[str, Any]] = []

        def log(self, values: dict[str, Any]) -> None:
            self.logged.append(values)
            raise RuntimeError("the mirror is having a day")

    client, recorder = api
    mirror = AngryWandb()
    with (
        client.run("run-10", model="unet", dataset=EXPORT, mirror=mirror) as run,
        run.epoch(0) as epoch,
    ):
        epoch.metrics(val_miou=0.5)

    assert mirror.logged == [{"val_miou": 0.5, "epoch": 0}]
    assert posts(recorder, "/finish")[0]["status"] == "succeeded"


def test_registering_a_model_separates_what_selection_watched_from_what_it_did_not(
    api: tuple[TrainingClient, type[_Recorder]],
) -> None:
    client, recorder = api
    recorder.stubbed[("POST", "/api/v1/models")] = (
        201,
        {"created": True, "version": {"version": "ab" * 32}, "head": {}},
    )
    client.register_model(
        "floor-plan.segmenter",
        run_id="run-11",
        checkpoint_uri="s3://models/run-11.pt",
        validation={"miou": 0.81},
        test={"miou": 0.74},
    )

    body = posts(recorder, "/api/v1/models")[0]
    assert body["metrics"]["validation"] == {"miou": 0.81}
    assert body["metrics"]["test"] == {"miou": 0.74}


def test_a_refused_promotion_arrives_as_the_reason_rather_than_a_boolean(
    api: tuple[TrainingClient, type[_Recorder]],
) -> None:
    client, recorder = api
    recorder.stubbed[("POST", "/api/v1/models/floor-plan.segmenter/labels")] = (
        422,
        {
            "code": "promotion_refused",
            "message": "has no held-out measurement; the validation score is the number "
            "training selected against",
        },
    )
    with pytest.raises(TrainingError) as failure:
        client.promote("floor-plan.segmenter", "ab" * 32)

    assert failure.value.code == "promotion_refused"
    assert "held-out" in str(failure.value)


def test_the_curve_helper_drops_an_epoch_that_did_not_report_the_metric() -> None:
    run = {
        "epochs": [
            {"epoch": 0, "metrics": {"loss": 1.0, "val_miou": 0.4}},
            {"epoch": 1, "metrics": {"loss": 0.5}},
            {"epoch": 2, "metrics": {"loss": 0.25, "val_miou": 0.6}},
        ]
    }
    assert curve(run, "loss") == [(0, 1.0), (1, 0.5), (2, 0.25)]
    assert curve(run, "val_miou") == [(0, 0.4), (2, 0.6)]


def test_the_held_out_gap_is_none_when_the_two_splits_measured_different_things() -> None:
    version = {"metrics": {"validation": {"miou": 0.81}, "test": {"miou": 0.74}}}
    assert held_out_gap(version, "miou") == pytest.approx(0.07)
    assert held_out_gap(version, "f1") is None
    assert held_out_gap({"metrics": {"validation": {"miou": 0.81}}}, "miou") is None


def test_the_lightning_callback_drives_a_run_without_importing_lightning(
    api: tuple[TrainingClient, type[_Recorder]],
) -> None:
    from aiwatcher_sdk.integrations.torch import TrainingCallback

    client, recorder = api
    callback = TrainingCallback(client, run_id="run-12", model="efficientnetv2-s", dataset=EXPORT)

    class Tensor:
        def __init__(self, value: float) -> None:
            self._value = value

        def item(self) -> float:
            return self._value

    class Checkpoint:
        best_model_path = "/ckpt/epoch=3.ckpt"
        best_model_score = Tensor(0.83)
        monitor = "val_miou"

    class Trainer:
        current_epoch = 0
        callback_metrics: ClassVar[dict[str, Any]] = {
            "val_miou": Tensor(0.83),
            "note": "not a number",
        }
        checkpoint_callback = Checkpoint()

    trainer = Trainer()
    callback.on_train_start(trainer, None)
    callback.on_train_epoch_start(trainer, None)
    callback.on_train_batch_end(trainer, None, {"loss": Tensor(0.4)}, None, 0)
    callback.on_validation_epoch_end(trainer, None)
    callback.on_train_epoch_end(trainer, None)
    callback.on_train_end(trainer, None)

    epoch = posts(recorder, "/progress")[0]["epochs"][0]
    assert epoch["steps"] == 1
    assert epoch["metrics"]["val_miou"] == 0.83
    # A metric that is not a number is dropped rather than stringified.
    assert "note" not in epoch["metrics"]
    checkpoint = next(
        entry for body in posts(recorder, "/progress") for entry in body["checkpoints"]
    )
    assert checkpoint["uri"] == "/ckpt/epoch=3.ckpt"
    assert posts(recorder, "/finish")[0]["best"]["value"] == 0.83


def test_a_profiler_summary_keeps_the_top_operators_and_not_the_trace() -> None:
    from aiwatcher_sdk.integrations.torch import profile_summary

    class Event:
        def __init__(self, key: str, self_cpu: float, count: int) -> None:
            self.key = key
            self.self_cpu_time_total = self_cpu
            self.cpu_time_total = self_cpu * 2
            self.count = count

    class Profile:
        def key_averages(self) -> list[Event]:
            return [
                Event("aten::conv2d", 800.0, 120),
                Event("aten::batch_norm", 150.0, 120),
                Event("aten::relu", 50.0, 120),
            ]

    summary = profile_summary(Profile(), top=2)
    assert [entry["name"] for entry in summary["operators"]] == [
        "aten::conv2d",
        "aten::batch_norm",
    ]
    assert summary["total_self_cpu_us"] == 1000.0
    assert summary["top_share"] == pytest.approx(0.8)


def test_a_profiler_this_build_cannot_read_produces_an_empty_summary() -> None:
    # The attributes on a profiler event have moved between PyTorch releases
    # more than once. A summary that raises on an unknown build would take a
    # training run down to report on it.
    from aiwatcher_sdk.integrations.torch import profile_summary

    assert profile_summary(object()) == {}
