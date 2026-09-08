"""What this service remembers about a managed step: that it ran. Never the rows.

Section 15.4 of `docs/PIPELINE_ARCHITECTURE.md`, arriving at the second runtime.
A reactor's timeout says the caller stopped waiting and nothing about whether
this service stopped working, so before it runs the same key again it asks.
Three answers, and each sends the reactor somewhere different:

    running  the notebook is still executing here — wait, do not send it again
    done     it finished, and this is the notebook revision it ran
    absent   no record of that key — running it again is safe

The rows are not here. They go to the artifact the *reactor* uploads, and what
stays is a note; `done` answers "it ran", and "here is what it produced" is a
different question answered by the object store the reactor wrote to.

## Why a dict and not files

The query service keeps its notes in files because `php -S` forks workers and
PHP-FPM forks children, so a static array would answer `absent` to whichever
worker took the lookup. This service is one uvicorn process, so the equivalent
of that reasoning is a dict — and every write happens on the event loop, before
the run is handed to a worker thread and after it comes back, so there is no
lock here and nothing to get wrong about one.

A restart forgets everything, which reads as `absent`, which is the safe
answer: the reactor retries, and a deterministic notebook over the same rows
produces the same output.
"""

from __future__ import annotations

import time
from dataclasses import dataclass
from typing import Any

RUNNING = "running"
DONE = "done"
ABSENT = "absent"

#: How long a note is worth reading.
#:
#: The reactor's lookup window: it asks once, immediately after a timeout it did
#: not expect. Minutes rather than hours because a note that outlived the
#: attempt it describes would answer for the *next* attempt at the same key —
#: and there is no next attempt at the same key, because the attempt number is
#: in it.
TTL_SECONDS = 900


@dataclass(frozen=True)
class Note:
    state: str
    at: float
    revision: str | None = None
    rows: int | None = None


class ExecutionMemory:
    """Notes about keys this process is running or has run."""

    def __init__(self, ttl_seconds: int = TTL_SECONDS) -> None:
        self._ttl = ttl_seconds
        self._notes: dict[str, Note] = {}

    def started(self, key: str) -> None:
        self._expire()
        self._notes[key] = Note(state=RUNNING, at=time.monotonic())

    def finished(self, key: str, revision: str, rows: int) -> None:
        self._notes[key] = Note(state=DONE, at=time.monotonic(), revision=revision, rows=rows)

    def failed(self, key: str) -> None:
        """Forget a run that ended badly.

        A failed notebook must not leave a `running` note behind: the reactor
        would wait for something that stopped, and only the lease expiring would
        free it. `absent` is the honest answer — nothing here ran to completion,
        so running it again is safe.
        """
        self._notes.pop(key, None)

    def seen(self, key: str) -> dict[str, Any]:
        """What this service knows about that key."""
        note = self._notes.get(key)
        if note is None:
            return {"state": ABSENT}
        if self._stale(note):
            del self._notes[key]
            return {"state": ABSENT}

        answer: dict[str, Any] = {"state": note.state}
        if note.revision is not None:
            # Which notebook produced it, so the caller can tell this attempt's
            # answer from an earlier one at the same key under a different
            # revision. It is compared against what the *receipt* recorded, never
            # against a digest of the rows: those are two encodings by two
            # languages and making them agree byte for byte is a contract nobody
            # could keep.
            answer["revision"] = note.revision
        if note.rows is not None:
            answer["rows"] = note.rows
        return answer

    def _stale(self, note: Note) -> bool:
        return (time.monotonic() - note.at) > self._ttl

    def _expire(self) -> None:
        """Drop what nobody will read again.

        On write rather than on a timer: this dict lives as long as the process
        and a note nobody looks up would otherwise stay in it forever, which is
        the one way an in-process store is worse than the file the query service
        writes.
        """
        for key in [key for key, note in self._notes.items() if self._stale(note)]:
            del self._notes[key]
