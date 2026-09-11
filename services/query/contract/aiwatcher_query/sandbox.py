"""Where a query runs: a child of a fork server that imported the engine and nothing else.

Three choices, each measured or forced (AW-3's decisions):

* **A fork server, not a fresh interpreter.** A fresh interpreter per query pays the
  engine's import every time — 464 ms for DataFusion, 47 ms for DuckDB — which is the
  cost the benchmark's `tmp_file.py` row showed. The fork server pays it once.
* **The fork server never runs a query.** A child forked after `import datafusion`
  answers; one forked after the parent *ran* a DataFusion query panics in Tokio
  (`failed to wake I/O driver`), because a fork copies one thread of a live runtime. So
  the engine is preloaded and nothing more, and `strict` queries run in a child too.
* **No credentials cross.** The service reads its configuration, removes `AIWATCHER_*`
  from its own environment, and only then starts the fork server — so the fork server's
  environment, the one every child inherits, never held them. On Linux the service
  also marks itself non-dumpable, which is what stops a child reading
  `/proc/<service>/environ`: that file is the environment the process *started* with,
  whatever was removed from it since. macOS has no `/proc`; a process of the same user
  can still read another's initial environment there through `sysctl`, which is one of
  the reasons `open` admission is a localhost posture.

The parent waits on the clock as well as the child's CPU limit, because a query blocked
on the network spends no CPU. A ceiling is a 422 naming it: the same query reaches the
same ceiling on every retry.
"""

from __future__ import annotations

import ctypes
import multiprocessing
import multiprocessing.forkserver
import os
import shutil
import signal
import sys
import tempfile

from aiwatcher_query import child
from aiwatcher_query.child import Limits
from aiwatcher_query.engine import engine_module
from aiwatcher_query.errors import Failure
from aiwatcher_query.evaluate import Job, Outcome
from aiwatcher_query.log import logger

log = logger(__name__)

#: How long past the CPU ceiling the clock lets a child run before killing it.
GRACE_SECONDS = 2.0
#: How long a killed child gets to be reaped.
REAP_SECONDS = 5.0
_PR_SET_DUMPABLE = 4


class Sandbox:
    """Runs one query per child. `run` blocks, so the service calls it from a thread."""

    def __init__(self, engine: str, limits: Limits) -> None:
        self._engine = engine
        self._limits = limits
        self._context = multiprocessing.get_context("forkserver")

    @property
    def limits(self) -> Limits:
        return self._limits

    def start(self) -> None:
        """Start the fork server, after the environment it will inherit has been cleaned."""
        child.scrub(os.environ)
        undumpable()
        self._context.set_forkserver_preload([engine_module(self._engine), child.__name__])
        multiprocessing.forkserver.ensure_running()

    def run(self, job: Job) -> Outcome | Failure:
        receiver, sender = self._context.Pipe(duplex=False)
        scratch = tempfile.mkdtemp(prefix="aiwatcher-query-")
        process = self._context.Process(
            target=child.main, args=(sender, job, self._limits, scratch), daemon=True
        )
        stopped = False
        try:
            process.start()
            sender.close()
            answer: Outcome | Failure | None = None
            ended = receiver.poll(self._limits.cpu_seconds + GRACE_SECONDS)
            if ended:
                try:
                    answer = receiver.recv()
                except EOFError:
                    # The child closed its end without answering: it is exiting, and its
                    # status says why. Killing it here, because it has not quite gone yet,
                    # reported a crash as the clock running out.
                    answer = None
            if not ended and process.is_alive():
                process.kill()
                stopped = True
            process.join(REAP_SECONDS)
            return answer if answer is not None else self._death(process.exitcode, stopped)
        finally:
            receiver.close()
            shutil.rmtree(scratch, ignore_errors=True)

    def _death(self, exitcode: int | None, stopped: bool) -> Failure:
        """What to say about a child that ended without answering."""
        seconds = self._limits.cpu_seconds
        if stopped:
            return Failure(
                422,
                f"The query ran {seconds + GRACE_SECONDS:g} seconds without answering and was "
                f"stopped; its ceiling is {seconds} (AIWATCHER_QUERY_TIMEOUT_SECONDS).",
            )
        if exitcode == -signal.SIGXCPU:
            return Failure(
                422,
                f"The query used {seconds} seconds of CPU, its ceiling "
                "(AIWATCHER_QUERY_TIMEOUT_SECONDS), and was stopped. CPU time counts every "
                "thread the engine runs, so a parallel query reaches it before the clock does.",
            )
        if exitcode == -signal.SIGKILL:
            return Failure(
                422,
                "The query's process was killed, most likely at its memory ceiling "
                "(AIWATCHER_QUERY_MEMORY_MB, or the container's limit).",
            )
        if exitcode is not None and exitcode < 0:
            return Failure(
                502, f"The query's process ended with signal {-exitcode} without answering."
            )
        return Failure(502, f"The query's process ended with status {exitcode} without answering.")


def undumpable() -> None:
    """On Linux, make this process's `/proc` entries unreadable to its children."""
    if sys.platform != "linux":
        return
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.prctl(_PR_SET_DUMPABLE, 0, 0, 0, 0) != 0:
        log.warning("sandbox.dumpable", errno=ctypes.get_errno())
