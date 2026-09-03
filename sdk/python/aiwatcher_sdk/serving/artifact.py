"""Artifact readers, one verification rule, and the persistent version cache.

The package names an address and a digest. The address decides *where* a
reader goes; the digest decides whether the bytes are the model that was
measured. Those are separate decisions, and every loader reaches them through
the same :func:`read_verified` call.

Three readers ship:

``FileReader``
    local ``file://`` paths, primarily for development and the runnable demo.
``S3Reader``
    bounded, redirect-free ``s3://`` reads against one configured bucket,
    signed with AWS Signature Version 4. Credentials can therefore authorise
    the configured store without becoming part of an artifact URI.
``VersionCacheReader``
    an atomic on-disk cache around either reader. Entries are addressed by the
    immutable model version *and* artifact digest, admitted only after their
    SHA-256 matches, rechecked on every cache hit, and evicted least-recently
    used when the configured byte budget is exceeded.

Nothing infers a runtime from a file and nothing trusts transport integrity.
TLS, SigV4 and an S3 ETag answer different questions from the digest in the
package, so :func:`read_verified` hashes a cache hit exactly as it hashes a new
download.
"""

from __future__ import annotations

import datetime as dt
import hashlib
import hmac
import threading
import urllib.error
import urllib.parse
import urllib.request
import uuid
from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Protocol

__all__ = [
    "ArtifactReader",
    "FileReader",
    "LoadError",
    "S3Credentials",
    "S3Reader",
    "SchemeReader",
    "VersionCacheReader",
    "read_verified",
    "sha256_file",
]

_EMPTY_SHA256 = hashlib.sha256(b"").hexdigest()
_SIGV4_ALGORITHM = "AWS4-HMAC-SHA256"
_HEX = frozenset("0123456789abcdef")


class LoadError(RuntimeError):
    """A version that cannot become ready.

    Never fatal to a *running* server: the rollout records the candidate's
    refusal and leaves the ready version in place.
    """


class ArtifactReader(Protocol):
    """The bytes at one artifact URI.

    ``version`` and ``expected_digest`` are explicit because a persistent
    cache must be keyed by immutable identity rather than by a mutable URI.
    Readers that do not cache, such as :class:`FileReader`, ignore them.
    """

    def read(self, uri: str, *, version: str, expected_digest: str) -> bytes: ...

    @property
    def schemes(self) -> tuple[str, ...]:
        """URI schemes this host can fetch, reported on ``/v1/model``."""


class FileReader:
    """``file://`` and nothing else, said out loud rather than implied."""

    @property
    def schemes(self) -> tuple[str, ...]:
        return ("file",)

    def read(self, uri: str, *, version: str = "", expected_digest: str = "") -> bytes:
        del version, expected_digest
        parsed = urllib.parse.urlparse(uri)
        if parsed.scheme != "file":
            raise LoadError(
                f"this host reads {'/'.join(self.schemes)}:// artifacts and this one is {uri!r}. "
                "Signed readers for configured stores plug in behind ArtifactReader"
            )
        path = Path(urllib.parse.unquote(parsed.path))
        try:
            return path.read_bytes()
        except OSError as error:
            raise LoadError(f"cannot read {path}: {error}") from error


@dataclass(frozen=True, slots=True)
class S3Credentials:
    """The SigV4 identity and scope used for one configured object store."""

    access_key_id: str
    secret_access_key: str = field(repr=False)
    region: str = "us-east-1"
    session_token: str | None = field(default=None, repr=False)


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    """Turn every 3xx into an answer the caller can refuse."""

    def redirect_request(
        self,
        req: urllib.request.Request,
        fp: Any,
        code: int,
        msg: str,
        headers: Any,
        newurl: str,
    ) -> None:
        del req, fp, code, msg, headers, newurl
        return None


class S3Reader:
    """Signed, bounded GETs from one S3 endpoint and one approved bucket.

    Path-style addressing is deliberate: it works for AWS and for the RustFS,
    MinIO, Ceph and SeaweedFS endpoints people run inside a cluster without
    requiring wildcard DNS. Redirects are refused so credentials and bounds
    are never silently applied to a destination that was not configured.
    """

    def __init__(
        self,
        endpoint: str,
        bucket: str,
        credentials: S3Credentials,
        *,
        timeout_seconds: float = 30.0,
        max_bytes: int = 4 * 1024 * 1024 * 1024,
    ) -> None:
        parsed = urllib.parse.urlsplit(endpoint.rstrip("/"))
        if parsed.scheme not in {"http", "https"} or not parsed.hostname:
            raise ValueError(f"the S3 endpoint {endpoint!r} is not an http(s) URL with a host")
        if parsed.username or parsed.password or parsed.query or parsed.fragment:
            raise ValueError("the S3 endpoint may not carry credentials, a query or a fragment")
        if parsed.path not in {"", "/"}:
            raise ValueError("the S3 endpoint must not carry a path; the bucket supplies it")
        if not bucket or "/" in bucket or bucket in {".", ".."}:
            raise ValueError(f"the configured S3 bucket {bucket!r} is not a bucket name")
        if not credentials.access_key_id or not credentials.secret_access_key:
            raise ValueError("the S3 reader needs an access key id and secret access key")
        if timeout_seconds <= 0:
            raise ValueError("the S3 timeout must be positive")
        if max_bytes <= 0:
            raise ValueError("the S3 artifact byte ceiling must be positive")

        self._endpoint = parsed
        self._bucket = bucket
        self._credentials = credentials
        self._timeout = timeout_seconds
        self._max_bytes = max_bytes
        self._opener = urllib.request.build_opener(_NoRedirect())

    @property
    def schemes(self) -> tuple[str, ...]:
        return ("s3",)

    @property
    def endpoint(self) -> str:
        """The non-secret endpoint, for operator-visible reader metadata."""
        return urllib.parse.urlunsplit((self._endpoint.scheme, self._endpoint.netloc, "", "", ""))

    @property
    def bucket(self) -> str:
        return self._bucket

    def read(self, uri: str, *, version: str = "", expected_digest: str = "") -> bytes:
        del version, expected_digest
        path = self._path(uri)
        url = urllib.parse.urlunsplit((self._endpoint.scheme, self._endpoint.netloc, path, "", ""))
        headers = _sign_get(
            credentials=self._credentials,
            canonical_path=path,
            host=self._endpoint.netloc,
            at=dt.datetime.now(dt.UTC),
        )
        # Both endpoint and final URL were parsed and restricted to http(s)
        # above; no user-controlled scheme reaches urlopen.
        request = urllib.request.Request(url, method="GET", headers=headers)  # noqa: S310
        try:
            response = self._opener.open(request, timeout=self._timeout)
        except urllib.error.HTTPError as error:
            detail = error.read(8192).decode("utf-8", errors="replace").strip()
            if 300 <= error.code < 400:
                raise LoadError(
                    f"reading {uri}: the object store answered with redirect {error.code}; "
                    "signed artifact reads never follow redirects"
                ) from error
            suffix = f" — {detail}" if detail else ""
            raise LoadError(
                f"reading {uri}: the object store answered {error.code} {error.reason}{suffix}"
            ) from error
        except (urllib.error.URLError, TimeoutError, OSError) as error:
            raise LoadError(f"reading {uri} from {self.endpoint}: {error}") from error

        try:
            declared = response.headers.get("content-length")
            if declared is not None:
                try:
                    declared_bytes = int(declared)
                except ValueError:
                    declared_bytes = -1
                if declared_bytes > self._max_bytes:
                    raise LoadError(
                        f"{uri} declares {declared_bytes} bytes and this host allows "
                        f"{self._max_bytes} per artifact"
                    )

            chunks: list[bytes] = []
            received = 0
            while True:
                remaining = self._max_bytes - received
                chunk = response.read(min(1024 * 1024, remaining + 1))
                if not chunk:
                    break
                received += len(chunk)
                if received > self._max_bytes:
                    raise LoadError(
                        f"{uri} exceeded this host's {self._max_bytes}-byte artifact ceiling "
                        "while it was streaming"
                    )
                chunks.append(chunk)
            return b"".join(chunks)
        except LoadError:
            raise
        except (TimeoutError, OSError) as error:
            raise LoadError(f"reading the body of {uri}: {error}") from error
        finally:
            response.close()

    def _path(self, uri: str) -> str:
        parsed = urllib.parse.urlsplit(uri)
        if parsed.scheme != "s3" or not parsed.netloc:
            raise LoadError(f"the S3 reader expected s3://bucket/key and received {uri!r}")
        if parsed.username or parsed.password or parsed.query or parsed.fragment:
            raise LoadError(f"the S3 artifact URI {uri!r} may not carry credentials or a query")
        if parsed.netloc != self._bucket:
            raise LoadError(
                f"the artifact names S3 bucket {parsed.netloc!r} and this host approves only "
                f"{self._bucket!r}"
            )
        try:
            key = urllib.parse.unquote(parsed.path.lstrip("/"), errors="strict")
        except UnicodeDecodeError as error:
            raise LoadError(f"the object key in {uri!r} is not UTF-8") from error
        if not key:
            raise LoadError(f"the S3 artifact URI {uri!r} names no object key")
        raw_path = f"/{self._bucket}/{key}"
        return urllib.parse.quote(raw_path, safe="/-_.~")

    def describe(self) -> dict[str, Any]:
        return {
            "type": "s3",
            "endpoint": self.endpoint,
            "bucket": self._bucket,
            "region": self._credentials.region,
            "max_bytes": self._max_bytes,
            "signed": True,
            "redirects": False,
        }


class SchemeReader:
    """Route a URI to exactly one reader by its declared scheme."""

    def __init__(self, readers: Sequence[ArtifactReader]) -> None:
        routes: dict[str, ArtifactReader] = {}
        for reader in readers:
            for scheme in reader.schemes:
                if scheme in routes:
                    raise ValueError(f"two artifact readers answer to {scheme!r}")
                routes[scheme] = reader
        if not routes:
            raise ValueError("at least one artifact reader is required")
        self._routes = routes

    @property
    def schemes(self) -> tuple[str, ...]:
        return tuple(sorted(self._routes))

    def read(self, uri: str, *, version: str = "", expected_digest: str = "") -> bytes:
        scheme = urllib.parse.urlsplit(uri).scheme
        reader = self._routes.get(scheme)
        if reader is None:
            available = ", ".join(f"{value}://" for value in self.schemes)
            raise LoadError(f"this host reads {available} artifacts and the package names {uri!r}")
        return reader.read(uri, version=version, expected_digest=expected_digest)

    def describe(self) -> dict[str, Any]:
        details: list[Mapping[str, Any]] = []
        seen: set[int] = set()
        for reader in self._routes.values():
            identity = id(reader)
            if identity in seen:
                continue
            seen.add(identity)
            describe = getattr(reader, "describe", None)
            if callable(describe):
                details.append(describe())
            else:
                details.append({"type": "+".join(reader.schemes)})
        return {"type": "scheme", "readers": details}


class VersionCacheReader:
    """A verified, atomic, size-bounded cache keyed by version and digest."""

    def __init__(
        self,
        inner: ArtifactReader,
        directory: Path,
        *,
        max_bytes: int = 10 * 1024 * 1024 * 1024,
        cache_schemes: Sequence[str] | None = None,
    ) -> None:
        if max_bytes <= 0:
            raise ValueError("the artifact cache byte budget must be positive")
        self._inner = inner
        self._directory = directory
        self._max_bytes = max_bytes
        self._cache_schemes = frozenset(cache_schemes or inner.schemes)
        self._lock = threading.Lock()

    @property
    def schemes(self) -> tuple[str, ...]:
        return self._inner.schemes

    def read(self, uri: str, *, version: str = "", expected_digest: str = "") -> bytes:
        scheme = urllib.parse.urlsplit(uri).scheme
        if (
            scheme not in self._cache_schemes
            or not _is_sha256(version)
            or not _is_sha256(expected_digest)
        ):
            return self._inner.read(uri, version=version, expected_digest=expected_digest)

        path = self._directory / version / expected_digest
        with self._lock:
            cached = self._read_valid(path, expected_digest)
            if cached is not None:
                return cached

        body = self._inner.read(uri, version=version, expected_digest=expected_digest)
        found = hashlib.sha256(body).hexdigest()
        if found != expected_digest:
            raise LoadError(
                f"{uri} hashes to {found} and the package says {expected_digest}. These are not "
                "the bytes that version was measured on"
            )
        if len(body) > self._max_bytes:
            return body

        with self._lock:
            # Another process or thread may have won the same download. Its
            # bytes still have to pass before they are trusted.
            cached = self._read_valid(path, expected_digest)
            if cached is not None:
                return cached
            self._write_atomic(path, body)
            self._evict()
        return body

    def _read_valid(self, path: Path, expected_digest: str) -> bytes | None:
        try:
            if not path.is_file():
                return None
            if sha256_file(path) != expected_digest:
                path.unlink(missing_ok=True)
                return None
            body = path.read_bytes()
            path.touch()
            return body
        except OSError:
            # A cache is an optimisation, not a new source of load failures.
            return None

    def _write_atomic(self, path: Path, body: bytes) -> None:
        try:
            path.parent.mkdir(parents=True, exist_ok=True)
            temporary = path.parent / f".{path.name}.{uuid.uuid4().hex}.tmp"
            try:
                temporary.write_bytes(body)
                temporary.replace(path)
            finally:
                temporary.unlink(missing_ok=True)
        except OSError as error:
            raise LoadError(f"cannot populate the artifact cache at {path}: {error}") from error

    def _evict(self) -> None:
        try:
            entries = [
                path
                for path in self._directory.glob("*/*")
                if path.is_file() and _is_sha256(path.name)
            ]
            sized = [(path.stat().st_mtime_ns, path.stat().st_size, path) for path in entries]
            total = sum(size for _, size, _ in sized)
            for _, size, path in sorted(sized):
                if total <= self._max_bytes:
                    break
                path.unlink(missing_ok=True)
                total -= size
            for directory in self._directory.iterdir():
                if directory.is_dir() and not any(directory.iterdir()):
                    directory.rmdir()
        except OSError:
            # The just-written entry is valid. Failing to reclaim an older
            # cache file is observable in disk usage and not a model failure.
            return

    def describe(self) -> dict[str, Any]:
        inner = getattr(self._inner, "describe", None)
        detail: dict[str, Any] = {
            "type": "version-cache",
            "directory": str(self._directory),
            "max_bytes": self._max_bytes,
            "cache_schemes": sorted(self._cache_schemes),
        }
        if callable(inner):
            detail["reader"] = inner()
        return detail


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def read_verified(
    reader: ArtifactReader,
    artifact: Mapping[str, Any],
    *,
    version: str = "",
) -> bytes:
    """Read one artifact and compare its bytes before anything opens them."""
    uri = str(artifact.get("uri", ""))
    if not uri:
        raise LoadError(f"the artifact {artifact.get('name', '?')!r} names no uri")
    expected = str(artifact.get("digest") or "")
    body = reader.read(uri, version=version, expected_digest=expected)
    found = hashlib.sha256(body).hexdigest()
    if expected and found != expected:
        raise LoadError(
            f"{uri} hashes to {found} and the package says {expected}. These are not the bytes "
            "that version was measured on"
        )
    return body


def _is_sha256(value: str) -> bool:
    return len(value) == 64 and all(character in _HEX for character in value)


def _sign_get(
    *,
    credentials: S3Credentials,
    canonical_path: str,
    host: str,
    at: dt.datetime,
) -> dict[str, str]:
    """SigV4 headers for an S3 GET, matching the repository's Rust signer."""
    stamp = at.astimezone(dt.UTC)
    amz_date = stamp.strftime("%Y%m%dT%H%M%SZ")
    date_stamp = amz_date[:8]
    headers = {
        "host": host,
        "x-amz-content-sha256": _EMPTY_SHA256,
        "x-amz-date": amz_date,
    }
    if credentials.session_token:
        headers["x-amz-security-token"] = credentials.session_token
    names = ";".join(sorted(headers))
    canonical_headers = "".join(f"{name}:{headers[name].strip()}\n" for name in sorted(headers))
    canonical_request = f"GET\n{canonical_path}\n\n{canonical_headers}\n{names}\n{_EMPTY_SHA256}"
    scope = f"{date_stamp}/{credentials.region}/s3/aws4_request"
    request_digest = hashlib.sha256(canonical_request.encode()).hexdigest()
    string_to_sign = f"{_SIGV4_ALGORITHM}\n{amz_date}\n{scope}\n{request_digest}"
    key = _signing_key(credentials.secret_access_key, date_stamp, credentials.region)
    signature = hmac.new(key, string_to_sign.encode(), hashlib.sha256).hexdigest()
    headers["authorization"] = (
        f"{_SIGV4_ALGORITHM} Credential={credentials.access_key_id}/{scope}, "
        f"SignedHeaders={names}, Signature={signature}"
    )
    return headers


def _signing_key(secret: str, date_stamp: str, region: str) -> bytes:
    date = hmac.new(f"AWS4{secret}".encode(), date_stamp.encode(), hashlib.sha256).digest()
    regional = hmac.new(date, region.encode(), hashlib.sha256).digest()
    service = hmac.new(regional, b"s3", hashlib.sha256).digest()
    return hmac.new(service, b"aws4_request", hashlib.sha256).digest()
