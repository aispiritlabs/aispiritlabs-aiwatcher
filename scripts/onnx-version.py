#!/usr/bin/env python3
"""Re-express the promoted model as an ONNX graph, prove it agrees, and label it.

The second runtime profile needs a second runtime to serve, and inventing a
different model to be the ONNX one would prove nothing about the rollout: two
models that disagree cannot tell you whether a swap was safe. So this takes
the version `production` already names — the eight-weight edge detector
`just e2e-train` fits — and writes the *same function* as a graph:

    features ──▶ Gemm(W, B) ──▶ Sigmoid ──▶ probability

Then it checks. Both are run over a deterministic spread of inputs, including
the extremes that separate a sigmoid from a step, and a disagreement anywhere
beyond 1e-6 is a refusal. That check is the whole licence for what happens
next: the new version is registered with the *same held-out score*, and the
reason that is honest is that it computes the same function to six decimal
places — not because a score was carried across on trust. A re-expression that
drifts is a different model and would need a different measurement.

What it is really for is the rollout. With `just serve-model` running, moving
the label here makes the server read, verify and warm an ONNX graph *while the
weight vector keeps serving*, and swap only if all three succeed. A runtime
change is not a special case of that: it is the same three phases with a
different loader on the far side, which is the property the loader seam exists
to have.

    just run                        # the API
    just e2e-train                  # fit and promote the weight vector
    just serve-model                # serve it, watching the label
    just onnx-version               # this — the label moves, the server follows

Needs `onnx` and `onnxruntime`: `pip install 'aiwatcher-sdk[onnx]'`.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import urllib.parse
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "sdk" / "python"))

from aiwatcher_sdk.serving import FileReader, read_verified  # noqa: E402
from aiwatcher_sdk.training import TrainingClient, TrainingError  # noqa: E402

DEFAULT_URL = "http://127.0.0.1:8080"
DEFAULT_MODEL = "e2e.mini-edge-detector"
PANEL = os.environ.get("AIWATCHER_PANEL", "http://127.0.0.1:5173")
#: How far the graph may drift from the vector before this refuses to call
#: them the same model. Six decimal places is far tighter than any metric this
#: score is reported to, and loose enough for float32 accumulation order.
TOLERANCE = 1e-6


def build_graph(weights: list[float], classes: list[str]) -> bytes:
    """The same linear model, as a graph with a dynamic batch axis.

    The batch dimension is a *symbol* rather than a number on purpose: a graph
    that pins it serves exactly one row per request and fails on every other
    size at run time. The serving profile refuses one at load for that reason,
    so this is also the shape that gets past its own gate.
    """
    import numpy as np
    import onnx
    from onnx import TensorProto, helper

    features = len(weights)
    column = np.asarray(weights, dtype=np.float32).reshape(features, 1)
    graph = helper.make_graph(
        [
            helper.make_node("Gemm", ["features", "W", "B"], ["logit"], name="linear"),
            helper.make_node("Sigmoid", ["logit"], ["probability"], name="squash"),
        ],
        "edge-detector",
        [helper.make_tensor_value_info("features", TensorProto.FLOAT, ["batch", features])],
        [helper.make_tensor_value_info("probability", TensorProto.FLOAT, ["batch", 1])],
        [
            helper.make_tensor("W", TensorProto.FLOAT, [features, 1], column.flatten().tolist()),
            helper.make_tensor("B", TensorProto.FLOAT, [1], [0.0]),
        ],
        doc_string=f"classes in output order: {', '.join(classes)}",
    )
    model = helper.make_model(
        graph, producer_name="aiwatcher-onnx-version", opset_imports=[helper.make_opsetid("", 13)]
    )
    model.ir_version = 9
    onnx.checker.check_model(model)
    serialized: bytes = model.SerializeToString()
    return serialized


def spread(features: int) -> list[list[float]]:
    """Inputs the two implementations have to agree on.

    Zeros, ones and the extremes — because a sigmoid and a step function agree
    everywhere except near the middle and at saturation, which is exactly where
    a re-expression that got the sign or the scale wrong would still look
    right on typical data.
    """
    rows: list[list[float]] = [
        [0.0] * features,
        [1.0] * features,
        [-1.0] * features,
        [1e3] * features,
        [-1e3] * features,
    ]
    state = 12345
    for _ in range(256):
        row: list[float] = []
        for _ in range(features):
            state = (1103515245 * state + 12345) % (2**31)
            row.append((state / (2**31)) * 4.0 - 2.0)
        rows.append(row)
    return rows


def agree(graph: bytes, weights: list[float]) -> tuple[bool, float]:
    """Both implementations over the same rows, and the worst disagreement."""
    import math

    import numpy as np
    import onnxruntime as ort

    session = ort.InferenceSession(graph, providers=["CPUExecutionProvider"])
    rows = spread(len(weights))
    served = session.run(None, {"features": np.asarray(rows, dtype=np.float32)})[0]
    worst = 0.0
    for row, produced in zip(rows, served.reshape(len(rows)).tolist(), strict=True):
        total = sum(weight * value for weight, value in zip(weights, row, strict=True))
        expected = 1.0 / (1.0 + math.exp(-max(-30.0, min(30.0, total))))
        worst = max(worst, abs(expected - produced))
    return worst <= TOLERANCE, worst


def main() -> int:  # noqa: PLR0911 - one early return per checked step, on purpose
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--api", default=os.environ.get("AIWATCHER_URL", DEFAULT_URL))
    parser.add_argument("--model", default=os.environ.get("AIWATCHER_E2E_MODEL", DEFAULT_MODEL))
    parser.add_argument(
        "--no-promote",
        action="store_true",
        help="register the graph and leave the label where it is.",
    )
    args = parser.parse_args()

    try:
        import onnx  # noqa: F401
        import onnxruntime as ort
    except ImportError:
        print(
            "error: this needs onnx and onnxruntime — `pip install 'aiwatcher-sdk[onnx]'`",
            file=sys.stderr,
        )
        return 1

    training = TrainingClient(args.api)
    print(f"1. read what {args.model!r} has in production")
    try:
        model = training.get_model(args.model)
    except TrainingError as error:
        print(f"error: {error}. Run `just e2e-train` first", file=sys.stderr)
        return 1
    current: dict[str, Any] = model.get("current") or {}
    package: dict[str, Any] = current.get("package") or {}
    if package.get("runtime") != "weights":
        print(
            f"error: production is a {package.get('runtime', 'package-less')} version and this "
            "re-expresses a weight vector",
            file=sys.stderr,
        )
        return 1
    print(f"   {current['version'][:12]}   run {current['run_id'][:12]}")

    print("2. read the weights, against the digest that names them")
    artifact = next(item for item in package["artifacts"] if item.get("name") == "weights")
    weights = [float(value) for value in json.loads(read_verified(FileReader(), artifact))]
    classes = [str(name) for name in (package.get("outputs") or [{}])[0].get("classes") or []]
    print(f"   {len(weights)} weights, classes {classes or ['(none declared)']}")

    print("3. write the same function as a graph")
    graph = build_graph(weights, classes)
    checkpoint = Path(urllib.parse.urlparse(artifact["uri"]).path)
    destination = checkpoint.with_suffix(".onnx")
    destination.write_bytes(graph)
    print(f"   {destination}  ({len(graph)} bytes, opset 13, dynamic batch)")

    print("4. check they agree before claiming the same score")
    rows = len(spread(len(weights)))
    same, worst = agree(graph, weights)
    if not same:
        print(
            f"✗ the graph and the vector differ by {worst:.3g}, which is more than {TOLERANCE:g}. "
            "A re-expression that drifts is a different model and needs its own measurement",
            file=sys.stderr,
        )
        return 1
    print(f"   {rows} rows, worst disagreement {worst:.3g} — the same function")

    print("5. register the graph as its own version")
    metrics = current.get("metrics") or {}
    onnx_package = {
        "runtime": "onnx",
        "runtime_version": ort.__version__,
        "entry_point": destination.name,
        "inputs": [
            {
                "name": "features",
                "dtype": "float32",
                "shape": [None, len(weights)],
                "description": (package.get("inputs") or [{}])[0].get("description", ""),
            }
        ],
        "outputs": [
            {"name": "probability", "dtype": "float32", "shape": [None, 1], "classes": classes}
        ],
        "preprocessing": list(package.get("preprocessing") or []),
        "artifacts": [
            {
                "name": "model",
                "uri": f"file://{destination}",
                "digest": hashlib.sha256(graph).hexdigest(),
                "size_bytes": len(graph),
                "content_type": "application/octet-stream",
            }
        ],
        "resources": dict(package.get("resources") or {}),
    }
    registered = training.register_model(
        args.model,
        run_id=current["run_id"],
        checkpoint_uri=f"file://{destination}",
        validation=metrics.get("validation") or {},
        test=metrics.get("test") or {},
        package=onnx_package,
        description="The same fit as an ONNX graph. Checked against it row by row.",
        notes=(
            f"Re-expressed from {current['version'][:12]}. Agrees with the weight vector to "
            f"{worst:.3g} over {rows} rows, which is why it carries that version's held-out score."
        ),
    )
    blocked = registered.get("promotion_blocked")
    version = registered["version"]["version"]
    if blocked:
        print(f"✗ refused a promotion: {blocked}", file=sys.stderr)
        return 1
    if version == current["version"]:
        print("✗ the registry called this the same version as the weight vector", file=sys.stderr)
        return 1
    print(f"   {version[:12]}   runtime onnx, package digest names the graph")

    if args.no_promote:
        print("\n  registered and not promoted. Dropping --no-promote moves the label.")
        return 0

    print("6. move the label")
    training.promote(args.model, version)
    print(f"   production → {version[:12]}")
    print()
    print("✓ a running `just serve-model` reads, verifies and warms the graph while the")
    print("  weight vector keeps serving, and swaps only if all three succeed:")
    print("    curl -s localhost:8091/v1/model | python3 -m json.tool")
    print(f"  model   {PANEL}/training/models?model={args.model}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
