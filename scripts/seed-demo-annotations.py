#!/usr/bin/env python3
"""Seed an annotation project with plans, drawings, an export and a training run.

Everything here is synthetic and drawn in pure Python — no Pillow, no numpy,
nothing this repository does not already have. The plans are crude on purpose:
what the demo is for is the *shape* of the data, and a realistic-looking plan
would invite somebody to judge the model that has not been trained yet.

What it puts in front of you:

* three buildings, each as a plain plan and its mirror — six images and three
  families, which is what the split key exists for;
* one CC BY-NC image, so a commercial export has something to exclude by name;
* one image left as a draft, so the export has something to exclude for the
  other reason;
* a regions/edges/markers drawing on each accepted image;
* a built export, and a short training run recorded against its reference.

    just run                # in one terminal
    just seed-annotations   # in another
"""

from __future__ import annotations

import os
import struct
import sys
import time
import urllib.error
import urllib.request
import zlib
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "sdk" / "python"))

from aiwatcher_sdk.annotations import AnnotationRegistry, RegistryError  # noqa: E402
from aiwatcher_sdk.training import TrainingClient  # noqa: E402

BASE = os.environ.get("AIWATCHER_URL", "http://127.0.0.1:8080")
PROJECT = os.environ.get("AIWATCHER_ANNOTATION_PROJECT", "corpora/demo")
WIDTH, HEIGHT = 640, 480


# ── A very small PNG writer ──────────────────────────────────────────────────


class Canvas:
    """An RGB raster with the three primitives a plan needs."""

    def __init__(self, width: int, height: int) -> None:
        self.width = width
        self.height = height
        self.pixels = bytearray(b"\xff" * (width * height * 3))

    def dot(self, x: int, y: int, colour: tuple[int, int, int]) -> None:
        if 0 <= x < self.width and 0 <= y < self.height:
            offset = (y * self.width + x) * 3
            self.pixels[offset : offset + 3] = bytes(colour)

    def line(
        self,
        start: tuple[float, float],
        end: tuple[float, float],
        colour: tuple[int, int, int],
        thickness: int = 1,
    ) -> None:
        (x0, y0), (x1, y1) = start, end
        steps = int(max(abs(x1 - x0), abs(y1 - y0))) or 1
        half = thickness // 2
        for index in range(steps + 1):
            t = index / steps
            x = round(x0 + (x1 - x0) * t)
            y = round(y0 + (y1 - y0) * t)
            for dx in range(-half, half + 1):
                for dy in range(-half, half + 1):
                    self.dot(x + dx, y + dy, colour)

    def rect(
        self,
        top_left: tuple[float, float],
        bottom_right: tuple[float, float],
        colour: tuple[int, int, int],
        thickness: int = 1,
    ) -> None:
        (x0, y0), (x1, y1) = top_left, bottom_right
        self.line((x0, y0), (x1, y0), colour, thickness)
        self.line((x1, y0), (x1, y1), colour, thickness)
        self.line((x1, y1), (x0, y1), colour, thickness)
        self.line((x0, y1), (x0, y0), colour, thickness)

    def to_png(self) -> bytes:
        raw = bytearray()
        stride = self.width * 3
        for row in range(self.height):
            raw.append(0)  # filter: none
            raw += self.pixels[row * stride : (row + 1) * stride]

        def chunk(tag: bytes, body: bytes) -> bytes:
            return (
                struct.pack(">I", len(body))
                + tag
                + body
                + struct.pack(">I", zlib.crc32(tag + body) & 0xFFFFFFFF)
            )

        header = struct.pack(">2I5B", self.width, self.height, 8, 2, 0, 0, 0)
        return (
            b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", header)
            + chunk(b"IDAT", zlib.compress(bytes(raw), 6))
            + chunk(b"IEND", b"")
        )


BLACK = (24, 24, 27)
GREY = (150, 150, 155)


def plan(seed: int, mirrored: bool) -> tuple[bytes, list[dict[str, Any]]]:
    """One synthetic plan and the shapes that describe it.

    The image and the annotation are generated together, so the demo's labels
    are actually correct for its pixels — which is the one thing a seeded
    dataset can get wrong in a way nobody notices.
    """
    canvas = Canvas(WIDTH, HEIGHT)
    left, top, right, bottom = 60.0, 60.0, 580.0, 400.0
    divider = 60.0 + 200.0 + seed * 20.0

    def flip(x: float) -> float:
        return (WIDTH - x) if mirrored else x

    canvas.rect((flip(left), top), (flip(right), bottom), BLACK, 3)
    canvas.line((flip(divider), top), (flip(divider), bottom), BLACK, 3)
    # A dimension line under the building, which is what fixes the scale.
    canvas.line((flip(left), bottom + 30), (flip(right), bottom + 30), GREY, 1)
    # Furniture, so the ignore class has something to cover.
    canvas.rect((flip(left + 30), top + 40), (flip(left + 110), top + 100), GREY, 1)

    def edge(identifier: str, points: list[list[float]], role: str) -> dict[str, Any]:
        return {
            "id": identifier,
            "class": "edge",
            "geometry": {"kind": "polyline", "points": [[flip(x), y] for x, y in points]},
            "attributes": {"role": role, "thickness_px": 3.0},
            "origin": "human",
        }

    def region(identifier: str, x0: float, y0: float, x1: float, y1: float, label: str):
        return [
            {
                "id": identifier,
                "class": "region",
                "geometry": {
                    "kind": "polygon",
                    "exterior": [
                        [flip(x0), y0],
                        [flip(x1), y0],
                        [flip(x1), y1],
                        [flip(x0), y1],
                    ],
                },
                "attributes": {"label": label},
                "origin": "human",
            },
        ]

    annotations: list[dict[str, Any]] = [
        edge("edge_north", [[left, top], [right, top]], "outer"),
        edge("edge_east", [[right, top], [right, bottom]], "outer"),
        edge("edge_south", [[right, bottom], [left, bottom]], "outer"),
        edge("edge_west", [[left, bottom], [left, top]], "outer"),
        edge("edge_divider", [[divider, top], [divider, bottom]], "inner"),
        *region("region_1", left, top, divider, bottom, "left"),
        *region("region_2", divider, top, right, bottom, "right"),
        {
            # On layer 1, so it overlays `edge_divider` without erasing it —
            # the one thing layers exist for.
            "id": "marker_1",
            "class": "marker",
            "geometry": {
                "kind": "keypoints",
                "points": [
                    {"name": "start", "at": [flip(divider), 180.0], "visible": True},
                    {"name": "end", "at": [flip(divider), 260.0], "visible": True},
                ],
            },
            "links": {"edge": ["edge_divider"]},
            "origin": "human",
        },
        {
            "id": "marker_2",
            "class": "marker",
            "geometry": {
                "kind": "keypoints",
                "points": [
                    {"name": "start", "at": [flip(160.0), top], "visible": True},
                    {"name": "end", "at": [flip(280.0), top], "visible": True},
                ],
            },
            "links": {"edge": ["edge_north"]},
            "origin": "human",
        },
        {
            "id": "ignore_1",
            "class": "ignore",
            "geometry": {
                "kind": "polygon",
                "exterior": [
                    [flip(left + 30), top + 40],
                    [flip(left + 110), top + 40],
                    [flip(left + 110), top + 100],
                    [flip(left + 30), top + 100],
                ],
            },
            "origin": "human",
        },
    ]

    return canvas.to_png(), annotations


OWNED = {"kind": "owned", "grant": "demo"}
RESEARCH = {"kind": "research_only", "license": "CC BY-NC 4.0"}


def demo_classes() -> list[dict[str, Any]]:
    """The vocabulary this demo draws with.

    aiwatcher ships none — a project brings its own — so the demo brings one,
    and it is chosen to show what the schema can express rather than to model
    anything: a filled region, a stroked line carrying its own width, an
    overlay on its own layer so it does not erase what it crosses, and a class
    the loss must skip.
    """
    return [
        {
            "name": "region",
            "geometry": "polygon",
            "color": "#2563eb",
            "description": "An enclosed area.",
            "attributes": [{"name": "label", "kind": "text"}],
        },
        {
            "name": "edge",
            "geometry": "polyline",
            "color": "#1f2937",
            "description": "A boundary, drawn as a centreline with a width.",
            "attributes": [
                {
                    "name": "role",
                    "kind": "enum",
                    "values": ["outer", "inner", "unknown"],
                    "required": True,
                    "default": "unknown",
                },
                {"name": "thickness_px", "kind": "number", "required": True},
            ],
        },
        {
            "name": "marker",
            "geometry": "keypoints",
            "color": "#f97316",
            "description": "Something sitting on an edge. Its own layer, so it does not erase one.",
            "keypoints": ["start", "end"],
            "links": [{"name": "edge", "targets": ["edge"], "min": 0, "max": 1}],
            "layer": 1,
        },
        {
            "name": "ignore",
            "geometry": "polygon",
            "color": "#dc2626",
            "description": "Excluded from every target and from the loss.",
            "ignore": True,
        },
    ]


def main() -> int:
    try:
        urllib.request.urlopen(f"{BASE}/livez", timeout=2).read()  # noqa: S310
    except (urllib.error.URLError, OSError):
        print(f"✗ nothing is listening on {BASE} — start it with `just run`", file=sys.stderr)
        return 1

    registry = AnnotationRegistry(BASE)
    try:
        classes = demo_classes()
        registry.save_project(
            PROJECT,
            classes,
            description="Synthetic plans, for looking at the tool rather than the model",
            split_salt="demo",
        )
    except RegistryError as error:
        if error.code == "registry_disabled":
            print(
                "✗ this instance has no object store; set AIWATCHER_PROMPT_STORE=file",
                file=sys.stderr,
            )
            return 1
        raise

    families = ["subject-a", "subject-b", "subject-c"]
    accepted = 0
    for index, family in enumerate(families):
        for mirrored in (False, True):
            png, annotations = plan(index, mirrored)
            stored = registry.upload(png, content_type="image/png")
            image_id = stored["image_id"]
            registry.register_image(
                PROJECT,
                image_id=image_id,
                uri=stored["uri"],
                width=WIDTH,
                height=HEIGHT,
                group_id=family,
                source="synthetic",
                # One research-only image, so a commercial export has something
                # to exclude by name rather than an empty exclusion table.
                rights=RESEARCH if family == "subject-c" and mirrored else OWNED,
                metadata={"note": "generated by seed-demo-annotations.py"},
            )
            # One image left as a draft, for the other exclusion reason.
            draft = family == "subject-b" and mirrored
            registry.save_revision(PROJECT, image_id, annotations, accept=not draft)
            accepted += 0 if draft else 1
            print(f"  {'draft ' if draft else 'accept'} {family}{' (mirror)' if mirrored else ''}")

    export = registry.build_export(PROJECT, note="seeded")
    print()
    print(f"✓ {accepted} accepted drawings across {len(families)} families")
    print(f"✓ export {export.reference}")
    print(f"  {export.counts['images']} images, {export.counts['instances']} instances, "
          f"{export.counts['excluded']} excluded")
    for exclusion in export.excluded:
        print(f"  excluded {exclusion['group_id']}: {exclusion['reason']} — {exclusion['detail']}")

    # And a training run against it, so the Training area has a curve to draw.
    # It goes to `/api/v1/training-runs`, not to the event log: a training run
    # is a record that grows in place, not a trace. See ADR_0018.
    training = TrainingClient(BASE)
    run_id = f"demo-train-{int(time.time())}"
    best = 0.0
    with training.run(
        run_id,
        model="unet-s",
        dataset=export.reference,
        framework="pytorch",
        device="cuda:0",
        code="git:demo",
        params={"batch_size": 4, "lr": 3e-4, "input": "1024x1024"},
    ) as run:
        loss = 1.6
        for index in range(12):
            with run.epoch(index) as epoch:
                for _ in range(25):
                    loss *= 0.94
                    epoch.step(loss=loss)
                best = 0.42 + index * 0.028
                epoch.metrics(val_miou=best, val_loss=loss * 1.15)
            run.sample(lr=3e-4 * (0.9**index))
        run.checkpoint(
            "s3://models/demo/unet-s-e11.pt",
            epoch=11,
            metric="val_miou",
            value=best,
            best=True,
        )
        run.profile(
            {
                "sort_by": "self_cpu_time_total",
                "total_self_cpu_us": 1_000_000.0,
                "top_share": 0.41,
                "operators": [
                    {"name": "aten::conv2d", "count": 960, "self_cpu_us": 410_000.0},
                    {"name": "aten::batch_norm", "count": 960, "self_cpu_us": 180_000.0},
                ],
            },
            uri="s3://profiles/demo/e0.trace.json",
        )

    # And the model it produced. Held-out is deliberately below validation:
    # that gap is the number worth watching across a series of versions.
    registered = training.register_model(
        "demo.segmenter",
        run_id=run_id,
        checkpoint_uri="s3://models/demo/unet-s-e11.pt",
        validation={"miou": best},
        test={"miou": best - 0.06},
        description="Walls, rooms and openings from a catalogue plan",
    )
    training.promote("demo.segmenter", registered["version"]["version"])

    print(f"✓ training run {run_id} and one promoted model version")
    print("  look at both in the Training area")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
