"""The rasteriser, and the properties that make its output a training target.

This module is tested harder than the clients around it for one reason: every
other failure in this SDK raises. A rasteriser that is subtly wrong does not.
A mask offset by half a pixel, a wall that vanished below a rounding threshold
or a hole that filled solid all produce a finite loss, a completed run and a
model that scores well against the same broken target it was fitted to.

So what is asserted here is geometry, not shape agreement: where the pixels
are, which class won a contested one, and what the loss was told to skip.
"""

from __future__ import annotations

import hashlib
import importlib.util
import io
import json
import threading
from collections.abc import Iterator
from http.server import BaseHTTPRequestHandler, HTTPServer
from typing import Any, ClassVar

import numpy as np
import pytest
from PIL import Image

from aiwatcher_sdk.annotations import (
    AnnotationRegistry,
    Export,
    ImageSource,
    RegistryError,
    Sample,
)
from aiwatcher_sdk.integrations.vision import (
    BlobCache,
    ExportDataset,
    SegmentationScore,
    decode_image,
    fit_letterbox,
    layers_for,
    rasterize,
)

# A vocabulary with no domain in it, because the SDK ships none. It is chosen
# to exercise the rasteriser: a filled region, a stroked line whose width is an
# attribute, an overlay on its own layer that must not erase what it crosses,
# and a class the loss must skip entirely.
CLASSES: list[dict[str, Any]] = [
    {"name": "region", "geometry": "polygon", "layer": 0},
    {"name": "block", "geometry": "polygon", "layer": 0},
    {"name": "line_outer", "geometry": "polyline", "layer": 0},
    {"name": "line_inner", "geometry": "polyline", "layer": 0},
    {"name": "mark_a", "geometry": "keypoints", "layer": 1},
    {"name": "mark_b", "geometry": "keypoints", "layer": 1},
    {"name": "ignore", "geometry": "polygon", "ignore": True},
]

BASE, OVERLAY = 0, 1
LAYERS = layers_for(CLASSES)
OUTER = LAYERS[BASE].classes.index("line_outer")
INNER = LAYERS[BASE].classes.index("line_inner")
REGION = LAYERS[BASE].classes.index("region")
MARK_A = LAYERS[OVERLAY].classes.index("mark_a")
MARK_B = LAYERS[OVERLAY].classes.index("mark_b")


def line(
    identifier: str, points: list[list[float]], role: str = "outer", thickness: float = 4.0
) -> dict[str, Any]:
    return {
        "id": identifier,
        "class": f"line_{role}",
        "geometry": {"kind": "polyline", "points": points},
        "attributes": {"thickness_px": thickness},
        "origin": "human",
    }


def polygon(
    identifier: str, klass: str, exterior: list[list[float]], holes: Any = ()
) -> dict[str, Any]:
    return {
        "id": identifier,
        "class": klass,
        "geometry": {"kind": "polygon", "exterior": exterior, "holes": list(holes)},
        "origin": "human",
    }


def mark(
    identifier: str,
    klass: str,
    start: list[float],
    end: list[float],
    line_id: str | None = None,
) -> dict[str, Any]:
    shape: dict[str, Any] = {
        "id": identifier,
        "class": klass,
        "geometry": {
            "kind": "keypoints",
            "points": [
                {"name": "start", "at": start, "visible": True},
                {"name": "end", "at": end, "visible": True},
            ],
        },
        "origin": "human",
    }
    if line_id:
        shape["links"] = {"line": [line_id]}
    return shape


# -- The transform -----------------------------------------------------------


def test_a_letterbox_preserves_aspect_ratio_and_centres_the_remainder() -> None:
    box = fit_letterbox(200, 100, 64)
    assert box.scale == pytest.approx(0.32)
    assert box.pad_x == 0
    assert box.pad_y == 16
    assert box.content == (0, 16, 64, 32)


def test_a_point_survives_the_round_trip_back_to_image_pixels() -> None:
    box = fit_letterbox(1064, 1021, 512)
    there = box.apply([612.0, 491.0])
    back = box.invert(there)
    assert back[0] == pytest.approx(612.0)
    assert back[1] == pytest.approx(491.0)


# -- What lands in the grids -------------------------------------------------


def test_a_wall_rasterises_where_it_was_drawn_and_nowhere_else() -> None:
    targets = rasterize([line("w", [[10, 20], [50, 20]], thickness=6.0)], CLASSES, 64, 64, size=64)
    assert targets.layers[BASE][20, 30] == OUTER
    # Half of six, so the band reaches three pixels either side and stops.
    assert targets.layers[BASE][17, 30] == OUTER
    assert targets.layers[BASE][10, 30] == 0
    # Round caps, so it does not run to the edge of the image.
    assert targets.layers[BASE][20, 0] == 0


def test_an_interior_wall_is_a_different_class_from_an_exterior_one() -> None:
    targets = rasterize(
        [line("a", [[10, 10], [50, 10]]), line("b", [[30, 20], [30, 50]], role="inner")],
        CLASSES,
        64,
        64,
        size=64,
    )
    assert targets.layers[BASE][10, 30] == OUTER
    assert targets.layers[BASE][40, 30] == INNER


def test_a_wall_keeps_the_pixels_it_shares_with_the_room_it_bounds() -> None:
    """The z-order, which is the one ordering decision in the rasteriser.

    A room polygon is drawn to the wall centreline, so it covers the wall. If
    it were painted last, every wall between two rooms would be a wall with a
    hole in it exactly where two rooms meet.
    """
    targets = rasterize(
        [
            polygon("r", "region", [[10, 10], [50, 10], [50, 50], [10, 50]]),
            line("w", [[10, 30], [50, 30]], role="inner", thickness=4.0),
        ],
        CLASSES,
        64,
        64,
        size=64,
    )
    assert targets.layers[BASE][30, 30] == INNER
    assert targets.layers[BASE][20, 30] == REGION


def test_a_polygon_hole_stays_out_of_the_fill() -> None:
    targets = rasterize(
        [
            polygon(
                "r",
                "region",
                [[5, 5], [55, 5], [55, 55], [5, 55]],
                holes=[[[20, 20], [40, 20], [40, 40], [20, 40]]],
            )
        ],
        CLASSES,
        64,
        64,
        size=64,
    )
    assert targets.layers[BASE][10, 10] == REGION
    assert targets.layers[BASE][30, 30] == 0
    assert targets.layers[BASE][50, 50] == REGION


def test_a_thin_wall_does_not_disappear_when_the_plan_is_downscaled() -> None:
    """A 3 px wall on a 1024 px plan at 512 input is 1.5 px, and at 256 is
    0.75 - which rounds to a target that is not there at all."""
    shapes = [line("w", [[100, 500], [900, 500]], thickness=3.0)]
    for size in (512, 256, 128):
        targets = rasterize(shapes, CLASSES, 1024, 1024, size=size)
        assert targets.layers[BASE].max() == OUTER, f"the line vanished at {size}"


def test_an_opening_goes_on_its_own_grid_and_leaves_the_wall_intact() -> None:
    targets = rasterize(
        [
            line("w", [[10, 30], [50, 30]], thickness=6.0),
            mark("d", "mark_a", [20, 30], [30, 30], line_id="w"),
        ],
        CLASSES,
        64,
        64,
        size=64,
    )
    assert targets.layers[OVERLAY][30, 25] == MARK_A
    # The wall is still a wall underneath it. On one grid it could only have
    # been drawn by erasing what it sits in.
    assert targets.layers[BASE][30, 25] == OUTER


def test_an_opening_is_as_deep_as_the_wall_it_links_to() -> None:
    thick = rasterize(
        [
            line("w", [[5, 30], [60, 30]], thickness=12.0),
            mark("d", "mark_b", [20, 30], [30, 30], line_id="w"),
        ],
        CLASSES,
        64,
        64,
        size=64,
    )
    thin = rasterize(
        [
            line("w", [[5, 30], [60, 30]], thickness=3.0),
            mark("d", "mark_b", [20, 30], [30, 30], line_id="w"),
        ],
        CLASSES,
        64,
        64,
        size=64,
    )
    assert int((thick.layers[OVERLAY] == MARK_B).sum()) > int(
        (thin.layers[OVERLAY] == MARK_B).sum()
    )


def test_a_keypoint_instance_with_nothing_visible_paints_nothing() -> None:
    """A shape the plan shows and the labeller could not place is not half a
    shape. Painting it somewhere would put a label where nobody saw one."""
    invisible = {
        "id": "d",
        "class": "mark_a",
        "geometry": {
            "kind": "keypoints",
            "points": [{"name": "start", "at": [20, 30], "visible": False}],
        },
    }
    targets = rasterize([invisible], CLASSES, 64, 64, size=64)
    assert int(targets.layers[OVERLAY].sum()) == 0


# -- What the loss is told to skip -------------------------------------------


def test_the_letterbox_bars_are_excluded_from_the_loss_not_taught_as_background() -> None:
    targets = rasterize([], CLASSES, 200, 100, size=64)
    assert bool(targets.ignore[0, 0]) is True
    assert bool(targets.ignore[32, 32]) is False


def test_furniture_is_excluded_rather_than_labelled_background() -> None:
    targets = rasterize(
        [polygon("f", "ignore", [[10, 10], [30, 10], [30, 30], [10, 30]])],
        CLASSES,
        64,
        64,
        size=64,
    )
    assert bool(targets.ignore[20, 20]) is True
    assert bool(targets.ignore[50, 50]) is False


def test_the_counts_say_an_empty_target_came_from_an_empty_drawing() -> None:
    targets = rasterize(
        [line("a", [[1, 1], [10, 1]]), line("b", [[1, 5], [10, 5]])],
        CLASSES,
        64,
        64,
        size=64,
    )
    assert targets.counts["line_outer"] == 2


# -- Pixels ------------------------------------------------------------------


def png(width: int, height: int, value: int = 200) -> bytes:
    buffer = io.BytesIO()
    Image.new("L", (width, height), value).save(buffer, format="PNG")
    return buffer.getvalue()


def test_an_image_is_letterboxed_onto_white_not_black() -> None:
    box = fit_letterbox(200, 100, 64)
    array = decode_image(png(200, 100, 128), box, channels=1)
    assert array.shape == (1, 64, 64)
    assert array[0, 0, 0] == pytest.approx(1.0)
    assert array[0, 32, 32] == pytest.approx(128 / 255, abs=0.02)


def test_a_cached_blob_survives_and_a_partial_write_never_becomes_one(tmp_path: Any) -> None:
    cache = BlobCache(tmp_path)
    assert cache.get("abc") is None
    cache.put("abc", b"bytes")
    assert cache.get("abc") == b"bytes"
    assert not list(tmp_path.glob("*.partial"))


# -- Scoring -----------------------------------------------------------------


def test_iou_is_computed_over_the_split_not_averaged_over_batches() -> None:
    score = SegmentationScore(["a", "b"])
    prediction = np.array([[0, 0], [1, 1]])
    target = np.array([[0, 1], [1, 1]])
    score.update(prediction, target)
    # `a`: one hit, one false positive, no misses.  `b`: two hits, one miss.
    assert score.get_iou()["a"] == pytest.approx(1 / 2)
    assert score.get_iou()["b"] == pytest.approx(2 / 3)
    assert score.get_pixel_accuracy() == pytest.approx(3 / 4)


def test_ignored_pixels_are_left_out_of_the_score_as_well_as_the_loss() -> None:
    score = SegmentationScore(["a", "b"])
    score.update(
        np.array([[0, 0]]),
        np.array([[0, 1]]),
        ignore=np.array([[False, True]]),
    )
    assert score.get_iou()["a"] == pytest.approx(1.0)
    assert np.isnan(score.get_iou()["b"])


def test_a_class_this_split_cannot_score_reads_as_absent_rather_than_zero() -> None:
    score = SegmentationScore(["a", "b"])
    score.update(np.array([[0]]), np.array([[0]]))
    assert np.isnan(score.get_iou()["b"])
    # And it is left out of the mean rather than dragging it to a half.
    assert score.get_mean_iou() == pytest.approx(1.0)
    assert "val_iou_b" not in score.as_metrics("val")


# -- Against the API ---------------------------------------------------------

IMAGE = png(64, 48)
IMAGE_ID = hashlib.sha256(IMAGE).hexdigest()
PROJECT = "floor-plans/dom-projekt"
REVISION = "ab" * 32


class Registry(BaseHTTPRequestHandler):
    seen: ClassVar[list[str]] = []

    def log_message(self, *args: Any) -> None:
        return

    def do_GET(self) -> None:
        type(self).seen.append(self.path)
        path = self.path.split("?")[0]
        if path.startswith("/api/v1/annotation-blobs/"):
            self.send_response(200)
            self.send_header("content-type", "image/png")
            self.send_header("content-length", str(len(IMAGE)))
            self.end_headers()
            self.wfile.write(IMAGE)
            return
        body = json.dumps(
            {
                "image_id": IMAGE_ID,
                "revision": {
                    "revision": REVISION,
                    "annotations": [line("w", [[5, 5], [58, 5]], thickness=4.0)],
                },
            }
        ).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


@pytest.fixture
def registry() -> Iterator[AnnotationRegistry]:
    Registry.seen = []
    server = HTTPServer(("127.0.0.1", 0), Registry)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield AnnotationRegistry(f"http://127.0.0.1:{server.server_port}")
    finally:
        server.shutdown()
        server.server_close()


def manifest(registry: AnnotationRegistry | None = None) -> Export:
    return Export.from_json(
        {
            "project": PROJECT,
            "export": "9f" * 32,
            "schema_version": "cc" * 32,
            "classes": [entry["name"] for entry in CLASSES],
            "samples": [
                {
                    "image_id": IMAGE_ID,
                    "uri": f"aiwatcher://blob/{IMAGE_ID}",
                    "width": 64,
                    "height": 48,
                    "group_id": "komancza-dws",
                    "split": "train",
                    "revision": REVISION,
                    "instances": 1,
                }
            ],
            "excluded": [],
            "counts": {},
            "rights_policy": "commercial",
        },
        registry=registry,
    )


def test_a_sample_comes_out_as_the_arrays_a_collate_can_stack(
    registry: AnnotationRegistry,
) -> None:
    dataset = manifest(registry).get_split("train").as_dataset(image_size=64, classes=CLASSES)
    assert len(dataset) == 1
    assert dataset.get_groups() == {"komancza-dws"}

    item = dataset[0]
    assert item["image"].shape == (1, 64, 64)
    assert item["image"].dtype == np.float32
    assert item["targets"][BASE].shape == (64, 64)
    assert item["targets"][BASE].dtype == np.int64
    assert item["group_id"] == "komancza-dws"
    assert int((item["targets"][BASE] == OUTER).sum()) > 0


def test_the_dataset_can_be_built_from_the_split_directly(
    registry: AnnotationRegistry,
) -> None:
    # `SplitView.as_dataset` is the short way round; this is what it calls, and the
    # two have to hold the same samples or the split rule has two homes.
    split = manifest().get_split("train")
    direct = ExportDataset(registry, split, image_size=64, classes=CLASSES)

    assert direct.get_groups() == split.get_groups()
    assert [sample.image_id for sample in direct.samples] == [IMAGE_ID]
    assert direct.export is split.export
    assert "train" in repr(direct)


def test_the_loader_is_torch_s_own_step_and_says_so_when_torch_is_absent(
    registry: AnnotationRegistry,
) -> None:
    """The last line of the PyTorch data tutorial, and the one place in this
    SDK that imports torch — inside the method, so a process that only
    rasterises never pays for it."""
    dataset = manifest().get_split("train").as_dataset(registry, image_size=64, classes=CLASSES)
    if importlib.util.find_spec("torch") is None:
        with pytest.raises(ImportError, match="torch"):
            dataset.as_torch_dataloader(batch_size=1)
        return
    loader = dataset.as_torch_dataloader(batch_size=1)
    assert len(loader.dataset) == 1


def test_the_dataset_reads_the_revision_the_export_pinned(
    registry: AnnotationRegistry,
) -> None:
    """Not the project's current head. An export is a claim about which
    drawings a run saw, and re-reading the head breaks it the first time
    somebody fixes a label while a run is training."""
    manifest().get_split("train").as_dataset(registry, image_size=64, classes=CLASSES)[0]
    asked = [path for path in Registry.seen if path.startswith("/api/v1/annotation-image?")]
    assert asked and f"revision={REVISION}" in asked[0]


def test_a_cached_corpus_is_downloaded_once_rather_than_once_per_epoch(
    registry: AnnotationRegistry, tmp_path: Any
) -> None:
    dataset = (
        manifest()
        .get_split("train")
        .as_dataset(registry, image_size=64, cache_dir=tmp_path, classes=CLASSES)
    )
    for _ in range(3):
        dataset[0]
    blobs = [path for path in Registry.seen if "annotation-blobs" in path]
    assert len(blobs) == 1


def test_a_revision_the_server_could_not_resolve_raises_instead_of_training_on_nothing() -> None:
    class NoRevision(Registry):
        def do_GET(self) -> None:
            body = json.dumps({"image_id": IMAGE_ID}).encode()
            self.send_response(200)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    server = HTTPServer(("127.0.0.1", 0), NoRevision)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        client = AnnotationRegistry(f"http://127.0.0.1:{server.server_port}")
        with pytest.raises(RegistryError, match="empty target"):
            client.get_revision_annotations(PROJECT, IMAGE_ID, revision=REVISION)
    finally:
        server.shutdown()
        server.server_close()


# -- What the schema decides -------------------------------------------------


def test_a_vocabulary_that_never_mentions_layers_gets_exactly_one() -> None:
    """The common case, and it should cost nothing to think about."""
    layers = layers_for([{"name": "thing", "geometry": "polygon"}])
    assert len(layers) == 1
    assert layers[0].classes == ("background", "thing")


def test_an_ignore_class_is_in_no_layer_because_it_is_not_a_label() -> None:
    layers = layers_for(
        [
            {"name": "thing", "geometry": "polygon"},
            {"name": "skip", "geometry": "polygon", "ignore": True},
        ]
    )
    assert [name for layer in layers for name in layer.classes] == ["background", "thing"]


def test_declaration_order_decides_which_class_wins_a_contested_pixel() -> None:
    """The one ordering decision, and it belongs to the schema.

    A vocabulary that declares regions before the lines separating them gets
    lines that survive; declared the other way round, the lines are covered.
    Both are legitimate and only the schema knows which is meant, so the
    rasteriser reads it rather than choosing.
    """
    shapes = [
        polygon("r", "region", [[10, 10], [50, 10], [50, 50], [10, 50]]),
        line("w", [[10, 30], [50, 30]], thickness=4.0),
    ]
    drawn_last = rasterize(shapes, CLASSES, 64, 64, size=64)
    assert drawn_last.layers[BASE][30, 30] == OUTER

    # The same shapes, the same drawing order, a schema that reverses them.
    reversed_schema = [entry for entry in CLASSES if entry["name"] != "region"] + [
        {"name": "region", "geometry": "polygon", "layer": 0}
    ]
    covered = rasterize(shapes, reversed_schema, 64, 64, size=64)
    covered_region = layers_for(reversed_schema)[BASE].classes.index("region")
    assert covered.layers[BASE][30, 30] == covered_region


def test_drawing_order_never_changes_the_grids() -> None:
    """Two labellers who drew the same shapes in a different sequence have to
    produce the same target, or the revision's content address stops meaning
    what it says."""
    shapes = [
        polygon("r", "region", [[10, 10], [50, 10], [50, 50], [10, 50]]),
        line("w", [[10, 30], [50, 30]], thickness=4.0),
    ]
    forwards = rasterize(shapes, CLASSES, 64, 64, size=64)
    backwards = rasterize(list(reversed(shapes)), CLASSES, 64, 64, size=64)
    assert np.array_equal(forwards.layers[BASE], backwards.layers[BASE])


def test_an_overlay_takes_its_width_from_what_it_links_to() -> None:
    """Generic form of "a window is as deep as its wall". Guessing a constant
    makes every overlay on a thick feature too thin and every one on a thin
    feature too fat."""
    thick = rasterize(
        [
            line("w", [[5, 30], [60, 30]], thickness=12.0),
            mark("m", "mark_a", [20, 30], [30, 30], line_id="w"),
        ],
        CLASSES,
        64,
        64,
        size=64,
    )
    thin = rasterize(
        [
            line("w", [[5, 30], [60, 30]], thickness=3.0),
            mark("m", "mark_a", [20, 30], [30, 30], line_id="w"),
        ],
        CLASSES,
        64,
        64,
        size=64,
    )
    assert int((thick.layers[OVERLAY] == MARK_A).sum()) > int(
        (thin.layers[OVERLAY] == MARK_A).sum()
    )


def test_a_schema_that_moved_since_the_export_is_refused_rather_than_permuted(
    registry: AnnotationRegistry,
) -> None:
    """The failure this catches produces no error of its own: rasterising
    against a reordered vocabulary permutes every label, every metric stays
    finite, and nothing says so."""
    with pytest.raises(RegistryError, match="permutes every label"):
        manifest().get_split("train").as_dataset(registry, image_size=64)


class OfflineImages:
    """Three methods and no network, which is the whole point of the port.

    Not a subclass of anything and it names `ImageSource` nowhere — a corpus
    already on a GPU box, or a cache in front of a slow link, is this shape.
    """

    def __init__(self) -> None:
        self.asked: list[str | None] = []

    def get_project(self, name: str) -> dict[str, Any]:
        return {"schema": {"version": "cc" * 32, "classes": CLASSES}}

    def get_revision_annotations(
        self, project: str, image_id: str, *, revision: str | None = None
    ) -> list[dict[str, Any]]:
        self.asked.append(revision)
        return [line("w", [[5, 5], [58, 5]], thickness=4.0)]

    def fetch_image(self, sample: Sample | str) -> bytes:
        return IMAGE


def test_a_dataset_reads_through_the_port_rather_than_through_the_client() -> None:
    """`ExportDataset` needs three answers and nothing else about an HTTP
    client, so anything that gives those three drives it — no server, no
    connection pool, and no subclassing."""
    offline = OfflineImages()
    source: ImageSource = offline

    dataset = manifest().get_split("train").as_dataset(source, image_size=64)
    item = dataset[0]

    assert item["image"].shape == (1, 64, 64)
    assert int((item["targets"][BASE] == OUTER).sum()) > 0
    # And it asked for the revision the export pinned, not the project's head.
    assert offline.asked == [REVISION]
