"""Only local, explicitly imported modules may register code for this worker."""

import argparse
import importlib
import os
import signal
from types import FrameType
from typing import Any

from aiwatcher_sdk.worker import Task, Worker


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", nargs="?", choices=["serve", "run-attempt"], default="serve")
    parser.add_argument("--ref", help="execution/step/attempt for run-attempt")
    parser.add_argument("--url", default=os.environ.get("AIWATCHER_URL", "http://localhost:8080"))
    parser.add_argument("--queue", action="append", required=True)
    parser.add_argument(
        "--task",
        action="append",
        required=True,
        help="explicit task export module:function; repeatable",
    )
    args = parser.parse_args()
    if (args.command == "run-attempt") != (args.ref is not None):
        parser.error("run-attempt requires --ref; serve does not accept it")
    tasks: list[Task[Any, Any]] = []
    for reference in args.task:
        module, separator, name = reference.partition(":")
        if not separator or not module or not name:
            parser.error("--task must be module:function")
        entry = getattr(importlib.import_module(module), name)
        if not isinstance(entry, Task):
            parser.error(f"{reference} is not an @task declaration")
        tasks.append(entry)
    with Worker(args.url, queues=args.queue, tasks=tasks) as worker:

        def stop(signum: int, frame: FrameType | None) -> None:
            worker.stop()

        previous = {sig: signal.signal(sig, stop) for sig in (signal.SIGINT, signal.SIGTERM)}
        try:
            if args.command == "run-attempt":
                if not worker.run_attempt(args.ref):
                    raise SystemExit(1)
            else:
                worker.run()
        finally:
            for sig, handler in previous.items():
                signal.signal(sig, handler)


if __name__ == "__main__":
    main()
