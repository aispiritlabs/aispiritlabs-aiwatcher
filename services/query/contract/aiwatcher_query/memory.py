"""What this service remembers about a managed step: that it ran, and what the result
hashed to. Never the rows.

A reactor's timeout says the caller stopped waiting and nothing about whether this
service stopped working, so before it runs the same key again it asks by the
idempotency key `<execution>/<step>/<attempt>`:

    running  the query is still executing here — wait, do not send it again
    done     it finished, and this is what it hashed to
    absent   no record of that key — running it again is safe

The rows go to the artifact the *reactor* uploads (ADR_0014), so `done` answers
"it ran" and "here is what it produced" is the object store's question.

A dict rather than Flow's files, for the notebook runtime's reason: this service is
one uvicorn process, and every write happens on the event loop — before a query is
handed to its child and after it comes back — so there is nothing to lock. A restart
forgets everything, which reads as `absent`, which is the safe answer.
"""

from __future__ import annotations

import hashlib
import json
import time
from dataclasses import dataclass
from typing import Any

RUNNING = "running"
DONE = "done"
ABSENT = "absent"

#: The reactor's lookup window: it asks once, immediately after a timeout it did not
#: expect. Minutes rather than hours, because the attempt number is in the key and a
#: note never has to answer for a later attempt.
TTL_SECONDS = 900


@dataclass(frozen=True)
class Note:
    state: str
    at: float
    digest: str | None = None
    rows: int | None = None


class ExecutionMemory:
    """Notes about keys this process is running or has run."""

    def __init__(self, ttl_seconds: int = TTL_SECONDS) -> None:
        self._ttl = ttl_seconds
        self._notes: dict[str, Note] = {}

    def started(self, key: str) -> None:
        """Noted before the query runs: `running` is worth something only while it is."""
        self._expire()
        self._notes[key] = Note(state=RUNNING, at=time.monotonic())

    def finished(self, key: str, digest: str, rows: int) -> None:
        self._notes[key] = Note(state=DONE, at=time.monotonic(), digest=digest, rows=rows)

    def failed(self, key: str) -> None:
        """A failed query leaves `absent`, never a `running` nothing will complete."""
        self._notes.pop(key, None)

    def seen(self, key: str) -> dict[str, Any]:
        note = self._notes.get(key)
        if note is None:
            return {"state": ABSENT}
        if self._stale(note):
            del self._notes[key]
            return {"state": ABSENT}
        answer: dict[str, Any] = {"state": note.state}
        if note.digest is not None:
            answer["digest"] = note.digest
        if note.rows is not None:
            answer["rows"] = note.rows
        return answer

    def _stale(self, note: Note) -> bool:
        return (time.monotonic() - note.at) > self._ttl

    def _expire(self) -> None:
        """On write rather than on a timer: a note nobody looks up again would otherwise
        stay for the life of the process."""
        for key in [key for key, note in self._notes.items() if self._stale(note)]:
            del self._notes[key]


def digest_of(rows: list[dict[str, Any]]) -> str:
    """What a result hashed to, in this service's encoding.

    Compact JSON with non-ASCII escaped and slashes left alone — the shape Flow hashes —
    but a fingerprint for comparing two runs of one query here, never a key into
    anybody's store: two languages encoding row data identically is a contract nobody
    could keep, and the reactor compares this only with the receipt this service wrote.
    """
    encoded = json.dumps(rows, separators=(",", ":"), ensure_ascii=True, allow_nan=False)
    return hashlib.sha256(encoded.encode("ascii")).hexdigest()
