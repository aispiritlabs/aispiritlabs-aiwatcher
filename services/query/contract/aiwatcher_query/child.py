"""The child a query runs in, and what it does before the query sees it.

Forked by the fork server, which imported the engine and nothing else (`sandbox`). In
order, before a line of the query runs:

1. `AIWATCHER_*` leaves the environment. The fork server was started after the service
   removed it from its own, so this is the second of two, and the one a query reading
   `os.environ` would otherwise meet.
2. The working directory is a fresh scratch directory the service made for this query
   and removes after it, so a relative path the query writes lands there.
3. `RLIMIT_CPU` is the timeout, as CPU seconds across every thread the engine runs — so
   a parallel query reaches it sooner than the clock does — and on Linux `RLIMIT_AS`
   is the memory ceiling. macOS does not enforce the second, and the service's own
   wall-clock is the backstop there.

Then the query runs, and what crosses back is an `Outcome` or a `Failure`: plain
records, because an exception with its own constructor does not survive pickling.
"""

from __future__ import annotations

import os
import resource
import sys
from collections.abc import MutableMapping
from dataclasses import dataclass
from multiprocessing.connection import Connection

import httpx

from aiwatcher_query.engine import load_engine
from aiwatcher_query.errors import Failure
from aiwatcher_query.evaluate import Job, Outcome, execute

#: One request to aiwatcher. The query's own ceiling is the one that matters; this only
#: stops a single page that never arrives from holding the child until the clock does.
READ_TIMEOUT_SECONDS = 60.0


@dataclass(frozen=True)
class Limits:
    """The ceilings a child runs under, from `AIWATCHER_QUERY_TIMEOUT_SECONDS` and
    `AIWATCHER_QUERY_MEMORY_MB`."""

    cpu_seconds: int
    memory_mb: int


def main(connection: Connection, job: Job, limits: Limits, scratch: str) -> None:
    scrub(os.environ)
    os.chdir(scratch)
    confine(limits)
    answer: Outcome | Failure
    try:
        engine = load_engine(job.engine)
        with api_client() as client:
            answer = execute(job, engine, engine.open(job.corpus), client)
    except MemoryError:
        answer = Failure(
            422,
            "The query ran out of memory: its child reached the address-space ceiling "
            "(AIWATCHER_QUERY_MEMORY_MB) or the container's limit.",
        )
    except Exception as error:  # noqa: BLE001 — every failure crosses back as a record
        answer = Failure.of(error)
    connection.send(answer)
    connection.close()


def api_client() -> httpx.Client:
    """The child's client for aiwatcher, which takes nothing from its environment.

    Two reasons, one setting. The environment is where credentials live, and `trust_env`
    would read `~/.netrc` and the proxy variables into a process whose whole design is
    that none crosses. And on macOS, asking for the system's proxies calls
    `SCDynamicStoreCopyProxies`, which is CoreFoundation in a process that was forked and
    never exec'd — Apple's unsupported case. It crashed every child of some fork servers
    with SIGSEGV and none of others, which reads as a flaky engine. aiwatcher's address
    is configuration, and nothing between the query service and it is a proxy.
    """
    return httpx.Client(timeout=READ_TIMEOUT_SECONDS, trust_env=False)


def scrub(environ: MutableMapping[str, str]) -> None:
    """Remove every `AIWATCHER_*` variable: this service's configuration and, beside it,
    whatever credentials the deployment put in the same environment."""
    for name in [name for name in environ if name.startswith("AIWATCHER_")]:
        del environ[name]


def confine(limits: Limits) -> None:
    # The hard limit a second above the soft one: SIGXCPU at the soft limit is what the
    # service reads as the ceiling, and SIGKILL at the hard one is what stops a query
    # that caught it.
    resource.setrlimit(resource.RLIMIT_CPU, (limits.cpu_seconds, limits.cpu_seconds + 1))
    if sys.platform == "linux" and limits.memory_mb > 0:
        ceiling = limits.memory_mb * 1024 * 1024
        resource.setrlimit(resource.RLIMIT_AS, (ceiling, ceiling))
