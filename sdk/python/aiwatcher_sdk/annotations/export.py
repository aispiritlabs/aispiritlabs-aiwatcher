"""The frozen manifest, and the string a training run records.

What a training script calls its *data loader*: the export the server built,
plus the source its images are read through. Frozen because the thing it models
is frozen — an export is a content address over exactly these samples.
"""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass, field
from typing import Any

from aiwatcher_sdk.annotations.errors import RegistryError
from aiwatcher_sdk.annotations.image_source import ImageSource
from aiwatcher_sdk.annotations.sample import ExcludedSample, Sample
from aiwatcher_sdk.annotations.split import SPLITS, Split
from aiwatcher_sdk.annotations.view import SplitView

__all__ = ["Export", "parse_source"]


@dataclass(frozen=True, slots=True, repr=False)
class Export:
    """An immutable manifest, and the string a training run records.

    What :meth:`~aiwatcher_sdk.annotations.registry.AnnotationRegistry.build_dataloader`
    hands back: the frozen export the server built, together with the registry
    its images are read through. Every field is a tuple or a mapping rather
    than a list, because the thing this models is frozen on the server — an
    export is a content address over exactly these samples, and a caller that
    appended one locally would hold a manifest that is no longer the manifest
    its source names.
    """

    project: str
    export: str
    schema_version: str
    classes: tuple[str, ...]
    #: The rows the export kept. ``samples[]`` on the wire, and the same word
    #: a ``Dataset`` uses for what it hands back.
    samples: tuple[Sample, ...]
    #: The rows it left out, each with why. ``excluded[]`` on the wire.
    excluded_samples: tuple[ExcludedSample, ...]
    counts: Mapping[str, Any]
    rights_policy: str
    raw: Mapping[str, Any]
    #: Where the images are readable from, when this came from a registry.
    #:
    #: Typed as the protocol rather than as the client, which is what keeps this
    #: file from importing the thing that imports it. Provenance rather than
    #: content, so it is out of ``==`` and out of ``repr``: two manifests with
    #: the same source *are* the same export, whichever client read them. It is
    #: here so that reading an export and then reading its images is one object
    #: rather than two — a manifest that knew where it came from and made the
    #: caller say it again would be asking for something it already has. A
    #: manifest built by :meth:`from_json` off a file has none, and
    #: :meth:`~aiwatcher_sdk.annotations.view.SplitView.as_dataset` then needs
    #: one passed.
    registry: ImageSource | None = field(default=None, compare=False, repr=False)

    def __repr__(self) -> str:
        """What a manifest is, not what it holds.

        The generated one printed `raw` — the whole server response, every
        sample twice over, tens of kilobytes of it. `repr` is what somebody
        types when they want to know what they are holding, and an answer that
        scrolls the terminal is not one. The counts are the answer; `samples`
        and `raw` are still there to be asked for.
        """
        return (
            f"Export({self.source!r}, {len(self.samples)} images, "
            f"{len(self.get_groups())} groups, {len(self.excluded_samples)} excluded)"
        )

    @property
    def source(self) -> str:
        """``project@export-sha256``. Put this in ``train.started.data.dataset``.

        The one string that says where a model's training data came from: the
        project is the name people use, and the export is the content address
        that pins what the name meant at the time. A run that recorded only
        the first half cannot be repeated.
        """
        return f"{self.project}@{self.export}"

    def get_split(self, split: Split) -> SplitView:
        """One side, as a sequence that knows what it is::

        test = dataloader.get_split("test")
        len(test)               # images
        test.get_groups()       # subjects, which is what a score is over
        test.as_dataset(image_size=512)
        """
        if split not in SPLITS:
            raise RegistryError(f"{split!r} is not a split; expected one of {', '.join(SPLITS)}")
        return SplitView(
            self, split, tuple(sample for sample in self.samples if sample.split == split)
        )

    def get_splits(self) -> dict[Split, SplitView]:
        """All three sides, in order. What a report iterates."""
        return {name: self.get_split(name) for name in SPLITS}

    def get_all(self) -> SplitView:
        """Every sample, as one view. The whole corpus, not one side."""
        return SplitView(self, None, self.samples)

    def get_groups(self) -> frozenset[str]:
        """Every subject in the export. Per side, ``get_split(...).get_groups()``."""
        return self.get_all().get_groups()

    @classmethod
    def from_json(
        cls, payload: Mapping[str, Any], *, registry: ImageSource | None = None
    ) -> Export:
        return cls(
            registry=registry,
            project=str(payload["project"]),
            export=str(payload["export"]),
            schema_version=str(payload["schema_version"]),
            classes=tuple(payload.get("classes", ())),
            samples=tuple(Sample.from_json(row) for row in payload.get("samples", ())),
            excluded_samples=tuple(
                ExcludedSample.from_json(row) for row in payload.get("excluded", ())
            ),
            counts=dict(payload.get("counts", {})),
            rights_policy=str(payload.get("rights_policy", "")),
            raw=dict(payload),
        )


def parse_source(source: str) -> tuple[str, str]:
    """``project@export-sha256``, split in two. A bare name is refused.

    Internal to this package — the registry calls it on the way into a request
    — and not exported, because a caller holding an :class:`Export` has
    :attr:`Export.project` and :attr:`Export.export` already.

    The pair is the whole point: a project name is mutable and an export id is
    a content address, and a run that recorded only the first cannot say what
    it was trained on.
    """
    project, _, export = source.rpartition("@")
    if not project or not export:
        raise RegistryError(f"{source!r} is not an export source; expected project@sha256")
    return project, export
