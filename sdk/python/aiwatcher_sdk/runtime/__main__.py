"""Start a runtime from the application's composition factory."""

import argparse
import importlib
import signal
from types import FrameType

from aiwatcher_sdk.runtime import Runtime


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--factory", required=True, help="local module:function returning Runtime")
    parser.add_argument("--attempt", help="execute exactly execution/step/attempt, then exit")
    parser.add_argument("--pool", help="execution pool used with --attempt")
    args = parser.parse_args()
    if (args.attempt is None) != (args.pool is None):
        parser.error("--attempt and --pool must be used together")
    module, separator, name = args.factory.partition(":")
    if not separator or not module or not name:
        parser.error("--factory must be module:function")
    factory = getattr(importlib.import_module(module), name)
    runtime = factory()
    if not isinstance(runtime, Runtime):
        parser.error("the factory must return Runtime")

    def stop(signum: int, frame: FrameType | None) -> None:
        runtime.stop()

    previous = {sig: signal.signal(sig, stop) for sig in (signal.SIGINT, signal.SIGTERM)}
    try:
        if args.attempt is not None:
            if not runtime.run_attempt(args.attempt, pool=args.pool):
                raise SystemExit(1)
        else:
            runtime.register()
            runtime.serve()
    finally:
        runtime.close()
        for sig, handler in previous.items():
            signal.signal(sig, handler)


if __name__ == "__main__":
    main()
