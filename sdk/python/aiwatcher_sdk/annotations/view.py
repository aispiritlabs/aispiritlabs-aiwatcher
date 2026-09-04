"""One side of an export, as a sequence you can measure.

The *view* over the split rule's result — :mod:`aiwatcher_sdk.annotations.split`
is the rule itself, and it sits below this file because a sample names its side
before anything holds a collection of them.

This is where PyTorch picks the corpus up, and the shape follows PyTorch's own:
a split is a sequence, a dataset is built from it, and a ``DataLoader`` batches
that. It is also the only place the group rule is applied, so nothing
downstream gets a second chance to decide which subjects a model may see.
"""

from __future__ import annotations

from collections.abc import Iterator, Mapping, Sequence
from dataclasses import dataclass
from typing import TYPE_CHECKING, Any, overload

from aiwatcher_sdk.annotations.errors import RegistryError
from aiwatcher_sdk.annotations.image_source import ImageSource
from aiwatcher_sdk.annotations.sample import Sample
from aiwatcher_sdk.annotations.split import Split

if TYPE_CHECKING:  # pragma: no cover - the vision extra is optional at runtime
    from pathlib import Path

    from aiwatcher_sdk.annotations.export import Export
    from aiwatcher_sdk.integrations.vision import ExportDataset

__all__ = ["SplitCounts", "SplitView"]


@dataclass(frozen=True, slots=True)
class SplitCounts:
    """What one side of an export holds.

    ``groups`` is the number that bounds what a score can mean, and it is
    usually the smaller and more surprising of the first two.
    """

    images: int
    groups: int
    instances: int

    def __str__(self) -> str:
        return f"{self.images} images, {self.groups} groups, {self.instances} instances"


@dataclass(frozen=True, slots=True)
class SplitView(Sequence[Sample]):
    """One side of an export: a sequence of :class:`Sample`, and what it means.

    A plain ``Sequence``, so ``len``, indexing, slicing, iteration and ``in``
    all work and a caller that only wanted the samples has lost nothing. What
    it adds is the two questions worth asking of a split before training on
    it — :meth:`get_groups` and :meth:`get_counts` — and :meth:`as_dataset`,
    which is where PyTorch picks it up.

    Slicing returns another view, so
    ``dataloader.get_split("train")[:8].get_groups()`` is a sentence rather
    than a re-implementation.

    The forward reference to :class:`~aiwatcher_sdk.annotations.export.Export`
    is the one place in this package where the dependency line bends: a view
    names the manifest it is a side of, and the manifest builds views. It is a
    type-checking import only, so the runtime line stays straight.
    """

    export: Export
    #: ``None`` for a view over every split.
    name: Split | None
    samples: tuple[Sample, ...]

    def __len__(self) -> int:
        return len(self.samples)

    @overload
    def __getitem__(self, index: int) -> Sample: ...

    @overload
    def __getitem__(self, index: slice) -> SplitView: ...

    def __getitem__(self, index: int | slice) -> Sample | SplitView:
        if isinstance(index, slice):
            return SplitView(self.export, self.name, self.samples[index])
        return self.samples[index]

    def __iter__(self) -> Iterator[Sample]:
        return iter(self.samples)

    def __repr__(self) -> str:
        return f"SplitView({self.export.source!r}, {self.name or 'all'}, {self.get_counts()})"

    def get_groups(self) -> frozenset[str]:
        """The distinct subjects on this side.

        A group is every rendering of one subject — ``group_id`` on the wire,
        the key the split was dealt by — and its count is the number that
        bounds what a score can mean. Forty images of eight houses is eight
        observations, and a recall figure quoted over the forty is a different
        number from the one anybody meant.
        """
        return frozenset(sample.group_id for sample in self.samples)

    def get_counts(self) -> SplitCounts:
        return SplitCounts(
            images=len(self.samples),
            groups=len(self.get_groups()),
            instances=sum(sample.instances for sample in self.samples),
        )

    def as_dataset(
        self,
        registry: ImageSource | None = None,
        *,
        image_size: int = 512,
        channels: int = 1,
        cache_dir: str | Path | None = None,
        flip: bool = False,
        classes: Sequence[Mapping[str, Any]] | None = None,
    ) -> ExportDataset:
        """This split as a map-style dataset, ready for a ``DataLoader``::

            dataloader = data_registry.build_dataloader("corpora/plans")
            train = dataloader.get_split("train").as_dataset(image_size=512)

        ``registry`` defaults to the one the loader came from, which is the
        usual case and the one worth not repeating. Pass it only for a manifest
        that came from somewhere else — :meth:`Export.from_json` off a file
        stored beside a run, say, whose images are still readable from a live
        instance — or for anything else that answers
        :class:`~aiwatcher_sdk.annotations.image_source.ImageSource`.

        Needs the ``vision`` extra — the rasteriser is numpy and Pillow — and
        imports it here rather than at the top of this module, so a caller that
        only wanted to know which houses are held out never pays for either::

            pip install 'aiwatcher-sdk[vision]'
        """
        from aiwatcher_sdk.integrations.vision import ExportDataset

        return ExportDataset(
            self.get_registry(registry),
            self,
            image_size=image_size,
            channels=channels,
            cache_dir=cache_dir,
            flip=flip,
            classes=classes,
        )

    def get_registry(self, override: ImageSource | None = None) -> ImageSource:
        """The source the images behind this split are readable from.

        Refuses rather than guesses. A manifest read off a file has no
        registry, and a default base URL invented here would go looking for a
        training set on somebody's laptop.
        """
        found = override if override is not None else self.export.registry
        if found is None:
            raise RegistryError(
                f"{self.export.source} was not read from a registry, so there is nothing "
                "to fetch its images from; pass one — `split.as_dataset(registry, ...)`"
            )
        return found
