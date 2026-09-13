"""Stopping a managed notebook run by its key.

A run is a subprocess — see `runner` for why — and the reactor that sent it stops
waiting when its run is cancelled or its step's deadline passes. Without a way to be
told, the notebook went on to its own timeout, holding its memory and whatever model it
loaded for an execution nobody wanted any more.

So a run registers its process under its context, and a cancel kills the process
*group*: a notebook that started a pool of workers took them into its session with it.
A request that arrives before the process exists is kept, and the run never starts.
"""

from __future__ import annotations

import os
import signal
import subprocess
import threading
import time

#: How long a request to stop a key waits for a run that has not started yet.
TTL_SECONDS = 900.0


class NotebookCancelledError(RuntimeError):
    """A run somebody stopped, rather than one that failed."""


class Stopping:
    """The runs this process can stop, by their context."""

    def __init__(self) -> None:
        # Written by the event loop's cancel and read by each run's worker thread.
        self._lock = threading.Lock()
        self._running: dict[str, subprocess.Popen[str]] = {}
        self._requested: dict[str, float] = {}

    def request(self, key: str) -> None:
        """Stop the run under `key`, or the one about to start under it."""
        now = time.monotonic()
        with self._lock:
            for stale in [k for k, at in self._requested.items() if now - at > TTL_SECONDS]:
                del self._requested[stale]
            self._requested[key] = now
            process = self._running.get(key)
        if process is not None:
            kill(process)

    def raise_if_requested(self, key: str | None) -> None:
        """Before anything starts: a stop that came first means nothing runs."""
        if key is not None and self.requested(key):
            self.forget(key)
            raise NotebookCancelledError("the notebook run was cancelled before it started")

    def attach(self, key: str | None, process: subprocess.Popen[str]) -> None:
        if key is None:
            return
        with self._lock:
            self._running[key] = process
            requested = key in self._requested
        # A request that arrived between the check before starting and now found nothing.
        if requested:
            kill(process)

    def detach(self, key: str | None) -> bool:
        """Forget the run, and say whether it was stopped."""
        if key is None:
            return False
        with self._lock:
            self._running.pop(key, None)
            return self._requested.pop(key, None) is not None

    def requested(self, key: str) -> bool:
        with self._lock:
            return key in self._requested

    def forget(self, key: str) -> None:
        with self._lock:
            self._requested.pop(key, None)


def kill(process: subprocess.Popen[str]) -> None:
    """The whole group the run started in, and the run itself if the group is gone."""
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError, PermissionError:
        process.kill()
