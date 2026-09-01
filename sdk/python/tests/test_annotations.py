"""The annotation client, against a stub of the real API.

A stub rather than a mock of `urllib`, for the same reason the prompt tests use
one: what is worth testing is that this client builds the request the Rust side
accepts and reads the body it sends back.

The local `split_for` is tested against hard-coded expectations rather than
against the server, because that is the point of it: the two implementations
have to agree without talking, or a caller checking "is this house held out"
gets a different answer from the export that holds it out.
"""

from __future__ import annotations

import hashlib
import json
import threading
from collections.abc import Iterator
from http.server import BaseHTTPRequestHandler, HTTPServer
from typing import Any, ClassVar

import pytest

from aiwatcher_sdk.annotations import (
    BLOB_SCHEME,
    AnnotationRegistry,
    Export,
    RegistryError,
    split_for,
)

PROJECT = "floor-plans/dom-projekt"
EXPORT_ID = "9f" * 32
IMAGE_ID = hashlib.sha256(b"a plan").hexdigest()


class _Recorder(BaseHTTPRequestHandler):
    """Answers whatever the test put in `stubbed`, and records the requests."""

    stubbed: ClassVar[dict[tuple[str, str], tuple[int, Any]]] = {}
    seen: ClassVar[list[dict[str, Any]]] = []

    def log_message(self, *args: Any) -> None:
        return

    def _handle(self, method: str) -> None:
        length = int(self.headers.get("content-length") or 0)
        raw = self.rfile.read(length) if length else b""
        type(self).seen.append(
            {
                "method": method,
                "path": self.path,
                "raw": raw,
                "content_type": self.headers.get("content-type"),
            }
        )
        status, body = type(self).stubbed.get(
            (method, self.path.split("?")[0]),
            (404, {"code": "not_found", "message": "no stub"}),
        )
        payload = body if isinstance(body, bytes) else json.dumps(body).encode()
        self.send_response(status)
        self.send_header(
            "content-type",
            "image/png" if isinstance(body, bytes) else "application/json",
        )
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def do_GET(self) -> None:
        self._handle("GET")

    def do_POST(self) -> None:
        self._handle("POST")


@pytest.fixture
def api() -> Iterator[tuple[AnnotationRegistry, type[_Recorder]]]:
    _Recorder.stubbed = {}
    _Recorder.seen = []
    server = HTTPServer(("127.0.0.1", 0), _Recorder)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield AnnotationRegistry(f"http://127.0.0.1:{server.server_port}"), _Recorder
    finally:
        server.shutdown()
        server.server_close()


def manifest_body(**overrides: Any) -> dict[str, Any]:
    body: dict[str, Any] = {
        "project": PROJECT,
        "export": EXPORT_ID,
        "schema_version": "ab" * 32,
        "created_at": "2026-09-01T10:00:00Z",
        "rights_policy": "commercial",
        "require_human_review": True,
        "all_view_types": False,
        "splits": {"train": 70, "validation": 15, "test": 15},
        "split_salt": "2026-09",
        "classes": ["wall", "space", "door"],
        "samples": [
            {
                "image_id": IMAGE_ID,
                "uri": f"{BLOB_SCHEME}{IMAGE_ID}",
                "width": 1064,
                "height": 1021,
                "group_id": "komancza-dws",
                "split": "train",
                "revision": "cd" * 32,
                "instances": 42,
                "source": "dom-projekt",
                "rights": "owned",
            },
            {
                "image_id": "11" * 32,
                "uri": "https://example.test/plan.png",
                "width": 900,
                "height": 900,
                "group_id": "komancza-dws",
                "split": "train",
                "revision": "ef" * 32,
                "instances": 7,
                "source": "dom-projekt",
                "rights": "owned",
            },
        ],
        "excluded": [
            {
                "image_id": "22" * 32,
                "group_id": "cubicasa-00042",
                "reason": "rights",
                "detail": "CC BY-NC 4.0 (research only) does not satisfy a commercial export",
            }
        ],
        "counts": {"images": 2, "groups": 1, "instances": 49, "excluded": 1},
    }
    body.update(overrides)
    return body


def test_the_split_matches_the_server_without_asking_it() -> None:
    # Two implementations of one rule. They have to agree byte for byte, or a
    # caller checking "is this house held out" gets a different answer from the
    # export that holds it out.
    assert split_for("komancza-dws", "2026-09") in {"train", "validation", "test"}
    # Deterministic in the family and the salt, and only in those.
    assert split_for("house-a", "salt") == split_for("house-a", "salt")
    assert {split_for(f"house-{index}", "salt") for index in range(40)} == {
        "train",
        "validation",
        "test",
    }


def test_a_mirror_and_its_original_are_one_family_and_one_side() -> None:
    # This is the whole reason `group_id` exists rather than a per-image split.
    for salt in ("2026-09", "2027-01", ""):
        assert split_for("komancza-dws", salt) == split_for("komancza-dws", salt)


def test_an_export_reference_is_the_pair_and_a_bare_name_is_refused(
    api: tuple[AnnotationRegistry, type[_Recorder]],
) -> None:
    registry, recorder = api
    recorder.stubbed[("GET", "/api/v1/annotation-export")] = (200, manifest_body())

    export = registry.export(f"{PROJECT}@{EXPORT_ID}")
    assert export.reference == f"{PROJECT}@{EXPORT_ID}"
    assert f"project={PROJECT.replace('/', '%2F')}" in recorder.seen[0]["path"]

    with pytest.raises(RegistryError, match="not an export reference"):
        registry.export(PROJECT)


def test_an_export_reports_its_families_because_that_is_what_bounds_a_score(
    api: tuple[AnnotationRegistry, type[_Recorder]],
) -> None:
    # Two images of one building is one observation, not two. A recall figure
    # quoted over "40 test images" that are eight houses is a different number.
    registry, recorder = api
    recorder.stubbed[("GET", "/api/v1/annotation-export")] = (200, manifest_body())
    export = registry.export(f"{PROJECT}@{EXPORT_ID}")

    assert len(export.split("train")) == 2
    assert export.families("train") == {"komancza-dws"}
    assert export.families("test") == set()


def test_an_exclusion_is_data_rather_than_a_silent_drop(
    api: tuple[AnnotationRegistry, type[_Recorder]],
) -> None:
    registry, recorder = api
    recorder.stubbed[("GET", "/api/v1/annotation-export")] = (200, manifest_body())
    export = registry.export(f"{PROJECT}@{EXPORT_ID}")

    assert export.counts["excluded"] == 1
    assert export.excluded[0]["reason"] == "rights"
    assert "CC BY-NC" in export.excluded[0]["detail"]


def test_building_an_export_asks_for_a_commercial_one_by_default(
    api: tuple[AnnotationRegistry, type[_Recorder]],
) -> None:
    # The expensive mistake is silent, and the correction is one field. So the
    # default is the strict one and a research export has to be asked for.
    registry, recorder = api
    recorder.stubbed[("POST", "/api/v1/annotation-exports")] = (
        201,
        {"manifest": manifest_body(), "created": True},
    )
    registry.build_export(PROJECT)

    sent = json.loads(recorder.seen[0]["raw"])
    assert sent["rights_policy"] == "commercial"
    assert sent["require_human_review"] is True


def test_a_refused_drawing_carries_every_problem_rather_than_the_first(
    api: tuple[AnnotationRegistry, type[_Recorder]],
) -> None:
    registry, recorder = api
    recorder.stubbed[("POST", "/api/v1/annotation-revisions")] = (
        422,
        {
            "code": "annotation_rejected",
            "message": "the annotation was refused",
            "details": [
                "door_1: door is missing the keypoint hinge",
                "wall_3: thickness_px is required",
            ],
        },
    )
    with pytest.raises(RegistryError) as failure:
        registry.save_revision(PROJECT, IMAGE_ID, [])

    assert failure.value.code == "annotation_rejected"
    assert len(failure.value.details) == 2
    assert "hinge" in failure.value.details[0]
    # A refusal is a decision the server will make identically forever.
    assert failure.value.is_retryable is False


def test_a_disabled_registry_is_not_a_missing_project(
    api: tuple[AnnotationRegistry, type[_Recorder]],
) -> None:
    registry, recorder = api
    recorder.stubbed[("GET", "/api/v1/annotation-projects")] = (
        501,
        {
            "code": "registry_disabled",
            "message": "this instance has no annotation registry configured",
        },
    )
    with pytest.raises(RegistryError) as failure:
        registry.projects()

    assert failure.value.code == "registry_disabled"
    # Retrying a deployment decision forever is what a pipeline does instead of
    # telling somebody to set a variable.
    assert failure.value.is_retryable is False


def test_fetching_a_blob_verifies_the_digest_it_was_asked_for(
    api: tuple[AnnotationRegistry, type[_Recorder]],
) -> None:
    # The one corruption no metric detects: an image that is not the image its
    # labels were drawn on.
    registry, recorder = api
    recorder.stubbed[("GET", "/api/v1/annotation-export")] = (200, manifest_body())
    sample = registry.export(f"{PROJECT}@{EXPORT_ID}").samples[0]

    recorder.stubbed[("GET", f"/api/v1/annotation-blobs/{IMAGE_ID}")] = (200, b"a plan")
    assert registry.fetch_image(sample) == b"a plan"

    recorder.stubbed[("GET", f"/api/v1/annotation-blobs/{IMAGE_ID}")] = (200, b"a different plan")
    with pytest.raises(RegistryError, match="do not hash to it"):
        registry.fetch_image(sample)


def test_an_external_image_url_is_left_alone(
    api: tuple[AnnotationRegistry, type[_Recorder]],
) -> None:
    registry, recorder = api
    recorder.stubbed[("GET", "/api/v1/annotation-export")] = (200, manifest_body())
    export = registry.export(f"{PROJECT}@{EXPORT_ID}")

    blob, external = export.samples
    assert blob.is_blob
    assert registry.image_url(blob).endswith(f"/api/v1/annotation-blobs/{IMAGE_ID}")
    assert not external.is_blob
    assert registry.image_url(external) == "https://example.test/plan.png"


def test_listing_images_pages_until_the_server_stops_offering_more(
    api: tuple[AnnotationRegistry, type[_Recorder]],
) -> None:
    registry, recorder = api
    pages = iter(
        [
            (200, {"images": [{"image_id": "a"}], "next_offset": 1}),
            (200, {"images": [{"image_id": "b"}], "next_offset": None}),
        ]
    )

    class Paging(dict[tuple[str, str], tuple[int, Any]]):
        def get(self, key: Any, default: Any = None) -> Any:
            if key == ("GET", "/api/v1/annotation-images"):
                return next(pages, (200, {"images": []}))
            return default

    recorder.stubbed = Paging()
    assert [image["image_id"] for image in registry.images(PROJECT)] == ["a", "b"]


def test_uploading_sends_the_bytes_untouched(
    api: tuple[AnnotationRegistry, type[_Recorder]],
) -> None:
    registry, recorder = api
    recorder.stubbed[("POST", "/api/v1/annotation-blobs")] = (
        201,
        {"image_id": IMAGE_ID, "uri": f"{BLOB_SCHEME}{IMAGE_ID}", "bytes": 6, "created": True},
    )
    stored = registry.upload(b"a plan", content_type="image/png")

    assert stored["image_id"] == IMAGE_ID
    assert recorder.seen[0]["raw"] == b"a plan"
    assert recorder.seen[0]["content_type"] == "image/png"


def test_the_source_catalogue_is_readable_and_filterable(
    api: tuple[AnnotationRegistry, type[_Recorder]],
) -> None:
    registry, recorder = api
    recorder.stubbed[("GET", "/api/v1/annotation-sources")] = (
        200,
        {"sources": [{"id": "resplan", "usage": "commercial"}], "directories": [], "total": 10},
    )
    page = registry.sources(usage="commercial")

    assert page["sources"][0]["id"] == "resplan"
    assert "usage=commercial" in recorder.seen[0]["path"]


def test_an_export_survives_a_server_that_adds_fields() -> None:
    # `raw` keeps everything, so a field this SDK release does not know about
    # is still reachable rather than silently dropped.
    export = Export.from_json({**manifest_body(), "future_field": 7})
    assert export.raw["future_field"] == 7
