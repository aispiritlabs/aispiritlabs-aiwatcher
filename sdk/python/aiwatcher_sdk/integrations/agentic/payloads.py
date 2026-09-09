"""Where a hosted decider's words live, and how a reference to them is made.

Section 40.4 of the architecture, from the worker's side. Every hop in an agent
graph carries text — a prompt, a completion, a tool result — and that is
conversation content. aiwatcher's workflow stream carries a **reference**, a
plaintext digest and a size, never the words; under the default `external`
policy the words stay wherever the worker keeps them, which is what this module
is about.

The port is one thing rather than two: a store that can hand a payload back is
the only kind worth writing one to, and the digest is what joins the two halves.

## Why a digest and not a name

Two turns that said the same thing store one payload, a repeated append writes
the same bytes to the same place, and a reference that came back from the
server can be checked against what was fetched. It is the prompt registry's
rule, one store along, and the reason `AiwatcherEventStore` refuses a payload
whose digest does not match on the way out: the alternative is a graph replayed
from somebody else's words with nothing to say so.
"""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
from typing import Any, Protocol

__all__ = [
    "FilePayloadStore",
    "MemoryPayloadStore",
    "PayloadStore",
    "digest_of",
    "encode_payload",
]


def encode_payload(data: Any) -> bytes:
    """One payload, as the bytes its digest is taken over.

    Sorted keys and no incidental whitespace, so two processes that built the
    same payload different ways still address it the same. `default=str` is the
    concession: a decider's payload may hold a datetime or an enum, and a
    `TypeError` at the moment of storing somebody's turn is worse than a stable
    textual form of it.
    """
    return json.dumps(data, sort_keys=True, separators=(",", ":"), default=str).encode()


def digest_of(data: Any) -> str:
    """`sha256` of :func:`encode_payload`, hex — the address a payload is known by."""
    return hashlib.sha256(encode_payload(data)).hexdigest()


class PayloadStore(Protocol):
    """Where the words go, when aiwatcher is only holding the reference.

    A protocol rather than a base class, so `agentic`'s own SQLite, an object
    store or a test double all satisfy it without inheriting anything — the
    rule `ImageSource` already sets in `aiwatcher_sdk.annotations`.
    """

    def store_payload(self, digest: str, data: Any) -> str:
        """Write one payload and answer the reference it is fetched by.

        Called with the digest already computed, because the caller needs it for
        the message either way and hashing twice is the kind of duplication that
        later disagrees with itself.
        """
        ...

    def get_payload(self, reference: str) -> Any:
        """The payload behind a reference.

        Raises `KeyError` when there is none — a reference this store never
        wrote, or one whose bytes have since been removed by a retention sweep
        the store keeps and aiwatcher does not.
        """
        ...


class MemoryPayloadStore:
    """A payload store that lives as long as the process does.

    For tests, and for a decider that genuinely has nowhere durable to put
    anything. **Not a default**: a worker restarting is the case Phase 13
    exists for, and one that came back to references it can no longer resolve
    would have kept a history it cannot replay. `AiwatcherEventStore` takes the
    store as an argument for exactly that reason — where the words live is a
    decision, not something to fall into.
    """

    def __init__(self) -> None:
        self._payloads: dict[str, Any] = {}

    def store_payload(self, digest: str, data: Any) -> str:
        self._payloads[digest] = data
        return f"memory://{digest}"

    def get_payload(self, reference: str) -> Any:
        digest = reference.removeprefix("memory://")
        return self._payloads[digest]


class FilePayloadStore:
    """One directory, one file per payload, named by its digest.

    Enough to survive the restart the whole phase is about, with no dependency
    and no schema. Writes go to a temporary name and are renamed into place, so
    a reader never opens a half-written payload — and because the name is a
    content address, a repeated write is byte-identical and the rename is free
    to happen twice.
    """

    def __init__(self, root: str | os.PathLike[str]) -> None:
        self.root = Path(root)
        self.root.mkdir(parents=True, exist_ok=True)

    def store_payload(self, digest: str, data: Any) -> str:
        target = self._path(digest)
        if not target.exists():
            scratch = target.with_suffix(".tmp")
            scratch.write_bytes(encode_payload(data))
            scratch.replace(target)
        return f"file://{target}"

    def get_payload(self, reference: str) -> Any:
        digest = reference.rsplit("/", 1)[-1].removesuffix(".json")
        target = self._path(digest)
        try:
            return json.loads(target.read_bytes())
        except FileNotFoundError as error:
            raise KeyError(reference) from error

    def _path(self, digest: str) -> Path:
        return self.root / f"{digest}.json"
