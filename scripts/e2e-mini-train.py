#!/usr/bin/env python3
"""End-to-end: annotate → export → train a real model → register → promote.

The smallest thing that is not a mock. Twelve 64×48 plans are generated,
labelled, exported, fetched back through the API, rasterised into a coarse
edge/not-edge grid, and used to fit a **real** classifier by gradient descent —
seven weights, a few thousand samples, two seconds. It is a toy model and it
genuinely learns: the script fails if the loss does not fall or the held-out
IoU does not clear a floor, so a green run means the chain moved data rather
than that every call returned 200.

Pure Python on purpose — no numpy, no torch, no Pillow — so it runs anywhere
`just run` runs. Swapping in a real network is the `TrainingCallback` one-liner
in EXAMPLES.md; what this proves is the plumbing on either side of it.

What it checks, in order:

1. an image is stored under the digest the *server* computed;
2. drawings validate against the project's own schema;
3. the export splits by family, so no building straddles train and test;
4. COCO comes back carrying the geometry that was drawn;
5. a training run records a curve that actually descends;
6. a checkpoint and a profiler summary land as pointers, not payloads;
7. a model version takes its provenance from the run;
8. **a version with no held-out score is refused promotion** — the guardrail
   that matters, exercised rather than described.

    just run                     # in one terminal
    just e2e-train               # in another
"""

from __future__ import annotations

import hashlib
import math
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
from aiwatcher_sdk.training import TrainingClient, TrainingError  # noqa: E402

BASE = os.environ.get("AIWATCHER_URL", "http://127.0.0.1:8080")
PANEL = os.environ.get("AIWATCHER_PANEL_URL", "http://127.0.0.1:5173")
PROJECT = os.environ.get("AIWATCHER_E2E_PROJECT", "e2e/mini-shapes")
MODEL = os.environ.get("AIWATCHER_E2E_MODEL", "e2e.mini-edge-detector")
WIDTH, HEIGHT = 64, 48
#: The coarse grid the model predicts. One cell is 4×4 image pixels.
COLUMNS, ROWS = 16, 12
CELL = WIDTH // COLUMNS
EPOCHS = 300
LEARNING_RATE = 4.0
#: Below this the run has not learned the task and the script fails.
IOU_FLOOR = 0.5


# ── PNG, both ways ───────────────────────────────────────────────────────────


def encode_png(pixels: list[list[int]]) -> bytes:
    """Greyscale, 8-bit, filter 0. Enough for a plan drawn in three shades."""
    raw = bytearray()
    for row in pixels:
        raw.append(0)
        raw.extend(row)

    def chunk(tag: bytes, body: bytes) -> bytes:
        return (
            struct.pack(">I", len(body))
            + tag
            + body
            + struct.pack(">I", zlib.crc32(tag + body) & 0xFFFFFFFF)
        )

    header = struct.pack(">2I5B", WIDTH, HEIGHT, 8, 0, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(bytes(raw), 6))
        + chunk(b"IEND", b"")
    )


def decode_png(body: bytes) -> list[list[int]]:
    """Greyscale 8-bit, all five filter types.

    Written out rather than skipped, because the point of fetching the image
    back is that the trainer reads the *served* bytes and not the ones it
    happened to have in memory a moment ago.
    """
    if not body.startswith(b"\x89PNG\r\n\x1a\n"):
        raise ValueError("not a PNG")
    offset, width, height, data = 8, 0, 0, bytearray()
    while offset < len(body):
        (length,) = struct.unpack(">I", body[offset : offset + 4])
        tag = body[offset + 4 : offset + 8]
        payload = body[offset + 8 : offset + 8 + length]
        if tag == b"IHDR":
            width, height, depth, colour = struct.unpack(">2IBB", payload[:10])
            if depth != 8 or colour != 0:
                raise ValueError(f"expected 8-bit greyscale, got depth {depth} colour {colour}")
        elif tag == b"IDAT":
            data += payload
        elif tag == b"IEND":
            break
        offset += 12 + length

    raw = zlib.decompress(bytes(data))
    rows: list[list[int]] = []
    previous = [0] * width
    at = 0
    for _ in range(height):
        filter_type = raw[at]
        line = list(raw[at + 1 : at + 1 + width])
        at += 1 + width
        for index in range(width):
            left = line[index - 1] if index else 0
            up = previous[index]
            up_left = previous[index - 1] if index else 0
            if filter_type == 1:
                line[index] = (line[index] + left) & 0xFF
            elif filter_type == 2:
                line[index] = (line[index] + up) & 0xFF
            elif filter_type == 3:
                line[index] = (line[index] + (left + up) // 2) & 0xFF
            elif filter_type == 4:
                estimate = left + up - up_left
                nearest = min(
                    ((0, left), (1, up), (2, up_left)),
                    key=lambda pair: (abs(estimate - pair[1]), pair[0]),
                )[1]
                line[index] = (line[index] + nearest) & 0xFF
            elif filter_type != 0:
                raise ValueError(f"unknown PNG filter {filter_type}")
        rows.append(line)
        previous = line
    return rows


# ── Twelve plans, and labels that are true of them ───────────────────────────


def plan(seed: int) -> tuple[bytes, list[dict[str, Any]]]:
    """One tiny drawing: an outer rectangle, one inner edge, one piece of clutter.

    The image and the annotation are generated together, so the labels are
    *correct for these pixels* rather than approximately correct — the one
    thing a synthetic dataset can get wrong in a way no metric detects.
    """
    pixels = [[245] * WIDTH for _ in range(HEIGHT)]
    left, top = 6, 5
    right, bottom = WIDTH - 7, HEIGHT - 6
    divider = 20 + (seed % 5) * 5

    def draw(x0: int, y0: int, x1: int, y1: int) -> None:
        for y in range(min(y0, y1), max(y0, y1) + 1):
            for x in range(min(x0, x1), max(x0, x1) + 1):
                if 0 <= x < WIDTH and 0 <= y < HEIGHT:
                    pixels[y][x] = 20

    draw(left, top, right, top)
    draw(left, bottom, right, bottom)
    draw(left, top, left, bottom)
    draw(right, top, right, bottom)
    draw(divider, top, divider, bottom)
    # Mid-grey clutter, so "dark" alone is not the answer and the model has to
    # use its neighbourhood.
    box = 9 + (seed % 3) * 4
    for y in range(top + 4, top + 9):
        for x in range(box, box + 5):
            pixels[y][x] = 170

    def edge(identifier: str, points: list[list[float]], role: str) -> dict[str, Any]:
        return {
            "id": identifier,
            "class": "edge",
            "geometry": {"kind": "polyline", "points": points},
            "attributes": {"role": role, "thickness_px": 1.0},
            "origin": "human",
        }

    annotations: list[dict[str, Any]] = [
        edge("edge_n", [[left, top], [right, top]], "outer"),
        edge("edge_s", [[left, bottom], [right, bottom]], "outer"),
        edge("edge_w", [[left, top], [left, bottom]], "outer"),
        edge("edge_e", [[right, top], [right, bottom]], "outer"),
        edge("edge_div", [[divider, top], [divider, bottom]], "inner"),
        {
            "id": "ignore_1",
            "class": "ignore",
            "geometry": {
                "kind": "polygon",
                "exterior": [
                    [box, top + 4],
                    [box + 5, top + 4],
                    [box + 5, top + 9],
                    [box, top + 9],
                ],
            },
            "origin": "human",
        },
    ]
    return encode_png(pixels), annotations


# ── Rasterisation: vector labels → the grid the model predicts ───────────────


def target_grid(edges: list[list[list[float]]]) -> list[int]:
    """Which cells a edge centreline passes through.

    The step ADR_0017 exists to make possible: the raster target is *derived*
    from the vector label, so changing the grid is a re-derivation rather than
    a re-annotation.
    """
    grid = [0] * (COLUMNS * ROWS)
    for points in edges:
        for start, end in zip(points, points[1:], strict=False):
            x0, y0 = start
            x1, y1 = end
            steps = int(max(abs(x1 - x0), abs(y1 - y0))) or 1
            for index in range(steps + 1):
                t = index / steps
                column = int((x0 + (x1 - x0) * t) // CELL)
                row = int((y0 + (y1 - y0) * t) // CELL)
                if 0 <= column < COLUMNS and 0 <= row < ROWS:
                    grid[row * COLUMNS + column] = 1
    return grid


def features(pixels: list[list[int]]) -> list[list[float]]:
    """Eight numbers per cell, and the second one is the whole trick.

    Mean darkness alone does not separate a edge from furniture: a one-pixel
    black line crossing a 4×4 cell averages *lighter* than a solid mid-grey
    block filling it. The darkest pixel in the cell does separate them — 20
    against 170 — and it is exactly the kind of local extremum a first
    convolutional layer learns on its own. Handing it to a seven-weight linear
    model is the same information, arrived at by hand.
    """
    mean: list[list[float]] = []
    darkest: list[list[float]] = []
    for row in range(ROWS):
        mean_row: list[float] = []
        dark_row: list[float] = []
        for column in range(COLUMNS):
            window = [
                pixels[row * CELL + dy][column * CELL + dx]
                for dy in range(CELL)
                for dx in range(CELL)
            ]
            mean_row.append(1.0 - sum(window) / (len(window) * 255.0))
            dark_row.append(1.0 - min(window) / 255.0)
        mean.append(mean_row)
        darkest.append(dark_row)

    def at(grid: list[list[float]], row: int, column: int) -> float:
        if 0 <= row < ROWS and 0 <= column < COLUMNS:
            return grid[row][column]
        return 0.0

    return [
        [
            1.0,
            at(darkest, row, column),
            at(mean, row, column),
            at(darkest, row - 1, column),
            at(darkest, row + 1, column),
            at(darkest, row, column - 1),
            at(darkest, row, column + 1),
            at(darkest, row, column) ** 2,
        ]
        for row in range(ROWS)
        for column in range(COLUMNS)
    ]


# ── The model: logistic regression, fitted by hand ───────────────────────────


def predict(weights: list[float], row: list[float]) -> float:
    total = sum(weight * value for weight, value in zip(weights, row, strict=True))
    # Clamped, because `exp` on a diverged weight raises rather than saturating.
    return 1.0 / (1.0 + math.exp(-max(-30.0, min(30.0, total))))


def evaluate(weights: list[float], samples: list[tuple[list[float], int]]) -> dict[str, float]:
    loss = 0.0
    intersection = union = 0
    for row, label in samples:
        probability = predict(weights, row)
        loss -= label * math.log(max(probability, 1e-9)) + (1 - label) * math.log(
            max(1 - probability, 1e-9)
        )
        hit = probability >= 0.5
        if hit and label:
            intersection += 1
        if hit or label:
            union += 1
    return {
        "loss": loss / max(len(samples), 1),
        "iou": intersection / union if union else 0.0,
    }


def main() -> int:  # noqa: PLR0911 - one early return per checked step, on purpose
    try:
        urllib.request.urlopen(f"{BASE}/livez", timeout=2).read()  # noqa: S310
    except (urllib.error.URLError, OSError):
        print(f"✗ nothing is listening on {BASE} — start it with `just run`", file=sys.stderr)
        return 1

    annotations = AnnotationRegistry(BASE)
    training = TrainingClient(BASE)

    print("1. project and images")
    # Two families pinned to each side rather than left to the hash. With six
    # buildings the hash is lumpy — 70/15/15 over six items lands them all in
    # one bucket often enough — and a smoke test that sometimes has no test set
    # is a smoke test people learn to re-run. Pinning is also the feature that
    # exists for "this house has to be held out", so exercising it here is free.
    overrides = {
        "house-0": "validation",
        "house-1": "validation",
        "house-2": "test",
        "house-3": "test",
    }
    try:
        annotations.save_project(
            PROJECT,
            [
                {
                    "name": "edge",
                    "geometry": "polyline",
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
                {"name": "ignore", "geometry": "polygon", "ignore": True},
            ],
            description="End-to-end smoke corpus",
            split_salt="e2e",
            split_overrides=overrides,
        )
    except RegistryError as error:
        if error.code == "registry_disabled":
            print("✗ no object store; set AIWATCHER_PROMPT_STORE=file", file=sys.stderr)
            return 1
        raise

    families = [f"house-{index}" for index in range(6)]
    for index, family in enumerate(families):
        for variant in (0, 1):
            png, shapes = plan(index * 2 + variant)
            stored = annotations.upload(png, content_type="image/png")
            # The server hashed what it stored; the client never told it what
            # the digest should be.
            if stored["image_id"] != hashlib.sha256(png).hexdigest():
                print("✗ the server's digest is not the digest of the bytes", file=sys.stderr)
                return 1
            annotations.register_image(
                PROJECT,
                image_id=stored["image_id"],
                uri=stored["uri"],
                width=WIDTH,
                height=HEIGHT,
                group_id=family,
                source="e2e",
                rights={"kind": "owned", "grant": "generated here"},
            )
            annotations.save_revision(PROJECT, stored["image_id"], shapes, accept=True)
    print(f"   {len(families) * 2} images, {len(families)} families, all accepted")

    print("2. export")
    export = annotations.build_export(PROJECT, note="e2e")
    print(f"   {export.reference}")
    for split in ("train", "validation", "test"):
        print(
            f"   {split:<11} {len(export.split(split)):>2} images"
            f"   {len(export.families(split))} families"
        )
    overlap = export.families("train") & export.families("test")
    if overlap:
        print(f"✗ a building is in both train and test: {overlap}", file=sys.stderr)
        return 1
    # Both renderings of a pinned building went the same way, which is the
    # property the whole family-keyed split exists for.
    for family, side in overrides.items():
        placed = {sample.split for sample in export.samples if sample.group_id == family}
        if placed != {side}:
            print(f"✗ {family} landed in {placed or 'nothing'}, not {side}", file=sys.stderr)
            return 1
    print("   every rendering of a pinned building stayed on one side")

    print("3. rasterise")
    coco = annotations.coco(export)
    image_ids = {image["id"]: image["aiwatcher"]["image_id"] for image in coco["images"]}
    edges: dict[str, list[list[list[float]]]] = {}
    for record in coco["annotations"]:
        extra = record["aiwatcher"]
        if not extra["annotation_id"].startswith("edge"):
            continue
        edges.setdefault(image_ids[record["image_id"]], []).append(extra["geometry"]["points"])

    splits: dict[str, list[tuple[list[float], int]]] = {"train": [], "validation": [], "test": []}
    for sample in export.samples:
        pixels = decode_png(annotations.fetch_image(sample))
        rows = features(pixels)
        labels = target_grid(edges.get(sample.image_id, []))
        splits[sample.split].extend(zip(rows, labels, strict=True))
    print("   " + ", ".join(f"{name}: {len(rows)} cells" for name, rows in splits.items() if rows))
    if not splits["train"] or not splits["test"]:
        print("✗ the split left one side empty; nothing to measure", file=sys.stderr)
        return 1

    print("4. train")
    run_id = f"e2e-{int(time.time())}"
    weights = [0.0] * 8
    checkpoint = Path(os.environ.get("TMPDIR", "/tmp")) / f"{run_id}.weights.json"
    first_loss = last_loss = 0.0
    best = 0.0

    with training.run(
        run_id,
        model=MODEL,
        dataset=export.reference,
        framework="pure-python",
        device="cpu",
        code="scripts/e2e-mini-train.py",
        params={"epochs": EPOCHS, "lr": LEARNING_RATE, "grid": f"{COLUMNS}x{ROWS}"},
    ) as run:
        started = time.monotonic()
        for epoch in range(EPOCHS):
            with run.epoch(epoch) as tracker:
                for row, label in splits["train"]:
                    error = predict(weights, row) - label
                    for index, value in enumerate(row):
                        weights[index] -= LEARNING_RATE * error * value / len(splits["train"])
                    tracker.step(loss=abs(error))
                train = evaluate(weights, splits["train"])
                validation = evaluate(weights, splits["validation"] or splits["train"])
                tracker.metrics(
                    train_loss=train["loss"],
                    train_iou=train["iou"],
                    val_loss=validation["loss"],
                    val_iou=validation["iou"],
                )
            first_loss = first_loss or train["loss"]
            last_loss = train["loss"]
            best = max(best, validation["iou"])
            run.sample(lr=LEARNING_RATE)

        checkpoint.write_text(repr(weights))
        run.checkpoint(
            f"file://{checkpoint}", epoch=EPOCHS - 1, metric="val_iou", value=best, best=True
        )
        # A real summary of where the time went rather than a fabricated one:
        # this loop has one hot part and it is measured.
        elapsed = (time.monotonic() - started) * 1e6
        run.profile(
            {
                "sort_by": "self_cpu_time_total",
                "total_self_cpu_us": elapsed,
                "top_share": 1.0,
                "operators": [
                    {
                        "name": "gradient_step",
                        "count": EPOCHS * len(splits["train"]),
                        "self_cpu_us": elapsed,
                    }
                ],
            }
        )

    held_out = evaluate(weights, splits["test"])
    print(f"   loss {first_loss:.4f} → {last_loss:.4f}   held-out IoU {held_out['iou']:.3f}")

    print("5. register and promote")
    registered = training.register_model(
        MODEL,
        run_id=run_id,
        checkpoint_uri=f"file://{checkpoint}",
        validation={"iou": best},
        test={"iou": held_out["iou"]},
        description="A seven-weight edge detector. It exists to prove the chain.",
    )
    if registered.get("promotion_blocked"):
        print(f"✗ unexpectedly blocked: {registered['promotion_blocked']}", file=sys.stderr)
        return 1
    version = registered["version"]["version"]
    training.promote(MODEL, version)
    print(f"   {MODEL}@{version[:12]} promoted to production")

    print("6. the guardrail")
    unmeasured = training.register_model(
        MODEL,
        run_id=run_id,
        checkpoint_uri=f"file://{checkpoint}.unmeasured",
        validation={"iou": 0.99},
        description="Nothing measured this on data it had not seen.",
    )
    blocked = unmeasured.get("promotion_blocked")
    if not blocked:
        print("✗ a version with no held-out score was promotable", file=sys.stderr)
        return 1
    print(f"   recorded, and refused: {blocked}")
    try:
        training.promote(MODEL, unmeasured["version"]["version"])
    except TrainingError as error:
        print(f"   the route refuses it too: {error}")
    else:
        print("✗ the route promoted a version the registry called unpromotable", file=sys.stderr)
        return 1

    print()
    failures = []
    if last_loss >= first_loss:
        failures.append(f"the loss did not fall ({first_loss:.4f} → {last_loss:.4f})")
    if held_out["iou"] < IOU_FLOOR:
        failures.append(f"held-out IoU {held_out['iou']:.3f} is below the {IOU_FLOOR} floor")
    if failures:
        for failure in failures:
            print(f"✗ {failure}", file=sys.stderr)
        return 1

    print(f"✓ end to end: {len(export.samples)} images → a model that learned something")
    print(
        "  the IoU is high because the task is separable by construction — this "
        "measures the plumbing, not a model"
    )
    print(f"  run     {PANEL}/training/runs?run={run_id}")
    print(f"  model   {PANEL}/training/models?model={MODEL}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
