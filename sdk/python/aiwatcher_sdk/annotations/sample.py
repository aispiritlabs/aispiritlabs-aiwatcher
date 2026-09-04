"""One row of an export: the rows it kept, and the rows it left out.

``sample`` is the word the wire uses — a manifest's ``samples[]`` — and the
word a training loop uses for the thing a ``Dataset`` hands back, so the client
invents no third one for the same row.

:class:`ExcludedSample` is the other half of a manifest and lives here with it.
An export has two sides and reading only the first is how a corpus quietly
comes out a third smaller than anybody expected; keeping them in one file is
what makes that pairing hard to miss.
"""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass, field
from typing import Any, Final

from aiwatcher_sdk.annotations.errors import RegistryError
from aiwatcher_sdk.annotations.split import SPLITS, Split

__all__ = ["BLOB_SCHEME", "ExcludedSample", "Sample"]

#: What an uploaded image's URI looks like. Not `blob:`, which the browser owns.
BLOB_SCHEME: Final = "aiwatcher://blob/"


@dataclass(frozen=True, slots=True)
class Sample:
    """One image in an export, pinned to the revision that was accepted."""

    image_id: str
    uri: str
    width: int
    height: int
    group_id: str
    split: Split
    revision: str
    instances: int
    source: str = ""
    rights: str = ""
    level: str | None = None

    @property
    def is_blob(self) -> bool:
        """Whether the bytes live in the registry rather than somewhere else."""
        return self.uri.startswith(BLOB_SCHEME)

    @classmethod
    def from_json(cls, payload: Mapping[str, Any]) -> Sample:
        split = payload.get("split")
        if split not in SPLITS:
            # Checked rather than cast. A sample on a side this SDK does not
            # know about would be silently dropped by every `split ==` there
            # is, which is a training set quietly missing a third of itself.
            raise RegistryError(
                f"{payload.get('image_id', 'a sample')} is on split {split!r}, "
                f"which is not one of {', '.join(SPLITS)}"
            )
        return cls(
            image_id=str(payload["image_id"]),
            uri=str(payload["uri"]),
            width=int(payload["width"]),
            height=int(payload["height"]),
            group_id=str(payload["group_id"]),
            split=split,
            revision=str(payload["revision"]),
            instances=int(payload.get("instances", 0)),
            source=str(payload.get("source", "")),
            rights=str(payload.get("rights", "")),
            level=payload.get("level"),
        )


@dataclass(frozen=True, slots=True)
class ExcludedSample:
    """One image the export left out, and why.

    Typed for the same reason :class:`Sample` is: this is the half of a
    manifest somebody reads when a corpus came out smaller than they expected,
    and a mistyped key on a raw dict answers ``None`` — which reads as "nothing
    was excluded for that reason" rather than as the mistake it is.

    ``reason`` stays a plain string where :attr:`Sample.split` is checked
    against a closed set, and the difference is what each one drives. A split
    decides which side an image trains on, so an unknown one is a corpus
    quietly missing a third of itself and has to be refused. A reason is a
    label for a human — a release that adds one should print it, not raise.
    The set at the time of writing: ``rights``, ``unreviewed``, ``empty``,
    ``schema_mismatch``, ``view``, ``missing``, ``no_requested_class``.
    """

    image_id: str
    group_id: str
    reason: str
    detail: str = ""
    #: The row as the server sent it, for a field this release does not know.
    raw: Mapping[str, Any] = field(default_factory=dict, compare=False, repr=False)

    def __str__(self) -> str:
        return f"{self.group_id}: {self.reason}" + (f" — {self.detail}" if self.detail else "")

    @classmethod
    def from_json(cls, payload: Mapping[str, Any]) -> ExcludedSample:
        return cls(
            image_id=str(payload.get("image_id", "")),
            group_id=str(payload.get("group_id", "")),
            reason=str(payload.get("reason", "")),
            detail=str(payload.get("detail", "")),
            raw=dict(payload),
        )
