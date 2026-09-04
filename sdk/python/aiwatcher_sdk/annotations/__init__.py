"""The annotation registry, from Python.

Same failure policy as :mod:`aiwatcher_sdk.prompts` and for the same reason:
this is not telemetry. Reading the export a training run is about to consume is
the work, and a client that quietly returned an empty sample list would train a
model on nothing and report a loss for it. Every method here raises.

The shape follows PyTorch's own: a **data loader** is the frozen export plus
the source it reads images through, a **split** of it is a sequence you can
measure, and a dataset is built from that split and handed to a ``DataLoader``::

    from aiwatcher_sdk.annotations import AnnotationRegistry

    with AnnotationRegistry("http://aiwatcher:8080") as data_registry:
        dataloader = data_registry.build_dataloader("corpora/plans")
        print(dataloader.source)             # corpora/plans@9f3c…

        test = dataloader.get_split("test")
        print(len(test), len(test.get_groups()))

        data = test.as_dataset(image_size=512)           # needs [vision]
        loader = data.as_torch_dataloader(batch_size=4)  # needs torch
        batch = next(iter(loader))
        batch["image"], batch["targets"]

The loader carries the registry it was read from, so nothing downstream has to
be handed one again — `get_split(...).as_dataset(...)` reads its images through
the client that produced the manifest.

## What is where

This is the door, and it is the only thing a caller imports. Behind it, one
file per noun, in the order they depend on each other — every arrow points up
this list and none points back down:

``errors``        :class:`RegistryError`, and the sentence a disabled instance
                  should produce. Depends on nothing here, because a rule, a
                  sample and a manifest all raise.
``split``         the *rule*: the three sides, and :func:`split_for`, which is
                  the server's own computation done locally.
``sample``        one row of a manifest — :class:`Sample` for the rows an
                  export kept, :class:`ExcludedSample` for the rows it left out
                  and why. Both halves in one file, because reading only the
                  first is how a corpus quietly comes out smaller.
``image_source``  :class:`ImageSource`, the three reads a dataset needs. The
                  abstraction the two ends meet at, which is what keeps the
                  manifest and the client from importing each other.
``view``          :class:`SplitView`, one side as a ``Sequence[Sample]``, and
                  the only place the group rule is applied.
``export``        :class:`Export`, the frozen manifest and the string a run
                  records.
``registry``      :class:`AnnotationRegistry`, the one file that knows a
                  network exists.

## How things are named

A method is a verb phrase: a read is ``get_<noun>`` — ``iter_<noun>`` when it
pages — a conversion is ``as_<noun>``, and ``build_<noun>`` asks the server to
make something. A field or a property is a noun. A collection is named for what
it holds, so the rows an export kept are ``samples`` and the rows it left out
are ``excluded_samples``. A method that is a bare noun reads like a field and
has to be looked up to find out that it is not.

## Four things that are easy to get wrong by hand

* **The source is the pair.** ``project@export-sha256`` is what a training run
  records as its dataset. A project name alone is mutable, and a run that
  recorded only a name cannot prove what it was trained on.
* **The split is the group's, not the image's.** Nothing here lets a caller
  assign a split per image, because the mirrored and re-drawn variants of one
  subject have to stay together. :func:`split_for` is the local computation of
  the same rule the server applies, and :meth:`SplitView.get_groups` is the
  number a score is really over — forty images of eight houses is eight
  observations.
* **Exclusions are data.** :attr:`Export.excluded_samples` lists every image
  that did not make it and why. An export that silently loses a third of a
  corpus reads exactly like one that did not.
* **The bytes are checked against the id.** :meth:`AnnotationRegistry.fetch_image`
  re-hashes what it received. An image that is not the image its labels were
  drawn on is the one corruption no metric detects.
"""

from __future__ import annotations

from aiwatcher_sdk.annotations.errors import RegistryError
from aiwatcher_sdk.annotations.export import Export
from aiwatcher_sdk.annotations.image_source import ImageSource
from aiwatcher_sdk.annotations.registry import AnnotationRegistry, Review, RightsPolicy
from aiwatcher_sdk.annotations.sample import BLOB_SCHEME, ExcludedSample, Sample
from aiwatcher_sdk.annotations.split import SPLITS, Split, split_for
from aiwatcher_sdk.annotations.view import SplitCounts, SplitView

__all__ = [
    "BLOB_SCHEME",
    "SPLITS",
    "AnnotationRegistry",
    "ExcludedSample",
    "Export",
    "ImageSource",
    "RegistryError",
    "Review",
    "RightsPolicy",
    "Sample",
    "Split",
    "SplitCounts",
    "SplitView",
    "split_for",
]
