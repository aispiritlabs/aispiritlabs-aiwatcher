"""Vector annotations, as the arrays a training loop actually consumes.

The one piece that stood between :mod:`aiwatcher_sdk.annotations` and
:mod:`aiwatcher_sdk.training`: an export names shapes, a loss function needs
grids, and every project was writing the same rasteriser badly.

The direction is the whole point and it only goes one way. ADR_0017 says the
vector shape is the source and every raster is derived; this module is that
derivation, done once, in the open, and thrown away after each batch. Nothing
here writes a mask back to the registry, and nothing here reads one — a stored
mask beside the vector it came from is two sources of truth that will disagree,
with nothing able to say which is right.

    from aiwatcher_sdk.annotations import AnnotationRegistry

    data_registry = AnnotationRegistry("http://aiwatcher:8080")
    dataloader = data_registry.build_dataloader("corpora/plans")

    train = dataloader.get_split("train").as_dataset(image_size=512)
    loader = train.as_torch_dataloader(batch_size=4, shuffle=True)

That is PyTorch's own two steps — a ``Dataset``, then a ``DataLoader`` over
it — and the first of them is reached from the split rather than from here, so
a caller never has to name this module to use it.

What it asks for is not the client but
:class:`~aiwatcher_sdk.annotations.image_source.ImageSource` — a project's
schema, one revision's shapes, and an image's bytes. Three methods, so a cache,
a reader over a corpus somebody rsynced onto a GPU box, or a test double
substitutes for the registry without subclassing anything.

``ExportDataset`` is deliberately **not** a ``torch.utils.data.Dataset``
subclass. A map-style dataset in PyTorch is a duck: ``__len__`` and
``__getitem__`` are the whole protocol, ``default_collate`` turns the numpy
arrays it yields into tensors, and this file therefore stays importable in a
process that has never heard of torch — the same rule
:mod:`aiwatcher_sdk.integrations.torch` follows.
:meth:`ExportDataset.as_torch_dataloader` is the one method that imports it,
and only when it is called.

What it *does* need is numpy, and Pillow to decode a PNG. Both are imported
lazily, inside the functions that need them, so importing this module costs
nothing and a missing one produces a sentence naming the extra rather than a
traceback through somebody else's package::

    pip install 'aiwatcher-sdk[vision]'

## What comes out

**The project's own schema decides.** Nothing here knows a class name. The
label schema says what each class is — its geometry, whether it is an
``ignore`` class, and which ``layer`` it paints into — and this module reads
that. A vocabulary of walls and doors and one of components and defects
rasterise through the same code, because the code was never told which it was
looking at.

``layers``  one integer grid per declared layer, stacked. Index 0 in every
            layer is background; the rest are that layer's classes in schema
            order, and painting follows the same order — last declared wins a
            contested pixel.
``ignore``  every shape whose class is marked ``ignore``, plus the letterbox
            bars. Excluded from the loss rather than labelled background,
            which is the difference between "the model need not care" and "the
            model should predict nothing here".

Layers are the generic form of a problem that does not look generic: some
classes *overlay* others and must not erase them. An opening in a wall, a
defect on a component, a marking on a road — the thing underneath is still
there, and one grid could only represent the overlay by deleting it. A schema
that puts the overlay on layer 1 gets two grids and a model with two heads; a
schema that never sets ``layer`` gets one of each, and never thinks about it.

The transform is returned with them, because a prediction is worth nothing
until it is back in the original image's pixels: that is where the annotation
lives, where the scale is measured, and where the product's JSON is written.
"""

from __future__ import annotations

import hashlib
import io
import itertools
import math
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from typing import TYPE_CHECKING, Any, TypedDict, cast

from aiwatcher_sdk.annotations import Export, ImageSource, RegistryError, Sample, SplitView

if TYPE_CHECKING:  # pragma: no cover - import-time only for the type checker
    import numpy as np

    Array = np.ndarray[Any, Any]
else:
    Array = Any

__all__ = [
    "DEFAULT_STROKE_PX",
    "STROKE_ATTRIBUTE",
    "BlobCache",
    "ExportDataset",
    "Item",
    "LabelLayer",
    "Letterbox",
    "SegmentationScore",
    "Targets",
    "fit_letterbox",
    "layers_for",
    "rasterize",
]

#: The attribute a stroked class carries its width in.
#:
#: A convention rather than a rule the registry enforces, and it only has to
#: hold for classes drawn as a polyline or a keypoint chain — a filled polygon
#: has no width to read. A class that declares no such attribute is stroked at
#: :data:`DEFAULT_STROKE_PX`.
STROKE_ATTRIBUTE = "thickness_px"

#: How wide a stroked shape is drawn when nothing says otherwise.
#:
#: The fallback, and the last one tried. A shape's own attribute wins, then the
#: width of anything it *links* to — an overlay is as wide as what it overlays,
#: which is the whole reason the link exists — and only then this.
DEFAULT_STROKE_PX = 8.0

#: The thinnest a wall is allowed to rasterise to, in *model input* pixels.
#:
#: A 240 mm wall on a 1024 px plan downsampled to 512 is a hair over one pixel,
#: and a target that is one pixel wide is one dilation away from being no
#: target at all. Below this the wall is drawn at this width instead — the
#: label is then slightly fatter than the drawing, which is a bias the whole
#: dataset shares, rather than a wall that vanishes from a third of it.
MIN_STROKE_PX = 1.5


def _numpy() -> Any:
    """numpy, or a sentence saying how to get it."""
    try:
        import numpy
    except ImportError as error:  # pragma: no cover - exercised by not having it
        raise ImportError(
            "aiwatcher_sdk.integrations.vision needs numpy; "
            "install it with `pip install 'aiwatcher-sdk[vision]'`"
        ) from error
    return numpy


def _pillow() -> Any:
    """Pillow's ``Image``, or the same sentence."""
    try:
        from PIL import Image
    except ImportError as error:  # pragma: no cover - exercised by not having it
        raise ImportError(
            "aiwatcher_sdk.integrations.vision decodes images with Pillow; "
            "install it with `pip install 'aiwatcher-sdk[vision]'`"
        ) from error
    return Image


# ── The transform ────────────────────────────────────────────────────────────


@dataclass(frozen=True, slots=True)
class Letterbox:
    """How the original image was fitted into a square model input.

    Aspect ratio is preserved and the remainder is padded, rather than the
    image being stretched to a square. A stretched plan has walls that are not
    perpendicular any more, and perpendicularity is most of what the structural
    branch has to learn; the pasted-together alternative teaches it that a
    right angle depends on the page.

    Every field is in *original image* pixels except :attr:`size`, so
    :meth:`invert` is the only thing a consumer needs in order to put a
    prediction back where the annotation lives.
    """

    width: int
    height: int
    size: int
    scale: float
    pad_x: int
    pad_y: int

    def apply(self, point: Sequence[float]) -> tuple[float, float]:
        """An original-image point, in model-input pixels."""
        return (point[0] * self.scale + self.pad_x, point[1] * self.scale + self.pad_y)

    def invert(self, point: Sequence[float]) -> tuple[float, float]:
        """A model-input point, back in original-image pixels."""
        return ((point[0] - self.pad_x) / self.scale, (point[1] - self.pad_y) / self.scale)

    @property
    def content(self) -> tuple[int, int, int, int]:
        """The pixels that hold image rather than padding: ``x, y, w, h``."""
        return (
            self.pad_x,
            self.pad_y,
            max(1, round(self.width * self.scale)),
            max(1, round(self.height * self.scale)),
        )

    def as_dict(self) -> dict[str, float]:
        """Serialisable, for the provenance block of an output document.

        Worth recording: without it a prediction is a set of coordinates in a
        space that existed only inside one training script.
        """
        return {
            "width": self.width,
            "height": self.height,
            "size": self.size,
            "scale": self.scale,
            "pad_x": self.pad_x,
            "pad_y": self.pad_y,
        }


def fit_letterbox(width: int, height: int, size: int) -> Letterbox:
    """Centre an image of this shape in a ``size x size`` input."""
    if width <= 0 or height <= 0:
        raise ValueError(f"an image cannot be {width}x{height}")
    scale = min(size / width, size / height)
    return Letterbox(
        width=width,
        height=height,
        size=size,
        scale=scale,
        pad_x=(size - round(width * scale)) // 2,
        pad_y=(size - round(height * scale)) // 2,
    )


# ── Rasterisation ────────────────────────────────────────────────────────────


@dataclass(frozen=True, slots=True)
class LabelLayer:
    """One grid, and the classes that paint into it.

    ``classes[0]`` is always ``"background"``: a pixel nothing claimed is not
    a missing label, it is the absence of every class in this layer, and giving
    that index 0 in every layer means a loss function needs no special case.
    """

    index: int
    classes: tuple[str, ...]

    def label(self, name: str) -> int | None:
        """The pixel value for a class name, or `None` if it is elsewhere."""
        try:
            return self.classes.index(name)
        except ValueError:
            return None


def layers_for(classes: Sequence[Mapping[str, Any]]) -> list[LabelLayer]:
    """The layers a schema declares, in order, each with its own vocabulary.

    ``ignore`` classes appear in no layer. They are not a label competing with
    the others — they are pixels the loss must skip, whichever layer they fall
    across — so they go to the ignore mask and nowhere else.

    A schema that never sets ``layer`` produces exactly one layer, which is
    what almost every vocabulary wants and what nobody should have to ask for.
    """
    grouped: dict[int, list[str]] = {}
    for entry in classes:
        if entry.get("ignore"):
            continue
        grouped.setdefault(int(entry.get("layer", 0) or 0), []).append(str(entry["name"]))
    if not grouped:
        return []
    return [
        LabelLayer(index=index, classes=("background", *grouped[index]))
        for index in sorted(grouped)
    ]


@dataclass(frozen=True, slots=True)
class Targets:
    """One sample's grids, all the same shape and all derived."""

    #: Layer index to its integer grid. Keys match :func:`layers_for`.
    layers: dict[int, Array]
    ignore: Array
    letterbox: Letterbox
    #: Class name to how many instances of it were drawn into these grids. The
    #: number that says an empty target is an empty *drawing* rather than a
    #: rasteriser that dropped everything.
    counts: dict[str, int] = field(default_factory=dict)

    def stack(self) -> Array:
        """Every layer as one ``(layers, size, size)`` array.

        The shape a collate stacks and a multi-head model indexes. Layers are
        in ascending index order, which is the order :func:`layers_for` returns
        and the order a model's heads should be read in.
        """
        numpy = _numpy()
        if not self.layers:
            return cast(Array, numpy.zeros((0, *self.ignore.shape), dtype=numpy.int64))
        stacked = numpy.stack([self.layers[index] for index in sorted(self.layers)])
        return cast(Array, stacked.astype(numpy.int64))


def rasterize(
    annotations: Iterable[Mapping[str, Any]],
    classes: Sequence[Mapping[str, Any]],
    width: int,
    height: int,
    *,
    size: int | None = None,
    letterbox: Letterbox | None = None,
    default_stroke_px: float = DEFAULT_STROKE_PX,
) -> Targets:
    """Turn one revision's shapes into the grids a loss function reads.

    ``classes`` is the project's label schema — the same list
    ``project["schema"]["classes"]`` returns. It decides everything: which
    grids exist, what index a class has in one, what is stroked and what is
    filled, and what is excluded from the loss. Nothing in this function knows
    a class name.

    ``width`` and ``height`` are the *original* image's, because that is the
    space annotation coordinates live in — see
    :class:`aiwatcher_sdk.annotations.Sample`. Pass ``size`` for a square model
    input, or a :class:`Letterbox` to reuse one already computed.

    Painting order within a layer is **schema order**: a class declared later
    wins a pixel it shares with one declared earlier. That is the one ordering
    decision here, and it is the schema's to make — a vocabulary where regions
    are declared before the boundaries that separate them gets boundaries that
    survive, and one that declares them the other way round gets boundaries
    with a hole at every junction. Both are legitimate; only the schema knows
    which is meant.
    """
    numpy = _numpy()
    box = letterbox or fit_letterbox(width, height, size or max(width, height))
    layers = layers_for(classes)

    grids = {layer.index: numpy.zeros((box.size, box.size), dtype=numpy.uint8) for layer in layers}
    ignore = numpy.ones((box.size, box.size), dtype=bool)

    # Everything outside the content rectangle is padding. It is not
    # background — there is nothing there — so it is excluded from the loss
    # rather than taught as a class.
    content_x, content_y, content_w, content_h = box.content
    ignore[content_y : content_y + content_h, content_x : content_x + content_w] = False

    by_name = {str(entry["name"]): entry for entry in classes}
    placement: dict[str, tuple[int, int]] = {}
    for layer in layers:
        for value, name in enumerate(layer.classes):
            if value:
                placement[name] = (layer.index, value)

    shapes = list(annotations)
    strokes = _stroke_widths(shapes, by_name)
    counts: dict[str, int] = {}

    def scaled(points: Sequence[Sequence[float]]) -> list[tuple[float, float]]:
        return [box.apply(point) for point in points]

    # Schema order, not drawing order: two labellers who drew the same shapes
    # in a different sequence must produce the same grids, or the revision's
    # content address stops meaning what it says.
    ordered = sorted(
        shapes,
        key=lambda shape: _declaration_index(str(shape.get("class") or ""), classes),
    )

    for shape in ordered:
        name = str(shape.get("class") or "")
        counts[name] = counts.get(name, 0) + 1
        definition = by_name.get(name)
        if definition is None:
            continue

        if definition.get("ignore"):
            target, value = ignore, True
        else:
            found = placement.get(name)
            if found is None:
                continue
            target, value = grids[found[0]], found[1]

        _paint(
            target,
            value,
            shape,
            scaled=scaled,
            stroke=max(
                MIN_STROKE_PX,
                strokes.get(str(shape.get("id") or ""), default_stroke_px) * box.scale,
            ),
        )

    return Targets(layers=grids, ignore=ignore, letterbox=box, counts=counts)


def _declaration_index(name: str, classes: Sequence[Mapping[str, Any]]) -> int:
    for index, entry in enumerate(classes):
        if str(entry.get("name")) == name:
            return index
    return len(classes)


def _paint(
    target: Array,
    value: Any,
    shape: Mapping[str, Any],
    *,
    scaled: Any,
    stroke: float,
) -> None:
    """One shape, by the geometry it was drawn with."""
    geometry = shape.get("geometry") or {}
    kind = str(geometry.get("kind") or "")

    if kind == "polygon":
        _fill_polygon(
            target,
            scaled(geometry.get("exterior", [])),
            [scaled(hole) for hole in geometry.get("holes", [])],
            value,
        )
    elif kind == "polyline":
        _stroke_polyline(target, scaled(geometry.get("points", [])), stroke, value)
    elif kind == "bbox":
        low, high = geometry.get("min"), geometry.get("max")
        if low and high:
            corners = [[low[0], low[1]], [high[0], low[1]], [high[0], high[1]], [low[0], high[1]]]
            _fill_polygon(target, scaled(corners), [], value)
    elif kind == "keypoints":
        # The declared positions, in order, as a stroked chain. Two of them is
        # a segment — an opening in a wall, a crack across a component — and
        # more is a path. A single visible point is a dot of one stroke width.
        points = [
            point["at"]
            for point in geometry.get("points", [])
            if point.get("visible", True) and point.get("at")
        ]
        if len(points) >= 2:
            _stroke_polyline(target, scaled(points), stroke, value)
        elif len(points) == 1:
            _stroke_polyline(target, scaled([points[0], points[0]]), stroke, value)
    elif kind == "point":
        at = geometry.get("at")
        if at:
            _stroke_polyline(target, scaled([at, at]), stroke, value)


def _stroke_widths(
    shapes: Sequence[Mapping[str, Any]], by_name: Mapping[str, Mapping[str, Any]]
) -> dict[str, float]:
    """Every shape's stroke width, by instance id.

    Resolved in three steps, and the middle one is the interesting one: a shape
    that *links* to another takes the other's width. An overlay is exactly as
    wide as the thing it overlays — a window is as deep as its wall, a defect
    as wide as its weld — and guessing a constant makes every overlay on a
    thick feature too thin and every one on a thin feature too fat.
    """
    own: dict[str, float] = {}
    for shape in shapes:
        identifier = str(shape.get("id") or "")
        attributes = shape.get("attributes") or {}
        if STROKE_ATTRIBUTE in attributes:
            own[identifier] = _number(attributes[STROKE_ATTRIBUTE], DEFAULT_STROKE_PX)

    resolved = dict(own)
    for shape in shapes:
        identifier = str(shape.get("id") or "")
        if identifier in resolved:
            continue
        for targets in (shape.get("links") or {}).values():
            linked = next((own[str(t)] for t in targets if str(t) in own), None)
            if linked is not None:
                resolved[identifier] = linked
                break
    _ = by_name
    return resolved


def _number(value: Any, fallback: float) -> float:
    try:
        parsed = float(value)
    except (TypeError, ValueError):
        return fallback
    return parsed if math.isfinite(parsed) and parsed > 0 else fallback


def _fill_polygon(
    grid: Array,
    exterior: Sequence[tuple[float, float]],
    holes: Sequence[Sequence[tuple[float, float]]],
    value: Any,
) -> None:
    """Even-odd scanline fill, holes included by the rule rather than by a pass.

    Every ring contributes its crossings to the same sorted list, so a hole
    flips parity twice and the interior of a chimney ends up outside the room
    with no special case. A polygon with a hole is one shape in this schema
    precisely so that a consumer never has to infer the difference of two.
    """
    numpy = _numpy()
    rings = [ring for ring in [list(exterior), *[list(hole) for hole in holes]] if len(ring) >= 3]
    if not rings:
        return

    height, width = grid.shape
    top = max(0, (math.floor(min(point[1] for ring in rings for point in ring))))
    bottom = min(height - 1, (math.ceil(max(point[1] for ring in rings for point in ring))))

    for y in range(top, bottom + 1):
        centre = y + 0.5
        crossings: list[float] = []
        for ring in rings:
            for index, start in enumerate(ring):
                end = ring[(index + 1) % len(ring)]
                y0, y1 = start[1], end[1]
                # A half-open test on the y range: a vertex shared by two edges
                # is counted once, so a scanline through a corner does not
                # flood the rest of the row.
                if (y0 <= centre) == (y1 <= centre):
                    continue
                crossings.append(start[0] + (centre - y0) / (y1 - y0) * (end[0] - start[0]))
        if len(crossings) < 2:
            continue
        crossings.sort()
        for left, right in zip(crossings[0::2], crossings[1::2], strict=False):
            x0 = max(0, (math.ceil(left - 0.5)))
            x1 = min(width - 1, (math.floor(right - 0.5)))
            if x1 >= x0:
                grid[y, x0 : x1 + 1] = value

    _ = numpy


def _stroke_polyline(
    grid: Array,
    points: Sequence[tuple[float, float]],
    thickness: float,
    value: Any,
) -> None:
    """A polyline drawn with round caps and joins, one segment at a time.

    Round rather than mitred, and that is not a shortcut: a wall centreline
    meets three others at a T junction, and mitring each segment independently
    leaves a notch at every junction — which is precisely where a corner
    heatmap or a skeletonisation later has to find a node.
    """
    numpy = _numpy()
    if len(points) < 2 or thickness <= 0:
        return
    radius = thickness / 2.0
    height, width = grid.shape

    for start, end in itertools.pairwise(points):
        left = max(0, (math.floor(min(start[0], end[0]) - radius - 1)))
        right = min(width - 1, (math.ceil(max(start[0], end[0]) + radius + 1)))
        top = max(0, (math.floor(min(start[1], end[1]) - radius - 1)))
        bottom = min(height - 1, (math.ceil(max(start[1], end[1]) + radius + 1)))
        if right < left or bottom < top:
            continue

        ys = numpy.arange(top, bottom + 1, dtype=numpy.float64)[:, None] + 0.5
        xs = numpy.arange(left, right + 1, dtype=numpy.float64)[None, :] + 0.5
        dx, dy = end[0] - start[0], end[1] - start[1]
        length_squared = dx * dx + dy * dy
        if length_squared <= 0:
            t = numpy.zeros((ys.shape[0], xs.shape[1]))
        else:
            t = ((xs - start[0]) * dx + (ys - start[1]) * dy) / length_squared
            t = numpy.clip(t, 0.0, 1.0)
        distance = numpy.hypot(xs - (start[0] + t * dx), ys - (start[1] + t * dy))
        grid[top : bottom + 1, left : right + 1][distance <= radius] = value


# ── Pixels ───────────────────────────────────────────────────────────────────


class BlobCache:
    """Image bytes on disk, keyed by the digest that *is* the image id.

    A content address is the one cache key that never goes stale, which is why
    this is eleven lines and has no invalidation. Without it every epoch
    re-downloads the whole corpus: three hundred plans at 400 kB is 120 MB per
    epoch, and two hundred epochs of that is a training run bounded by an
    observability server's bandwidth.
    """

    def __init__(self, directory: str | Path) -> None:
        self.directory = Path(directory)
        self.directory.mkdir(parents=True, exist_ok=True)

    def path(self, image_id: str) -> Path:
        safe = hashlib.sha256(image_id.encode()).hexdigest() if "/" in image_id else image_id
        return self.directory / f"{safe}.bin"

    def get(self, image_id: str) -> bytes | None:
        path = self.path(image_id)
        return path.read_bytes() if path.exists() else None

    def put(self, image_id: str, body: bytes) -> None:
        # Written beside and renamed, so a killed trainer cannot leave a
        # half-written file that the next run reads as a valid image.
        target = self.path(image_id)
        staging = target.with_suffix(".partial")
        staging.write_bytes(body)
        staging.replace(target)


def decode_image(body: bytes, letterbox: Letterbox, *, channels: int = 1) -> Array:
    """Bytes to a ``channels x size x size`` float array in ``[0, 1]``.

    Greyscale by default. A catalogue floor plan is a line drawing whose colour
    carries no information the structural branch can use, and one channel is a
    third of the memory and a third of the first convolution. Pass
    ``channels=3`` for an encoder pretrained on photographs, which expects
    three and will not accept one.
    """
    numpy = _numpy()
    image_module = _pillow()
    with image_module.open(io.BytesIO(body)) as opened:
        image = opened.convert("L" if channels == 1 else "RGB")
        scaled_width = max(1, round(letterbox.width * letterbox.scale))
        scaled_height = max(1, round(letterbox.height * letterbox.scale))
        image = image.resize((scaled_width, scaled_height), image_module.BILINEAR)
        content = numpy.asarray(image, dtype=numpy.float32) / 255.0

    if channels == 1:
        content = content[:, :, None]
    # White rather than black padding. A plan's background is white, so black
    # bars would be the highest-contrast edge in the input — a border the first
    # convolution learns before it learns a wall.
    canvas = numpy.ones((letterbox.size, letterbox.size, channels), dtype=numpy.float32)
    canvas[
        letterbox.pad_y : letterbox.pad_y + scaled_height,
        letterbox.pad_x : letterbox.pad_x + scaled_width,
    ] = content
    return cast(Array, numpy.ascontiguousarray(canvas.transpose(2, 0, 1)))


# ── The dataset ──────────────────────────────────────────────────────────────


class Item(TypedDict):
    """One sample, as ``default_collate`` will stack it.

    Written down rather than left as ``dict[str, Any]`` because this is the
    contract between the rasteriser and somebody's loss function, and the two
    keys that are *not* arrays are the ones worth being explicit about:
    ``group_id`` is what makes a per-family metric possible, and a collate that
    silently dropped it would leave a per-image mean that counts one subject
    four times.
    """

    #: ``float32 (channels, size, size)`` in ``[0, 1]``.
    image: Array
    #: ``int64 (layers, size, size)`` — one grid per declared layer.
    targets: Array
    #: ``bool (size, size)`` — ``True`` is out of the loss.
    ignore: Array
    #: The content address of the pixels.
    image_id: str
    #: The family, not the drawing.
    group_id: str


class ExportDataset:
    """One :class:`~aiwatcher_sdk.annotations.SplitView`, as a map-style dataset.

    Built from a split rather than from a project, and the difference is the
    point twice over. A project is mutable — images are added to it while a run
    trains, and a dataset reading one would have a length that changed under a
    shuffled sampler — while the export is frozen and its source is what the
    run records. And the split is where the group rule already lives, so there
    is no second place that decides which subjects a model may see::

        test = dataloader.get_split("test")
        data = test.as_dataset(registry, image_size=512)
        assert data.get_groups() == test.get_groups()

    Each item is an :class:`Item` of numpy arrays, which is exactly what
    ``torch.utils.data.default_collate`` knows how to stack.

    :attr:`layers` says what the grids mean — layer order matches the first
    axis of ``targets``, and each layer's ``classes`` are its pixel values. A
    model reads one head per layer and gets its class counts from here rather
    than from a constant somebody has to keep in step.
    """

    def __init__(
        self,
        registry: ImageSource,
        split: SplitView,
        *,
        image_size: int = 512,
        channels: int = 1,
        cache_dir: str | Path | None = None,
        flip: bool = False,
        classes: Sequence[Mapping[str, Any]] | None = None,
    ) -> None:
        self.registry = registry
        self.split = split
        self.export = split.export
        self.image_size = image_size
        self.channels = channels
        self.flip = flip
        self.samples: tuple[Sample, ...] = tuple(split)
        self.cache = BlobCache(cache_dir) if cache_dir is not None else None
        self._shapes: dict[str, list[dict[str, Any]]] = {}
        self.classes: list[dict[str, Any]] = (
            [dict(entry) for entry in classes]
            if classes is not None
            else _schema_for(registry, self.export)
        )
        self.layers: list[LabelLayer] = layers_for(self.classes)
        if not self.layers:
            raise RegistryError(
                f"the schema behind {self.export.source} declares no paintable class, so every "
                "target would be empty; a vocabulary of nothing but `ignore` classes is not a "
                "training target"
            )

    def __len__(self) -> int:
        return len(self.samples)

    def __repr__(self) -> str:
        return (
            f"ExportDataset({self.export.source!r}, {self.split.name or 'all'}, "
            f"{len(self.samples)} images, {len(self.layers)} layers)"
        )

    def get_groups(self) -> frozenset[str]:
        """The subjects on this side. The number a score is really over.

        The same answer as ``dataloader.get_split("test").get_groups()``, and
        the same method name on both, because they are the same question asked
        of the same side.
        """
        return self.split.get_groups()

    def as_torch_dataloader(self, **options: Any) -> Any:
        """A ``torch.utils.data.DataLoader`` over this dataset.

        Named for the framework because the word is taken: the *loader* a
        training script holds is the export with its registry, and this is the
        batching iterator torch builds over one split of it.

        The last line of the PyTorch data tutorial, and the only place in this
        SDK that imports torch. It is imported *here*, inside the one method
        that cannot work without it, so a process that only rasterises never
        pays for it and the rule the rest of this file follows — read the other
        library structurally, never import it — is broken in exactly one
        visible place rather than at the top of the module::

            loader = dataloader.get_split("train").as_dataset().as_torch_dataloader(
                batch_size=4, shuffle=True, num_workers=4
            )

        Every keyword goes straight to ``DataLoader``. Nothing is defaulted
        here: a batch size that fits one corpus does not fit the next, and a
        default this layer picked would be a number nobody chose.
        """
        try:
            from torch.utils.data import DataLoader
        except ImportError as error:  # pragma: no cover - exercised by not having torch
            raise ImportError(
                "ExportDataset.as_torch_dataloader needs torch; install it for your platform, "
                "or build the "
                "loader yourself — this dataset is map-style, so any framework that reads "
                "__len__ and __getitem__ takes it as it is"
            ) from error
        return DataLoader(self, **options)

    def __getitem__(self, index: int) -> Item:
        numpy = _numpy()
        sample = self.samples[index]
        box = fit_letterbox(sample.width, sample.height, self.image_size)
        targets = rasterize(
            self._annotations(sample),
            self.classes,
            sample.width,
            sample.height,
            letterbox=box,
        )
        image = decode_image(self._bytes(sample), box, channels=self.channels)

        stacked = targets.stack()
        ignore = targets.ignore

        if self.flip and bool(numpy.random.random() < 0.5):
            # Safe for a mask and only for a mask. A direction recorded as a
            # keypoint pair is a *vector* and a flip reverses it; nothing in
            # these grids is one, which is exactly why the augmentation lives
            # here and not in a pipeline that also carries keypoints.
            image = numpy.ascontiguousarray(image[:, :, ::-1])
            stacked = numpy.ascontiguousarray(stacked[:, :, ::-1])
            ignore = numpy.ascontiguousarray(ignore[:, ::-1])

        return {
            "image": image,
            "targets": stacked,
            "ignore": ignore,
            "image_id": sample.image_id,
            "group_id": sample.group_id,
        }

    def get_letterbox(self, sample: Sample) -> Letterbox:
        """The transform this dataset would apply to one sample.

        Needed at inference: a prediction is in model-input pixels and the
        answer has to be in the plan's.
        """
        return fit_letterbox(sample.width, sample.height, self.image_size)

    def _annotations(self, sample: Sample) -> list[dict[str, Any]]:
        """The shapes of the revision the export pinned. Cached per process.

        The *pinned* revision, not the project's current accepted one. An
        export is a claim about which drawings a run saw, and re-reading the
        head would quietly break it the first time somebody fixes a label
        while a run is training.
        """
        cached = self._shapes.get(sample.image_id)
        if cached is None:
            cached = self.registry.get_revision_annotations(
                self.export.project, sample.image_id, revision=sample.revision
            )
            self._shapes[sample.image_id] = cached
        return cached

    def _bytes(self, sample: Sample) -> bytes:
        if self.cache is None:
            return self.registry.fetch_image(sample)
        stored = self.cache.get(sample.image_id)
        if stored is not None:
            return stored
        # `fetch_image` verifies the digest, so what is cached has already been
        # checked against the id the labels belong to.
        body = self.registry.fetch_image(sample)
        self.cache.put(sample.image_id, body)
        return body


# ── Scoring ──────────────────────────────────────────────────────────────────


class SegmentationScore:
    """A confusion matrix, and the three numbers worth reading off it.

    Accumulated over a whole split rather than averaged over batches: a
    per-batch mIoU averaged at the end is not the mIoU of the split, and the
    gap grows with how rare the class is — which is to say, it is largest for
    exactly the classes anybody cares about.

    ``ignore`` is honoured here as well as in the loss. Scoring furniture as
    background inflates every number by the fraction of the plan that is
    furniture, and does it identically for every model, so it hides the change
    it is asked to measure.
    """

    def __init__(self, classes: Sequence[str]) -> None:
        numpy = _numpy()
        self.classes = list(classes)
        self.matrix = numpy.zeros((len(self.classes), len(self.classes)), dtype=numpy.int64)

    def update(self, prediction: Array, target: Array, ignore: Array | None = None) -> None:
        numpy = _numpy()
        predicted = numpy.asarray(prediction).reshape(-1)
        actual = numpy.asarray(target).reshape(-1)
        if ignore is not None:
            keep = ~numpy.asarray(ignore).reshape(-1)
            predicted, actual = predicted[keep], actual[keep]
        count = len(self.classes)
        valid = (actual >= 0) & (actual < count) & (predicted >= 0) & (predicted < count)
        flat = numpy.bincount(actual[valid] * count + predicted[valid], minlength=count * count)
        self.matrix += flat.reshape(count, count)

    def get_iou(self) -> dict[str, float]:
        """Per class. ``nan`` for a class with no ground truth and no
        prediction — which is a class this split cannot score, and saying so is
        better than reporting the zero that would drag a mean down."""
        numpy = _numpy()
        intersection = numpy.diag(self.matrix).astype(numpy.float64)
        union = self.matrix.sum(axis=0) + self.matrix.sum(axis=1) - intersection
        with numpy.errstate(invalid="ignore", divide="ignore"):
            values = numpy.where(union > 0, intersection / union, numpy.nan)
        return dict(zip(self.classes, (float(value) for value in values), strict=True))

    def get_mean_iou(self) -> float:
        """Over the classes that appeared. Never over the absent ones."""
        numpy = _numpy()
        values = [value for value in self.get_iou().values() if not math.isnan(value)]
        return float(numpy.mean(values)) if values else 0.0

    def get_pixel_accuracy(self) -> float:
        numpy = _numpy()
        total = float(self.matrix.sum())
        return float(numpy.diag(self.matrix).sum()) / total if total else 0.0

    def as_metrics(self, prefix: str) -> dict[str, float]:
        """Flat ``prefix_class`` keys, for
        :meth:`aiwatcher_sdk.training.EpochContext.metrics`.

        Per-class numbers and not only the mean, because the mean is the one
        that hides the finding: a run whose mIoU rose while `wall_exterior`
        fell has got better at rooms and worse at the building outline.
        """
        metrics = {
            f"{prefix}_miou": self.get_mean_iou(),
            f"{prefix}_accuracy": self.get_pixel_accuracy(),
        }
        for name, value in self.get_iou().items():
            if not math.isnan(value):
                metrics[f"{prefix}_iou_{name}"] = value
        return metrics


def _schema_for(registry: ImageSource, export: Export) -> list[dict[str, Any]]:
    """The label schema the export was built against.

    Read from the project and then *checked* against the export's pinned
    ``schema_version`` rather than trusted. A project whose vocabulary moved
    after the export was built would hand back class indices that do not match
    the revisions in it — the labels would be silently permuted, every metric
    would be finite, and nothing anywhere would say so.

    A caller who genuinely has the right classes can pass them instead.
    """
    project = registry.get_project(export.project)
    schema = project.get("schema") or {}
    version = str(schema.get("version") or "")
    if version != export.schema_version:
        raise RegistryError(
            f"{export.project} is now on schema {version[:12]} and "
            f"{export.source} was built against {export.schema_version[:12]}; "
            "rasterising against the wrong vocabulary permutes every label without failing. "
            "Rebuild the export, or pass `classes=` explicitly."
        )
    return [dict(entry) for entry in schema.get("classes", [])]
