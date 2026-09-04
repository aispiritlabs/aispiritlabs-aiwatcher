"""What a dataset needs from a registry, written down as a protocol.

Two things it buys, and the second is why it exists at all.

**Substitution.** Turning a split into tensors needs exactly three answers — a
project's schema, one revision's shapes, and an image's bytes — and nothing
else about an HTTP client. Stated as three methods, a cache in front of a slow
link, a reader over a corpus somebody rsynced onto a GPU box, or a test double
is a small class rather than a subclass of a client with a connection pool
inside it. :class:`~aiwatcher_sdk.integrations.vision.ExportDataset` asks for
this and never for the client.

**Direction.** A manifest carries the registry it was read from, and the
registry hands back manifests. Pointed at the concrete client that is a
circular import; pointed at this port it is a straight line —
``errors → split → sample → image_source → view → export → registry`` — with every
arrow going one way and the two ends meeting at the abstraction rather than at
each other. The file layout of this package is that line, which is why a
change to what one thing *is* touches one file.

Structural, in the manner of :class:`aiwatcher_sdk.serving.server.ModelSource`:
:class:`~aiwatcher_sdk.annotations.registry.AnnotationRegistry` satisfies this
by having the methods, and names it nowhere. Nothing is registered and there is
no base class to inherit.
"""

from __future__ import annotations

from typing import Any, Protocol

from aiwatcher_sdk.annotations.sample import Sample

__all__ = ["ImageSource"]


class ImageSource(Protocol):
    """The three reads that turn a split into arrays.

    Every one of them raises rather than answering emptily, which is the whole
    failure policy of this half of the SDK: reading the export a run is about
    to consume *is* the work, and an empty answer trains a model on nothing and
    reports a loss for it.
    """

    def get_project(self, name: str) -> dict[str, Any]:
        """The project, whose ``schema`` says what a class index means.

        Read and then *checked* against the export's pinned ``schema_version``
        by the caller — a vocabulary that moved after the export was built
        permutes every label while every metric stays finite.
        """
        ...

    def get_revision_annotations(
        self, project: str, image_id: str, *, revision: str | None = ...
    ) -> list[dict[str, Any]]:
        """The shapes of one revision — the one an export pinned, never the head.

        Raises rather than answering ``[]``. A blank target is not an error
        anywhere downstream: it is a picture the model is told is empty, and
        the loss it produces is finite.
        """
        ...

    def fetch_image(self, sample: Sample | str) -> bytes:
        """The pixels, verified against the digest the image id claims.

        An implementation that skips the check hands a trainer an image that is
        not the image its labels were drawn on, which is the one corruption no
        metric detects.
        """
        ...
