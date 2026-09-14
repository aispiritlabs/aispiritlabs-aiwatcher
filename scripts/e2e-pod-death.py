#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.13"
# dependencies = ["aiwatcher-sdk"]
#
# [tool.uv.sources]
# aiwatcher-sdk = { path = "../sdk/python", editable = true }
# ///
"""A step's pod dies mid-attempt, and the run goes on (stage C's pod acceptance).

Stage C's acceptance asks one thing of the pod path that `e2e-pods` does not:
that a worker dying while it holds an attempt is seen, ended and retried, and
that its Job ends on a real cluster. The four stages of `pod_stages` run in
pods, `analyze` holding its attempt for a while, and while it holds, the pod's
container is killed from outside — `SIGKILL`, no destructor, no report:

1. **the dead attempt ends as `infrastructure` with the cluster's own reason**,
   seen by the launcher's watch well inside the attempt's lease;
2. **the budget runs it again in a pod of its own**: a second Job for the
   second attempt, which completes;
3. **the run completes**, `persist` after it, and the review is written;
4. **no Job is left behind**, the dead one's included, and every pod that ran
   to an end has its log kept — a pod deleted at once has none left to read.

The helpers are `e2e-pod-steps.py`'s, read from that file rather than copied, so
the two gates start their server, their namespace and their images the one way.
It uses a namespace of its own, `aiwatcher-pod-death`, and refuses any context
that is not a known-local cluster.

    just e2e-pod-death                   # a Job per attempt, on the local cluster
    just e2e-pod-death --runtime docker  # a container per attempt, on this host
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent.parent
_spec = importlib.util.spec_from_file_location("pod_steps", ROOT / "scripts" / "e2e-pod-steps.py")
if _spec is None or _spec.loader is None:
    raise SystemExit(
        "scripts/e2e-pod-steps.py is where this gate's helpers live, and it is not there"
    )
steps = importlib.util.module_from_spec(_spec)
sys.modules["pod_steps"] = steps
_spec.loader.exec_module(steps)

#: How long `analyze` holds each attempt. Long enough to be killed inside, short
#: enough that the second attempt, which holds too, does not make the gate slow.
HOLD_SECONDS = 25
#: How soon after the kill the attempt has to have ended: the launcher's tick,
#: a pod status to settle and a log to keep — and nothing like a lease.
ENDED_WITHIN = 60


def kill(backend: Any, job: str) -> str:
    """Kill the container running one Job's pod, the way a node losing it would."""
    if isinstance(backend, steps.Cluster):
        pod = backend.kubectl(
            "get", "pods", "-l", f"job-name={job}", "-o", "jsonpath={.items[0].metadata.name}"
        ).strip()
        if not pod:
            raise SystemExit(f"{job} has no pod to kill")
        # A cluster whose containers this host's engine runs (OrbStack, Docker
        # Desktop) is killed at the container, which is a worker dying; anywhere
        # else the pod is deleted at once, which is a node losing it.
        containers = steps.run(
            "docker",
            "ps",
            "--quiet",
            "--filter",
            f"label=io.kubernetes.pod.name={pod}",
            "--filter",
            "label=io.kubernetes.container.name!=POD",
            check=False,
        ).split()
        containers = [
            container
            for container in containers
            if steps.run(
                "docker",
                "inspect",
                "--format",
                '{{index .Config.Labels "io.kubernetes.container.name"}}',
                container,
                check=False,
            ).strip()
            != "POD"
        ]
        if containers:
            steps.run("docker", "kill", "--signal", "KILL", *containers)
            return f"killed the container of pod {pod}"
        backend.kubectl("delete", "pod", pod, "--grace-period=0", "--force", "--wait=false")
        return f"deleted pod {pod} at once"
    if isinstance(backend, steps.Containers):
        steps.run("docker", "kill", "--signal", "KILL", job)
        return f"killed container {job}"
    pids = steps.run("pgrep", "-f", steps.PROCESS_PATTERN, check=False).split()
    if not pids:
        raise SystemExit("no step process to kill")
    steps.run("kill", "-KILL", *pids)
    return f"killed process {', '.join(pids)}"


def attempts_of(view: dict[str, Any], step: str) -> list[dict[str, Any]]:
    found = {record["step_id"]: record for record in view["execution"]["steps"]}
    return list(found.get(step, {}).get("attempts", []))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runtime", choices=("pods", "docker", "process"), default="pods")
    parser.add_argument("--namespace", default="aiwatcher-pod-death")
    parser.add_argument("--api-host", default=None)
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument("--keep", action="store_true")
    arguments = parser.parse_args()

    in_cluster = arguments.runtime == "pods"
    in_image = arguments.runtime in ("pods", "docker")
    context = ""
    if in_cluster:
        context = steps.current_context()
        if not steps.local(context):
            raise SystemExit(
                f"refusing to run against {context!r}: it is not a known-local cluster"
            )
        print(f"· cluster {context}")
    api_host = arguments.api_host or steps.api_host_for(arguments.runtime, context)
    if not arguments.no_build:
        steps.build_server(arguments.runtime)
        if in_image:
            steps.build_image()
    if in_cluster:
        steps.load_image(context)

    home = Path(tempfile.mkdtemp(prefix="aiwatcher-pod-death-"))
    server: subprocess.Popen[bytes] | None = None
    backend: Any = None
    try:
        if in_cluster:
            backend = steps.open_namespace(steps.kubeconfig_for(context, home), arguments.namespace)
            print(f"· namespace {backend.namespace}")
            steps.grant(backend)
        elif in_image:
            backend = steps.Containers()
        else:
            backend = steps.Host(log=home / "server.log")
        templates = steps.templates_file(home, arguments.runtime)
        server, api_url = steps.serve(home, backend, templates, api_host)
        backend.probe(api_url)

        pod = steps.PodRequest(template=steps.TEMPLATE, image=steps.IMAGE, memory="192Mi")
        holding = steps.pod_stages.build_workflow(pod, hold_seconds=HOLD_SECONDS)
        runtime = steps.Runtime(
            name="pod-death-e2e",
            url=steps.BASE,
            token=steps.TOKEN,
            workflows=[holding],
            pools=[steps.ExecutionPool("local", steps.QUEUE, concurrency=1)],
            placement={holding.ref: "local"},
            poll_interval=0.2,
        )
        with runtime:
            runtime.register()
            runtime.start()
            print("· analyze's pod is killed while it holds its attempt")
            handle = runtime.run(holding)
            seen, deadline = steps.Seen(), time.monotonic() + steps.RUN_WITHIN
            first: str | None = None
            while time.monotonic() < deadline:
                seen.take(backend)
                for_analyze = [
                    name
                    for name, step in seen.steps(handle.execution_id).items()
                    if step == "analyze"
                ]
                status, view = steps.call("GET", f"/api/v1/executions/{handle.execution_id}")
                running = (
                    [
                        record
                        for record in attempts_of(view or {"execution": {"steps": []}}, "analyze")
                        if record.get("state", {}).get("state_type") == "running"
                    ]
                    if status == 200
                    else []
                )
                if for_analyze and running:
                    first = for_analyze[0]
                    break
                time.sleep(1)
            if first is None:
                raise SystemExit("analyze never held an attempt in a pod")
            # Into the hold, not at its first second: the pod has claimed and
            # reported the attempt started, which is what a worker dying means.
            time.sleep(3)
            how = kill(backend, first)
            killed = time.monotonic()
            print(f"  · {how}")

            ended_after: float | None = None
            view: dict[str, Any] = {}
            while time.monotonic() < killed + steps.RUN_WITHIN:
                seen.take(backend)
                status, view = steps.call("GET", f"/api/v1/executions/{handle.execution_id}")
                records = attempts_of(view, "analyze") if status == 200 else []
                if ended_after is None and records and (records[0].get("error") or {}):
                    ended_after = time.monotonic() - killed
                if view and view["execution"]["state"]["state_type"] in (
                    "completed",
                    "failed",
                    "cancelled",
                    "crashed",
                ):
                    break
                time.sleep(1)

            records = attempts_of(view, "analyze")
            dead = records[0] if records else {}
            error = dead.get("error") or {}
            if error.get("class") != "infrastructure" or not error.get("message"):
                raise SystemExit(f"the dead attempt ended as {error or dead}")
            if ended_after is None or ended_after > ENDED_WITHIN:
                raise SystemExit(f"the dead attempt was seen ending only after {ended_after}s")
            said = error["message"]
            print(f"  ✓ attempt 1 ended as infrastructure {ended_after:.0f}s after: {said}")

            # By attempt, never by name: a name is a hash of the key, so the
            # killed pod's sorts after its successor's as often as before it.
            jobs = sorted(
                (
                    name
                    for name, step in seen.steps(handle.execution_id).items()
                    if step == "analyze"
                ),
                key=lambda name: int(seen.jobs[name]["attempt"] or 0),
            )
            if len(records) != 2 or records[1].get("state", {}).get("state_type") != "completed":
                raise SystemExit(
                    f"analyze was not run again to completion: {json.dumps(records)[:2000]}"
                )
            if len(jobs) != 2:
                raise SystemExit(f"expected a second pod for the second attempt, saw {jobs}")
            print(f"  ✓ attempt 2 ran in a pod of its own and completed: {jobs[1]}")

            state = view["execution"]["state"]["state_type"]
            persist = {
                record["step_id"]: record["state"]["state_type"]
                for record in view["execution"]["steps"]
            }
            if state != "completed" or persist.get("persist") != "completed":
                raise SystemExit(f"the run ended {state}: {persist}")
            print("  ✓ the run completed, persist after it")
        pods = {
            name for name, step in seen.steps(handle.execution_id).items() if step != "analyze"
        } | set(jobs[1:])
        steps.logs_kept(home, backend, pods)
    finally:
        if server is not None and server.poll() is None:
            server.terminate()
            try:
                server.wait(timeout=15)
            except subprocess.TimeoutExpired:
                server.kill()
        if backend is not None and not arguments.keep:
            backend.teardown()
        if arguments.keep:
            print(f"· kept {home}")
    print(
        "\n✓ a pod killed mid-attempt was ended by the watch, run again in a pod of its own, "
        "and no Job was left"
    )


if __name__ == "__main__":
    main()
