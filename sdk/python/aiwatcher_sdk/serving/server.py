"""Everything a serving process does that is not the loader.

Resolve, verify, warm, bound, validate, watch, roll back and report — the half
that is the same for every framework, which is why it is here once and why
:mod:`aiwatcher_sdk.serving.runtimes` is four members per runtime. The first
profile in this repository had this half and one loader inlined together; the
second one is what proved they had to come apart, because the alternative was
a second copy of the rollout that would drift from the first.

    resolve      ask the registry which version `production` names, and refuse
                 a registry that answers with a different one
    verify       hash every artifact the package declares, before loading it.
                 An address is not an identity
    warm         run a synthetic request through the loaded model before
                 reporting ready
    serve        bounded body, bounded batch, bounded concurrency, validated
                 request, optional bearer token
    watch        poll the label; when it moves, read → verify → warm the new
                 version *while the old one keeps serving*, then swap
    roll back    keep the previous version loaded, and refuse to re-attempt a
                 version that already failed to become ready
    shadow       optionally mirror validated rows into an independently loaded
                 label, discard its answer and never queue its work
    report       emit one run per model call carrying model, version, traffic,
                 latency and outcome — and no inputs and no outputs, ever

That last line is ADR_0021's rule restated for serving: inference inputs and
outputs do not go on the event log. A serving runtime that wants to keep them
writes turns to the conversation archive, with consent and a retention clock,
exactly as an agent does.
"""

from __future__ import annotations

import contextlib
import json
import math
import threading
import time
from collections.abc import Mapping, Sequence
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any, Protocol

from aiwatcher_sdk import AiwatcherClient
from aiwatcher_sdk.serving.artifact import ArtifactReader, FileReader, LoadError
from aiwatcher_sdk.serving.loader import Loaded, Loader, load
from aiwatcher_sdk.serving.runtimes import available

__all__ = [
    "LABEL",
    "MAX_BODY_BYTES",
    "ModelSource",
    "Server",
    "resolve",
    "resolve_label",
    "serve",
    "warm",
]

#: The label a serving process follows. One name, so "what is in production"
#: is a question with one answer per model rather than one per deployment.
LABEL = "production"

#: A request body ceiling, applied from `content-length` before a byte is read.
MAX_BODY_BYTES = 1024 * 1024


class ModelSource(Protocol):
    """The registry, as this process uses it.

    A protocol rather than :class:`~aiwatcher_sdk.training.TrainingClient`
    itself, because everything here has to be testable without an HTTP server
    and because a deployment may well resolve the label through its own cache.
    """

    def get_model(self, name: str, *, version: str | None = ...) -> dict[str, Any]: ...


def resolve_label(client: ModelSource, name: str, label: str) -> tuple[str, dict[str, Any]]:
    """Which version one label names, and that exact version's record.

    The registry is asked and its answer is *checked*: a ``current`` that is
    not the labelled version means something resolved a different one, and
    using it would be this process quietly disagreeing with the registry about
    what that label selected.
    """
    model = client.get_model(name)
    labelled = model.get("head", {}).get("labels", {}).get(label)
    current = model.get("current") or {}
    if not labelled:
        raise LoadError(f"model {name!r} has no {label} label")
    if current.get("version") != labelled and label != LABEL:
        model = client.get_model(name, version=str(labelled))
        # The label may have moved between the head read and the exact-version
        # read. Loading the old answer would turn a normal race into shadowing
        # a version the control plane no longer selected.
        confirmed = model.get("head", {}).get("labels", {}).get(label)
        if confirmed != labelled:
            raise LoadError(
                f"the {label} label moved from {labelled!r} to {confirmed!r} while it was "
                "being resolved"
            )
        current = model.get("current") or {}
    if current.get("version") != labelled:
        raise LoadError(
            f"the registry resolved {current.get('version')!r} for a {label} label naming "
            f"{labelled!r}"
        )
    return str(labelled), dict(current)


def resolve(client: ModelSource, name: str) -> tuple[str, dict[str, Any]]:
    """Which version ``production`` names, and that version's record."""
    return resolve_label(client, name, LABEL)


def warm(model: Loaded) -> None:
    """One synthetic request before this version is allowed to be ready.

    Cheap for a weight vector and not cheap in general, which is exactly why it
    is in the shape: a framework's first inference pays for lazy kernel
    compilation, allocator warm-up and a graph optimisation pass, and a rollout
    that became ready before that has moved traffic onto a version whose first
    hundred requests are its slowest. It is also the last gate that runs the
    model itself — a graph that verifies, opens and then cannot execute is
    refused here rather than by the first caller.
    """
    scores = model.predictor.predict([[0.0] * model.predictor.features])
    if len(scores) != 1 or not scores[0] or not all(math.isfinite(value) for value in scores[0]):
        raise LoadError("the warm-up request did not produce a finite prediction")


class Server:
    """What is serving, what served before it, and what must not be tried again."""

    def __init__(
        self,
        client: ModelSource,
        name: str,
        telemetry: AiwatcherClient | None,
        *,
        loaders: Mapping[str, Loader] | None = None,
        reader: ArtifactReader | None = None,
        shadow_label: str | None = None,
        shadow_concurrency: int = 1,
    ) -> None:
        if shadow_label == LABEL:
            raise ValueError("the shadow label must differ from production")
        if shadow_concurrency <= 0:
            raise ValueError("shadow concurrency must be positive")
        self._client = client
        self._name = name
        self._telemetry = telemetry
        self._loaders: Mapping[str, Loader] = loaders if loaders is not None else available()
        self._reader: ArtifactReader = reader or FileReader()
        self._lock = threading.Lock()
        self._current: Loaded | None = None
        self._previous: Loaded | None = None
        self._shadow: Loaded | None = None
        self._rejected: dict[str, str] = {}
        self._shadow_rejected: dict[tuple[str, str | None], str] = {}
        self._pinned: str | None = None
        self._shadow_label = shadow_label
        self._shadow_gate = threading.Semaphore(shadow_concurrency)
        self._shadow_requests = 0
        self._shadow_failures = 0
        self._shadow_dropped = 0
        self._shadow_duration_ms = 0.0
        self._shadow_last_error: str | None = None
        self.rollout_error: str | None = None
        self.shadow_rollout_error: str | None = None
        self.rollouts = 0
        self.rollbacks = 0
        self.shadow_rollouts = 0

    @property
    def current(self) -> Loaded | None:
        with self._lock:
            return self._current

    @property
    def previous(self) -> Loaded | None:
        with self._lock:
            return self._previous

    @property
    def shadow(self) -> Loaded | None:
        with self._lock:
            return self._shadow

    @property
    def runtimes(self) -> list[str]:
        return sorted(self._loaders)

    @property
    def artifact_reader(self) -> dict[str, Any]:
        """Non-secret fetch and cache configuration for ``/v1/model``."""
        describe = getattr(self._reader, "describe", None)
        if callable(describe):
            return dict(describe())
        return {"type": "reader", "schemes": list(self._reader.schemes)}

    @property
    def shadow_status(self) -> dict[str, Any]:
        """The loaded shadow and the runtime-only signal a canary could read."""
        with self._lock:
            requests = self._shadow_requests
            return {
                "enabled": self._shadow_label is not None,
                "label": self._shadow_label,
                "model": self._shadow.describe() if self._shadow else {},
                "rollouts": self.shadow_rollouts,
                "requests": requests,
                "failures": self._shadow_failures,
                "failure_rate": round(self._shadow_failures / requests, 6) if requests else None,
                "dropped": self._shadow_dropped,
                "mean_duration_ms": round(self._shadow_duration_ms / requests, 3)
                if requests
                else None,
                "rollout_error": self.shadow_rollout_error,
                "last_error": self._shadow_last_error,
            }

    def ready(self) -> bool:
        return self.current is not None

    def start(self) -> None:
        """Load the labelled version. The one place a failure is fatal."""
        _, current = resolve(self._client, self._name)
        loaded = load(current, self._name, self._loaders, self._reader)
        warm(loaded)
        with self._lock:
            self._current = loaded
        self._poll_shadow()

    def poll(self) -> bool:
        """One production rollout tick, plus an independent shadow-label tick."""
        swapped = self._poll_production()
        self._poll_shadow()
        return swapped

    def _poll_production(self) -> bool:
        """One tick of the label watch. Two-phase, and never fatal.

        Read, verify and warm the candidate *while the current version keeps
        serving*, and swap only if all three succeed. A broken new label
        therefore never removes the ready old version — which is the property
        that makes a label a safe thing to move. Returns whether it swapped.
        """
        try:
            version, current = resolve(self._client, self._name)
        except Exception as error:  # noqa: BLE001 - a registry blip is not an outage
            self.rollout_error = f"cannot read the registry: {error}"
            return False

        with self._lock:
            serving = self._current.version if self._current else None
            pinned = self._pinned
            already = self._rejected.get(version)
        if version in (serving, pinned) or already:
            return False

        try:
            candidate = load(current, self._name, self._loaders, self._reader)
            warm(candidate)
        except Exception as error:  # noqa: BLE001 - any failure leaves the old one serving
            # Recorded against the version so the next tick does not read and
            # fail on the same bytes every ten seconds — and forever, which is
            # right: nothing about that version will change under the same
            # digest. Moving the label somewhere else clears it.
            with self._lock:
                self._rejected[version] = str(error)
            self.rollout_error = f"{version[:12]} cannot become ready: {error}"
            return False

        with self._lock:
            self._previous = self._current
            self._current = candidate
            self.rollouts += 1
        self.rollout_error = None
        return True

    def _poll_shadow(self) -> bool:
        """Load and warm the shadow label without changing readiness or output."""
        label = self._shadow_label
        if label is None:
            return False
        try:
            version, current = resolve_label(self._client, self._name, label)
        except Exception as error:  # noqa: BLE001 - shadow control-plane failure is not an outage
            # Shadow has no availability promise. Continuing to spend work on
            # a version whose label cannot be confirmed would produce a stale
            # health window, so a registry failure pauses mirroring instead of
            # preserving the old candidate as production rollout does.
            with self._lock:
                self._shadow = None
                self._reset_shadow_stats_locked()
            self.shadow_rollout_error = f"cannot read the {label} label: {error}"
            return False

        with self._lock:
            serving = self._current
            shadow = self._shadow
            rejected = self._shadow_rejected.get(
                (version, serving.version if serving is not None else None)
            )
        if serving is not None and version == serving.version:
            with self._lock:
                self._shadow = None
                self._reset_shadow_stats_locked()
            self.shadow_rollout_error = None
            return False
        if shadow is not None and shadow.version == version:
            if serving is None or shadow.predictor.features == serving.predictor.features:
                self.shadow_rollout_error = None
                return False
            with self._lock:
                self._shadow = None
                self._reset_shadow_stats_locked()
        if rejected:
            return False

        try:
            candidate = load(current, self._name, self._loaders, self._reader)
            warm(candidate)
            if serving is not None and candidate.predictor.features != serving.predictor.features:
                raise LoadError(
                    f"the {label} version eats {candidate.predictor.features} features and the "
                    f"serving version eats {serving.predictor.features}; the same request cannot "
                    "be mirrored to both"
                )
        except Exception as error:  # noqa: BLE001 - shadow failure cannot affect primary traffic
            with self._lock:
                self._shadow_rejected[
                    (version, serving.version if serving is not None else None)
                ] = str(error)
                if self._shadow is not None and self._shadow.version != version:
                    self._shadow = None
                    self._reset_shadow_stats_locked()
            self.shadow_rollout_error = f"{version[:12]} cannot become shadow-ready: {error}"
            return False

        with self._lock:
            self._shadow = candidate
            self._reset_shadow_stats_locked()
            self.shadow_rollouts += 1
        self.shadow_rollout_error = None
        return True

    def dispatch_shadow(self, rows: Sequence[Sequence[float]]) -> bool:
        """Mirror rows in the background; never delay or alter the primary answer.

        A separate non-blocking semaphore bounds shadow work. When it is full,
        the mirror is dropped and counted instead of becoming a queue that can
        consume the serving process after primary traffic has recovered.
        """
        with self._lock:
            model = self._shadow
            serving = self._current
        if model is None:
            return False
        if serving is not None and model.predictor.features != serving.predictor.features:
            with self._lock:
                self._shadow_dropped += 1
            return False
        if not self._shadow_gate.acquire(blocking=False):
            with self._lock:
                self._shadow_dropped += 1
            return False
        copied = [list(row) for row in rows]
        worker = threading.Thread(
            target=self._run_shadow,
            args=(model, copied),
            name=f"shadow-{model.version[:12]}",
            daemon=True,
        )
        worker.start()
        return True

    def _run_shadow(self, model: Loaded, rows: list[list[float]]) -> None:
        started = time.monotonic()
        outcome = "succeeded"
        problem: str | None = None
        try:
            scores = model.predictor.predict(rows)
            if len(scores) != len(rows) or any(
                not score or not all(math.isfinite(value) for value in score) for score in scores
            ):
                raise LoadError("the shadow did not produce one finite prediction per row")
        except Exception as error:  # noqa: BLE001 - recorded, never reaches the primary request
            outcome = "failed"
            problem = str(error)
        finally:
            duration_ms = (time.monotonic() - started) * 1000
            with self._lock:
                # A label can move while an old mirror is still in flight.
                # Its telemetry still names the version that ran, but its
                # result must not contaminate the new candidate's health
                # window.
                if self._shadow is not None and self._shadow.version == model.version:
                    self._shadow_requests += 1
                    self._shadow_duration_ms += duration_ms
                    if problem is not None:
                        self._shadow_failures += 1
                        self._shadow_last_error = problem
            self.record(
                rows=len(rows),
                duration_ms=duration_ms,
                outcome=outcome,
                model=model,
                label=self._shadow_label or "shadow",
                traffic="shadow",
            )
            self._shadow_gate.release()

    def _reset_shadow_stats_locked(self) -> None:
        """Start a new health window. Caller holds ``_lock``."""
        self._shadow_requests = 0
        self._shadow_failures = 0
        self._shadow_dropped = 0
        self._shadow_duration_ms = 0.0
        self._shadow_last_error = None

    def roll_back(self) -> Loaded:
        """Put the previous version back, and stop rolling forward onto this one.

        No image to rebuild and nothing to fetch: the previous version is
        already loaded and warm, which is the whole reason it is kept. The
        version being left is pinned out so the watcher does not immediately
        roll forward onto it again — a rollback the next poll undoes is not a
        rollback.
        """
        with self._lock:
            if self._previous is None:
                raise LoadError(
                    "nothing has been rolled out yet, so there is nothing to go back to"
                )
            leaving = self._current
            self._current = self._previous
            self._previous = leaving
            if leaving is not None:
                self._rejected[leaving.version] = "rolled back by an operator"
            self._pinned = self._current.version if self._current else None
            self.rollbacks += 1
            return self._current

    def record(
        self,
        *,
        rows: int,
        duration_ms: float,
        outcome: str,
        model: Loaded | None,
        label: str = LABEL,
        traffic: str = "primary",
    ) -> None:
        """One inference, as telemetry — and with nothing that was said in it.

        A primary or shadow invocation is a model call, so this is
        ``run.started`` → ``llm.started`` → ``llm.completed``, the same events
        an agent's model call emits. That is not a convenience: it means an
        inference joins the same traces, the same model dimension and the same
        "which version ran this" question as everything else, instead of
        arriving as a second kind of thing with its own rules.

        What it carries is the model, the version, the runtime, the row count,
        the latency and the outcome. What it never carries is ``instances`` or
        ``predictions``.
        """
        if self._telemetry is None or model is None:
            return
        with (
            contextlib.suppress(Exception),
            self._telemetry.run(f"serve-{model.version[:12]}-{time.time_ns()}") as run,
            run.agent("serving") as agent,
            agent.llm(
                model=model.name,
                provider="aiwatcher-serve",
                model_version=model.version,
                label=label,
                traffic=traffic,
                runtime=model.runtime,
                rows=rows,
                outcome=outcome,
                duration_ms=round(duration_ms, 3),
            ),
        ):
            pass


def label_for(scores: Sequence[float], classes: Sequence[str], threshold: float) -> dict[str, Any]:
    """One row's scores, as the answer a caller reads.

    Three shapes, and the package decides which by what it declared. A width
    matching the class list is an argmax. A single score against two classes is
    the binary convention — the score is the probability of the second class,
    and `threshold` picks. No classes at all means raw scores, because naming
    them from a convention here would be inventing a label order, which is the
    failure ``TensorSpec::classes`` exists to prevent.
    """
    if not classes:
        return {"scores": [round(value, 6) for value in scores]}
    if len(scores) == 1 and len(classes) == 2:
        probability = scores[0]
        return {
            "class": classes[1] if probability >= threshold else classes[0],
            "probability": round(probability, 6),
        }
    best = max(range(len(scores)), key=lambda index: scores[index])
    answer: dict[str, Any] = {"probability": round(scores[best], 6)}
    answer["class"] = classes[best] if best < len(classes) else str(best)
    return answer


class Handler(BaseHTTPRequestHandler):
    server_state: Server
    token: str | None
    max_batch: int
    gate: threading.Semaphore

    # ── Plumbing ─────────────────────────────────────────────────────────

    def send_json(self, status: HTTPStatus, body: dict[str, Any], **headers: str) -> None:
        payload = json.dumps(body, separators=(",", ":")).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(payload)))
        for name, value in headers.items():
            self.send_header(name, value)
        self.end_headers()
        self.wfile.write(payload)

    def authorised(self) -> bool:
        """The probes are public and everything else is not.

        The same exception list ``auth::is_public`` keeps, for the same reason:
        a liveness probe that needed a credential is a probe that reports an
        outage when the credential is wrong.
        """
        if self.token is None:
            return True
        header = self.headers.get("authorization", "")
        return header.startswith("Bearer ") and header[7:] == self.token

    # ── Routes ───────────────────────────────────────────────────────────

    def do_GET(self) -> None:
        state = self.server_state
        if self.path == "/livez":
            self.send_json(HTTPStatus.OK, {"status": "ok"})
            return
        if self.path == "/readyz":
            # 503 until a version is loaded *and* warm. A rolling deploy that
            # sent traffic here earlier would send it to a process whose first
            # requests are its slowest, or to one that has no model at all.
            ready = state.ready()
            self.send_json(
                HTTPStatus.OK if ready else HTTPStatus.SERVICE_UNAVAILABLE,
                {"status": "ready" if ready else "loading"},
            )
            return
        if not self.authorised():
            self.send_json(HTTPStatus.UNAUTHORIZED, {"error": "a bearer token is required"})
            return
        if self.path == "/v1/model":
            current = state.current
            previous = state.previous
            self.send_json(
                HTTPStatus.OK,
                {
                    **(current.describe() if current else {}),
                    "previous": previous.describe() if previous else {},
                    "runtimes": state.runtimes,
                    "artifact_reader": state.artifact_reader,
                    "shadow": state.shadow_status,
                    "rollouts": state.rollouts,
                    "rollbacks": state.rollbacks,
                    "rollout_error": state.rollout_error,
                },
            )
            return
        self.send_json(HTTPStatus.NOT_FOUND, {"error": "not found"})

    def do_POST(self) -> None:
        if not self.authorised():
            self.send_json(HTTPStatus.UNAUTHORIZED, {"error": "a bearer token is required"})
            return
        if self.path == "/v1/rollback":
            self.rollback()
            return
        if self.path != "/v1/predict":
            self.send_json(HTTPStatus.NOT_FOUND, {"error": "not found"})
            return
        self.infer()

    def rollback(self) -> None:
        try:
            restored = self.server_state.roll_back()
        except LoadError as error:
            self.send_json(HTTPStatus.CONFLICT, {"error": str(error)})
            return
        self.send_json(HTTPStatus.OK, {"rolled_back_to": restored.describe()})

    def infer(self) -> None:
        state = self.server_state
        model = state.current
        if model is None:
            self.send_json(HTTPStatus.SERVICE_UNAVAILABLE, {"error": "no model is loaded"})
            return

        # A ceiling on work in flight rather than on requests accepted. Past
        # it, a bounded refusal with a Retry-After is a better answer than an
        # unbounded queue, because a queue turns one slow minute into an outage
        # that outlasts it.
        if not self.gate.acquire(blocking=False):
            self.send_json(
                HTTPStatus.SERVICE_UNAVAILABLE,
                {"error": "too many requests in flight"},
                **{"retry-after": "1"},
            )
            return

        started = time.monotonic()
        try:
            instances, threshold = self.read_request(model.predictor.features)
        except ValueError as error:
            self.gate.release()
            state.record(
                rows=0,
                duration_ms=(time.monotonic() - started) * 1000,
                outcome="rejected",
                model=model,
            )
            self.send_json(HTTPStatus.BAD_REQUEST, {"error": str(error)})
            return

        try:
            state.dispatch_shadow(instances)
            scores = model.predictor.predict(instances)
        except Exception as error:  # noqa: BLE001 - a runtime failure is this request's
            self.gate.release()
            state.record(
                rows=len(instances),
                duration_ms=(time.monotonic() - started) * 1000,
                outcome="failed",
                model=model,
            )
            self.send_json(HTTPStatus.INTERNAL_SERVER_ERROR, {"error": str(error)})
            return
        else:
            self.gate.release()

        duration_ms = (time.monotonic() - started) * 1000
        state.record(rows=len(instances), duration_ms=duration_ms, outcome="succeeded", model=model)
        classes = model.predictor.classes
        self.send_json(
            HTTPStatus.OK,
            {
                "model": model.name,
                "version": model.version,
                "runtime": model.runtime,
                "digest": model.digest,
                "duration_ms": round(duration_ms, 3),
                "predictions": [label_for(row, classes, threshold) for row in scores],
            },
        )

    def read_request(self, features: int) -> tuple[list[list[float]], float]:
        """Everything the model is not allowed to be handed.

        Validated against the shape the *loaded version* declares rather than
        against a constant, so a rollout that changed the feature count refuses
        the old caller instead of reshaping its rows into something that
        predicts confidently and wrongly.
        """
        length = int(self.headers.get("content-length", "0"))
        if length <= 0 or length > MAX_BODY_BYTES:
            raise ValueError(f"content-length must be between 1 and {MAX_BODY_BYTES}")
        try:
            body = json.loads(self.rfile.read(length))
        except json.JSONDecodeError as error:
            raise ValueError(f"the body is not JSON: {error}") from error
        return read_instances(body, features, self.max_batch)

    def log_message(self, message: str, *args: Any) -> None:
        print(f"{self.address_string()} - {message % args}")


def read_instances(body: Any, features: int, max_batch: int) -> tuple[list[list[float]], float]:
    """The parsed body, or a sentence saying exactly what is wrong with it."""
    if not isinstance(body, dict):
        raise ValueError("the body must be a JSON object")

    threshold = body.get("threshold", 0.5)
    if isinstance(threshold, bool) or not isinstance(threshold, (int, float)):
        raise ValueError("threshold must be a number between 0 and 1")
    threshold = float(threshold)
    if not math.isfinite(threshold) or not 0.0 <= threshold <= 1.0:
        raise ValueError("threshold must be a finite number between 0 and 1")

    instances = body.get("instances")
    if not isinstance(instances, list) or not 1 <= len(instances) <= max_batch:
        raise ValueError(f"instances must hold between 1 and {max_batch} rows")

    rows: list[list[float]] = []
    for row in instances:
        if not isinstance(row, list) or len(row) != features:
            raise ValueError(f"every instance must have {features} features")
        if any(isinstance(value, bool) or not isinstance(value, (int, float)) for value in row):
            raise ValueError("features must be numbers")
        values = [float(value) for value in row]
        if not all(math.isfinite(value) for value in values):
            raise ValueError("features must be finite")
        rows.append(values)
    return rows, threshold


def watch(state: Server, seconds: float, stop: threading.Event) -> None:
    while not stop.wait(seconds):
        if state.poll():
            current = state.current
            if current is not None:
                print(f"rolled forward to {current.version[:12]} ({current.runtime})", flush=True)
        elif state.rollout_error:
            print(f"rollout refused: {state.rollout_error}", flush=True)


def serve(
    state: Server,
    *,
    host: str = "127.0.0.1",
    port: int = 8091,
    token: str | None = None,
    max_batch: int = 1_000,
    max_concurrency: int = 8,
    poll_seconds: float = 10.0,
) -> None:
    """Start the loaded server and keep it serving until interrupted."""
    Handler.server_state = state
    Handler.token = token
    Handler.max_batch = max(1, max_batch)
    Handler.gate = threading.Semaphore(max(1, max_concurrency))

    stop = threading.Event()
    watcher: threading.Thread | None = None
    if poll_seconds > 0:
        watcher = threading.Thread(target=watch, args=(state, poll_seconds, stop), daemon=True)
        watcher.start()

    server = ThreadingHTTPServer((host, port), Handler)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        stop.set()
        if watcher is not None:
            watcher.join(timeout=2)
        server.server_close()
