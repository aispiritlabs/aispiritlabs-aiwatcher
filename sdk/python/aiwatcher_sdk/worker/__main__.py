"""Only local, explicitly imported modules may register code for this worker."""

import argparse
import importlib
import os
import signal
from collections.abc import Sequence
from types import FrameType
from typing import Any

from aiwatcher_sdk.worker import Task, Worker


def main(argv: Sequence[str] | None = None) -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", nargs="?", choices=["serve", "run-attempt"], default="serve")
    parser.add_argument(
        "--ref",
        help="execution/step/attempt for run-attempt; AIWATCHER_ATTEMPT when omitted, "
        "which is how a pod aiwatcher started is told its attempt",
    )
    parser.add_argument("--url", default=os.environ.get("AIWATCHER_URL", "http://localhost:8080"))
    parser.add_argument("--queue", action="append", required=True)
    parser.add_argument(
        "--task",
        action="append",
        required=True,
        help="explicit task export module:function; repeatable",
    )
    args = parser.parse_args(argv)
    # A launched pod's command is its template's, the same for every attempt,
    # so the attempt arrives in the environment rather than on the line.
    ref = args.ref
    if args.command == "run-attempt" and ref is None:
        ref = os.environ.get("AIWATCHER_ATTEMPT") or None
    if (args.command == "run-attempt") != (ref is not None):
        parser.error("run-attempt requires --ref or AIWATCHER_ATTEMPT; serve does not accept --ref")
    tasks: list[Task[Any, Any]] = []
    for reference in args.task:
        module, separator, name = reference.partition(":")
        if not separator or not module or not name:
            parser.error("--task must be module:function")
        entry = getattr(importlib.import_module(module), name)
        if not isinstance(entry, Task):
            parser.error(f"{reference} is not an @task declaration")
        tasks.append(entry)
    # The pod's own name, when aiwatcher started it: the name its claim is held
    # under, and the one a person finds in `kubectl`.
    name = os.environ.get("AIWATCHER_WORKER_NAME") or None
    with Worker(args.url, queues=args.queue, tasks=tasks, name=name) as worker:

        def stop(signum: int, frame: FrameType | None) -> None:
            worker.stop()

        previous = {sig: signal.signal(sig, stop) for sig in (signal.SIGINT, signal.SIGTERM)}
        try:
            if ref is not None:
                if not worker.run_attempt(ref):
                    raise SystemExit(1)
            else:
                worker.run()
        finally:
            for sig, handler in previous.items():
                signal.signal(sig, handler)


if __name__ == "__main__":
    main()
