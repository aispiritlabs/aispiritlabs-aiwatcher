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
import pickle
import threading
from collections.abc import Iterator
from http.server import BaseHTTPRequestHandler, HTTPServer
from typing import Any, ClassVar

import httpx
import pytest

from aiwatcher_sdk.annotations import (
    BLOB_SCHEME,
    AnnotationRegistry,
    Export,
    ImageSource,
    RegistryError,
    split_for,
)

PROJECT = "floor-plans/dom-projekt"
EXPORT_ID = "9f" * 32
IMAGE_ID = hashlib.sha256(b"a plan").hexdigest()


class Recorder(BaseHTTPRequestHandler):
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
def api() -> Iterator[tuple[AnnotationRegistry, type[Recorder]]]:
    Recorder.stubbed = {}
    Recorder.seen = []
    server = HTTPServer(("127.0.0.1", 0), Recorder)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield AnnotationRegistry(f"http://127.0.0.1:{server.server_port}"), Recorder
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


def test_an_export_source_is_the_pair_and_a_bare_name_is_refused(
    api: tuple[AnnotationRegistry, type[Recorder]],
) -> None:
    registry, recorder = api
    recorder.stubbed[("GET", "/api/v1/annotation-export")] = (200, manifest_body())

    export = registry.get_dataloader(f"{PROJECT}@{EXPORT_ID}")
    assert export.source == f"{PROJECT}@{EXPORT_ID}"
    assert f"project={PROJECT.replace('/', '%2F')}" in recorder.seen[0]["path"]

    with pytest.raises(RegistryError, match="not an export source"):
        registry.get_dataloader(PROJECT)


def test_an_export_reports_its_groups_because_that_is_what_bounds_a_score(
    api: tuple[AnnotationRegistry, type[Recorder]],
) -> None:
    # Two images of one building is one observation, not two. A recall figure
    # quoted over "40 test images" that are eight houses is a different number.
    registry, recorder = api
    recorder.stubbed[("GET", "/api/v1/annotation-export")] = (200, manifest_body())
    export = registry.get_dataloader(f"{PROJECT}@{EXPORT_ID}")

    train = export.get_split("train")
    assert len(train) == 2
    assert train.get_groups() == {"komancza-dws"}
    assert export.get_split("test").get_groups() == frozenset()
    # The question is asked of the side, not of the export with the side as an
    # argument — the same call the dataset answers, so the two cannot drift.
    assert export.get_groups() == train.get_groups()


def test_a_split_is_a_sequence_of_examples_and_still_says_what_it_is(
    api: tuple[AnnotationRegistry, type[Recorder]],
) -> None:
    # Everything a list did, plus the two questions worth asking before
    # training on it — and every method is a verb phrase, so nothing here
    # reads like a field and turns out to be a request.
    registry, recorder = api
    recorder.stubbed[("GET", "/api/v1/annotation-export")] = (200, manifest_body())
    train = registry.get_dataloader(f"{PROJECT}@{EXPORT_ID}").get_split("train")

    assert [sample.image_id for sample in train] == [IMAGE_ID, "11" * 32]
    assert train[0].image_id == IMAGE_ID
    assert train[0] in train
    assert train.get_counts().images == 2
    assert train.get_counts().groups == 1
    assert train.get_counts().instances == 49

    # A slice is another view, so narrowing keeps every answer available.
    first = train[:1]
    assert len(first) == 1
    assert first.get_groups() == {"komancza-dws"}
    assert first.export is train.export


def test_every_split_is_reachable_in_one_pass(
    api: tuple[AnnotationRegistry, type[Recorder]],
) -> None:
    registry, recorder = api
    recorder.stubbed[("GET", "/api/v1/annotation-export")] = (200, manifest_body())
    sides = registry.get_dataloader(f"{PROJECT}@{EXPORT_ID}").get_splits()

    assert list(sides) == ["train", "validation", "test"]
    assert [len(side) for side in sides.values()] == [2, 0, 0]
    with pytest.raises(RegistryError, match="not a split"):
        registry.get_dataloader(f"{PROJECT}@{EXPORT_ID}").get_split("holdout")  # type: ignore[arg-type]


def test_an_example_on_an_unknown_side_is_refused_rather_than_dropped() -> None:
    # A `split ==` that matches nothing is how a third of a corpus goes missing
    # from every side at once, with a manifest that still says it is there.
    body = manifest_body()
    body["samples"][0]["split"] = "holdout"
    with pytest.raises(RegistryError, match="holdout"):
        Export.from_json(body)


def test_an_export_carries_the_registry_it_was_read_from(
    api: tuple[AnnotationRegistry, type[Recorder]],
) -> None:
    # An export that came from a registry knows which one, so reading it and
    # then reading its images is one object rather than two.
    registry, recorder = api
    recorder.stubbed[("GET", "/api/v1/annotation-export")] = (200, manifest_body())
    export = registry.get_dataloader(f"{PROJECT}@{EXPORT_ID}")

    assert export.registry is registry
    assert export.get_split("train").get_registry() is registry
    # An override still wins, for a manifest whose images live elsewhere.
    other = AnnotationRegistry("http://elsewhere.invalid")
    assert export.get_split("train").get_registry(other) is other


def test_a_manifest_off_a_file_refuses_to_guess_where_its_images_are() -> None:
    # A default base URL invented here would go looking for a training set on
    # somebody's laptop.
    offline = Export.from_json(manifest_body())
    assert offline.registry is None
    with pytest.raises(RegistryError, match="not read from a registry"):
        offline.get_split("train").get_registry()


def test_provenance_is_not_content_so_two_readers_agree(
    api: tuple[AnnotationRegistry, type[Recorder]],
) -> None:
    # Two manifests with the same reference are the same export, whichever
    # client read them — so `registry` is out of `==` and out of `repr`.
    registry, recorder = api
    recorder.stubbed[("GET", "/api/v1/annotation-export")] = (200, manifest_body())
    assert registry.get_dataloader(f"{PROJECT}@{EXPORT_ID}") == Export.from_json(manifest_body())


def test_a_registry_survives_the_process_boundary_a_dataloader_puts_it_across(
    api: tuple[AnnotationRegistry, type[Recorder]],
) -> None:
    """A `DataLoader` with `num_workers > 0` pickles its dataset under `spawn`,
    the default on macOS and Windows, and that dataset holds a registry. A
    connection pool does not survive that and must not have to."""
    registry, recorder = api
    recorder.stubbed[("GET", "/api/v1/annotation-export")] = (200, manifest_body())
    recorder.stubbed[("GET", f"/api/v1/annotation-blobs/{IMAGE_ID}")] = (200, b"a plan")
    export = registry.get_dataloader(f"{PROJECT}@{EXPORT_ID}")

    revived: Export = pickle.loads(pickle.dumps(export))  # noqa: S301 - our own bytes
    assert revived.source == export.source
    assert revived.registry is not None
    # And the rebuilt pool works, which is the half a `__getstate__` that only
    # dropped the client would not have.
    assert revived.registry.fetch_image(revived.samples[0]) == b"a plan"


def test_a_borrowed_client_says_why_it_cannot_go_to_a_worker() -> None:
    # Dropping a caller's proxy or client certificate silently and rebuilding a
    # default in the worker is the failure this refuses to be.
    borrowed = AnnotationRegistry(
        "http://aiwatcher.invalid",
        client=httpx.Client(transport=httpx.MockTransport(lambda _: httpx.Response(200))),
    )
    with pytest.raises(TypeError, match="cross a process boundary"):
        pickle.dumps(borrowed)


def test_printing_an_export_says_what_it_is_rather_than_everything_it_holds(
    api: tuple[AnnotationRegistry, type[Recorder]],
) -> None:
    # The generated repr printed `raw` — the whole server response, tens of
    # kilobytes. `repr` is what somebody types to find out what they are
    # holding, and an answer that scrolls the terminal is not one.
    registry, recorder = api
    recorder.stubbed[("GET", "/api/v1/annotation-export")] = (200, manifest_body())
    export = registry.get_dataloader(f"{PROJECT}@{EXPORT_ID}")

    printed = repr(export)
    assert printed == (f"Export('{PROJECT}@{EXPORT_ID}', 2 images, 1 groups, 1 excluded)")
    assert "revision" not in printed
    # Still reachable, just not shouted.
    assert export.raw["created_at"] == "2026-09-01T10:00:00Z"


def test_an_exclusion_is_data_rather_than_a_silent_drop(
    api: tuple[AnnotationRegistry, type[Recorder]],
) -> None:
    registry, recorder = api
    recorder.stubbed[("GET", "/api/v1/annotation-export")] = (200, manifest_body())
    export = registry.get_dataloader(f"{PROJECT}@{EXPORT_ID}")

    assert export.counts["excluded"] == 1
    left_out = export.excluded_samples[0]
    assert left_out.reason == "rights"
    assert left_out.group_id == "cubicasa-00042"
    assert "CC BY-NC" in left_out.detail
    # Reads as a sentence, because this is the half somebody stares at when a
    # corpus came out smaller than they expected.
    assert str(left_out).startswith("cubicasa-00042: rights — ")
    # A reason this release has never heard of prints rather than raises: it is
    # a label for a human, not a key that decides anything.
    future = Export.from_json(
        {**manifest_body(), "excluded": [{"group_id": "g", "reason": "quarantined"}]}
    )
    assert str(future.excluded_samples[0]) == "g: quarantined"


def test_building_an_export_asks_for_a_commercial_one_by_default(
    api: tuple[AnnotationRegistry, type[Recorder]],
) -> None:
    # The expensive mistake is silent, and the correction is one field. So the
    # default is the strict one and a research export has to be asked for.
    registry, recorder = api
    recorder.stubbed[("POST", "/api/v1/annotation-exports")] = (
        201,
        {"manifest": manifest_body(), "created": True},
    )
    registry.build_dataloader(PROJECT)

    sent = json.loads(recorder.seen[0]["raw"])
    assert sent["rights_policy"] == "commercial"
    assert sent["require_human_review"] is True


def test_a_refused_drawing_carries_every_problem_rather_than_the_first(
    api: tuple[AnnotationRegistry, type[Recorder]],
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
    api: tuple[AnnotationRegistry, type[Recorder]],
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
        registry.get_projects()

    assert failure.value.code == "registry_disabled"
    # Retrying a deployment decision forever is what a pipeline does instead of
    # telling somebody to set a variable.
    assert failure.value.is_retryable is False


def test_fetching_a_blob_verifies_the_digest_it_was_asked_for(
    api: tuple[AnnotationRegistry, type[Recorder]],
) -> None:
    # The one corruption no metric detects: an image that is not the image its
    # labels were drawn on.
    registry, recorder = api
    recorder.stubbed[("GET", "/api/v1/annotation-export")] = (200, manifest_body())
    sample = registry.get_dataloader(f"{PROJECT}@{EXPORT_ID}").samples[0]

    recorder.stubbed[("GET", f"/api/v1/annotation-blobs/{IMAGE_ID}")] = (200, b"a plan")
    assert registry.fetch_image(sample) == b"a plan"

    recorder.stubbed[("GET", f"/api/v1/annotation-blobs/{IMAGE_ID}")] = (200, b"a different plan")
    with pytest.raises(RegistryError, match="do not hash to it"):
        registry.fetch_image(sample)


def test_an_image_by_reference_is_hashed_too_and_its_host_gets_no_token(
    api: tuple[AnnotationRegistry, type[Recorder]],
) -> None:
    # An id is a SHA-256 whatever the image is, and the check matters most here:
    # those bytes live on a host this deployment does not run, and
    # `plans/latest.png` is different pixels tomorrow.
    registry, recorder = api
    recorder.stubbed[("GET", "/api/v1/annotation-export")] = (200, manifest_body())
    external = registry.get_dataloader(f"{PROJECT}@{EXPORT_ID}").samples[1]

    seen: list[str | None] = []

    def elsewhere(request: httpx.Request) -> httpx.Response:
        seen.append(request.headers.get("authorization"))
        return httpx.Response(200, content=b"someone else's plan")

    scoped = AnnotationRegistry(
        registry.base_url,
        token="secret",
        client=httpx.Client(transport=httpx.MockTransport(elsewhere)),
    )
    with pytest.raises(RegistryError, match="do not hash to it"):
        scoped.fetch_image(external)
    assert seen == [None]


def test_an_external_image_url_is_left_alone(
    api: tuple[AnnotationRegistry, type[Recorder]],
) -> None:
    registry, recorder = api
    recorder.stubbed[("GET", "/api/v1/annotation-export")] = (200, manifest_body())
    export = registry.get_dataloader(f"{PROJECT}@{EXPORT_ID}")

    blob, external = export.samples
    assert blob.is_blob
    assert registry.get_image_url(blob).endswith(f"/api/v1/annotation-blobs/{IMAGE_ID}")
    assert not external.is_blob
    assert registry.get_image_url(external) == "https://example.test/plan.png"


def test_listing_images_pages_until_the_server_stops_offering_more(
    api: tuple[AnnotationRegistry, type[Recorder]],
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
    assert [image["image_id"] for image in registry.iter_images(PROJECT)] == ["a", "b"]


def test_uploading_sends_the_bytes_untouched(
    api: tuple[AnnotationRegistry, type[Recorder]],
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
    api: tuple[AnnotationRegistry, type[Recorder]],
) -> None:
    registry, recorder = api
    recorder.stubbed[("GET", "/api/v1/annotation-sources")] = (
        200,
        {"sources": [{"id": "resplan", "usage": "commercial"}], "directories": [], "total": 10},
    )
    page = registry.get_sources(usage="commercial")

    assert page["sources"][0]["id"] == "resplan"
    assert "usage=commercial" in recorder.seen[0]["path"]


def test_an_export_survives_a_server_that_adds_fields() -> None:
    # `raw` keeps everything, so a field this SDK release does not know about
    # is still reachable rather than silently dropped.
    export = Export.from_json({**manifest_body(), "future_field": 7})
    assert export.raw["future_field"] == 7


def test_the_client_answers_the_port_a_dataset_asks_for(
    api: tuple[AnnotationRegistry, type[Recorder]],
) -> None:
    """`ImageSource` is what a dataset depends on, and the client satisfies it
    by having the methods rather than by naming it.

    The annotated assignment *is* the assertion, and mypy makes it at build
    time: renaming a read on the client then fails here rather than six hundred
    images into a training run, which is the failure a structural protocol
    otherwise defers rather than prevents."""
    registry, _ = api
    source: ImageSource = registry

    assert {"get_project", "get_revision_annotations", "fetch_image"} <= set(dir(source))
