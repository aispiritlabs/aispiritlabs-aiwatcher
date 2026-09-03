#!/usr/bin/env python3
"""Serve the model version the `production` label names, and keep serving it.

A command-line front for :mod:`aiwatcher_sdk.serving`, which is where the work
is: the hardened half — resolve, verify, warm, bound, validate, watch, roll
back, report — plus one loader per runtime, selected by the name the package
declares.

Two runtimes ship. `weights` is a JSON array of numbers and needs nothing;
`onnx` is a graph and needs `pip install 'aiwatcher-sdk[onnx]'`. Everything
else is refused **by name** rather than attempted, because a loader chosen by
looking at the file is a loader chosen by whoever wrote the file.

Artifacts default to `file://`. Configuring `AIWATCHER_MODEL_S3_ENDPOINT`, one
approved bucket and credentials adds signed, byte-bounded `s3://` reads. Only
digest-verified bytes enter the persistent version cache.

`--shadow-label candidate` independently loads that label and mirrors validated
requests under a separate no-queue concurrency bound. Shadow answers are
discarded; only version-scoped latency and runtime failures are reported.

    just e2e-train && just serve-model        # the weight vector
    just onnx-version && watch the rollout    # the same model, as a graph

The rollout across those two is worth watching: the label moves, this process
reads and verifies and warms the graph *while the weight vector keeps
serving*, and only then swaps. A runtime change is not a special case of that
— it is the same three phases with a different loader on the far side.
"""

from __future__ import annotations

import argparse
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "sdk" / "python"))

from aiwatcher_sdk import AiwatcherClient
from aiwatcher_sdk.serving import (
    LABEL,
    ArtifactReader,
    FileReader,
    LoadError,
    S3Credentials,
    S3Reader,
    SchemeReader,
    Server,
    VersionCacheReader,
    serve,
)
from aiwatcher_sdk.serving.runtimes import available
from aiwatcher_sdk.training import TrainingClient, TrainingError

DEFAULT_URL = "http://127.0.0.1:8080"
DEFAULT_MODEL = "e2e.mini-edge-detector"


def _cache_dir() -> str:
    root = os.environ.get("XDG_CACHE_HOME")
    if root:
        return str(Path(root) / "aiwatcher" / "models")
    return str(Path.home() / ".cache" / "aiwatcher" / "models")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--api", default=os.environ.get("AIWATCHER_URL", DEFAULT_URL))
    parser.add_argument("--model", default=os.environ.get("AIWATCHER_E2E_MODEL", DEFAULT_MODEL))
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8091)
    parser.add_argument(
        "--token",
        default=os.environ.get("AIWATCHER_SERVE_TOKEN"),
        help="require this bearer token on /v1/*. The probes stay public.",
    )
    parser.add_argument("--max-batch", type=int, default=1_000)
    parser.add_argument("--max-concurrency", type=int, default=8)
    parser.add_argument(
        "--threads",
        type=int,
        default=1,
        help="what one graph request may spend. --max-concurrency bounds how many are in flight.",
    )
    parser.add_argument(
        "--no-onnx",
        action="store_true",
        help="refuse an onnx package instead of loading it. For a host that should be "
        "unable to open a graph at all.",
    )
    parser.add_argument(
        "--poll",
        type=float,
        default=10.0,
        help=f"how often to check whether the {LABEL} label moved. 0 disables the watch.",
    )
    parser.add_argument(
        "--no-telemetry",
        action="store_true",
        help="do not report inferences. What is reported never includes inputs or outputs.",
    )
    parser.add_argument(
        "--s3-endpoint",
        default=os.environ.get("AIWATCHER_MODEL_S3_ENDPOINT"),
        help="enable signed s3:// reads against this endpoint.",
    )
    parser.add_argument(
        "--s3-bucket",
        default=os.environ.get("AIWATCHER_MODEL_S3_BUCKET"),
        help="the only bucket this process is allowed to read.",
    )
    parser.add_argument(
        "--s3-timeout",
        type=float,
        default=float(os.environ.get("AIWATCHER_MODEL_S3_TIMEOUT", "30")),
        help="seconds allowed for one artifact GET.",
    )
    parser.add_argument(
        "--max-artifact-mb",
        type=int,
        default=int(os.environ.get("AIWATCHER_SERVE_MAX_ARTIFACT_MB", "4096")),
        help="hard streamed byte ceiling for one remote artifact.",
    )
    parser.add_argument(
        "--cache-dir",
        default=os.environ.get("AIWATCHER_SERVE_CACHE_DIR", _cache_dir()),
        help="persistent cache, keyed by immutable version and artifact digest.",
    )
    parser.add_argument(
        "--cache-mb",
        type=int,
        default=int(os.environ.get("AIWATCHER_SERVE_CACHE_MB", "10240")),
        help="LRU byte budget for verified remote artifacts.",
    )
    parser.add_argument(
        "--no-cache",
        action="store_true",
        help="download remote artifacts on every process start.",
    )
    parser.add_argument(
        "--shadow-label",
        default=os.environ.get("AIWATCHER_SERVE_SHADOW_LABEL"),
        help="mirror requests to this independently loaded label and discard its answers.",
    )
    parser.add_argument(
        "--shadow-concurrency",
        type=int,
        default=int(os.environ.get("AIWATCHER_SERVE_SHADOW_CONCURRENCY", "1")),
        help="non-blocking bound for shadow work; excess mirrors are counted and dropped.",
    )
    args = parser.parse_args()

    if args.shadow_label == LABEL:
        parser.error("--shadow-label must differ from production")
    if args.shadow_concurrency <= 0:
        parser.error("--shadow-concurrency must be positive")

    client = TrainingClient(args.api)
    telemetry = None if args.no_telemetry else AiwatcherClient(base_url=args.api, service="serving")
    loaders = available(onnx=not args.no_onnx, threads=args.threads)
    readers: list[ArtifactReader] = [FileReader()]
    if args.s3_endpoint or args.s3_bucket:
        access_key = os.environ.get("AIWATCHER_MODEL_S3_ACCESS_KEY") or os.environ.get(
            "AWS_ACCESS_KEY_ID"
        )
        secret_key = os.environ.get("AIWATCHER_MODEL_S3_SECRET_KEY") or os.environ.get(
            "AWS_SECRET_ACCESS_KEY"
        )
        missing = [
            name
            for name, value in (
                ("AIWATCHER_MODEL_S3_ENDPOINT/--s3-endpoint", args.s3_endpoint),
                ("AIWATCHER_MODEL_S3_BUCKET/--s3-bucket", args.s3_bucket),
                ("AIWATCHER_MODEL_S3_ACCESS_KEY or AWS_ACCESS_KEY_ID", access_key),
                ("AIWATCHER_MODEL_S3_SECRET_KEY or AWS_SECRET_ACCESS_KEY", secret_key),
            )
            if not value
        ]
        if missing:
            parser.error("an S3 reader is partly configured; missing " + ", ".join(missing))
        if access_key is None or secret_key is None:
            parser.error("the S3 credential environment is incomplete")
        try:
            readers.append(
                S3Reader(
                    args.s3_endpoint,
                    args.s3_bucket,
                    S3Credentials(
                        access_key_id=access_key,
                        secret_access_key=secret_key,
                        region=os.environ.get("AIWATCHER_MODEL_S3_REGION")
                        or os.environ.get("AWS_REGION", "us-east-1"),
                        session_token=os.environ.get("AIWATCHER_MODEL_S3_SESSION_TOKEN")
                        or os.environ.get("AWS_SESSION_TOKEN"),
                    ),
                    timeout_seconds=args.s3_timeout,
                    max_bytes=args.max_artifact_mb * 1024 * 1024,
                )
            )
        except ValueError as error:
            parser.error(str(error))

    reader: ArtifactReader = SchemeReader(readers)
    if not args.no_cache and "s3" in reader.schemes:
        try:
            reader = VersionCacheReader(
                reader,
                Path(args.cache_dir),
                max_bytes=args.cache_mb * 1024 * 1024,
                cache_schemes=("s3",),
            )
        except ValueError as error:
            parser.error(str(error))
    state = Server(
        client,
        args.model,
        telemetry,
        loaders=loaders,
        reader=reader,
        shadow_label=args.shadow_label,
        shadow_concurrency=args.shadow_concurrency,
    )
    try:
        state.start()
    except (LoadError, TrainingError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1

    current = state.current
    if current is not None:
        print(
            f"loaded {args.model}@{current.version[:12]} as {current.runtime} "
            f"({'verified' if current.verified else 'UNVERIFIED — no package'})",
            flush=True,
        )
    print(
        f"serving on http://{args.host}:{args.port} "
        f"(runtimes={'+'.join(state.runtimes)}, auth={'on' if args.token else 'off'}, "
        f"watch={args.poll or 'off'}s)",
        flush=True,
    )
    try:
        serve(
            state,
            host=args.host,
            port=args.port,
            token=args.token,
            max_batch=args.max_batch,
            max_concurrency=args.max_concurrency,
            poll_seconds=args.poll,
        )
    finally:
        if telemetry is not None:
            telemetry.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
