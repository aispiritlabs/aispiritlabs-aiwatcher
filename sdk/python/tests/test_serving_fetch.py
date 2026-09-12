"""Placing a pinned artifact on a path: what it refuses, and what it skips.

Every case here runs over `file://`, because what is being tested is the
decision — is this the pinned artifact, is the destination already it, is what
landed allowed to replace what was there — and not the transport. The https
half is one `urlopen` behind the same decision.
"""

from __future__ import annotations

import hashlib
from pathlib import Path

import pytest

from aiwatcher_sdk.serving import LoadError, fetch_verified
from aiwatcher_sdk.serving.fetch import main

BODY = b"gguf bytes" * 4096
DIGEST = hashlib.sha256(BODY).hexdigest()


@pytest.fixture
def source(tmp_path: Path) -> Path:
    path = tmp_path / "source" / "model.gguf"
    path.parent.mkdir()
    path.write_bytes(BODY)
    return path


def uri(path: Path) -> str:
    return path.as_uri()


def test_a_pinned_artifact_lands_on_the_destination(source: Path, tmp_path: Path) -> None:
    into = tmp_path / "models" / "animica.gguf"

    result = fetch_verified(uri(source), digest=DIGEST, into=into)

    assert result.downloaded is True
    assert result.path == into
    assert result.bytes_written == len(BODY)
    assert into.read_bytes() == BODY


def test_a_destination_that_already_holds_these_bytes_is_not_fetched_again(
    source: Path, tmp_path: Path
) -> None:
    """A restarting container must not re-pull a gigabyte it already has."""
    into = tmp_path / "models" / "animica.gguf"
    fetch_verified(uri(source), digest=DIGEST, into=into)
    source.unlink()  # nothing may be read on the second call

    result = fetch_verified(uri(source), digest=DIGEST, into=into)

    assert result.downloaded is False
    assert result.bytes_written == len(BODY)
    assert into.read_bytes() == BODY


def test_a_destination_holding_other_bytes_is_replaced(source: Path, tmp_path: Path) -> None:
    into = tmp_path / "models" / "animica.gguf"
    into.parent.mkdir()
    into.write_bytes(b"an older pin")

    assert fetch_verified(uri(source), digest=DIGEST, into=into).downloaded is True
    assert into.read_bytes() == BODY


def test_bytes_that_do_not_match_the_pin_never_reach_the_destination(
    source: Path, tmp_path: Path
) -> None:
    """A fetch that hashes wrong must not be what replaces the file already there.

    The destination holds an older artifact, so the fetch does run — the
    short-circuit only covers a destination that already is the pin — and what
    it pulled has to be refused before anything is renamed onto it.
    """
    into = tmp_path / "models" / "animica.gguf"
    into.parent.mkdir()
    into.write_bytes(b"an older pin")
    source.write_bytes(b"neither of the two")

    with pytest.raises(LoadError) as raised:
        fetch_verified(uri(source), digest=DIGEST, into=into)

    assert DIGEST in str(raised.value)
    assert into.read_bytes() == b"an older pin"
    assert list(into.parent.glob("*.partial")) == []


def test_a_fetch_larger_than_its_bound_is_refused_and_leaves_nothing(
    source: Path, tmp_path: Path
) -> None:
    into = tmp_path / "models" / "animica.gguf"

    with pytest.raises(LoadError, match="larger than"):
        fetch_verified(uri(source), digest=DIGEST, into=into, max_bytes=len(BODY) - 1)

    assert not into.exists()
    assert list(into.parent.glob("*.partial")) == []


@pytest.mark.parametrize(
    ("digest", "message"),
    [("", "not a sha256"), ("nope", "not a sha256"), ("a" * 63, "not a sha256")],
)
def test_a_fetch_with_nothing_to_check_against_is_refused(
    source: Path, tmp_path: Path, digest: str, message: str
) -> None:
    with pytest.raises(LoadError, match=message):
        fetch_verified(uri(source), digest=digest, into=tmp_path / "animica.gguf")


def test_a_scheme_this_fetcher_does_not_read_is_named_rather_than_attempted(
    tmp_path: Path,
) -> None:
    with pytest.raises(LoadError, match="https://"):
        fetch_verified("s3://bucket/model.gguf", digest=DIGEST, into=tmp_path / "m.gguf")


def test_the_digest_is_compared_case_insensitively(source: Path, tmp_path: Path) -> None:
    into = tmp_path / "animica.gguf"
    assert fetch_verified(uri(source), digest=DIGEST.upper(), into=into).digest == DIGEST


def test_the_command_reports_the_outcome_and_fails_on_a_bad_pin(
    source: Path, tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    into = tmp_path / "animica.gguf"
    arguments = ["--uri", uri(source), "--digest", DIGEST, "--into", str(into)]

    assert main(arguments) == 0
    assert "fetched" in capsys.readouterr().out
    assert main(arguments) == 0
    assert "already present" in capsys.readouterr().out

    assert main([*arguments[:3], "b" * 64, *arguments[4:]]) == 1
    assert "not the bytes" in capsys.readouterr().err
