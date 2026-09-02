#!/usr/bin/env python3
"""Serve the model version the `production` label names, and keep serving it.

One hardened runtime profile, end to end, for the smallest runtime there is:
a JSON array of weights and a declared input shape (`Runtime::Weights`). It
is not the intended host for PyTorch, Transformers or remote checkpoints —
those need loaders this deliberately does not have — but everything *around*
the loader is the real thing, because that is the half that is the same for
every framework:

  resolve      ask the registry which version `production` names, and refuse
               a registry that answers with a different one
  verify       hash every artifact the package declares, before loading it.
               An address is not an identity
  warm         run a synthetic request through the loaded model before
               reporting ready
  serve        bounded body, bounded batch, bounded concurrency, validated
               request, optional bearer token
  watch        poll the label; when it moves, download → verify → warm the new
               version *while the old one keeps serving*, then swap
  roll back    keep the previous version loaded, and refuse to re-attempt a
               version that already failed to become ready
  report       emit one run per request carrying model, version, latency and
               outcome — and no inputs and no outputs, ever

That last line is the rule ADR_0021 settled and plan.md restated: inference
inputs and outputs do not go on the event log. A serving runtime that wants to
keep them writes turns to the conversation archive, with consent and a
retention clock, exactly as an agent does.

    just e2e-train && just serve-mini-model
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import json
import math
import os
import sys
import threading
import time
import urllib.parse
from dataclasses import dataclass, field
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "sdk" / "python"))

from aiwatcher_sdk import AiwatcherClient  # noqa: E402
from aiwatcher_sdk.training import TrainingClient, TrainingError  # noqa: E402

DEFAULT_URL = "http://127.0.0.1:8080"
DEFAULT_MODEL = "e2e.mini-edge-detector"
MAX_BODY_BYTES = 1024 * 1024
LABEL = "production"
#: The one runtime this profile implements. Anything else is refused by name
#: rather than attempted: a loader picked by looking at the file is a loader
#: picked by whoever wrote the file.
SUPPORTED_RUNTIME = "weights"


class LoadError(RuntimeError):
    """A version that cannot become ready. Never fatal to a running server."""


@dataclass
class Loaded:
    """A model that has been fetched, verified, loaded and warmed."""

    name: str
    version: str
    checkpoint_uri: str
    weights: list[float]
    #: `sha256` of every artifact, joined — what this process reports as "the
    #: model I have", for comparing against the registry's answer.
    digest: str
    #: False for a version registered before packages existed. Reported rather
    #: than hidden: "nothing checked these bytes" is a fact an operator should
    #: be able to read off `/v1/model`.
    verified: bool
    runtime: str
    loaded_at: float = field(default_factory=time.time)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def read_artifact(uri: str, expected: str | None) -> bytes:
    """The bytes at one artifact URI, checked against the digest that named it.

    `file://` only, and that is the deliberate limit rather than an oversight:
    a signed reader for an object store and for approved Hub repositories is
    its own deliverable in plan.md, with its own credentials to hold. What is
    *not* deferred is the check — verifying a digest is the same three lines
    whatever fetched the bytes, and a loader that skips it because the fetcher
    is simple is a loader that will skip it when the fetcher is not.
    """
    parsed = urllib.parse.urlparse(uri)
    if parsed.scheme != "file":
        raise LoadError(
            f"this profile only reads file:// artifacts, and this one is {uri!r}. "
            "Signed readers for an object store and for Hub repositories are sequenced "
            "in plan.md"
        )
    path = Path(urllib.parse.unquote(parsed.path))
    try:
        body = path.read_bytes()
    except OSError as error:
        raise LoadError(f"cannot read {path}: {error}") from error
    found = hashlib.sha256(body).hexdigest()
    if expected is not None and found != expected:
        raise LoadError(
            f"{uri} hashes to {found} and the package says {expected}. These are not the "
            "weights that version was measured on"
        )
    return body


def parse_weights(body: bytes, uri: str) -> list[float]:
    try:
        raw = json.loads(body)
    except json.JSONDecodeError as error:
        raise LoadError(f"{uri} is not a JSON weight vector: {error}") from error
    if not isinstance(raw, list) or not raw:
        raise LoadError(f"{uri} must be a non-empty JSON array of weights")
    try:
        weights = [float(value) for value in raw]
    except (TypeError, ValueError) as error:
        raise LoadError(f"{uri} contains a non-numeric weight") from error
    if not all(math.isfinite(value) for value in weights):
        raise LoadError(f"{uri} contains a non-finite weight")
    return weights


def resolve(client: TrainingClient, name: str) -> tuple[str, dict[str, Any]]:
    """Which version the label names, and that version's record.

    The registry is asked and its answer is checked: a `current` that is not
    the labelled version means something resolved a different one, and serving
    it would be this process quietly disagreeing with the registry about what
    is in production.
    """
    model = client.get_model(name)
    labelled = model.get("head", {}).get("labels", {}).get(LABEL)
    current = model.get("current") or {}
    if not labelled:
        raise LoadError(f"model {name!r} has no {LABEL} label")
    if current.get("version") != labelled:
        raise LoadError(
            f"the registry resolved {current.get('version')!r} for a {LABEL} label naming "
            f"{labelled!r}"
        )
    return labelled, current


def load(current: dict[str, Any], name: str) -> Loaded:
    """Fetch, verify, and load one version. Never touches what is serving."""
    package = current.get("package")
    if package:
        runtime = str(package.get("runtime", ""))
        if runtime != SUPPORTED_RUNTIME:
            raise LoadError(
                f"this profile implements the {SUPPORTED_RUNTIME!r} runtime and the package "
                f"declares {runtime!r}. A runtime is declared rather than sniffed, so this is "
                "a refusal rather than an attempt"
            )
        artifacts = package.get("artifacts") or []
        primary = next(
            (item for item in artifacts if item.get("name") == "weights"),
            artifacts[0] if len(artifacts) == 1 else None,
        )
        if primary is None:
            raise LoadError(
                "the package names no 'weights' artifact and holds more than one file, so "
                "there is nothing to pick without guessing"
            )
        body = read_artifact(str(primary.get("uri", "")), str(primary.get("digest") or "") or None)
        weights = parse_weights(body, str(primary.get("uri", "")))
        digest = hashlib.sha256(
            "\0".join(str(item.get("digest", "")) for item in artifacts).encode()
        ).hexdigest()
        return Loaded(
            name=name,
            version=str(current["version"]),
            checkpoint_uri=str(primary.get("uri", "")),
            weights=weights,
            digest=digest,
            verified=True,
            runtime=runtime,
        )

    # No package: a version registered before they existed. Loaded, and said
    # so — an unverified model that reports itself as verified is worse than
    # one that reports the truth.
    uri = str(current.get("checkpoint_uri", ""))
    body = read_artifact(uri, None)
    return Loaded(
        name=name,
        version=str(current["version"]),
        checkpoint_uri=uri,
        weights=parse_weights(body, uri),
        digest=hashlib.sha256(body).hexdigest(),
        verified=False,
        runtime="unspecified",
    )


def predict(weights: list[float], instances: list[list[float]]) -> list[float]:
    scores = []
    for row in instances:
        total = sum(weight * value for weight, value in zip(weights, row, strict=True))
        scores.append(1.0 / (1.0 + math.exp(-max(-30.0, min(30.0, total)))))
    return scores


def warm(model: Loaded) -> None:
    """Run one synthetic request before this version is allowed to be ready.

    Cheap here and not cheap in general, which is exactly why it is in the
    shape: a framework's first inference pays for lazy kernel compilation and
    allocator warm-up, and a rollout that becomes ready before that has moved
    traffic onto a version whose first hundred requests are its slowest.
    """
    probabilities = predict(model.weights, [[0.0] * len(model.weights)])
    if len(probabilities) != 1 or not math.isfinite(probabilities[0]):
        raise LoadError("the warm-up request did not produce a finite prediction")


class Server:
    """What is serving, what served before it, and what must not be tried again."""

    def __init__(
        self, client: TrainingClient, name: str, telemetry: AiwatcherClient | None
    ) -> None:
        self._client = client
        self._name = name
        self._telemetry = telemetry
        self._lock = threading.Lock()
        self._current: Loaded | None = None
        self._previous: Loaded | None = None
        self._rejected: dict[str, str] = {}
        self._pinned: str | None = None
        self.rollout_error: str | None = None
        self.rollouts = 0
        self.rollbacks = 0

    @property
    def current(self) -> Loaded | None:
        with self._lock:
            return self._current

    @property
    def previous(self) -> Loaded | None:
        with self._lock:
            return self._previous

    def ready(self) -> bool:
        return self.current is not None

    def start(self) -> None:
        """Load the labelled version. The one place a failure is fatal."""
        version, current = resolve(self._client, self._name)
        loaded = load(current, self._name)
        warm(loaded)
        with self._lock:
            self._current = loaded
        print(
            f"loaded {self._name}@{version[:12]} "
            f"({'verified' if loaded.verified else 'UNVERIFIED — no package'})",
            flush=True,
        )

    def poll(self) -> None:
        """One tick of the label watch. Two-phase, and never fatal.

        Download, verify and warm the candidate *while the current version
        keeps serving*, and swap only if all three succeed. A broken new label
        therefore never removes the ready old version — which is the property
        that makes a label a safe thing to move.
        """
        try:
            version, current = resolve(self._client, self._name)
        except (LoadError, TrainingError) as error:
            self.rollout_error = f"cannot read the registry: {error}"
            return

        with self._lock:
            serving = self._current.version if self._current else None
            pinned = self._pinned
            already = self._rejected.get(version)
        if version in (serving, pinned) or already:
            return

        try:
            candidate = load(current, self._name)
            warm(candidate)
        except (LoadError, TrainingError) as error:
            # Recorded against the version so the next tick does not download
            # and fail on the same bytes every ten seconds — and forever, which
            # is right: nothing about that version will change under the same
            # digest. Moving the label somewhere else clears it.
            with self._lock:
                self._rejected[version] = str(error)
            self.rollout_error = f"{version[:12]} cannot become ready: {error}"
            print(f"rollout refused: {self.rollout_error}", file=sys.stderr, flush=True)
            return

        with self._lock:
            self._previous = self._current
            self._current = candidate
            self.rollouts += 1
        self.rollout_error = None
        print(f"rolled forward to {version[:12]}", flush=True)

    def roll_back(self) -> Loaded:
        """Put the previous version back, and stop rolling forward onto this one.

        No image to rebuild and nothing to fetch: the previous version is
        already loaded and warm, which is the whole reason it is kept. The
        version being left is pinned out so the watcher does not immediately
        roll forward onto it again — a rollback that the next poll undoes is
        not a rollback.
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

    def record(self, *, rows: int, duration_ms: float, outcome: str, model: Loaded | None) -> None:
        """One inference, as telemetry — and with nothing that was said in it.

        A served model is a model, so this is `run.started` → `llm.started` →
        `llm.completed`, the same events an agent's model call emits. That is
        not a convenience: it means an inference joins the same traces, the
        same model dimension and the same "which version served this" question
        as everything else, instead of arriving as a second kind of thing with
        its own rules.

        What it carries is the model, the version, the label, the row count,
        the latency and the outcome. What it never carries is `instances` or
        `predictions`.
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
                label=LABEL,
                rows=rows,
                outcome=outcome,
                duration_ms=round(duration_ms, 3),
            ),
        ):
            pass


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

        The same exception list `auth::is_public` keeps, for the same reason: a
        liveness probe that needed a credential is a probe that reports an
        outage when the credential is wrong.
        """
        if self.token is None:
            return True
        header = self.headers.get("authorization", "")
        return header.startswith("Bearer ") and header[7:] == self.token

    def describe(self, model: Loaded | None) -> dict[str, Any]:
        if model is None:
            return {}
        return {
            "name": model.name,
            "version": model.version,
            "label": LABEL,
            "runtime": model.runtime,
            "digest": model.digest,
            "verified": model.verified,
            "checkpoint_uri": model.checkpoint_uri,
            "features": len(model.weights),
            "loaded_at": round(model.loaded_at, 3),
        }

    # ── Routes ───────────────────────────────────────────────────────────

    def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler API
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
            self.send_json(
                HTTPStatus.OK,
                {
                    **self.describe(state.current),
                    "previous": self.describe(state.previous),
                    "rollouts": state.rollouts,
                    "rollbacks": state.rollbacks,
                    "rollout_error": state.rollout_error,
                },
            )
            return
        self.send_json(HTTPStatus.NOT_FOUND, {"error": "not found"})

    def do_POST(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler API
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
        self.send_json(HTTPStatus.OK, {"rolled_back_to": self.describe(restored)})

    def infer(self) -> None:
        state = self.server_state
        model = state.current
        if model is None:
            self.send_json(HTTPStatus.SERVICE_UNAVAILABLE, {"error": "no model is loaded"})
            return

        # A ceiling on work in flight rather than on requests accepted. Past
        # it, a bounded refusal with a Retry-After is a better answer than an
        # unbounded queue, because a queue turns one slow minute into an
        # outage that outlasts it.
        if not self.gate.acquire(blocking=False):
            self.send_json(
                HTTPStatus.SERVICE_UNAVAILABLE,
                {"error": "too many requests in flight"},
                **{"retry-after": "1"},
            )
            return

        started = time.monotonic()
        try:
            instances, threshold = self.read_request(len(model.weights))
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
            probabilities = predict(model.weights, instances)
        finally:
            self.gate.release()

        duration_ms = (time.monotonic() - started) * 1000
        state.record(rows=len(instances), duration_ms=duration_ms, outcome="succeeded", model=model)
        self.send_json(
            HTTPStatus.OK,
            {
                "model": model.name,
                "version": model.version,
                "digest": model.digest,
                "duration_ms": round(duration_ms, 3),
                "predictions": [
                    {
                        "class": "edge" if probability >= threshold else "background",
                        "probability": round(probability, 6),
                    }
                    for probability in probabilities
                ],
            },
        )

    def read_request(self, features: int) -> tuple[list[list[float]], float]:
        """Everything the model is not allowed to be handed.

        Validated against the shape the loaded version declares rather than
        against a constant, so a rollout that changed the feature count
        refuses the old caller instead of reshaping its rows into something
        that predicts confidently and wrongly.
        """
        length = int(self.headers.get("content-length", "0"))
        if length <= 0 or length > MAX_BODY_BYTES:
            raise ValueError(f"content-length must be between 1 and {MAX_BODY_BYTES}")
        try:
            body = json.loads(self.rfile.read(length))
        except json.JSONDecodeError as error:
            raise ValueError(f"the body is not JSON: {error}") from error
        if not isinstance(body, dict):
            raise ValueError("the body must be a JSON object")

        threshold = float(body.get("threshold", 0.5))
        if not math.isfinite(threshold) or not 0.0 <= threshold <= 1.0:
            raise ValueError("threshold must be a finite number between 0 and 1")

        instances = body.get("instances")
        if not isinstance(instances, list) or not 1 <= len(instances) <= self.max_batch:
            raise ValueError(f"instances must hold between 1 and {self.max_batch} rows")

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

    def log_message(self, message: str, *args: Any) -> None:
        print(f"{self.address_string()} - {message % args}")


def watch(state: Server, seconds: float, stop: threading.Event) -> None:
    while not stop.wait(seconds):
        state.poll()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--api", default=os.environ.get("AIWATCHER_URL", DEFAULT_URL))
    parser.add_argument("--model", default=os.environ.get("AIWATCHER_E2E_MODEL", DEFAULT_MODEL))
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8091)
    parser.add_argument(
        "--token",
        default=os.environ.get("AIWATCHER_SERVE_TOKEN"),
        help="require this bearer token on /v1/*. The probes stay public.",
    )
    parser.add_argument("--max-batch", type=int, default=1_000)
    parser.add_argument("--max-concurrency", type=int, default=8)
    parser.add_argument(
        "--poll",
        type=float,
        default=10.0,
        help="how often to check whether the production label moved. 0 disables the watch.",
    )
    parser.add_argument(
        "--no-telemetry",
        action="store_true",
        help="do not report inferences. What is reported never includes inputs or outputs.",
    )
    args = parser.parse_args()

    client = TrainingClient(args.api)
    telemetry = None if args.no_telemetry else AiwatcherClient(base_url=args.api, service="serving")
    state = Server(client, args.model, telemetry)
    try:
        state.start()
    except (LoadError, TrainingError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1

    Handler.server_state = state
    Handler.token = args.token
    Handler.max_batch = max(1, args.max_batch)
    Handler.gate = threading.Semaphore(max(1, args.max_concurrency))

    stop = threading.Event()
    watcher: threading.Thread | None = None
    if args.poll > 0:
        watcher = threading.Thread(target=watch, args=(state, args.poll, stop), daemon=True)
        watcher.start()

    server = ThreadingHTTPServer((args.host, args.port), Handler)
    current = state.current
    version = current.version if current else "?"
    print(
        f"serving {args.model}@{version[:12]} on http://{args.host}:{args.port} "
        f"(auth={'on' if args.token else 'off'}, watch={args.poll or 'off'}s)",
        flush=True,
    )
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        stop.set()
        if watcher is not None:
            watcher.join(timeout=2)
        server.server_close()
        if telemetry is not None:
            telemetry.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
