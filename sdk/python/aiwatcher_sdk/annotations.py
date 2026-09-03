"""The annotation registry, from Python.

Same failure policy as :mod:`aiwatcher_sdk.prompts` and for the same reason:
this is not telemetry. Reading the export a training run is about to consume is
the work, and a client that quietly returned an empty sample list would train a
model on nothing and report a loss for it. Every method here raises.

The shape follows PyTorch's own: an export is frozen, a **split** of it is a
sequence you can measure, and a dataset is built from that split and handed to
a ``DataLoader``::

    from aiwatcher_sdk.annotations import AnnotationRegistry

    with AnnotationRegistry("http://aiwatcher:8080") as registry:
        export = registry.build_export("corpora/plans")
        print(export.reference)              # corpora/plans@9f3c…

        test = export.split("test")
        print(len(test), len(test.families()))

        data = test.dataset(image_size=512)   # needs [vision]
        loader = data.loader(batch_size=4)    # needs torch
        batch = next(iter(loader))
        batch["image"], batch["targets"]

The export carries the registry it was read from, so nothing downstream has to
be handed one again — `split(...).dataset(...)` reads its images through the
client that produced the manifest.

Four things this client does that are easy to get wrong by hand:

* **The reference is the pair.** ``project@export-sha256`` is what a training
  run records. A project name alone is mutable, and a run that recorded only a
  name cannot prove what it was trained on.
* **The split is the family's, not the image's.** Nothing here lets a caller
  assign a split per image, because the mirrored and re-drawn variants of one
  subject have to stay together. :func:`split_for` is the local computation of
  the same rule the server applies, and :meth:`SplitView.families` is the
  number a score is really over — forty images of eight houses is eight
  observations.
* **Exclusions are data.** :attr:`Export.excluded` lists every image that did
  not make it and why. An export that silently loses a third of a corpus reads
  exactly like one that did not.
* **The bytes are checked against the id.** :meth:`AnnotationRegistry.fetch_image`
  re-hashes what it received. An image that is not the image its labels were
  drawn on is the one corruption no metric detects.
"""

from __future__ import annotations

import hashlib
from collections.abc import Iterator, Mapping, Sequence
from dataclasses import dataclass, field
from types import TracebackType
from typing import TYPE_CHECKING, Any, Literal, Self, get_args, overload

import httpx

from aiwatcher_sdk.api import ApiError, Transport

if TYPE_CHECKING:  # pragma: no cover - the vision extra is optional at runtime
    from pathlib import Path

    from aiwatcher_sdk.integrations.vision import ExportDataset

__all__ = [
    "BLOB_SCHEME",
    "SPLITS",
    "AnnotationRegistry",
    "Exclusion",
    "Export",
    "RegistryError",
    "Sample",
    "Split",
    "SplitCounts",
    "SplitView",
    "split_for",
]

Split = Literal["train", "validation", "test"]
RightsPolicy = Literal["commercial", "research", "any"]
Review = Literal["pending", "accepted", "rejected"]

#: The three sides, in the order a report reads them.
SPLITS: tuple[Split, ...] = get_args(Split)

#: What an uploaded image's URI looks like. Not `blob:`, which the browser owns.
BLOB_SCHEME = "aiwatcher://blob/"

_DISABLED = (
    "this aiwatcher instance was started without an annotation store; set AIWATCHER_PROMPT_STORE"
)


class RegistryError(ApiError):
    """The registry refused, or could not be reached.

    ``code`` is the machine-readable discriminator; switch on it rather than on
    the message. ``registry_disabled`` means the instance was started without an
    object store, which is a deployment problem rather than a missing project.
    ``annotation_rejected`` means a drawing did not validate, and ``details``
    holds one line per problem.
    """


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
            # know about would be silently dropped by every `split ==` below,
            # which is a training set quietly missing a third of itself.
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
class SplitCounts:
    """What one side of an export holds.

    ``families`` is the number that bounds what a score can mean, and it is
    usually the smaller and more surprising of the first two.
    """

    images: int
    families: int
    instances: int

    def __str__(self) -> str:
        return f"{self.images} images, {self.families} families, {self.instances} instances"


@dataclass(frozen=True, slots=True)
class SplitView(Sequence[Sample]):
    """One side of an export: a sequence of :class:`Sample`, and what it means.

    A plain ``Sequence``, so ``len``, indexing, slicing, iteration and ``in``
    all work and a caller that only wanted the samples has lost nothing. What
    it adds is the two questions worth asking of a split before training on
    it — :meth:`families` and :meth:`counts` — and :meth:`dataset`, which is
    where PyTorch picks it up.

    Slicing returns another view, so ``export.split("train")[:8].families()``
    is a sentence rather than a re-implementation.
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
        return f"SplitView({self.export.reference!r}, {self.name or 'all'}, {self.counts()})"

    def families(self) -> frozenset[str]:
        """The distinct subjects on this side.

        The number that bounds what a score can mean. Forty images of eight
        houses is eight observations, and a recall figure quoted over the forty
        is a different number from the one anybody meant.
        """
        return frozenset(sample.group_id for sample in self.samples)

    def counts(self) -> SplitCounts:
        return SplitCounts(
            images=len(self.samples),
            families=len(self.families()),
            instances=sum(sample.instances for sample in self.samples),
        )

    def dataset(
        self,
        registry: AnnotationRegistry | None = None,
        *,
        image_size: int = 512,
        channels: int = 1,
        cache_dir: str | Path | None = None,
        flip: bool = False,
        classes: Sequence[Mapping[str, Any]] | None = None,
    ) -> ExportDataset:
        """This split as a map-style dataset, ready for a ``DataLoader``::

            export = registry.build_export("corpora/plans")
            train = export.split("train").dataset(image_size=512)

        ``registry`` defaults to the one the export came from, which is the
        usual case and the one worth not repeating. Pass it only for a manifest
        that came from somewhere else — :meth:`Export.from_json` off a file
        stored beside a run, say, whose images are still readable from a live
        instance.

        Needs the ``vision`` extra — the rasteriser is numpy and Pillow — and
        imports it here rather than at the top of this module, so a caller that
        only wanted to know which houses are held out never pays for either::

            pip install 'aiwatcher-sdk[vision]'
        """
        from aiwatcher_sdk.integrations.vision import ExportDataset

        return ExportDataset(
            self.registry(registry),
            self,
            image_size=image_size,
            channels=channels,
            cache_dir=cache_dir,
            flip=flip,
            classes=classes,
        )

    def registry(self, override: AnnotationRegistry | None = None) -> AnnotationRegistry:
        """The client the images behind this split are readable from.

        Refuses rather than guesses. A manifest read off a file has no source,
        and a default base URL invented here would go looking for a training
        set on somebody's laptop.
        """
        found = override if override is not None else self.export.source
        if found is None:
            raise RegistryError(
                f"{self.export.reference} was not read from a registry, so there is nothing "
                "to fetch its images from; pass one — `split.dataset(registry, ...)`"
            )
        return found


@dataclass(frozen=True, slots=True)
class Exclusion:
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
    def from_json(cls, payload: Mapping[str, Any]) -> Exclusion:
        return cls(
            image_id=str(payload.get("image_id", "")),
            group_id=str(payload.get("group_id", "")),
            reason=str(payload.get("reason", "")),
            detail=str(payload.get("detail", "")),
            raw=dict(payload),
        )


@dataclass(frozen=True, slots=True, repr=False)
class Export:
    """An immutable manifest, and the string a training run records.

    Every field is a tuple or a mapping rather than a list, because the thing
    this models is frozen on the server: an export is a content address over
    exactly these samples, and a caller that appended one locally would hold a
    manifest that is no longer the manifest its reference names.
    """

    project: str
    export: str
    schema_version: str
    classes: tuple[str, ...]
    samples: tuple[Sample, ...]
    excluded: tuple[Exclusion, ...]
    counts: Mapping[str, Any]
    rights_policy: str
    raw: Mapping[str, Any]
    #: The registry this came from, when it came from one.
    #:
    #: Provenance rather than content, so it is out of ``==`` and out of
    #: ``repr``: two manifests with the same reference *are* the same export,
    #: whichever client read them. It is here so that reading an export and
    #: then reading its images is one object rather than two — a manifest that
    #: knew where it came from and made the caller say it again would be
    #: asking for something it already has. A manifest built by
    #: :meth:`from_json` off a file has none, and :meth:`SplitView.dataset`
    #: then needs one passed.
    source: AnnotationRegistry | None = field(default=None, compare=False, repr=False)

    def __repr__(self) -> str:
        """What a manifest is, not what it holds.

        The generated one printed `raw` — the whole server response, every
        sample twice over, tens of kilobytes of it. `repr` is what somebody
        types when they want to know what they are holding, and an answer that
        scrolls the terminal is not one. The counts are the answer; `samples`
        and `raw` are still there to be asked for.
        """
        return (
            f"Export({self.reference!r}, {len(self.samples)} images, "
            f"{len(self.families())} families, {len(self.excluded)} excluded)"
        )

    @property
    def reference(self) -> str:
        """``project@export-sha256``. Put this in ``train.started.data.dataset``."""
        return f"{self.project}@{self.export}"

    def split(self, split: Split) -> SplitView:
        """One side, as a sequence that knows what it is::

        test = export.split("test")
        len(test)            # images
        test.families()      # subjects, which is what a score is over
        test.dataset(registry, image_size=512)
        """
        if split not in SPLITS:
            raise RegistryError(f"{split!r} is not a split; expected one of {', '.join(SPLITS)}")
        return SplitView(
            self, split, tuple(sample for sample in self.samples if sample.split == split)
        )

    def splits(self) -> dict[Split, SplitView]:
        """All three sides, in order. What a report iterates."""
        return {name: self.split(name) for name in SPLITS}

    def all(self) -> SplitView:
        """Every sample, as one view. The whole corpus, not one side."""
        return SplitView(self, None, self.samples)

    def families(self) -> frozenset[str]:
        """Every subject in the export. Per side, use ``split(...).families()``."""
        return self.all().families()

    @classmethod
    def from_json(
        cls, payload: Mapping[str, Any], *, source: AnnotationRegistry | None = None
    ) -> Export:
        return cls(
            source=source,
            project=str(payload["project"]),
            export=str(payload["export"]),
            schema_version=str(payload["schema_version"]),
            classes=tuple(payload.get("classes", ())),
            samples=tuple(Sample.from_json(sample) for sample in payload.get("samples", ())),
            excluded=tuple(Exclusion.from_json(row) for row in payload.get("excluded", ())),
            counts=dict(payload.get("counts", {})),
            rights_policy=str(payload.get("rights_policy", "")),
            raw=dict(payload),
        )


def split_for(group_id: str, salt: str, ratios: tuple[int, int, int] = (70, 15, 15)) -> Split:
    """Which side of the split a family falls on.

    The same computation the server does, byte for byte, so a caller can answer
    "is this house in the test set" without a request. Deterministic in the
    family and the salt and *only* in those: adding an image never moves an
    existing family.
    """
    if sum(ratios) != 100 or any(ratio < 0 for ratio in ratios):
        raise RegistryError(f"split ratios must be three non-negative parts of 100, got {ratios}")
    digest = hashlib.sha256(salt.encode() + b"\x00" + group_id.encode()).digest()
    bucket = int.from_bytes(digest[:8], "big") % 100
    train, validation, _ = ratios
    if bucket < train:
        return "train"
    if bucket < train + validation:
        return "validation"
    return "test"


class AnnotationRegistry:
    """A client for `/api/v1/annotation-*`.

    A context manager, and worth using as one: an export of six hundred images
    is six hundred requests to one host, and the connection is held open across
    them rather than rebuilt per call.

    Synchronous and blocking, like every other registry client here. A training
    job reads an export once and then reads images inside a ``DataLoader``
    worker, and both of those are already the shape an async client would be
    unwinding.
    """

    def __init__(
        self,
        base_url: str,
        *,
        token: str | None = None,
        timeout: float = 30.0,
        attempts: int = 3,
        client: httpx.Client | None = None,
    ) -> None:
        self._http = Transport(
            base_url,
            token=token,
            timeout=timeout,
            attempts=attempts,
            error=RegistryError,
            subject="the annotation registry",
            hints={"registry_disabled": _DISABLED},
            client=client,
        )

    @property
    def base_url(self) -> str:
        return self._http.base_url

    def close(self) -> None:
        self._http.close()

    def __enter__(self) -> Self:
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        self.close()

    # ── Reads ────────────────────────────────────────────────────────────

    def projects(self) -> list[dict[str, Any]]:
        return list(self._http.json("GET", "/api/v1/annotation-projects").get("projects", []))

    def project(self, name: str) -> dict[str, Any]:
        """One project with its schema and the counts that say whether there is
        enough data yet."""
        return self._http.json("GET", "/api/v1/annotation-project", params={"name": name})

    def images(
        self,
        project: str,
        *,
        review: Review | None = None,
        split: Split | None = None,
        search: str | None = None,
        limit: int = 200,
    ) -> Iterator[dict[str, Any]]:
        """Every matching image, paging until the server stops offering more."""
        offset = 0
        while True:
            page = self._http.json(
                "GET",
                "/api/v1/annotation-images",
                params={
                    "project": project,
                    "offset": offset,
                    "limit": limit,
                    "review": review,
                    "split": split,
                    "search": search,
                },
            )
            yield from page.get("images", [])
            next_offset = page.get("next_offset")
            if next_offset is None:
                return
            offset = int(next_offset)

    def image(self, project: str, image_id: str, *, revision: str | None = None) -> dict[str, Any]:
        return self._http.json(
            "GET",
            "/api/v1/annotation-image",
            params={"project": project, "image_id": image_id, "revision": revision},
        )

    def revision_annotations(
        self, project: str, image_id: str, *, revision: str | None = None
    ) -> list[dict[str, Any]]:
        """The shapes of one revision. Raises rather than returning nothing.

        The one method here whose failure mode is silent by default and must
        not be: an image whose revision the server could not resolve comes back
        as a detail with no `revision` key, and a client that turned that into
        `[]` would hand a trainer a blank target for a plan that is fully
        drawn. A blank target is not an error anywhere downstream — it is a
        plan the model is told contains no walls, and the loss it produces is
        finite, so the run completes and the number is wrong.

        Pass the revision an export pinned, never the project's current head.
        The export is a claim about which drawings a run saw; re-reading the
        head breaks it the moment somebody fixes a label mid-run.
        """
        detail = self.image(project, image_id, revision=revision)
        found = detail.get("revision")
        if not isinstance(found, Mapping):
            raise RegistryError(
                f"{image_id} in {project!r} has no "
                f"{'revision ' + revision if revision else 'accepted revision'}; "
                "training on it would train on an empty target"
            )
        return [dict(shape) for shape in found.get("annotations", [])]

    def exports(self, project: str) -> list[dict[str, Any]]:
        return list(
            self._http.json("GET", "/api/v1/annotation-exports", params={"name": project}).get(
                "exports", []
            )
        )

    def export(self, reference: str) -> Export:
        """One manifest, by ``project@export-sha256``."""
        project, export = _reference(reference)
        return Export.from_json(
            self._http.json(
                "GET",
                "/api/v1/annotation-export",
                params={"project": project, "export": export},
            ),
            source=self,
        )

    def coco(self, export: Export | str, *, split: Split | None = None) -> dict[str, Any]:
        """The export as a COCO document, optionally one split.

        Generated on request rather than stored, because a second copy of the
        annotations is a copy that can disagree with the first.
        """
        project, identifier = _reference(export if isinstance(export, str) else export.reference)
        return self._http.json(
            "GET",
            "/api/v1/annotation-export/coco",
            params={"project": project, "export": identifier, "split": split},
        )

    def image_url(self, sample: Sample | str) -> str:
        """A URL the trainer can fetch the pixels from."""
        uri = sample if isinstance(sample, str) else sample.uri
        if uri.startswith(BLOB_SCHEME):
            return f"{self.base_url}/api/v1/annotation-blobs/{uri[len(BLOB_SCHEME) :]}"
        return uri

    def fetch_image(self, sample: Sample | str) -> bytes:
        """The pixels, verified against the digest the id claims.

        An image id is a SHA-256 of the bytes whatever the image is, so this
        holds for one registered by reference as well as for a blob — and it
        matters *more* there, because those bytes live on a host this
        deployment does not run and `plans/latest.png` is different pixels
        tomorrow. A silent mismatch is a training set that does not match its
        labels, which is the one corruption no metric detects.

        The token is a separate question with the same answer: an image by
        reference is fetched from somebody else's host, so the `Authorization`
        header goes only to this registry's own origin, and no redirect is
        followed that could move the request after that check.
        """
        url = self.image_url(sample)
        body = self._http.read(url)
        expected = sample.image_id if isinstance(sample, Sample) else None
        if expected and hashlib.sha256(body).hexdigest() != expected:
            raise RegistryError(
                f"the bytes served for {expected} do not hash to it; "
                "the image and its labels are not the same image"
            )
        return body

    # ── Writes ───────────────────────────────────────────────────────────

    def save_project(
        self,
        name: str,
        classes: Sequence[Mapping[str, Any]],
        *,
        description: str = "",
        splits: Mapping[str, int] | None = None,
        split_salt: str = "",
        split_overrides: Mapping[str, Split] | None = None,
    ) -> dict[str, Any]:
        body: dict[str, Any] = {
            "name": name,
            "description": description,
            "classes": list(classes),
            "split_salt": split_salt,
        }
        if splits:
            body["splits"] = dict(splits)
        if split_overrides:
            body["split_overrides"] = dict(split_overrides)
        return self._http.json("POST", "/api/v1/annotation-projects", body, idempotent=True)

    def upload(
        self, body: bytes, *, content_type: str = "application/octet-stream"
    ) -> dict[str, Any]:
        """Store image bytes under the digest the *server* computes."""
        return self._http.json(
            "POST",
            "/api/v1/annotation-blobs",
            content=body,
            content_type=content_type,
            # Content addressed: the same bytes land on the same key, so a
            # repeat after a lost answer stores nothing twice.
            idempotent=True,
        )

    def register_image(
        self,
        project: str,
        *,
        image_id: str,
        uri: str,
        width: int,
        height: int,
        group_id: str,
        rights: Mapping[str, Any],
        source: str = "",
        view: str = "",
        level: str | None = None,
        metadata: Mapping[str, Any] | None = None,
    ) -> dict[str, Any]:
        """Register an image into a project.

        ``group_id`` is the family — one subject, however many renderings of
        it — and it is what keeps a mirrored or re-shot copy out of the test
        set when its original is in the training set. ``rights`` is required
        for the other expensive mistake: a commercial export excludes anything
        that does not satisfy its policy, by name.

        ``view`` is free text and empty by default. A corpus with one kind of
        picture never needs it; one that mixes kinds names them, and an export
        then selects the ones a model reads.
        """
        return self._http.json(
            "POST",
            "/api/v1/annotation-images",
            {
                "project": project,
                "image_id": image_id,
                "uri": uri,
                "width": width,
                "height": height,
                "group_id": group_id,
                "source": source,
                "rights": dict(rights),
                "view": view,
                "level": level,
                "metadata": dict(metadata or {}),
            },
            idempotent=True,
        )

    def save_revision(
        self,
        project: str,
        image_id: str,
        annotations: Sequence[Mapping[str, Any]],
        *,
        notes: str = "",
        accept: bool = False,
    ) -> dict[str, Any]:
        """Save a drawing. Raises with one detail line per problem if refused.

        A model-assisted pass belongs here with ``origin: "model"`` on each
        shape and ``accept=False``: a proposal nobody has looked at must not
        become a training target, and the export enforces that.
        """
        return self._http.json(
            "POST",
            "/api/v1/annotation-revisions",
            {
                "project": project,
                "image_id": image_id,
                "annotations": list(annotations),
                "notes": notes,
                "accept": accept,
            },
            # A revision id is the content address of its shapes, so a repeat
            # after a lost answer lands on the revision that already exists.
            idempotent=True,
        )

    def review(
        self,
        project: str,
        image_id: str,
        *,
        review: Review,
        revision: str | None = None,
        note: str = "",
    ) -> dict[str, Any]:
        return self._http.json(
            "POST",
            "/api/v1/annotation-reviews",
            {
                "project": project,
                "image_id": image_id,
                "review": review,
                "revision": revision,
                "note": note,
            },
            idempotent=True,
        )

    def build_export(
        self,
        project: str,
        *,
        note: str = "",
        rights_policy: RightsPolicy = "commercial",
        require_human_review: bool = True,
        classes: Sequence[str] = (),
        views: Sequence[str] = (),
    ) -> Export:
        """Freeze the project as it stands.

        Idempotent: an unchanged project is the same export, so running this
        before every training run is free.

        ``views`` selects by the ``view`` an image was registered with, and
        empty means every one — the right default for a corpus with one kind of
        picture. A corpus that mixes kinds names the ones a model reads, and
        every other image is excluded *by name* in the manifest rather than
        quietly.
        """
        body: dict[str, Any] = {
            "project": project,
            "note": note,
            "rights_policy": rights_policy,
            "require_human_review": require_human_review,
        }
        if views:
            body["views"] = list(views)
        if classes:
            body["classes"] = list(classes)
        return Export.from_json(
            self._http.json("POST", "/api/v1/annotation-exports", body, idempotent=True)[
                "manifest"
            ],
            source=self,
        )

    # ── Where data comes from ────────────────────────────────────────────

    def sources(
        self,
        *,
        query: str | None = None,
        usage: Literal["commercial", "non_commercial", "unclear"] | None = None,
        label: str | None = None,
    ) -> dict[str, Any]:
        """The public corpora and what their licences permit.

        A dated table the instance was configured with, not a search — see
        `AIWATCHER_DATASET_SOURCES`. Empty until somebody loads one, which is
        a working state: nothing then outranks a mirror's claim. Every row
        links its original; the licence at that link is the only one that
        counts.
        """
        return self._http.json(
            "GET",
            "/api/v1/annotation-sources",
            params={"q": query, "usage": usage, "label": label},
        )


def _reference(reference: str) -> tuple[str, str]:
    """``project@export-sha256``, split. A bare name is refused.

    The pair is the whole point: a project name is mutable and an export id is
    a content address, and a run that recorded only the first cannot say what
    it was trained on.
    """
    project, _, export = reference.rpartition("@")
    if not project or not export:
        raise RegistryError(f"{reference!r} is not an export reference; expected project@sha256")
    return project, export
