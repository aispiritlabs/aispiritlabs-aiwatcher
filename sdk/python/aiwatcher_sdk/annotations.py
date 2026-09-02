"""The annotation registry, from Python.

Same failure policy as :mod:`aiwatcher_sdk.prompts` and for the same reason:
this is not telemetry. Reading the export a training run is about to consume is
the work, and a client that quietly returned an empty sample list would train a
model on nothing and report a loss for it. Every method here raises.

    from aiwatcher_sdk.annotations import AnnotationRegistry

    registry = AnnotationRegistry("http://aiwatcher:8080")
    export = registry.build_export("corpora/plans")
    print(export.reference)          # corpora/plans@9f3c…
    coco = registry.coco(export, split="train")

Three things this client does that are easy to get wrong by hand:

* **The reference is the pair.** ``project@export-sha256`` is what a training
  run records. A project name alone is mutable, and a run that recorded only a
  name cannot prove what it was trained on.
* **The split is the family's, not the image's.** Nothing here lets a caller
  assign a split per image, because the mirrored and re-drawn variants of one
  building have to stay together. :func:`split_for` is the local computation of
  the same rule the server applies.
* **Exclusions are data.** :attr:`Export.excluded` lists every image that did
  not make it and why. An export that silently loses a third of a corpus reads
  exactly like one that did not.
"""

from __future__ import annotations

import hashlib
import json
import os
import urllib.error
import urllib.parse
import urllib.request
from collections.abc import Iterator, Mapping, Sequence
from dataclasses import dataclass
from typing import Any, Literal

__all__ = [
    "AnnotationRegistry",
    "Export",
    "RegistryError",
    "Sample",
    "split_for",
]

Split = Literal["train", "validation", "test"]
RightsPolicy = Literal["commercial", "research", "any"]

#: What an uploaded image's URI looks like. Not `blob:`, which the browser owns.
BLOB_SCHEME = "aiwatcher://blob/"


class RegistryError(RuntimeError):
    """The registry refused, or could not be reached.

    ``code`` is the machine-readable discriminator; switch on it rather than on
    the message. ``registry_disabled`` means the instance was started without an
    object store, which is a deployment problem rather than a missing project.
    ``annotation_rejected`` means a drawing did not validate, and ``details``
    holds one line per problem.
    """

    def __init__(
        self,
        message: str,
        *,
        status: int | None = None,
        code: str | None = None,
        details: Sequence[str] = (),
    ) -> None:
        super().__init__(message)
        self.status = status
        self.code = code
        self.details = list(details)

    _RETRYABLE = frozenset({429, 500, 502, 503, 504})

    @property
    def is_retryable(self) -> bool:
        return self.status is None or self.status in self._RETRYABLE


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
        return cls(
            image_id=str(payload["image_id"]),
            uri=str(payload["uri"]),
            width=int(payload["width"]),
            height=int(payload["height"]),
            group_id=str(payload["group_id"]),
            split=payload["split"],
            revision=str(payload["revision"]),
            instances=int(payload.get("instances", 0)),
            source=str(payload.get("source", "")),
            rights=str(payload.get("rights", "")),
            level=payload.get("level"),
        )


@dataclass(frozen=True, slots=True)
class Export:
    """An immutable manifest, and the string a training run records."""

    project: str
    export: str
    schema_version: str
    classes: list[str]
    samples: list[Sample]
    excluded: list[dict[str, Any]]
    counts: dict[str, Any]
    rights_policy: str
    raw: dict[str, Any]

    @property
    def reference(self) -> str:
        """``project@export-sha256``. Put this in ``train.started.data.dataset``."""
        return f"{self.project}@{self.export}"

    def split(self, split: Split) -> list[Sample]:
        return [sample for sample in self.samples if sample.split == split]

    def families(self, split: Split | None = None) -> set[str]:
        """The distinct buildings on one side. The number that bounds what a
        score can mean — forty images of eight houses is eight observations."""
        return {
            sample.group_id for sample in self.samples if split is None or sample.split == split
        }

    @classmethod
    def from_json(cls, payload: Mapping[str, Any]) -> Export:
        return cls(
            project=str(payload["project"]),
            export=str(payload["export"]),
            schema_version=str(payload["schema_version"]),
            classes=list(payload.get("classes", [])),
            samples=[Sample.from_json(sample) for sample in payload.get("samples", [])],
            excluded=list(payload.get("excluded", [])),
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
    digest = hashlib.sha256(salt.encode() + b"\x00" + group_id.encode()).digest()
    bucket = int.from_bytes(digest[:8], "big") % 100
    train, validation, _ = ratios
    if bucket < train:
        return "train"
    if bucket < train + validation:
        return "validation"
    return "test"


class AnnotationRegistry:
    """A client for `/api/v1/annotation-*`."""

    def __init__(self, base_url: str, *, token: str | None = None, timeout: float = 30.0) -> None:
        self._base = base_url.rstrip("/")
        self._token = token if token is not None else os.environ.get("AIWATCHER_TOKEN")
        self._timeout = timeout

    # ── Reads ────────────────────────────────────────────────────────────

    def projects(self) -> list[dict[str, Any]]:
        return list(self._request("GET", "/api/v1/annotation-projects").get("projects", []))

    def project(self, name: str) -> dict[str, Any]:
        """One project with its schema and the counts that say whether there is
        enough data yet."""
        return self._request(
            "GET", f"/api/v1/annotation-project?{urllib.parse.urlencode({'name': name})}"
        )

    def images(
        self,
        project: str,
        *,
        review: str | None = None,
        split: Split | None = None,
        search: str | None = None,
        limit: int = 200,
    ) -> Iterator[dict[str, Any]]:
        """Every matching image, paging until the server stops offering more."""
        offset = 0
        while True:
            query: dict[str, Any] = {"project": project, "offset": offset, "limit": limit}
            for key, value in (("review", review), ("split", split), ("search", search)):
                if value:
                    query[key] = value
            page = self._request(
                "GET", f"/api/v1/annotation-images?{urllib.parse.urlencode(query)}"
            )
            yield from page.get("images", [])
            next_offset = page.get("next_offset")
            if next_offset is None:
                return
            offset = int(next_offset)

    def image(self, project: str, image_id: str, *, revision: str | None = None) -> dict[str, Any]:
        query: dict[str, str] = {"project": project, "image_id": image_id}
        if revision:
            query["revision"] = revision
        return self._request("GET", f"/api/v1/annotation-image?{urllib.parse.urlencode(query)}")

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
            self._request(
                "GET",
                f"/api/v1/annotation-exports?{urllib.parse.urlencode({'name': project})}",
            ).get("exports", [])
        )

    def export(self, reference: str) -> Export:
        """One manifest, by ``project@export-sha256``."""
        project, _, export = reference.rpartition("@")
        if not project or not export:
            raise RegistryError(
                f"{reference!r} is not an export reference; expected project@sha256"
            )
        query = {"project": project, "export": export}
        return Export.from_json(
            self._request("GET", f"/api/v1/annotation-export?{urllib.parse.urlencode(query)}")
        )

    def coco(self, export: Export | str, *, split: Split | None = None) -> dict[str, Any]:
        """The export as a COCO document, optionally one split.

        Generated on request rather than stored, because a second copy of the
        annotations is a copy that can disagree with the first.
        """
        reference = export if isinstance(export, str) else export.reference
        project, _, identifier = reference.rpartition("@")
        query: dict[str, str] = {"project": project, "export": identifier}
        if split:
            query["split"] = split
        return self._request(
            "GET", f"/api/v1/annotation-export/coco?{urllib.parse.urlencode(query)}"
        )

    def image_url(self, sample: Sample | str) -> str:
        """A URL the trainer can fetch the pixels from."""
        uri = sample if isinstance(sample, str) else sample.uri
        if uri.startswith(BLOB_SCHEME):
            return f"{self._base}/api/v1/annotation-blobs/{uri[len(BLOB_SCHEME) :]}"
        return uri

    def fetch_image(self, sample: Sample | str) -> bytes:
        """The pixels. Verified against the digest the id claims, for a blob.

        A silent mismatch here would be a training set that does not match its
        labels, which is the one corruption no metric detects.
        """
        url = self.image_url(sample)
        request = urllib.request.Request(url)  # noqa: S310 - the caller's own server
        if self._token:
            request.add_header("authorization", f"Bearer {self._token}")
        try:
            with urllib.request.urlopen(request, timeout=self._timeout) as response:  # noqa: S310
                body: bytes = response.read()
        except urllib.error.HTTPError as error:
            raise _from_http_error(error) from error
        except (urllib.error.URLError, OSError) as error:
            raise RegistryError(f"{url} is unreachable: {error}") from error

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
        return self._request("POST", "/api/v1/annotation-projects", body)

    def upload(
        self, body: bytes, *, content_type: str = "application/octet-stream"
    ) -> dict[str, Any]:
        """Store image bytes under the digest the *server* computes."""
        return self._request(
            "POST", "/api/v1/annotation-blobs", raw=body, content_type=content_type
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
        return self._request(
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
        return self._request(
            "POST",
            "/api/v1/annotation-revisions",
            {
                "project": project,
                "image_id": image_id,
                "annotations": list(annotations),
                "notes": notes,
                "accept": accept,
            },
        )

    def review(
        self,
        project: str,
        image_id: str,
        *,
        review: str,
        revision: str | None = None,
        note: str = "",
    ) -> dict[str, Any]:
        return self._request(
            "POST",
            "/api/v1/annotation-reviews",
            {
                "project": project,
                "image_id": image_id,
                "review": review,
                "revision": revision,
                "note": note,
            },
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

        ``views`` selects by :attr:`Sample.view`, and empty means every one —
        the right default for a corpus with one kind of picture. A corpus that
        mixes kinds names the ones a model reads, and every other image is
        excluded *by name* in the manifest rather than quietly.
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
            self._request("POST", "/api/v1/annotation-exports", body)["manifest"]
        )

    # ── Where data comes from ────────────────────────────────────────────

    def sources(
        self,
        *,
        query: str | None = None,
        usage: Literal["commercial", "non_commercial", "unclear"] | None = None,
        label: str | None = None,
    ) -> dict[str, Any]:
        """The public public corpora and what their licences permit.

        A dated table the instance was configured with, not a search — see
        `AIWATCHER_DATASET_SOURCES`. Empty until somebody loads one, which is
        a working state: nothing then outranks a mirror's claim. Every row
        links its original; the licence at that link is the only one that
        counts.
        """
        params: dict[str, str] = {}
        for key, value in (("q", query), ("usage", usage), ("label", label)):
            if value:
                params[key] = value
        suffix = f"?{urllib.parse.urlencode(params)}" if params else ""
        return self._request("GET", f"/api/v1/annotation-sources{suffix}")

    # ── Transport ────────────────────────────────────────────────────────

    def _request(
        self,
        method: str,
        path: str,
        body: Mapping[str, Any] | None = None,
        *,
        raw: bytes | None = None,
        content_type: str | None = None,
    ) -> dict[str, Any]:
        parsed = self._fetch(method, path, body, raw=raw, content_type=content_type)
        if not isinstance(parsed, dict):  # pragma: no cover - server contract
            raise RegistryError(f"expected an object from {path}, got {type(parsed).__name__}")
        return parsed

    def _fetch(
        self,
        method: str,
        path: str,
        body: Mapping[str, Any] | None = None,
        *,
        raw: bytes | None = None,
        content_type: str | None = None,
    ) -> Any:
        payload = raw if raw is not None else (None if body is None else json.dumps(body).encode())
        headers = {"content-type": content_type or "application/json"}
        if self._token:
            headers["authorization"] = f"Bearer {self._token}"
        request = urllib.request.Request(  # noqa: S310 - the base URL is the caller's own server
            f"{self._base}{path}",
            data=payload,
            headers=headers,
            method=method,
        )
        try:
            with urllib.request.urlopen(request, timeout=self._timeout) as response:  # noqa: S310
                received = response.read()
        except urllib.error.HTTPError as error:
            raise _from_http_error(error) from error
        except (urllib.error.URLError, OSError) as error:
            raise RegistryError(f"the registry at {self._base} is unreachable: {error}") from error
        if not received:
            return {}
        return json.loads(received)


def _from_http_error(error: urllib.error.HTTPError) -> RegistryError:
    """Turn the API's one error shape into this module's one exception.

    ``details`` is the field that matters here and nowhere else in this SDK: a
    drawing can be wrong in nine ways at once, and reporting the first would
    make fixing it nine round trips.
    """
    try:
        body = json.loads(error.read() or b"{}")
    except (ValueError, OSError):  # pragma: no cover - a proxy in the way
        body = {}
    message = body.get("message") or error.reason or "the registry refused"
    return RegistryError(
        str(message),
        status=error.code,
        code=body.get("code"),
        details=body.get("details", ()),
    )
