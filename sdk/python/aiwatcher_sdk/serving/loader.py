"""What a loaded model is, and which loader is allowed to produce one.

ADR_0023 says a runtime is *declared*, never sniffed: a loader chosen by
looking at the file is a loader chosen by whoever wrote the file. This module
is that sentence as code. :func:`load` reads ``package.runtime``, looks it up
in a table, and refuses an entry that is not there **by name** — never by
falling back to something that might work.

Two rules sit here rather than in any one runtime.

**A runtime that runs the package's own code is refused in a process that was
not built to isolate it.** ``Runtime::executes_packaged_code`` is the question
a host answers *before* it opens anything, and the answer decides whether a
loader may be selected at all. A ``python`` package is a program; loading one
in a process that holds an object store's credentials hands those credentials
to whoever trained the model. So the check is here, in the selection, where a
future subprocess loader has to opt in explicitly.

**A version with no package is loaded and reported unverified.** Versions
registered before packages existed have none, and an unverified model that
reports itself as verified is worse than one that reports the truth. What is
refused is a *half* package — a declared runtime with an artifact carrying no
digest — because that reads as provenance and is not. The registry refuses
that one on the way in; this side only has to keep the distinction visible.
"""

from __future__ import annotations

import hashlib
import time
from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field
from typing import Any, Protocol

from aiwatcher_sdk.serving.artifact import ArtifactReader, LoadError, read_verified

__all__ = [
    "LoadError",
    "Loaded",
    "Loader",
    "Predictor",
    "load",
    "package_digest",
    "pick_artifact",
]


class Predictor(Protocol):
    """One loaded model, ready to answer, and nothing else.

    The whole per-runtime surface. Everything a serving process does around it
    — resolve, verify, warm, bound, validate, watch, roll back, report — is
    the same for every framework, which is why that half lives in
    :mod:`aiwatcher_sdk.serving.server` and this is four members.
    """

    @property
    def features(self) -> int:
        """How wide a row this model eats. What a request is validated against."""

    @property
    def classes(self) -> tuple[str, ...]:
        """What index 0, 1, 2 … mean, from the package rather than from code.

        Empty when the package declared none, in which case a response carries
        raw scores: naming them here from a convention would be inventing a
        label order, which is the failure ``TensorSpec::classes`` exists to
        prevent.
        """

    def predict(self, rows: Sequence[Sequence[float]]) -> list[list[float]]:
        """One score vector per row, in the row's order.

        A single-element vector is the binary convention: the score is the
        probability of ``classes[-1]``.
        """

    def describe(self) -> Mapping[str, Any]:
        """What ``/v1/model`` reports about this runtime specifically.

        Whatever an operator would otherwise have to read the loader's source
        to find out: the entry point that was resolved, the graph's own
        declared shapes, the providers in use, the preprocessing the trainer
        named.
        """


class Loader(Protocol):
    """Turns one package into one :class:`Predictor`."""

    @property
    def runtime(self) -> str:
        """The ``package.runtime`` value this loader answers to."""

    @property
    def executes_packaged_code(self) -> bool:
        """Whether loading this runs code the package brought with it.

        Mirrors ``Runtime::executes_packaged_code`` on the Rust side. A loader
        answering ``True`` is refused by :func:`load` unless the host said it
        can isolate one — see the module docstring.
        """

    def load(
        self,
        package: Mapping[str, Any],
        reader: ArtifactReader,
        *,
        version: str,
    ) -> Predictor: ...


@dataclass(frozen=True, slots=True)
class Loaded:
    """A version that has been fetched, verified, loaded and warmed."""

    name: str
    version: str
    runtime: str
    #: ``sha256`` over every artifact digest, in declared order — the same
    #: number ``ModelPackage::digest`` computes, so "the model I have" and "the
    #: model the registry says is production" compare as an equality rather
    #: than as a review.
    digest: str
    #: False for a version registered before packages existed. Reported rather
    #: than hidden: "nothing checked these bytes" is a fact an operator should
    #: be able to read off ``/v1/model``.
    verified: bool
    checkpoint_uri: str
    predictor: Predictor
    loaded_at: float = field(default_factory=time.time)

    def describe(self) -> dict[str, Any]:
        return {
            "name": self.name,
            "version": self.version,
            "runtime": self.runtime,
            "digest": self.digest,
            "verified": self.verified,
            "checkpoint_uri": self.checkpoint_uri,
            "features": self.predictor.features,
            "classes": list(self.predictor.classes),
            "loaded_at": round(self.loaded_at, 3),
            "runtime_detail": dict(self.predictor.describe()),
        }


def package_digest(artifacts: Sequence[Mapping[str, Any]]) -> str:
    """``sha256`` over every artifact digest, joined — ``ModelPackage::digest``."""
    material = "\0".join(str(artifact.get("digest", "")) for artifact in artifacts)
    return hashlib.sha256(material.encode()).hexdigest()


def pick_artifact(
    package: Mapping[str, Any], *, prefer: Sequence[str] = ("weights",)
) -> Mapping[str, Any]:
    """Which artifact the loader starts from.

    ``entry_point`` first, and it is read as *a name in this package* — the
    artifact's own name, or the last segment of its URI. That is the answer to
    the question plan.md asked of this profile: free text is enough to act on
    exactly when it names something the package contains, and a value that
    names nothing is a refusal rather than a guess. A package with one file
    needs no entry point; one with several and no entry point has to say which,
    because picking for it is how a server loads the tokeniser as the graph.
    """
    artifacts: Sequence[Mapping[str, Any]] = package.get("artifacts") or []
    if not artifacts:
        raise LoadError("the package names no artifacts, so there is nothing to load")

    entry = str(package.get("entry_point") or "").strip()
    if entry:
        for artifact in artifacts:
            if str(artifact.get("name", "")) == entry:
                return artifact
        for artifact in artifacts:
            if str(artifact.get("uri", "")).rsplit("/", 1)[-1] == entry:
                return artifact
        names = ", ".join(sorted(str(artifact.get("name", "?")) for artifact in artifacts))
        raise LoadError(
            f"the entry point {entry!r} names neither an artifact of this package nor a file in "
            f"one. It holds: {names}"
        )

    if len(artifacts) == 1:
        return artifacts[0]
    for name in prefer:
        for artifact in artifacts:
            if str(artifact.get("name", "")) == name:
                return artifact
    names = ", ".join(sorted(str(artifact.get("name", "?")) for artifact in artifacts))
    raise LoadError(
        f"this package holds {len(artifacts)} artifacts ({names}) and names no entry point, so "
        "there is nothing to pick without guessing"
    )


class _OnceReader:
    """A reader that fetches each URI once per load.

    Two callers want the same bytes while one candidate is being built — the
    loader that parses them and the digest that names them — and a fetch is
    about to stop being a file read. Scoped to one load, so there is no
    eviction policy to get wrong: the long-lived cache keyed by the immutable
    version is a separate item in plan.md, with a directory and a bound.
    """

    def __init__(self, inner: ArtifactReader) -> None:
        self._inner = inner
        self._seen: dict[tuple[str, str, str], bytes] = {}

    @property
    def schemes(self) -> tuple[str, ...]:
        return self._inner.schemes

    def read(self, uri: str, *, version: str = "", expected_digest: str = "") -> bytes:
        key = (uri, version, expected_digest)
        if key not in self._seen:
            self._seen[key] = self._inner.read(
                uri,
                version=version,
                expected_digest=expected_digest,
            )
        return self._seen[key]


def load(
    current: Mapping[str, Any],
    name: str,
    loaders: Mapping[str, Loader],
    reader: ArtifactReader,
    *,
    isolates_packaged_code: bool = False,
) -> Loaded:
    """Fetch, verify and load one version. Never touches what is serving.

    The candidate is built beside the running model and handed back; the caller
    warms it and only then swaps. A failure anywhere in here is a
    :class:`LoadError` about a *candidate*, which is why a broken new label
    cannot remove a ready old version.
    """
    version = str(current.get("version", ""))
    if not version:
        raise LoadError("the registry returned a version with no id")

    once = _OnceReader(reader)
    package = current.get("package")
    if not package:
        return _unpackaged(current, name, loaders, once)

    declared = str(package.get("runtime", "") or "unspecified")
    if declared == "unspecified":
        raise LoadError(
            "the package does not name its runtime. A loader chosen by looking at the file is a "
            "loader chosen by whoever wrote the file"
        )
    loader = loaders.get(declared)
    if loader is None:
        implemented = ", ".join(sorted(loaders)) or "none"
        raise LoadError(
            f"this host implements the {implemented} runtime(s) and the package declares "
            f"{declared!r}. A runtime is declared rather than sniffed, so this is a refusal "
            "rather than an attempt"
        )
    if loader.executes_packaged_code and not isolates_packaged_code:
        raise LoadError(
            f"a {declared!r} package runs code it brought with it, and this process does not "
            "isolate one. Loading it here would hand the object store's credentials to whoever "
            "trained the model"
        )

    # A malformed entry point is a package error, not a fetch error. Refuse it
    # before downloading every artifact so the operator sees the declaration
    # that needs fixing rather than whichever auxiliary digest happened to be
    # checked first.
    if str(package.get("entry_point") or "").strip():
        pick_artifact(package)

    artifacts: Sequence[Mapping[str, Any]] = package.get("artifacts") or []
    # A package is every file it declares, not merely its entry point. A label
    # file or tokenizer with the wrong digest is the same provenance break as
    # a graph with the wrong digest, even when this runtime does not open it.
    # Do this before the loader opens anything; _OnceReader then makes the
    # loader's own read-and-check of the entry point a memory read.
    for artifact in artifacts:
        read_verified(once, artifact, version=version)
    predictor = loader.load(package, once, version=version)
    primary = pick_artifact(package)
    return Loaded(
        name=name,
        version=version,
        runtime=declared,
        digest=package_digest(artifacts),
        verified=True,
        checkpoint_uri=str(primary.get("uri", "")),
        predictor=predictor,
    )


def _unpackaged(
    current: Mapping[str, Any],
    name: str,
    loaders: Mapping[str, Loader],
    reader: ArtifactReader,
) -> Loaded:
    """A version registered before packages existed.

    There is one runtime it can be — the one this repository's own trainer
    wrote before ``ModelPackage`` existed — and it is loaded through the same
    loader as everything else so that the only difference between this path and
    the checked one is what it reports about itself.
    """
    uri = str(current.get("checkpoint_uri", ""))
    if not uri:
        raise LoadError("the version has neither a package nor a checkpoint uri")
    loader = loaders.get("weights")
    if loader is None:
        raise LoadError(
            f"the version {str(current.get('version', ''))[:12]} has no package, and a host with "
            "no weights loader cannot guess which runtime wrote it. Re-register it with one"
        )
    synthetic = {
        "runtime": "weights",
        "artifacts": [{"name": "weights", "uri": uri, "digest": ""}],
    }
    version = str(current["version"])
    predictor = loader.load(synthetic, reader, version=version)
    return Loaded(
        name=name,
        version=version,
        runtime="weights",
        digest=hashlib.sha256(reader.read(uri, version=version, expected_digest="")).hexdigest(),
        verified=False,
        checkpoint_uri=uri,
        predictor=predictor,
    )
