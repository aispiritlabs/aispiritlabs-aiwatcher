"""The HTTP client for ``/api/v1/annotation-*``.

The top of this package: the one file that knows a network exists, and the only
one that may import every other. Everything below it is the vocabulary a
manifest is expressed in, and none of it depends on this.
"""

from __future__ import annotations

import hashlib
from collections.abc import Iterator, Mapping, Sequence
from types import TracebackType
from typing import Any, Literal, Self

import httpx

from aiwatcher_sdk.annotations.errors import DISABLED, RegistryError
from aiwatcher_sdk.annotations.export import Export, parse_source
from aiwatcher_sdk.annotations.sample import BLOB_SCHEME, Sample
from aiwatcher_sdk.annotations.split import Split
from aiwatcher_sdk.api import Transport

__all__ = ["AnnotationRegistry", "Review", "RightsPolicy"]

RightsPolicy = Literal["commercial", "research", "any"]
Review = Literal["pending", "accepted", "rejected"]


class AnnotationRegistry:
    """A client for `/api/v1/annotation-*`.

    A context manager, and worth using as one: an export of six hundred images
    is six hundred requests to one host, and the connection is held open across
    them rather than rebuilt per call::

        with AnnotationRegistry("http://aiwatcher:8080") as data_registry:
            dataloader = data_registry.build_dataloader("corpora/plans")

    Reads are ``get_<noun>``, or ``iter_<noun>`` when they page; writes are
    the verb that names what they do. The two that matter to a training job
    are :meth:`build_dataloader`, which freezes a project into an export and
    hands back the loader over it, and :meth:`get_dataloader`, which reads one
    that already exists by its ``source``.

    Three of its reads are also
    :class:`~aiwatcher_sdk.annotations.image_source.ImageSource`, which is what a
    dataset asks for. Structurally, so this class names that protocol nowhere
    and satisfies it by having the methods.

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
            hints={"registry_disabled": DISABLED},
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

    def get_projects(self) -> list[dict[str, Any]]:
        return list(self._http.json("GET", "/api/v1/annotation-projects").get("projects", []))

    def get_project(self, name: str) -> dict[str, Any]:
        """One project with its schema and the counts that say whether there is
        enough data yet."""
        return self._http.json("GET", "/api/v1/annotation-project", params={"name": name})

    def iter_images(
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

    def get_image(
        self, project: str, image_id: str, *, revision: str | None = None
    ) -> dict[str, Any]:
        return self._http.json(
            "GET",
            "/api/v1/annotation-image",
            params={"project": project, "image_id": image_id, "revision": revision},
        )

    def get_revision_annotations(
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
        detail = self.get_image(project, image_id, revision=revision)
        found = detail.get("revision")
        if not isinstance(found, Mapping):
            raise RegistryError(
                f"{image_id} in {project!r} has no "
                f"{'revision ' + revision if revision else 'accepted revision'}; "
                "training on it would train on an empty target"
            )
        return [dict(shape) for shape in found.get("annotations", [])]

    def get_exports(self, project: str) -> list[dict[str, Any]]:
        return list(
            self._http.json("GET", "/api/v1/annotation-exports", params={"name": project}).get(
                "exports", []
            )
        )

    def get_dataloader(self, source: str) -> Export:
        """The loader over an export that already exists, by ``project@export-sha256``.

        What a script that recorded ``dataloader.source`` calls to read the
        same data again — a re-run, an evaluation, a reviewer checking which
        houses a model never saw. A bare project name is refused: it is
        mutable, and "the export that project had at some point" is not a
        thing anybody can train on twice.
        """
        project, export = parse_source(source)
        return Export.from_json(
            self._http.json(
                "GET",
                "/api/v1/annotation-export",
                params={"project": project, "export": export},
            ),
            registry=self,
        )

    def get_coco(self, dataloader: Export | str, *, split: Split | None = None) -> dict[str, Any]:
        """The export as a COCO document, optionally one split.

        Generated on request rather than stored, because a second copy of the
        annotations is a copy that can disagree with the first. Takes the
        loader or its ``source``.
        """
        project, identifier = parse_source(
            dataloader if isinstance(dataloader, str) else dataloader.source
        )
        return self._http.json(
            "GET",
            "/api/v1/annotation-export/coco",
            params={"project": project, "export": identifier, "split": split},
        )

    def get_image_url(self, sample: Sample | str) -> str:
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
        url = self.get_image_url(sample)
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

        ``group_id`` is the group — one subject, however many renderings of
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

    def build_dataloader(
        self,
        project: str,
        *,
        note: str = "",
        rights_policy: RightsPolicy = "commercial",
        require_human_review: bool = True,
        classes: Sequence[str] = (),
        views: Sequence[str] = (),
    ) -> Export:
        """Freeze the project as it stands, and hand back the loader over it.

        The server builds an **export** — a content-addressed manifest of
        every accepted image that satisfies ``rights_policy`` — and what comes
        back is that export with this registry attached, which is everything a
        training script needs: :meth:`Export.get_split` for the sides,
        :meth:`~aiwatcher_sdk.annotations.view.SplitView.as_dataset` for the
        tensors, :attr:`Export.source` for the run record and
        :attr:`Export.excluded_samples` for what was left out and why.

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
            registry=self,
        )

    # ── Where data comes from ────────────────────────────────────────────

    def get_sources(
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
