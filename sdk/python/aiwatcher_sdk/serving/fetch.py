"""Put one pinned artifact on a path, verified, without reading it into memory.

:mod:`~aiwatcher_sdk.serving.artifact` answers "are these the bytes that
version was measured on" for artifacts a process *opens*. This answers the
same question for artifacts a process *hands to something else* — a weights
file a serving container mounts, a GGUF a model server is started against —
where the consumer is a program that takes a path, not a reader.

The difference that forces a separate module is size. ``ArtifactReader.read``
returns ``bytes``, which is the right shape for a package a loader parses and
the wrong one for a 1.3 GiB checkpoint: an init container sized to hold its
own model in memory is sized wrong. Everything here streams, and the digest is
computed on the way past rather than over a buffer.

Three properties, and they are why this exists rather than five lines of
``urllib`` inlined wherever a model has to arrive:

**Restart-safe.** A destination that already hashes to the expected digest is
left alone and nothing is fetched. A container that restarts does not re-pull
a gigabyte, and a rolled-back node does not re-pull one either.

**Atomic.** Bytes land beside the destination and are renamed onto it only
after they hash correctly. An interrupted fetch cannot leave a truncated file
where a verified one used to be, which is the failure that turns one bad
network minute into a model server that starts and serves nonsense.

**Digest-decided.** Redirects are followed — a pinned Hugging Face URL is a
redirect to a CDN, and refusing them would mean refusing the normal case — but
only to ``https``, and what makes the bytes acceptable is the digest, never the
hop they arrived over. A fetch with no digest is refused rather than trusted.
"""

from __future__ import annotations

import argparse
import hashlib
import sys
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from pathlib import Path

from aiwatcher_sdk.serving.artifact import LoadError, sha256_file

__all__ = ["Fetched", "fetch_verified", "main"]

#: Read size. Large enough that a gigabyte is not a million syscalls, small
#: enough to stay out of the way of a 64 MiB init container.
_CHUNK = 1024 * 1024

#: What a fetch may pull before it is treated as the wrong address. A default
#: rather than a policy: a caller that knows the size says so.
DEFAULT_MAX_BYTES = 32 * 1024 * 1024 * 1024

_SCHEMES = ("https", "file")


@dataclass(frozen=True, slots=True)
class Fetched:
    """Where the artifact is, and whether this call is what put it there."""

    path: Path
    digest: str
    bytes_written: int
    #: False when the destination already held these bytes. The distinction an
    #: operator reads to tell a cold start from a re-pull.
    downloaded: bool


def fetch_verified(
    uri: str,
    *,
    digest: str,
    into: Path | str,
    max_bytes: int = DEFAULT_MAX_BYTES,
    timeout: float = 120.0,
) -> Fetched:
    """Stream `uri` onto `into`, and refuse anything that is not `digest`.

    Returns without fetching when `into` already hashes to `digest`.
    """
    expected = digest.strip().lower()
    if len(expected) != 64 or any(character not in "0123456789abcdef" for character in expected):
        raise LoadError(
            f"{expected!r} is not a sha256 digest, and a fetch with nothing to check against is "
            "a download, not a verification"
        )
    scheme = urllib.parse.urlsplit(uri).scheme
    if scheme not in _SCHEMES:
        available = ", ".join(f"{value}://" for value in _SCHEMES)
        raise LoadError(f"this fetcher reads {available} artifacts and was given {uri!r}")

    destination = Path(into)
    if destination.is_file() and sha256_file(destination) == expected:
        return Fetched(destination, expected, destination.stat().st_size, downloaded=False)

    destination.parent.mkdir(parents=True, exist_ok=True)
    # Beside the destination, not in a temp directory: a rename across file
    # systems is a copy, and a copy is the non-atomic step this avoids.
    partial = destination.with_name(destination.name + ".partial")
    try:
        found, written = _stream(uri, partial, max_bytes=max_bytes, timeout=timeout)
        if found != expected:
            raise LoadError(
                f"{uri} hashes to {found} and the pin says {expected}. These are not the bytes "
                "that version was measured on"
            )
        partial.replace(destination)
    finally:
        partial.unlink(missing_ok=True)
    return Fetched(destination, expected, written, downloaded=True)


def _stream(uri: str, partial: Path, *, max_bytes: int, timeout: float) -> tuple[str, int]:
    """Copy the URI into `partial`, hashing as it goes. Returns digest and size."""
    running = hashlib.sha256()
    written = 0
    try:
        with _open(uri, timeout=timeout) as source, partial.open("wb") as target:
            while chunk := source.read(_CHUNK):
                written += len(chunk)
                if written > max_bytes:
                    raise LoadError(
                        f"{uri} is larger than the {max_bytes} bytes this fetch allows. A pin "
                        "that grew is a different artifact"
                    )
                running.update(chunk)
                target.write(chunk)
    except (urllib.error.URLError, OSError) as error:
        raise LoadError(f"cannot fetch {uri}: {error}") from error
    return running.hexdigest(), written


def _open(uri: str, *, timeout: float):  # type: ignore[no-untyped-def]
    parsed = urllib.parse.urlsplit(uri)
    if parsed.scheme == "file":
        return Path(urllib.parse.unquote(parsed.path)).open("rb")
    # The scheme was checked above; this opener reaches https and nothing else.
    request = urllib.request.Request(uri, headers={"user-agent": "aiwatcher-sdk"})  # noqa: S310
    opener = urllib.request.build_opener(_HttpsOnlyRedirect())
    return opener.open(request, timeout=timeout)


class _HttpsOnlyRedirect(urllib.request.HTTPRedirectHandler):
    """Follow a redirect, but never off TLS.

    A pinned model URL redirects to a CDN, so refusing redirects would refuse
    the normal case. Downgrading to plain HTTP on the way is not the normal
    case, and the digest cannot tell the difference between a hop and an
    interception that serves the bytes it was asked for.
    """

    def redirect_request(self, req, fp, code, msg, headers, newurl):  # type: ignore[no-untyped-def]
        if urllib.parse.urlsplit(newurl).scheme != "https":
            raise urllib.error.HTTPError(newurl, code, "redirect leaves https", headers, fp)
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def main(argv: list[str] | None = None) -> int:
    """`python -m aiwatcher_sdk.serving.fetch --uri … --digest … --into …`."""
    parser = argparse.ArgumentParser(
        prog="aiwatcher-fetch",
        description="Place one pinned, digest-verified artifact on a path.",
    )
    parser.add_argument("--uri", required=True, help="https:// or file:// address of the artifact")
    parser.add_argument("--digest", required=True, help="expected sha256, lowercase hex")
    parser.add_argument("--into", required=True, help="destination path")
    parser.add_argument("--max-bytes", type=int, default=DEFAULT_MAX_BYTES)
    parser.add_argument("--timeout", type=float, default=120.0)
    args = parser.parse_args(argv)
    try:
        result = fetch_verified(
            args.uri,
            digest=args.digest,
            into=args.into,
            max_bytes=args.max_bytes,
            timeout=args.timeout,
        )
    except LoadError as error:
        print(str(error), file=sys.stderr)
        return 1
    verb = "fetched" if result.downloaded else "already present"
    print(f"{result.path}: {verb}, {result.bytes_written} bytes, sha256 {result.digest}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
