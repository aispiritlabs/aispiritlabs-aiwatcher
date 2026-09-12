"""Put a pinned ``llama-server`` binary on disk, for the machine under the desk.

:mod:`~aiwatcher_sdk.serving.fetch` places the weights; this places the program
that serves them. Together they are the local half of the chart's ``chatModel``
component — the same model, the same digest discipline, on a laptop instead of
a node, so a developer can reproduce what the cluster runs without Docker.

**The release is declared, never discovered.** The obvious implementation asks
the GitHub API for the latest release, takes whatever asset matches the host,
and runs it. That is three decisions made by a remote server at install time,
and the one it gets to make last is which binary executes. A :class:`Build`
names the release, the asset and its SHA256, so this module makes one request —
for bytes it already knows the hash of — and the API is not in the path at all.

**Nothing unverified is extracted.** An archive that does not match its pin is
never opened, and a member whose path escapes the destination is refused rather
than written. Both are the same rule the weights get: what makes bytes
acceptable is the digest, not where they arrived from.

Pinning costs something real — a new llama.cpp release is a values change and a
new digest, not a re-run — and that is the trade this module takes on purpose.
An unpinned local toolchain is the one that stops matching the cluster's, which
is the whole reason to have it.
"""

from __future__ import annotations

import argparse
import platform
import stat
import sys
import tarfile
import zipfile
from dataclasses import dataclass
from pathlib import Path

from aiwatcher_sdk.serving.artifact import LoadError
from aiwatcher_sdk.serving.fetch import fetch_verified

__all__ = ["Build", "host_platform", "install_llama_server", "main"]

#: The *platform* part of llama.cpp's asset names, keyed by what this machine
#: calls itself. The mapping exists so a caller can look up the host without
#: also learning how `platform.machine()` spells a Mac. It is half a file name
#: and never the whole one — see :class:`Build`.
_HOSTS: dict[tuple[str, str], str] = {
    ("darwin", "arm64"): "macos-arm64",
    ("darwin", "x86_64"): "macos-x64",
    ("linux", "x86_64"): "ubuntu-x64",
    ("linux", "aarch64"): "ubuntu-arm64",
    ("linux", "arm64"): "ubuntu-arm64",
    ("windows", "amd64"): "win-cpu-x64",
    ("windows", "arm64"): "win-cpu-arm64",
}

_RELEASES = "https://github.com/ggml-org/llama.cpp/releases/download"

#: The archive is tens of megabytes. A cap two orders above that is a wrong
#: address, not a big build.
_MAX_BYTES = 2 * 1024 * 1024 * 1024


@dataclass(frozen=True, slots=True)
class Build:
    """One llama.cpp release asset, pinned the way a model artifact is.

    `asset` is upstream's file name — ``llama-b1234-bin-macos-arm64.zip`` — and
    carries the platform inside it. It is named rather than derived because the
    digest below is only true of one file, and a name this module assembled
    would be a guess the digest would then contradict.
    """

    release: str
    asset: str
    sha256: str
    #: Where releases are published. Overridden for an internal mirror, and for
    #: an air-gapped host where the whole point is that github.com is not
    #: reachable — the digest is what makes the bytes acceptable either way, so
    #: a mirror is a different address rather than a weaker check.
    base: str = _RELEASES

    @property
    def url(self) -> str:
        return f"{self.base.rstrip('/')}/{self.release}/{self.asset}"


def host_platform() -> str:
    """What this machine needs, in upstream's asset spelling.

    Reading the host's own OS and architecture is not sniffing a runtime
    (ADR_0023): it is a fact about the machine, and the thing that *is* declared
    — which build to install — stays with the caller.
    """
    system = platform.system().lower()
    machine = platform.machine().lower()
    name = _HOSTS.get((system, machine))
    if name is None:
        raise LoadError(
            f"llama.cpp publishes no binary this module knows for {system}/{machine}. "
            f"Known hosts: {', '.join(sorted(set(_HOSTS.values())))}"
        )
    return name


def install_llama_server(build: Build, *, into: Path | str, timeout: float = 300.0) -> Path:
    """Fetch and unpack `build` under `into`, and return the ``llama-server`` path.

    Restart-safe in both halves: an archive already on disk with the right
    digest is not fetched again, and a release already unpacked is not unpacked
    again.
    """
    root = Path(into) / build.release
    binary = _find_server(root)
    if binary is not None:
        return binary

    archive = Path(into) / build.asset
    fetch_verified(
        build.url,
        digest=build.sha256,
        into=archive,
        max_bytes=_MAX_BYTES,
        timeout=timeout,
    )
    _unpack(archive, root)
    binary = _find_server(root)
    if binary is None:
        raise LoadError(
            f"{build.asset} unpacked without a llama-server binary in it. The pin names an "
            "asset that is not a server build"
        )
    binary.chmod(binary.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    return binary


def _find_server(root: Path) -> Path | None:
    if not root.is_dir():
        return None
    for name in ("llama-server", "llama-server.exe"):
        for candidate in root.rglob(name):
            if candidate.is_file():
                return candidate
    return None


def _unpack(archive: Path, root: Path) -> None:
    """Extract into `root`, refusing any member that would land outside it."""
    root.mkdir(parents=True, exist_ok=True)
    resolved = root.resolve()
    if archive.name.endswith((".tar.gz", ".tgz")):
        with tarfile.open(archive, "r:gz") as tar:
            # `data` already refuses absolute paths, `..` and links out of the
            # tree. The check below is what covers the zip, which has no filter.
            tar.extractall(root, filter="data")
        return
    if not archive.name.endswith(".zip"):
        raise LoadError(f"{archive.name} is neither a .zip nor a .tar.gz, and is not unpacked")
    with zipfile.ZipFile(archive) as zipped:
        for member in zipped.infolist():
            target = (resolved / member.filename).resolve()
            if not target.is_relative_to(resolved):
                raise LoadError(
                    f"{archive.name} contains {member.filename!r}, which unpacks outside "
                    f"{root}. A verified archive is still an archive somebody wrote"
                )
        zipped.extractall(root)  # noqa: S202 - every member was checked above


def _host_hint() -> str:
    """Name this machine's half of the asset name, for a reader of ``--help``."""
    try:
        return f"This host takes the {host_platform()} asset of a release."
    except LoadError as error:
        return str(error)


def main(argv: list[str] | None = None) -> int:
    """``python -m aiwatcher_sdk.serving.llama_cpp --release … --asset … --sha256 …``.

    ``--asset`` is required, and deriving it from the host would be the obvious
    convenience to add back. It was there, and it was wrong: it assembled
    ``llama-<release>-bin-<host>.zip`` while upstream ships ``.tar.gz`` for
    every host in :data:`_HOSTS` except Windows, and had renamed the Windows
    ones besides. Nobody noticed, because a caller who has the digest has
    already read the release page and passes the name it gave.

    That is the rule from :class:`Build` applied one level up: a digest is true
    of exactly one file, so a name this module assembled could only ever turn a
    typo upstream made into a checksum mismatch here — which reads as corrupt
    bytes rather than as the wrong file.
    """
    parser = argparse.ArgumentParser(
        prog="aiwatcher-llama-cpp",
        description="Install one pinned llama.cpp server build, verified by digest.",
        epilog=_host_hint(),
    )
    parser.add_argument("--release", required=True, help="upstream release tag, e.g. b1234")
    parser.add_argument(
        "--asset",
        required=True,
        help="asset file name, exactly as the release page spells it",
    )
    parser.add_argument("--sha256", required=True, help="expected sha256 of the asset")
    parser.add_argument("--into", required=True, help="directory to unpack under")
    parser.add_argument("--base", default=_RELEASES, help="release download base, for a mirror")
    parser.add_argument("--timeout", type=float, default=300.0)
    args = parser.parse_args(argv)
    try:
        binary = install_llama_server(
            Build(release=args.release, asset=args.asset, sha256=args.sha256, base=args.base),
            into=args.into,
            timeout=args.timeout,
        )
    except LoadError as error:
        print(str(error), file=sys.stderr)
        return 1
    print(binary)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
