"""Installing a pinned llama.cpp build: what it refuses, and what it skips.

Every case runs over a `file://` "release page", the same seam
`test_serving_fetch.py` uses and for the same reason: what is under test is the
decision — is this the pinned asset, is it already unpacked, is this member
allowed to be written — and not the transport.
"""

from __future__ import annotations

import hashlib
import os
import shutil
import zipfile
from pathlib import Path

import pytest

from aiwatcher_sdk.serving.artifact import LoadError
from aiwatcher_sdk.serving.llama_cpp import Build, host_platform, install_llama_server, main

RELEASE = "b9999"


def publish(releases: Path, *, names: tuple[str, ...] = ("build/bin/llama-server",)) -> Build:
    """Write an asset under a file:// release layout and pin it."""
    asset = f"llama-{RELEASE}-bin-test-x64.zip"
    directory = releases / RELEASE
    directory.mkdir(parents=True, exist_ok=True)
    path = directory / asset
    with zipfile.ZipFile(path, "w") as archive:
        for name in names:
            archive.writestr(name, "#!/bin/sh\necho llama\n")
    return Build(
        release=RELEASE,
        asset=asset,
        sha256=hashlib.sha256(path.read_bytes()).hexdigest(),
        base=releases.as_uri(),
    )


def test_a_pinned_build_is_unpacked_and_made_executable(tmp_path: Path) -> None:
    build = publish(tmp_path / "releases")

    binary = install_llama_server(build, into=tmp_path / "bin")

    assert binary == tmp_path / "bin" / RELEASE / "build" / "bin" / "llama-server"
    assert os.access(binary, os.X_OK)


def test_a_release_already_unpacked_is_not_fetched_again(tmp_path: Path) -> None:
    releases = tmp_path / "releases"
    build = publish(releases)
    first = install_llama_server(build, into=tmp_path / "bin")

    # Take the release page away entirely. A second call that reached for it
    # would fail; one that reads the unpacked tree does not.
    shutil.rmtree(releases)

    assert install_llama_server(build, into=tmp_path / "bin") == first


def test_bytes_that_do_not_match_the_pin_are_never_unpacked(tmp_path: Path) -> None:
    build = publish(tmp_path / "releases")
    wrong = Build(release=build.release, asset=build.asset, sha256="0" * 64, base=build.base)

    with pytest.raises(LoadError, match="hashes to"):
        install_llama_server(wrong, into=tmp_path / "bin")

    assert not (tmp_path / "bin" / RELEASE).exists()


def test_a_member_that_unpacks_outside_the_destination_is_refused(tmp_path: Path) -> None:
    build = publish(tmp_path / "releases", names=("../escaped", "build/bin/llama-server"))

    with pytest.raises(LoadError, match="unpacks outside"):
        install_llama_server(build, into=tmp_path / "bin")

    assert not (tmp_path / "escaped").exists()


def test_an_asset_without_a_server_binary_is_named_as_the_wrong_pin(tmp_path: Path) -> None:
    build = publish(tmp_path / "releases", names=("build/bin/llama-cli",))

    with pytest.raises(LoadError, match="not a server build"):
        install_llama_server(build, into=tmp_path / "bin")


def test_an_archive_this_module_does_not_open_says_so(tmp_path: Path) -> None:
    releases = tmp_path / "releases" / RELEASE
    releases.mkdir(parents=True)
    asset = releases / f"llama-{RELEASE}-bin-test-x64.7z"
    asset.write_bytes(b"not an archive we open")
    build = Build(
        release=RELEASE,
        asset=asset.name,
        sha256=hashlib.sha256(asset.read_bytes()).hexdigest(),
        base=(tmp_path / "releases").as_uri(),
    )

    with pytest.raises(LoadError, match=r"neither a \.zip nor a \.tar\.gz"):
        install_llama_server(build, into=tmp_path / "bin")


def test_the_host_maps_to_an_upstream_asset_name_or_says_it_does_not(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr("platform.system", lambda: "Linux")
    monkeypatch.setattr("platform.machine", lambda: "x86_64")
    assert host_platform() == "ubuntu-x64"

    monkeypatch.setattr("platform.machine", lambda: "riscv64")
    with pytest.raises(LoadError, match="no binary this module knows"):
        host_platform()


def test_the_command_line_prints_the_binary_it_installed(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    build = publish(tmp_path / "releases")

    code = main(
        [
            "--release",
            build.release,
            "--asset",
            build.asset,
            "--sha256",
            build.sha256,
            "--into",
            str(tmp_path / "bin"),
            "--base",
            build.base,
        ]
    )

    assert code == 0
    assert capsys.readouterr().out.strip().endswith("llama-server")
