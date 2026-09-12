"""Four stages of one import, each able to run in a pod of its own (ADR_0029).

The shape planner's house import has: ``acquire → normalize → analyze →
persist``, three artifact edges, and a review document at the end. It is here
so the pod path can be run against a real cluster and compared with the path
every other workflow takes — one long-lived worker claiming all four steps —
and ``scripts/e2e-pod-steps.py`` is what does the comparing.

**The review is a pure function of the rows**, and that is the whole point: the
only thing that differs between the two paths is the transport, so a review
whose bytes differ is a transport that changed the data. Nothing here reads a
clock or a hostname into it. Who *did* the work is a separate artifact — four
distinct pod names on the pod path, one worker name on the direct one —
because that difference is what proves the two runs were not the same run
twice.
"""

from __future__ import annotations

import os
import time

from aiwatcher_sdk.runtime import ExecutionPool, Runtime
from aiwatcher_sdk.task import task
from aiwatcher_sdk.worker import JsonObject, JsonValue, get_task_context
from aiwatcher_sdk.workflow import (
    PodRequest,
    RetryPolicy,
    Workflow,
    WorkflowInput,
    WorkflowStep,
)

#: The corpus, in millimetres as a drawing carries them. Fixed rather than
#: generated: a stage that invented its own rows would compare two guesses.
PLAN: tuple[JsonObject, ...] = (
    {"room": "kitchen", "width_mm": 3600, "depth_mm": 4200, "openings": 2},
    {"room": "living", "width_mm": 5400, "depth_mm": 4800, "openings": 3},
    {"room": "bath", "width_mm": 2100, "depth_mm": 2400, "openings": 1},
    {"room": "hall", "width_mm": 1200, "depth_mm": 3300, "openings": 4},
)

#: The stages, in the order the graph runs them, as ``--task`` spells them:
#: one template's command registers all four, so one template serves every
#: stage of this workflow.
STAGES = ("acquire", "normalize", "analyze", "persist")

#: Where the name of whoever ran a stage comes from. A pod is told its own name
#: by the launcher (``metadata.name`` through the downward API); a long-lived
#: worker is told nothing and says so.
WORKER = os.environ.get("AIWATCHER_WORKER_NAME") or "long-lived-worker"


def _text(value: JsonValue) -> str:
    if not isinstance(value, str):
        raise ValueError(f"expected text, got {value!r}")
    return value


def _number(value: JsonValue) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ValueError(f"expected a number, got {value!r}")
    return float(value)


def _count(value: JsonValue) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise ValueError(f"expected a whole number, got {value!r}")
    return value


def _round(value: float) -> float:
    """Six places, so a float's own repr is not part of the claim.

    The review's bytes are compared between two runs, and two paths that
    computed the same number must not differ because one of them printed more
    of it.
    """
    return round(value, 6)


def _by(stage: str) -> list[JsonObject]:
    """Who ran this stage, as its own artifact and never in the review.

    Printed as well as written, and the print is not decoration: it is what a
    pod's log has in it, so a log read back out of the catalog can be told from
    the six other pods' by the one thing that differs between them. A log named
    by its own hash means identical output is one object.
    """
    print(f"{stage}: this is {WORKER}", flush=True)
    return [{"stage": stage, "worker": WORKER}]


@task("pods.acquire", version="1")
def acquire() -> dict[str, int]:
    """Read the drawing. One pod's worth of work, and the graph's root."""
    ctx = get_task_context()
    ctx.write_artifact("plan", [dict(row) for row in PLAN])
    ctx.write_artifact("acquired_by", _by("acquire"))
    return {"rooms": len(PLAN)}


@task("pods.normalize", version="1")
def normalize(unit: str = "m") -> dict[str, int]:
    """Millimetres to metres — the step where a JSON round trip could bite."""
    ctx = get_task_context()
    scale = {"m": 1000.0, "cm": 10.0}[unit]
    rows: list[JsonObject] = [
        {
            "room": _text(row["room"]),
            "width": _round(_number(row["width_mm"]) / scale),
            "depth": _round(_number(row["depth_mm"]) / scale),
            "openings": _count(row["openings"]),
        }
        for row in ctx.read_artifact("plan")
    ]
    ctx.write_artifact("normalized", sorted(rows, key=lambda row: _text(row["room"])))
    ctx.write_artifact("normalized_by", _by("normalize"))
    return {"rooms": len(rows)}


@task("pods.analyze", version="1")
def analyze(hold_seconds: int = 0, hog_mb: int = 0) -> dict[str, int]:
    """The stage planner runs out of memory in, and the reason for one pod each.

    Both parameters exist for one caller, a gate that needs this stage to
    misbehave on purpose. `hold_seconds` is how long it does nothing before it
    starts, so a cancel can arrive while its pod is running — nothing
    cooperates with that cancel, because a pod is stopped by having its Job
    deleted. `hog_mb` is how much memory it takes, so the kernel stops it: the
    isolation this whole design is for is that one stage exceeding its limit
    fails that stage and not the import.
    """
    ctx = get_task_context()
    if hold_seconds:
        time.sleep(hold_seconds)
    if hog_mb:
        # A list of written pages rather than one big allocation: zeroed memory
        # can go uncommitted, and a stage that asked for a gigabyte and never
        # touched it is a stage that is not killed.
        print(f"analyze: taking {hog_mb}MiB, which is more than it has", flush=True)
        held = [b"x" * (1024 * 1024) for _ in range(hog_mb)]
        print(f"analyze: still here with {len(held)}MiB", flush=True)
    measured: list[JsonObject] = [
        {
            "room": _text(row["room"]),
            "area": _round(_number(row["width"]) * _number(row["depth"])),
            "aspect": _round(_number(row["width"]) / _number(row["depth"])),
            "openings": _count(row["openings"]),
        }
        for row in ctx.read_artifact("normalized")
    ]
    ctx.write_artifact("analysis", measured)
    ctx.write_artifact("analyzed_by", _by("analyze"))
    return {"rooms": len(measured)}


@task("pods.persist", version="1")
def persist() -> dict[str, int]:
    """The review, which is what the two paths are compared by."""
    ctx = get_task_context()
    rows = ctx.read_artifact("analysis")
    total = _round(sum(_number(row["area"]) for row in rows))
    review: list[JsonObject] = [
        {
            "room": _text(row["room"]),
            "area": _round(_number(row["area"])),
            "aspect": _round(_number(row["aspect"])),
            "openings": _count(row["openings"]),
            "share": _round(_number(row["area"]) / total),
        }
        for row in rows
    ]
    review.append({"room": "__total__", "area": total, "rooms": len(rows)})
    ctx.write_artifact("review", review)
    ctx.write_artifact("persisted_by", _by("persist"))
    return {"rooms": len(rows)}


def _version(pod: PodRequest | None, hold_seconds: int, hog_mb: int) -> str:
    """Which of the four shapes this is.

    Derived rather than passed in: a definition is pinned by name and version,
    and two of these registered under one version would be one workflow whose
    steps changed underneath it.
    """
    if pod is None:
        return "1"
    if hold_seconds:
        return "3"
    if hog_mb:
        return "4"
    return "2"


def build_workflow(
    pod: PodRequest | None = None, hold_seconds: int = 0, hog_mb: int = 0
) -> Workflow:
    """The four stages, in pods or not.

    ``pod`` is the same request on all four steps: a template's resources are
    what one stage needs, and these four need the same. ``None`` is the path
    every other workflow takes, and the two differ in nothing else — the pod is
    an executable field, so they are two plans, and the review they produce has
    to be the same bytes.

    ``hold_seconds`` and ``hog_mb`` go to ``analyze`` and are how a gate asks
    for a stage that can be cancelled in and a stage the kernel stops.
    """
    analyze_params: dict[str, object] = {}
    if hold_seconds:
        analyze_params["hold_seconds"] = hold_seconds
    if hog_mb:
        analyze_params["hog_mb"] = hog_mb
    return Workflow(
        "pods.house-import",
        _version(pod, hold_seconds, hog_mb),
        (
            WorkflowStep("acquire", acquire, outputs=("plan", "acquired_by"), pod=pod),
            WorkflowStep(
                "normalize",
                normalize,
                inputs=(WorkflowInput("acquire", "plan"),),
                outputs=("normalized", "normalized_by"),
                pod=pod,
            ),
            WorkflowStep(
                "analyze",
                analyze,
                inputs=(WorkflowInput("normalize", "normalized"),),
                outputs=("analysis", "analyzed_by"),
                params=analyze_params,
                # Two attempts rather than three, for the one variant that is
                # built to fail: enough to show the budget scheduling another
                # pod, few enough that the gate is not waiting on a third.
                retry=RetryPolicy(max_attempts=2, delays_seconds=(0,)) if hog_mb else RetryPolicy(),
                pod=pod,
            ),
            WorkflowStep(
                "persist",
                persist,
                inputs=(WorkflowInput("analyze", "analysis"),),
                outputs=("review", "persisted_by"),
                pod=pod,
            ),
        ),
    )


def build_runtime(queue: str = "pods") -> Runtime:
    """A long-lived worker for the direct path: one process, all four stages.

    The pod path needs no runtime at all — the launcher starts a pod per
    attempt and each runs ``aiwatcher_sdk.worker run-attempt`` — so this is
    only the other half of the comparison.
    """
    workflow = build_workflow()
    return Runtime(
        name="pods-direct",
        url=os.environ.get("AIWATCHER_URL", "http://127.0.0.1:8080"),
        workflows=[workflow],
        pools=[ExecutionPool("local", queue, concurrency=1)],
        placement={workflow.ref: "local"},
        poll_interval=0.1,
    )
