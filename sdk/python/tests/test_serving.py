"""The serving profile: what it refuses, and what it keeps serving anyway.

Two things are worth testing here and they are different in kind.

The **rollout** is a state machine over "what is loaded", and every one of its
interesting states is a failure — a candidate that will not load, a registry
that disagrees with itself, a rollback the next poll would undo. Those are
exercised through the weights runtime, which needs no dependency, so a change
to the shared half is caught before any graph is anywhere near the process.

The **ONNX cross-check** is the other kind: two independent descriptions of
one model, compared. It runs against a stub session rather than the wheel,
which is deliberate — the gates are pure functions of what a session says
about itself, and a hundred megabytes of runtime in CI would test onnxruntime
rather than this file. `just serve-onnx` is what runs it against a real graph.
"""

from __future__ import annotations

import hashlib
import json
import threading
import time
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any, ClassVar

import numpy as np
import pytest

from aiwatcher_sdk import AiwatcherClient
from aiwatcher_sdk.serving import (
    FileReader,
    LoadError,
    S3Credentials,
    S3Reader,
    SchemeReader,
    Server,
    VersionCacheReader,
    load,
    resolve,
    resolve_label,
    warm,
)
from aiwatcher_sdk.serving.runtimes import available
from aiwatcher_sdk.serving.runtimes import onnx as onnx_runtime
from aiwatcher_sdk.serving.server import label_for, read_instances

WEIGHTS = [0.5, -0.25, 1.0]


# ── Stands-in ────────────────────────────────────────────────────────────────


@dataclass(frozen=True)
class StubArg:
    """One graph input or output, as onnxruntime would describe it."""

    name: str
    type: str
    shape: Sequence[Any]


class StubSession:
    """A session that answers about itself and returns one score per row."""

    def __init__(
        self,
        inputs: Sequence[StubArg],
        outputs: Sequence[StubArg],
        *,
        width: int = 1,
    ) -> None:
        self._inputs = list(inputs)
        self._outputs = list(outputs)
        self._width = width
        self.fed: dict[str, Any] = {}

    def get_inputs(self) -> Sequence[StubArg]:
        return self._inputs

    def get_outputs(self) -> Sequence[StubArg]:
        return self._outputs

    def get_providers(self) -> Sequence[str]:
        return ["CPUExecutionProvider"]

    def run(self, output_names: Sequence[str] | None, feed: Mapping[str, Any]) -> Sequence[Any]:
        del output_names
        self.fed = dict(feed)
        rows = len(next(iter(feed.values())))
        return [[[0.75] * self._width for _ in range(rows)]]


class FakeRegistry:
    """The model registry, as this process uses it."""

    def __init__(
        self,
        versions: Mapping[str, dict[str, Any]],
        labelled: str,
        *,
        labels: Mapping[str, str] | None = None,
    ) -> None:
        self.versions = dict(versions)
        self.labelled = labelled
        self.labels = dict(labels or {})
        self.reads = 0
        self.resolves_to: str | None = None

    def get_model(self, name: str, *, version: str | None = None) -> dict[str, Any]:
        del name
        self.reads += 1
        resolved = version or self.resolves_to or self.labelled
        labels = dict(self.labels)
        labels["production"] = self.labelled
        return {
            "head": {"labels": labels},
            "current": self.versions[resolved],
        }


class Recording:
    """A transport that keeps every envelope instead of sending it."""

    def __init__(self) -> None:
        self.events: list[dict[str, Any]] = []

    def send(self, batch: list[dict[str, Any]]) -> None:
        self.events.extend(batch)

    def close(self) -> None:
        return None


class CountingReader:
    """A remote reader whose calls make cache misses visible."""

    def __init__(self, body: bytes) -> None:
        self.body = body
        self.reads = 0

    @property
    def schemes(self) -> tuple[str, ...]:
        return ("s3",)

    def read(self, uri: str, *, version: str, expected_digest: str) -> bytes:
        del uri, version, expected_digest
        self.reads += 1
        return self.body


class BlockingPredictor:
    """Warms once, then holds a shadow call so the non-blocking bound is testable."""

    def __init__(self) -> None:
        self.warmed = False
        self.started = threading.Event()
        self.release = threading.Event()

    @property
    def features(self) -> int:
        return 3

    @property
    def classes(self) -> tuple[str, ...]:
        return ("background", "edge")

    def predict(self, rows: Sequence[Sequence[float]]) -> list[list[float]]:
        if not self.warmed:
            self.warmed = True
            return [[0.5] for _ in rows]
        self.started.set()
        self.release.wait(timeout=2)
        return [[0.75] for _ in rows]

    def describe(self) -> Mapping[str, Any]:
        return {"blocking": True}


class BlockingLoader:
    def __init__(self, predictor: BlockingPredictor) -> None:
        self.predictor = predictor

    @property
    def runtime(self) -> str:
        return "shadow-test"

    @property
    def executes_packaged_code(self) -> bool:
        return False

    def load(
        self,
        package: Mapping[str, Any],
        reader: Any,
        *,
        version: str,
    ) -> BlockingPredictor:
        del package, reader, version
        return self.predictor


class ObjectHandler(BaseHTTPRequestHandler):
    """One controllable object-store answer over a real loopback socket."""

    body = b"artifact"
    status = 200
    response_headers: ClassVar[dict[str, str]] = {}
    seen: ClassVar[list[dict[str, str]]] = []
    paths: ClassVar[list[str]] = []

    def do_GET(self) -> None:
        type(self).seen.append({name.lower(): value for name, value in self.headers.items()})
        type(self).paths.append(self.path)
        self.send_response(type(self).status)
        for name, value in type(self).response_headers.items():
            self.send_header(name, value)
        if "content-length" not in {name.lower() for name in type(self).response_headers}:
            self.send_header("content-length", str(len(type(self).body)))
        self.end_headers()
        self.wfile.write(type(self).body)

    def log_message(self, format: str, *args: Any) -> None:
        del format, args


def object_server() -> tuple[ThreadingHTTPServer, threading.Thread, str]:
    ObjectHandler.body = b"artifact"
    ObjectHandler.status = 200
    ObjectHandler.response_headers = {}
    ObjectHandler.seen = []
    ObjectHandler.paths = []
    server = ThreadingHTTPServer(("127.0.0.1", 0), ObjectHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    raw_host, port = server.server_address[:2]
    host = raw_host.decode() if isinstance(raw_host, bytes) else raw_host
    return server, thread, f"http://{host}:{port}"


def s3_reader(endpoint: str, *, max_bytes: int = 1024) -> S3Reader:
    return S3Reader(
        endpoint,
        "models",
        S3Credentials(
            access_key_id="AKIAIOSFODNN7EXAMPLE",
            secret_access_key="wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            region="us-east-1",
            session_token="temporary-session",
        ),
        max_bytes=max_bytes,
    )


# ── Builders ─────────────────────────────────────────────────────────────────


def write_weights(tmp_path: Path, weights: Sequence[float], name: str = "model.json") -> Path:
    path = tmp_path / name
    path.write_text(json.dumps(list(weights)))
    return path


def weights_version(path: Path, version: str = "a" * 64, **overrides: Any) -> dict[str, Any]:
    package: dict[str, Any] = {
        "runtime": "weights",
        "entry_point": path.name,
        "inputs": [{"name": "features", "dtype": "float32", "shape": [None, 3]}],
        "outputs": [{"name": "probability", "shape": [None], "classes": ["background", "edge"]}],
        "artifacts": [
            {
                "name": "weights",
                "uri": f"file://{path}",
                "digest": hashlib.sha256(path.read_bytes()).hexdigest(),
            }
        ],
    }
    package.update(overrides)
    return {"version": version, "checkpoint_uri": f"file://{path}", "package": package}


def onnx_version(
    tmp_path: Path,
    *,
    inputs: Sequence[Mapping[str, Any]] | None = None,
    outputs: Sequence[Mapping[str, Any]] | None = None,
    version: str = "b" * 64,
    **overrides: Any,
) -> dict[str, Any]:
    graph = tmp_path / "model.onnx"
    graph.write_bytes(b"a serialized graph, as far as this test is concerned")
    package: dict[str, Any] = {
        "runtime": "onnx",
        "entry_point": "model.onnx",
        "inputs": list(inputs) if inputs is not None else [],
        "outputs": list(outputs) if outputs is not None else [],
        "artifacts": [
            {
                "name": "model",
                "uri": f"file://{graph}",
                "digest": hashlib.sha256(graph.read_bytes()).hexdigest(),
            }
        ],
    }
    package.update(overrides)
    return {"version": version, "checkpoint_uri": f"file://{graph}", "package": package}


def stub_loaders(session: StubSession) -> dict[str, Any]:
    loaders = available(onnx=False)
    loaders["onnx"] = onnx_runtime.OnnxLoader(session_factory=lambda graph, threads: session)
    return loaders


def loaded(current: Mapping[str, Any], loaders: Mapping[str, Any] | None = None) -> Any:
    return load(dict(current), "m", loaders or available(onnx=False), FileReader())


# ── Which loader runs, and whether one runs at all ───────────────────────────


def test_a_runtime_this_host_does_not_implement_is_refused_by_name(tmp_path: Path) -> None:
    current = weights_version(write_weights(tmp_path, WEIGHTS), runtime="torchscript")

    with pytest.raises(LoadError) as refusal:
        loaded(current)

    assert "declares 'torchscript'" in str(refusal.value)
    assert "declared rather than sniffed" in str(refusal.value)


def test_a_package_that_names_no_runtime_is_refused_rather_than_guessed_at(tmp_path: Path) -> None:
    current = weights_version(write_weights(tmp_path, WEIGHTS), runtime="unspecified")

    with pytest.raises(LoadError) as refusal:
        loaded(current)

    assert "chosen by whoever wrote the file" in str(refusal.value)


def test_a_loader_that_runs_the_packages_own_code_is_not_selected_here(tmp_path: Path) -> None:
    class PythonLoader:
        @property
        def runtime(self) -> str:
            return "python"

        @property
        def executes_packaged_code(self) -> bool:
            return True

        def load(self, package: Mapping[str, Any], reader: Any) -> Any:
            raise AssertionError("a process that cannot isolate one must not reach the loader")

    current = weights_version(write_weights(tmp_path, WEIGHTS), runtime="python")

    with pytest.raises(LoadError) as refusal:
        loaded(current, {"python": PythonLoader()})

    assert "does not isolate one" in str(refusal.value)
    assert "credentials" in str(refusal.value)


def test_an_entry_point_that_names_nothing_in_the_package_is_refused(tmp_path: Path) -> None:
    path = write_weights(tmp_path, WEIGHTS)
    current = weights_version(path, entry_point="tokenizer.json")
    current["package"]["artifacts"].append(
        {"name": "labels", "uri": f"file://{path}", "digest": "0" * 64}
    )

    with pytest.raises(LoadError) as refusal:
        loaded(current)

    assert "names neither an artifact of this package nor a file in one" in str(refusal.value)
    assert "labels, weights" in str(refusal.value)


def test_an_entry_point_may_name_an_artifact_or_the_file_at_its_uri(tmp_path: Path) -> None:
    path = write_weights(tmp_path, WEIGHTS, name="edges.json")

    by_file = loaded(weights_version(path, entry_point="edges.json"))
    by_name = loaded(weights_version(path, entry_point="weights"))

    assert by_file.predictor.features == by_name.predictor.features == 3


# ── An address is not an identity ────────────────────────────────────────────


def test_an_artifact_that_does_not_hash_to_its_digest_is_refused(tmp_path: Path) -> None:
    path = write_weights(tmp_path, WEIGHTS)
    current = weights_version(path)
    path.write_text(json.dumps([9.0, 9.0, 9.0]))  # the bytes moved under the digest

    with pytest.raises(LoadError) as refusal:
        loaded(current)

    assert "not the bytes that version was measured on" in str(refusal.value)


def test_a_scheme_this_host_cannot_read_is_refused_by_name(tmp_path: Path) -> None:
    current = weights_version(write_weights(tmp_path, WEIGHTS), entry_point="weights")
    current["package"]["artifacts"][0]["uri"] = "s3://models/latest.json"

    with pytest.raises(LoadError) as refusal:
        loaded(current)

    assert "reads file:// artifacts" in str(refusal.value)
    assert "plug in behind ArtifactReader" in str(refusal.value)


def test_an_s3_read_is_signed_once_against_the_configured_bucket() -> None:
    server, thread, endpoint = object_server()
    try:
        body = s3_reader(endpoint).read(
            "s3://models/a folder/model+one.json",
            version="a" * 64,
            expected_digest="b" * 64,
        )
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=2)

    assert body == b"artifact"
    assert ObjectHandler.paths == ["/models/a%20folder/model%2Bone.json"]
    assert len(ObjectHandler.seen) == 1
    headers = ObjectHandler.seen[0]
    assert headers["x-amz-content-sha256"] == hashlib.sha256(b"").hexdigest()
    assert headers["x-amz-security-token"] == "temporary-session"
    assert headers["authorization"].startswith("AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/")
    assert (
        "SignedHeaders=host;x-amz-content-sha256;x-amz-date;x-amz-security-token"
        in headers["authorization"]
    )


def test_s3_credentials_cannot_be_used_against_an_unapproved_bucket() -> None:
    server, thread, endpoint = object_server()
    try:
        with pytest.raises(LoadError) as refusal:
            s3_reader(endpoint).read(
                "s3://somebody-elses-models/model.onnx",
                version="a" * 64,
                expected_digest="b" * 64,
            )
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=2)

    assert "approves only 'models'" in str(refusal.value)
    assert ObjectHandler.seen == [], "an unapproved bucket is refused before a request is signed"


def test_a_signed_artifact_read_never_follows_a_redirect() -> None:
    server, thread, endpoint = object_server()
    ObjectHandler.status = 302
    ObjectHandler.response_headers = {"location": f"{endpoint}/models/elsewhere"}
    try:
        with pytest.raises(LoadError) as refusal:
            s3_reader(endpoint).read(
                "s3://models/model.onnx",
                version="a" * 64,
                expected_digest="b" * 64,
            )
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=2)

    assert "never follow redirects" in str(refusal.value)
    assert len(ObjectHandler.seen) == 1


def test_the_remote_artifact_ceiling_is_applied_while_streaming() -> None:
    server, thread, endpoint = object_server()
    ObjectHandler.body = b"five!"
    # No useful declared size: the streaming gate must be independent of it.
    ObjectHandler.response_headers = {"content-length": "not-a-number"}
    try:
        with pytest.raises(LoadError) as refusal:
            s3_reader(endpoint, max_bytes=4).read(
                "s3://models/model.onnx",
                version="a" * 64,
                expected_digest="b" * 64,
            )
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=2)

    assert "4-byte artifact ceiling" in str(refusal.value)


def test_every_artifact_is_verified_even_when_the_loader_opens_only_one(tmp_path: Path) -> None:
    weights = write_weights(tmp_path, WEIGHTS)
    labels = tmp_path / "labels.json"
    labels.write_text('["background","edge"]')
    current = weights_version(weights)
    current["package"]["artifacts"].append(
        {
            "name": "labels",
            "uri": f"file://{labels}",
            "digest": "0" * 64,
        }
    )

    with pytest.raises(LoadError) as refusal:
        loaded(current)

    assert str(labels) in str(refusal.value)
    assert "not the bytes that version was measured on" in str(refusal.value)


def test_the_version_cache_avoids_a_second_remote_read(tmp_path: Path) -> None:
    body = b"the immutable graph"
    digest = hashlib.sha256(body).hexdigest()
    remote = CountingReader(body)
    cache = VersionCacheReader(remote, tmp_path, max_bytes=1024)

    first = cache.read("s3://models/model.onnx", version="a" * 64, expected_digest=digest)
    second = cache.read("s3://models/model.onnx", version="a" * 64, expected_digest=digest)

    assert first == second == body
    assert remote.reads == 1
    assert (tmp_path / ("a" * 64) / digest).read_bytes() == body


def test_a_corrupt_cache_entry_is_discarded_and_fetched_again(tmp_path: Path) -> None:
    body = b"the immutable graph"
    digest = hashlib.sha256(body).hexdigest()
    remote = CountingReader(body)
    cache = VersionCacheReader(remote, tmp_path, max_bytes=1024)
    cache.read("s3://models/model.onnx", version="a" * 64, expected_digest=digest)
    cached = tmp_path / ("a" * 64) / digest
    cached.write_bytes(b"tampered on disk")

    assert cache.read("s3://models/model.onnx", version="a" * 64, expected_digest=digest) == body
    assert remote.reads == 2
    assert cached.read_bytes() == body


def test_unverified_bytes_never_enter_the_persistent_cache(tmp_path: Path) -> None:
    remote = CountingReader(b"not the declared object")
    cache = VersionCacheReader(remote, tmp_path, max_bytes=1024)

    with pytest.raises(LoadError):
        cache.read(
            "s3://models/model.onnx",
            version="a" * 64,
            expected_digest=hashlib.sha256(b"the declared object").hexdigest(),
        )

    assert [path for path in tmp_path.rglob("*") if path.is_file()] == []


def test_the_cache_evicts_least_recent_entries_to_its_byte_budget(tmp_path: Path) -> None:
    first = b"1111"
    second = b"2222"
    remote = CountingReader(first)
    cache = VersionCacheReader(remote, tmp_path, max_bytes=5)
    cache.read(
        "s3://models/one.onnx",
        version="a" * 64,
        expected_digest=hashlib.sha256(first).hexdigest(),
    )
    remote.body = second
    cache.read(
        "s3://models/two.onnx",
        version="b" * 64,
        expected_digest=hashlib.sha256(second).hexdigest(),
    )

    files = [path for path in tmp_path.rglob("*") if path.is_file()]
    assert sum(path.stat().st_size for path in files) <= 5
    assert len(files) == 1


def test_reader_metadata_reports_bounds_and_never_credentials(tmp_path: Path) -> None:
    reader = VersionCacheReader(
        SchemeReader([FileReader(), s3_reader("http://127.0.0.1:9010")]),
        tmp_path,
        max_bytes=1234,
        cache_schemes=("s3",),
    )

    detail = reader.describe()
    rendered = json.dumps(detail)
    assert detail["max_bytes"] == 1234
    assert "http://127.0.0.1:9010" in rendered
    assert '"bucket": "models"' in rendered
    assert "wJalrXUtnFEMI" not in rendered
    assert "temporary-session" not in rendered


def test_a_version_from_before_packages_existed_is_loaded_and_reported_unverified(
    tmp_path: Path,
) -> None:
    path = write_weights(tmp_path, WEIGHTS)

    model = loaded({"version": "c" * 64, "checkpoint_uri": f"file://{path}"})

    assert model.verified is False
    assert model.runtime == "weights"
    assert model.describe()["verified"] is False


def test_a_declared_feature_count_the_weight_vector_contradicts_is_refused(
    tmp_path: Path,
) -> None:
    current = weights_version(write_weights(tmp_path, WEIGHTS))
    current["package"]["inputs"][0]["shape"] = [None, 8]

    with pytest.raises(LoadError) as refusal:
        loaded(current)

    assert "declares 8 features" in str(refusal.value)
    assert "holds 3 weights" in str(refusal.value)


# ── The rollout ──────────────────────────────────────────────────────────────


def test_a_registry_that_resolves_a_version_it_did_not_label_is_refused(tmp_path: Path) -> None:
    good = weights_version(write_weights(tmp_path, WEIGHTS))
    registry = FakeRegistry({"a" * 64: good}, labelled="d" * 64)
    registry.resolves_to = "a" * 64

    with pytest.raises(LoadError) as refusal:
        resolve(registry, "m")

    assert "resolved 'aaaa" in str(refusal.value)
    assert "label naming 'dddd" in str(refusal.value)


def test_a_broken_new_label_does_not_remove_the_ready_old_version(tmp_path: Path) -> None:
    path = write_weights(tmp_path, WEIGHTS)
    good = weights_version(path)
    broken_path = tmp_path / "broken.json"
    broken_path.write_text("not json at all")
    broken = weights_version(broken_path, version="e" * 64)
    registry = FakeRegistry({"a" * 64: good, "e" * 64: broken}, labelled="a" * 64)
    state = Server(registry, "m", None, loaders=available(onnx=False))
    state.start()

    registry.labelled = "e" * 64
    swapped = state.poll()

    assert swapped is False
    current = state.current
    assert current is not None and current.version == "a" * 64
    assert state.rollout_error is not None and "cannot become ready" in state.rollout_error


def test_a_version_that_failed_to_become_ready_is_not_read_again(tmp_path: Path) -> None:
    good = weights_version(write_weights(tmp_path, WEIGHTS))
    broken_path = tmp_path / "broken.json"
    broken_path.write_text("[]")
    broken = weights_version(broken_path, version="e" * 64)
    registry = FakeRegistry({"a" * 64: good, "e" * 64: broken}, labelled="a" * 64)
    state = Server(registry, "m", None, loaders=available(onnx=False))
    state.start()
    registry.labelled = "e" * 64
    state.poll()
    broken_path.write_text(json.dumps(WEIGHTS))  # even if the bytes are fixed under it

    assert state.poll() is False
    current = state.current
    assert current is not None and current.version == "a" * 64


def test_a_rollback_pins_out_the_version_it_left(tmp_path: Path) -> None:
    first = weights_version(write_weights(tmp_path, WEIGHTS))
    second = weights_version(write_weights(tmp_path, [1.0, 1.0, 1.0], name="two.json"), "e" * 64)
    registry = FakeRegistry({"a" * 64: first, "e" * 64: second}, labelled="a" * 64)
    state = Server(registry, "m", None, loaders=available(onnx=False))
    state.start()
    registry.labelled = "e" * 64
    assert state.poll() is True

    restored = state.roll_back()

    assert restored.version == "a" * 64
    assert state.poll() is False, "a rollback the next poll undoes is not a rollback"
    current = state.current
    assert current is not None and current.version == "a" * 64
    assert state.rollbacks == 1


def test_the_label_moving_to_another_runtime_swaps_the_loader_with_it(tmp_path: Path) -> None:
    session = StubSession(
        [StubArg("features", "tensor(float)", ["batch", 3])],
        [StubArg("probability", "tensor(float)", ["batch", 1])],
    )
    vector = weights_version(write_weights(tmp_path, WEIGHTS))
    graph = onnx_version(
        tmp_path,
        inputs=[{"name": "features", "dtype": "float32", "shape": [None, 3]}],
        outputs=[{"name": "probability", "classes": ["background", "edge"]}],
    )
    registry = FakeRegistry({"a" * 64: vector, "b" * 64: graph}, labelled="a" * 64)
    state = Server(registry, "m", None, loaders=stub_loaders(session))
    state.start()

    registry.labelled = "b" * 64
    assert state.poll() is True

    current, previous = state.current, state.previous
    assert current is not None and current.runtime == "onnx"
    assert previous is not None and previous.runtime == "weights"
    assert current.predictor.features == 3, "the request surface follows the loaded version"


# ── Shadow routing ──────────────────────────────────────────────────────────


def test_a_non_production_label_resolves_to_its_exact_version(tmp_path: Path) -> None:
    production = weights_version(write_weights(tmp_path, WEIGHTS))
    candidate = weights_version(
        write_weights(tmp_path, [1.0, 1.0, 1.0], "candidate.json"),
        version="e" * 64,
    )
    registry = FakeRegistry(
        {"a" * 64: production, "e" * 64: candidate},
        labelled="a" * 64,
        labels={"shadow": "e" * 64},
    )

    version, current = resolve_label(registry, "m", "shadow")

    assert version == "e" * 64
    assert current["version"] == "e" * 64


def test_a_broken_shadow_never_changes_primary_readiness(tmp_path: Path) -> None:
    production = weights_version(write_weights(tmp_path, WEIGHTS))
    broken_path = tmp_path / "broken-shadow.json"
    broken_path.write_text("not json")
    broken = weights_version(broken_path, version="e" * 64)
    registry = FakeRegistry(
        {"a" * 64: production, "e" * 64: broken},
        labelled="a" * 64,
        labels={"shadow": "e" * 64},
    )
    state = Server(
        registry,
        "m",
        None,
        loaders=available(onnx=False),
        shadow_label="shadow",
    )

    state.start()

    assert state.ready() is True
    assert state.current is not None and state.current.version == "a" * 64
    assert state.shadow is None
    assert "cannot become shadow-ready" in str(state.shadow_rollout_error)


def test_a_shadow_answer_is_discarded_and_its_work_never_queues(tmp_path: Path) -> None:
    production = weights_version(write_weights(tmp_path, WEIGHTS))
    shadow_path = write_weights(tmp_path, WEIGHTS, "shadow.json")
    candidate = weights_version(shadow_path, version="e" * 64, runtime="shadow-test")
    registry = FakeRegistry(
        {"a" * 64: production, "e" * 64: candidate},
        labelled="a" * 64,
        labels={"shadow": "e" * 64},
    )
    predictor = BlockingPredictor()
    loaders = available(onnx=False)
    loaders["shadow-test"] = BlockingLoader(predictor)
    state = Server(
        registry,
        "m",
        None,
        loaders=loaders,
        shadow_label="shadow",
        shadow_concurrency=1,
    )
    state.start()

    assert state.dispatch_shadow([[1.0, 2.0, 3.0]]) is True
    assert predictor.started.wait(timeout=1)
    assert state.dispatch_shadow([[4.0, 5.0, 6.0]]) is False
    current = state.current
    assert current is not None
    assert current.predictor.predict([[1.0, 2.0, 3.0]]) != [[0.75]]

    predictor.release.set()
    for _ in range(100):
        if state.shadow_status["requests"] == 1:
            break
        time.sleep(0.01)

    status = state.shadow_status
    assert status["requests"] == 1
    assert status["failures"] == 0
    assert status["dropped"] == 1
    assert status["model"]["version"] == "e" * 64


def test_moving_the_shadow_label_swaps_only_the_discarded_model(tmp_path: Path) -> None:
    production = weights_version(write_weights(tmp_path, WEIGHTS))
    first = weights_version(
        write_weights(tmp_path, [1.0, 1.0, 1.0], "shadow-one.json"),
        version="e" * 64,
    )
    second = weights_version(
        write_weights(tmp_path, [2.0, 2.0, 2.0], "shadow-two.json"),
        version="f" * 64,
    )
    registry = FakeRegistry(
        {"a" * 64: production, "e" * 64: first, "f" * 64: second},
        labelled="a" * 64,
        labels={"shadow": "e" * 64},
    )
    state = Server(
        registry,
        "m",
        None,
        loaders=available(onnx=False),
        shadow_label="shadow",
    )
    state.start()
    registry.labels["shadow"] = "f" * 64

    assert state.poll() is False
    assert state.current is not None and state.current.version == "a" * 64
    assert state.shadow is not None and state.shadow.version == "f" * 64
    assert state.shadow_rollouts == 2


def test_an_unresolvable_shadow_label_pauses_only_mirroring(tmp_path: Path) -> None:
    production = weights_version(write_weights(tmp_path, WEIGHTS))
    candidate = weights_version(
        write_weights(tmp_path, [1.0, 1.0, 1.0], "shadow.json"),
        version="e" * 64,
    )
    registry = FakeRegistry(
        {"a" * 64: production, "e" * 64: candidate},
        labelled="a" * 64,
        labels={"shadow": "e" * 64},
    )
    state = Server(
        registry,
        "m",
        None,
        loaders=available(onnx=False),
        shadow_label="shadow",
    )
    state.start()
    del registry.labels["shadow"]

    assert state.poll() is False
    assert state.current is not None and state.current.version == "a" * 64
    assert state.shadow is None
    assert "has no shadow label" in str(state.shadow_rollout_error)


def test_a_shadow_that_cannot_eat_primary_requests_is_refused(tmp_path: Path) -> None:
    production = weights_version(write_weights(tmp_path, WEIGHTS))
    candidate = weights_version(
        write_weights(tmp_path, [1.0, 1.0, 1.0, 1.0], "wide.json"),
        version="e" * 64,
    )
    candidate["package"]["inputs"][0]["shape"] = [None, 4]
    registry = FakeRegistry(
        {"a" * 64: production, "e" * 64: candidate},
        labelled="a" * 64,
        labels={"shadow": "e" * 64},
    )
    state = Server(
        registry,
        "m",
        None,
        loaders=available(onnx=False),
        shadow_label="shadow",
    )

    state.start()

    assert state.shadow is None
    assert "same request cannot be mirrored to both" in str(state.shadow_rollout_error)


# ── What a request may be ────────────────────────────────────────────────────


@pytest.mark.parametrize(
    ("body", "problem"),
    [
        ({"instances": [[1.0, 2.0]]}, "must have 3 features"),
        ({"instances": []}, "between 1 and 4 rows"),
        ({"instances": [[1.0, 2.0, 3.0]] * 5}, "between 1 and 4 rows"),
        ({"instances": [[1.0, 2.0, "3"]]}, "must be numbers"),
        ({"instances": [[1.0, 2.0, True]]}, "must be numbers"),
        ({"instances": [[1.0, 2.0, float("nan")]]}, "must be finite"),
        ({"instances": [[1.0, 2.0, 3.0]], "threshold": 2.0}, "between 0 and 1"),
        ({"instances": [[1.0, 2.0, 3.0]], "threshold": "high"}, "must be a number"),
        ([1, 2, 3], "must be a JSON object"),
    ],
)
def test_a_request_is_validated_against_the_shape_the_loaded_version_declares(
    body: Any, problem: str
) -> None:
    with pytest.raises(ValueError, match=problem):
        read_instances(body, 3, 4)


def test_a_binary_head_reads_as_the_probability_of_the_second_class() -> None:
    assert label_for([0.75], ["background", "edge"], 0.5)["class"] == "edge"
    assert label_for([0.25], ["background", "edge"], 0.5)["class"] == "background"
    assert label_for([0.1, 0.9], ["background", "edge"], 0.5)["class"] == "edge"
    assert "scores" in label_for([0.1, 0.9], [], 0.5), "no vocabulary means no invented one"


def test_an_inference_report_carries_no_inputs_and_no_outputs(tmp_path: Path) -> None:
    transport = Recording()
    telemetry = AiwatcherClient(service="serving", transport=transport)
    registry = FakeRegistry(
        {"a" * 64: weights_version(write_weights(tmp_path, WEIGHTS))}, labelled="a" * 64
    )
    state = Server(registry, "m", telemetry, loaders=available(onnx=False))
    state.start()

    state.record(rows=4, duration_ms=1.5, outcome="succeeded", model=state.current)

    completed = [event for event in transport.events if event["event_type"] == "llm.completed"]
    assert len(completed) == 1
    data = completed[0]["data"]
    assert data["rows"] == 4
    assert data["runtime"] == "weights"
    assert data["model_version"] == "a" * 64
    assert not {"instances", "predictions", "input", "output"} & set(data)
    body = json.dumps(transport.events)
    assert "instances" not in body and "predictions" not in body


def test_a_shadow_report_names_its_traffic_without_its_answer(tmp_path: Path) -> None:
    transport = Recording()
    telemetry = AiwatcherClient(service="serving", transport=transport)
    registry = FakeRegistry(
        {"a" * 64: weights_version(write_weights(tmp_path, WEIGHTS))},
        labelled="a" * 64,
    )
    state = Server(registry, "m", telemetry, loaders=available(onnx=False))
    state.start()

    state.record(
        rows=2,
        duration_ms=2.5,
        outcome="succeeded",
        model=state.current,
        label="candidate",
        traffic="shadow",
    )

    completed = [event for event in transport.events if event["event_type"] == "llm.completed"]
    assert len(completed) == 1
    data = completed[0]["data"]
    assert data["label"] == "candidate"
    assert data["traffic"] == "shadow"
    assert not {"instances", "predictions", "input", "output"} & set(data)


# ── The graph's own answer, against what the package claimed ─────────────────


def onnx_load(
    tmp_path: Path,
    session: StubSession,
    *,
    inputs: Sequence[Mapping[str, Any]] | None = None,
    outputs: Sequence[Mapping[str, Any]] | None = None,
    **overrides: Any,
) -> Any:
    current = onnx_version(tmp_path, inputs=inputs, outputs=outputs, **overrides)
    return load(current, "m", stub_loaders(session), FileReader())


def test_a_declared_input_the_graph_does_not_have_is_refused(tmp_path: Path) -> None:
    session = StubSession(
        [StubArg("features", "tensor(float)", ["batch", 3])],
        [StubArg("probability", "tensor(float)", ["batch", 1])],
    )

    with pytest.raises(LoadError) as refusal:
        onnx_load(tmp_path, session, inputs=[{"name": "pixels", "shape": [None, 3]}])

    assert "no such input" in str(refusal.value)
    assert "It has: features" in str(refusal.value)


def test_a_declared_shape_the_graph_contradicts_is_refused(tmp_path: Path) -> None:
    session = StubSession(
        [StubArg("features", "tensor(float)", ["batch", 16])],
        [StubArg("probability", "tensor(float)", ["batch", 1])],
    )

    with pytest.raises(LoadError) as refusal:
        onnx_load(tmp_path, session, inputs=[{"name": "features", "shape": [None, 8]}])

    assert "dimension 1 as 8 and the graph declares 16" in str(refusal.value)
    assert "do not describe the same model" in str(refusal.value)


def test_a_declared_dtype_the_graph_contradicts_is_refused(tmp_path: Path) -> None:
    session = StubSession(
        [StubArg("features", "tensor(int64)", ["batch", 3])],
        [StubArg("probability", "tensor(float)", ["batch", 1])],
    )

    with pytest.raises(LoadError) as refusal:
        onnx_load(tmp_path, session, inputs=[{"name": "features", "dtype": "float32"}])

    assert "as float32 and the graph declares it int64" in str(refusal.value)


def test_a_free_batch_dimension_never_contradicts_a_declared_one(tmp_path: Path) -> None:
    session = StubSession(
        [StubArg("features", "tensor(float)", ["batch", 3])],
        [StubArg("probability", "tensor(float)", ["batch", 1])],
    )

    model = onnx_load(
        tmp_path, session, inputs=[{"name": "features", "dtype": "float32", "shape": [None, 3]}]
    )

    assert model.predictor.features == 3


def test_an_output_that_is_not_as_wide_as_its_vocabulary_is_refused(tmp_path: Path) -> None:
    session = StubSession(
        [StubArg("features", "tensor(float)", ["batch", 3])],
        [StubArg("probability", "tensor(float)", ["batch", 5])],
        width=5,
    )

    with pytest.raises(LoadError) as refusal:
        onnx_load(
            tmp_path,
            session,
            outputs=[{"name": "probability", "classes": ["background", "edge"]}],
        )

    assert "names 2 classes" in str(refusal.value)
    assert "is 5 wide" in str(refusal.value)


def test_a_graph_with_two_inputs_is_refused_because_the_surface_sends_one(tmp_path: Path) -> None:
    session = StubSession(
        [
            StubArg("features", "tensor(float)", ["batch", 3]),
            StubArg("mask", "tensor(float)", ["batch", 3]),
        ],
        [StubArg("probability", "tensor(float)", ["batch", 1])],
    )

    with pytest.raises(LoadError) as refusal:
        onnx_load(tmp_path, session)

    assert "takes 2 inputs (features, mask)" in str(refusal.value)


def test_a_graph_eating_an_image_tensor_is_refused_by_rank(tmp_path: Path) -> None:
    session = StubSession(
        [StubArg("pixels", "tensor(float)", ["batch", 3, 224, 224])],
        [StubArg("probability", "tensor(float)", ["batch", 1])],
    )

    with pytest.raises(LoadError) as refusal:
        onnx_load(tmp_path, session)

    assert "requests carry images" in str(refusal.value)


def test_a_graph_with_a_pinned_batch_axis_is_refused_before_it_serves(tmp_path: Path) -> None:
    session = StubSession(
        [StubArg("features", "tensor(float)", [1, 3])],
        [StubArg("probability", "tensor(float)", [1, 1])],
    )

    with pytest.raises(LoadError) as refusal:
        onnx_load(tmp_path, session)

    assert "pins its batch axis at 1" in str(refusal.value)


def test_a_graph_this_request_surface_cannot_feed_is_refused(tmp_path: Path) -> None:
    session = StubSession(
        [StubArg("prompt", "tensor(string)", ["batch", 1])],
        [StubArg("probability", "tensor(float)", ["batch", 1])],
    )

    with pytest.raises(LoadError) as refusal:
        onnx_load(tmp_path, session)

    assert "rows of JSON numbers" in str(refusal.value)


def test_an_onnxruntime_older_than_the_package_declares_is_refused(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(onnx_runtime, "_installed_version", lambda: "1.16.3")
    session = StubSession(
        [StubArg("features", "tensor(float)", ["batch", 3])],
        [StubArg("probability", "tensor(float)", ["batch", 1])],
    )

    with pytest.raises(LoadError) as refusal:
        onnx_load(tmp_path, session, runtime_version="1.20")

    assert "1.20" in str(refusal.value) and "1.16.3" in str(refusal.value)
    assert "crash loop" in str(refusal.value)

    monkeypatch.setattr(onnx_runtime, "_installed_version", lambda: "1.22.0")
    assert onnx_load(tmp_path, session, runtime_version="1.20") is not None


def test_the_declared_dtype_decides_what_the_graph_is_fed(tmp_path: Path) -> None:
    session = StubSession(
        [StubArg("features", "tensor(float)", ["batch", 3])],
        [StubArg("probability", "tensor(float)", ["batch", 1])],
    )
    model = onnx_load(tmp_path, session)

    scores = model.predictor.predict([[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]])

    assert session.fed["features"].dtype == np.dtype("float32")
    assert scores == [[0.75], [0.75]]
    warm(model)


def test_preprocessing_is_reported_and_never_applied(tmp_path: Path) -> None:
    session = StubSession(
        [StubArg("features", "tensor(float)", ["batch", 3])],
        [StubArg("probability", "tensor(float)", ["batch", 1])],
    )
    model = onnx_load(tmp_path, session, preprocessing=["edge-grid:8x8", "normalize:imagenet"])

    detail = model.predictor.describe()
    assert detail["preprocessing"] == ["edge-grid:8x8", "normalize:imagenet"]
    model.predictor.predict([[1.0, 2.0, 3.0]])
    assert session.fed["features"].tolist() == [[1.0, 2.0, 3.0]], "reported, not applied"
