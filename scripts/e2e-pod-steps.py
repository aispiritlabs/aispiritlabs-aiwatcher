#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.13"
# dependencies = ["aiwatcher-sdk"]
#
# [tool.uv.sources]
# aiwatcher-sdk = { path = "../sdk/python", editable = true }
# ///
"""Four stages, four pods, one real cluster — and the same bytes (AW-4 2.5).

AW-4's exit criterion: the four stages of an import run as four pods on a local
Kubernetes, and the review they produce is byte-identical to the one every
other workflow takes — one long-lived worker claiming all four steps. The
stages are `sdk/python/examples/pod_stages.py`, the shape planner's house
import has: `acquire → normalize → analyze → persist`, three artifact edges,
a review at the end.

Everything up to here was tested against a stand-in cluster, which cannot
answer the questions this asks: whether a real API server accepts the manifest,
whether the pod claims the attempt it was told, whether deleting a Job stops
the pod, and whether the log survives the pod that printed it.

What it proves, in order:

1. **A step that asked for a pod gets one.** Four Jobs, one per attempt, each
   named from its key and annotated with it; each pod claims its own attempt
   and reports; the run completes.
2. **A long-lived worker on the same queue takes none of them.** It is up and
   polling the whole time, with all four tasks registered — the key-only claim
   rule is what stops it, and nothing else is.
3. **The same four stages on the direct path produce the same review**, byte
   for byte: one digest, computed over the stored bytes by the route that
   stored them. What *does* differ is who ran each stage, which is a separate
   artifact and is asserted to differ — four pod names against one worker's.
4. **A cancel reaches a running pod.** The run is cancelled while `analyze`
   holds; its Job goes, the attempt ends without producing anything, no later
   step starts, and the run reaches `cancelled` in seconds rather than in a
   lease.
5. **Every pod's log is kept.** The launcher deletes a Job only once its log is
   stored and recorded, so no Job left behind *is* the assertion — and the
   stored objects are counted beside it.

It starts **its own** aiwatcher, on a free port with every byte under a
temporary directory, so the one on :8080 is not touched. It needs the `kube`
feature, which the default build does not carry:

    cargo build --bin aiwatcher --features aiwatcher-server/kube
    just e2e-pods

Two things about the cluster. Its kubeconfig is a **minified copy** holding one
context, so the server this starts can reach nothing else — this repository's
kubeconfig has production EKS contexts in it (ADR_0006). And the pods reach the
API on the *host*, at `host.docker.internal` by default: `--api-host` is the
other name for it on a cluster that calls it something else (`host.k3d.internal`
on k3d), and an unreachable one is refused by a probe pod before any stage runs
rather than four attempts later.
"""

from __future__ import annotations

import argparse
import json
import os
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "sdk" / "python"))
sys.path.insert(0, str(ROOT / "sdk" / "python" / "examples"))

import pod_stages  # noqa: E402
from aiwatcher_sdk.runtime import ExecutionPool, Runtime  # noqa: E402
from aiwatcher_sdk.workflow import PodRequest, Workflow  # noqa: E402

#: The contexts this may touch. The same list the Tiltfile and
#: `just _assert-local-context` keep, and for the same reason.
LOCAL_CONTEXTS = ("orbstack", "docker-desktop", "minikube", "colima", "rancher-desktop")
LOCAL_PREFIXES = ("kind-", "k3d-", "k3s-")

#: Where a pod reaches the host. OrbStack and Docker Desktop both answer to
#: this one; k3d calls it `host.k3d.internal`.
HOST_ALIAS = "host.docker.internal"

QUEUE = "pods"
TEMPLATE = "e2e"
IMAGE_REPOSITORY = "aiwatcher-stage"
IMAGE = f"{IMAGE_REPOSITORY}:e2e"

#: How long the four pods have. A cold start is an image that is already there
#: plus a scheduling decision, and four of them run one after another.
RUN_WITHIN = 300
READY_WITHIN = 60
#: How long the cancel has to reach a pod. The launcher's own tick is two
#: seconds, so this is generous and still nothing like a lease.
CANCEL_WITHIN = 60

BASE = ""


# ── the cluster ──────────────────────────────────────────────────────────────


def local(context: str) -> bool:
    return context in LOCAL_CONTEXTS or context.startswith(LOCAL_PREFIXES)


def current_context() -> str:
    return run("kubectl", "config", "current-context").strip()


def kubeconfig_for(context: str, home: Path) -> Path:
    """One context, flattened into a file of its own.

    Not a convenience: `kube::Client::try_default` reads whatever context is
    current, and this kubeconfig has production clusters in it. A copy holding
    one local context is a server that cannot reach another if it tried.
    """
    path = home / "kubeconfig"
    path.write_text(run("kubectl", "config", "view", "--minify", "--flatten", "--context", context))
    path.chmod(0o600)
    return path


@dataclass
class Cluster:
    kubeconfig: Path
    namespace: str
    #: Whether this script created the namespace, and may therefore delete it.
    ours: bool

    def kubectl(self, *args: str, check: bool = True) -> str:
        return run(
            "kubectl",
            "--kubeconfig",
            str(self.kubeconfig),
            "--namespace",
            self.namespace,
            *args,
            check=check,
        )

    def jobs(self) -> list[dict[str, Any]]:
        listing = self.kubectl(
            "get", "jobs", "-l", "app.kubernetes.io/managed-by=aiwatcher", "-o", "json"
        )
        items = json.loads(listing).get("items", [])
        return [item for item in items if isinstance(item, dict)]

    def teardown(self) -> None:
        if not self.ours:
            print(f"  · leaving namespace {self.namespace} alone: it was not ours to create")
            return
        run(
            "kubectl",
            "--kubeconfig",
            str(self.kubeconfig),
            "delete",
            "namespace",
            self.namespace,
            "--wait=false",
            check=False,
        )


def open_namespace(kubeconfig: Path, namespace: str) -> Cluster:
    """Ours if we created it, and never deleted if it was already somebody's.

    A previous run's namespace is deleted without waiting, so the first thing
    this may find is one that is `Terminating` — which accepts no pods and is
    about to stop existing. Waiting for it is the difference between a second
    run in a row and a refusal that reads as an unreachable cluster.
    """
    deadline = time.monotonic() + 120
    while True:
        phase = run(
            "kubectl",
            "--kubeconfig",
            str(kubeconfig),
            "get",
            "namespace",
            namespace,
            "--ignore-not-found",
            "-o",
            "jsonpath={.status.phase}",
            check=False,
        ).strip()
        if phase != "Terminating":
            break
        if time.monotonic() > deadline:
            raise SystemExit(f"namespace {namespace} has been terminating for two minutes")
        print(f"  · waiting for namespace {namespace} to finish terminating")
        time.sleep(3)
    if not phase:
        run("kubectl", "--kubeconfig", str(kubeconfig), "create", "namespace", namespace)
    return Cluster(kubeconfig=kubeconfig, namespace=namespace, ours=not phase)


def reachable(cluster: Cluster, api_url: str) -> None:
    """Ask a pod whether the API is where the launcher will tell pods it is.

    Before anything is registered: an address that is right on the host and
    wrong in the cluster otherwise shows up as four attempts that never
    claimed, one start allowance later.
    """
    script = (
        "import sys, urllib.request;"
        f"sys.exit(0 if urllib.request.urlopen('{api_url}/healthz', timeout=5)"
        ".status == 200 else 1)"
    )
    outcome = subprocess.run(  # noqa: S603 — kubectl, with this script's own arguments
        [
            "kubectl",
            "--kubeconfig",
            str(cluster.kubeconfig),
            "--namespace",
            cluster.namespace,
            "run",
            "reachability",
            "--rm",
            "--attach",
            "--quiet",
            "--restart=Never",
            f"--image={IMAGE}",
            "--image-pull-policy=IfNotPresent",
            "--command",
            "--",
            "python",
            "-c",
            script,
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    if outcome.returncode != 0:
        raise SystemExit(
            f"a pod could not reach the API at {api_url}:\n"
            f"{outcome.stdout}{outcome.stderr}\n"
            "pass --api-host with the name this cluster calls the host "
            "(host.k3d.internal on k3d), or an address it can route to."
        )


# ── the image and the templates ──────────────────────────────────────────────


def build_image() -> None:
    print(f"· building {IMAGE}")
    run("docker", "build", "-f", "deploy/Dockerfile.worker", "-t", IMAGE, ".", cwd=ROOT)


def build_server() -> None:
    """The binary, with the launcher in it.

    Built here rather than asked for, because the feature is not the default
    and any plain `cargo build` or `cargo test` in this repository overwrites
    `target/debug/aiwatcher` with one that has no launcher — a run that then
    fails at start-up with a refusal about a variable nobody set. Current, it
    costs a cargo no-op.
    """
    print("· building the server with its launcher")
    run("cargo", "build", "--bin", "aiwatcher", "--features", "aiwatcher-server/kube", cwd=ROOT)


def templates_file(home: Path) -> Path:
    """The operator's file, as a deployment's chart values would render it.

    One template for all four stages: its command registers every task and
    `run-attempt` runs whichever attempt the pod was told. `imagePullPolicy`
    is the template's to set and is set, because the image is built here and
    was never pushed anywhere to pull it from.
    """
    command = ["python", "-m", "aiwatcher_sdk.worker", "run-attempt", "--queue", QUEUE]
    for stage in pod_stages.STAGES:
        command += ["--task", f"pod_stages:{stage}"]
    path = home / "pod-templates.json"
    path.write_text(
        json.dumps(
            {
                TEMPLATE: {
                    "images": [IMAGE_REPOSITORY],
                    "resources": {
                        "requests": {"cpu": "50m", "memory": "96Mi"},
                        "limits": {"cpu": "1", "memory": "256Mi"},
                        "max": {"cpu": "2", "memory": "512Mi"},
                    },
                    "command": command,
                    # Short: nothing here pulls an image, so a pod that has not
                    # claimed in a minute is a pod that is not going to.
                    "start_allowance_seconds": 60,
                    "pod": {"containers": [{"imagePullPolicy": "IfNotPresent"}]},
                }
            },
            indent=2,
        )
    )
    return path


# ── the grant ────────────────────────────────────────────────────────────────

#: What the launcher asks of a cluster, one entry per call `kubernetes.rs`
#: makes. The gate's own server runs on this machine's kubeconfig and is
#: therefore an administrator, so the chart's Role is the thing that would be
#: wrong in a deployment and right here — which is why it is asked about
#: directly rather than relied on.
NEEDS = (
    ("create", "jobs.batch"),
    ("get", "jobs.batch"),
    ("list", "jobs.batch"),
    ("delete", "jobs.batch"),
    ("list", "pods"),
    ("get", "pods/log"),
)

#: And what it must not be able to do. A pod runs a step's code, so a credential
#: that could create one directly could run anything anywhere the Role reaches —
#: the launcher only ever creates Jobs, in one namespace, and the grant says so.
REFUSED = (
    ("create", "pods", None),
    ("delete", "pods", None),
    ("get", "secrets", None),
    ("create", "jobs.batch", "kube-system"),
)


def grant(cluster: Cluster) -> None:
    """Apply the chart's launcher RBAC, and ask the cluster what it allows."""
    print("· the grant the chart hands the launcher")
    rendered = run(
        "helm",
        "template",
        "aiwatcher",
        "deploy/helm/aiwatcher",
        "--namespace",
        cluster.namespace,
        "--show-only",
        "templates/pods.yaml",
        # The chart refuses templates on anything but the postgres store, and
        # refuses that store with no database: a launched pod is a second
        # process claiming against the same store. Neither is rendered here —
        # only the Role is — and both have to be said for the render to happen.
        "--set",
        "execution.store=postgres",
        "--set",
        "postgresql.mode=install",
        f"--set=execution.pods.templates.{TEMPLATE}.images[0]={IMAGE_REPOSITORY}",
        f"--set=execution.pods.templates.{TEMPLATE}.command[0]=python",
        cwd=ROOT,
    )
    apply = subprocess.run(  # noqa: S603 — kubectl, reading this render on its stdin
        [
            "kubectl",
            "--kubeconfig",
            str(cluster.kubeconfig),
            "--namespace",
            cluster.namespace,
            "apply",
            "-f",
            "-",
        ],
        input=rendered,
        capture_output=True,
        text=True,
        check=False,
    )
    if apply.returncode != 0:
        raise SystemExit(f"the chart's launcher RBAC did not apply:\n{apply.stderr}")
    account = f"system:serviceaccount:{cluster.namespace}:aiwatcher-launcher"

    def may(verb: str, resource: str, namespace: str | None = None) -> bool:
        return (
            run(
                "kubectl",
                "--kubeconfig",
                str(cluster.kubeconfig),
                "auth",
                "can-i",
                verb,
                resource,
                "--namespace",
                namespace or cluster.namespace,
                f"--as={account}",
                check=False,
            ).strip()
            == "yes"
        )

    if missing := [f"{verb} {resource}" for verb, resource in NEEDS if not may(verb, resource)]:
        raise SystemExit(
            f"the chart's Role does not cover what the launcher does: {missing}. "
            "deploy/helm/aiwatcher/templates/pods.yaml is where it is granted."
        )
    if wider := [
        f"{verb} {resource}" + (f" in {ns}" if ns else "")
        for verb, resource, ns in REFUSED
        if may(verb, resource, ns)
    ]:
        raise SystemExit(f"the launcher's credential reaches further than its work: {wider}")
    print(f"  ✓ {len(NEEDS)} calls granted, {len(REFUSED)} refused, in one namespace")


# ── the server ───────────────────────────────────────────────────────────────


def serve(
    home: Path, cluster: Cluster, templates: Path, api_host: str
) -> tuple[subprocess.Popen[bytes], str]:
    """An aiwatcher of our own, listening where a pod can reach it."""
    global BASE
    binary = Path(os.environ.get("AIWATCHER_BINARY", ROOT / "target" / "debug" / "aiwatcher"))
    if not binary.exists():
        raise SystemExit(
            f"no server binary at {binary}: "
            "`cargo build --bin aiwatcher --features aiwatcher-server/kube`"
        )
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
    api_url = f"http://{api_host}:{port}"
    # Nothing inherited: an AIWATCHER_AUTH_MODE in somebody's shell would make
    # this a different test.
    env = {name: value for name, value in os.environ.items() if not name.startswith("AIWATCHER_")}
    env |= {
        # Every interface, because the pods are not on this host's loopback.
        "AIWATCHER_LISTEN": f"0.0.0.0:{port}",
        "AIWATCHER_DATA_DIR": str(home / ".data"),
        "AIWATCHER_BUS": "wal",
        # The pods and the local worker are other processes claiming over HTTP,
        # and the `file` store refuses a plan that needs one.
        "AIWATCHER_WORKFLOW_STORE": "memory",
        "AIWATCHER_INGEST_ENABLED": "true",
        "AIWATCHER_SEED_FILE": "none",
        "AIWATCHER_POD_TEMPLATES": str(templates),
        "AIWATCHER_POD_NAMESPACE": cluster.namespace,
        "AIWATCHER_POD_API_URL": api_url,
        "KUBECONFIG": str(cluster.kubeconfig),
        "AIWATCHER_LOG": "warn,aiwatcher_server::execution::pods=info",
    }
    log = (home / "server.log").open("wb")
    process = subprocess.Popen(  # noqa: S603 — the binary this repository builds
        [str(binary)], cwd=home, env=env, stdout=log, stderr=subprocess.STDOUT
    )
    BASE = f"http://127.0.0.1:{port}"
    deadline = time.monotonic() + READY_WITHIN
    while time.monotonic() < deadline:
        if process.poll() is not None:
            break
        if call("GET", "/readyz")[0] == 200:
            print(f"· aiwatcher on {BASE}, pods will report to {api_url}")
            return process, api_url
        time.sleep(0.2)
    process.kill()
    tail = (home / "server.log").read_text(errors="replace")[-3000:]
    if "kube" in tail and "AIWATCHER_POD_TEMPLATES" in tail:
        raise SystemExit(
            "this binary has no pod launcher in it — rebuild with "
            f"`cargo build --bin aiwatcher --features aiwatcher-server/kube`:\n{tail}"
        )
    raise SystemExit(f"aiwatcher did not come up on {BASE}:\n{tail}")


def call(method: str, path: str, body: Any = None) -> tuple[int, Any]:
    data = None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(  # noqa: S310 — a URL this script composed
        BASE + path, data=data, method=method
    )
    if data is not None:
        request.add_header("content-type", "application/json")
    try:
        with urllib.request.urlopen(request, timeout=30) as answer:  # noqa: S310
            return answer.status, decoded(answer.read())
    except urllib.error.HTTPError as error:
        return error.code, decoded(error.read())
    except (urllib.error.URLError, TimeoutError):
        return 0, None


def decoded(raw: bytes) -> Any:
    """JSON where there is some. The probes answer in plain text."""
    if not raw:
        return None
    try:
        return json.loads(raw)
    except json.JSONDecodeError:
        return raw.decode(errors="replace")


# ── what one run came to ─────────────────────────────────────────────────────


@dataclass
class Seen:
    """What the cluster showed while one execution ran.

    Accumulated rather than read at the end, because the launcher deletes every
    Job it has read: by the time a run is over there is nothing left to list,
    and that absence is itself one of the assertions.
    """

    jobs: dict[str, dict[str, str]] = field(default_factory=dict)

    def take(self, cluster: Cluster) -> None:
        for job in cluster.jobs():
            metadata = job.get("metadata", {})
            name = metadata.get("name")
            if not isinstance(name, str):
                continue
            annotations = metadata.get("annotations", {})
            self.jobs[name] = {
                "step": str(annotations.get("aiwatcher.dev/step", "")),
                "attempt": str(annotations.get("aiwatcher.dev/attempt", "")),
                "execution": str(annotations.get("aiwatcher.dev/execution", "")),
                "template": str(metadata.get("labels", {}).get("aiwatcher.dev/template", "")),
            }

    def steps(self, execution_id: str) -> dict[str, str]:
        """Which step each Job of this execution was for, by Job name."""
        return {
            name: job["step"] for name, job in self.jobs.items() if job["execution"] == execution_id
        }


def watch(execution_id: str, cluster: Cluster, *, within: int) -> tuple[dict[str, Any], Seen]:
    """Wait for one execution, watching the cluster while it runs."""
    seen = Seen()
    deadline = time.monotonic() + within
    while time.monotonic() < deadline:
        seen.take(cluster)
        status, view = call("GET", f"/api/v1/executions/{execution_id}")
        if status != 200:
            raise SystemExit(f"reading execution {execution_id}: {status} {view}")
        state = view["execution"]["state"]["state_type"]
        if state in ("completed", "failed", "cancelled", "crashed"):
            seen.take(cluster)
            return view, seen
        time.sleep(1)
    raise SystemExit(f"execution {execution_id} did not finish within {within}s")


def fold(execution_id: str, wanted: set[tuple[str, str]]) -> dict[str, Any]:
    """The log's own account of the run — where an artifact's digest lives.

    Waited on until it holds every artifact this is about to read, not until it
    holds any: a run is terminal in the store before the outbox has published
    the last of its facts, so a fold that merely has nodes in it has a node
    whose artifacts are still on their way.
    """
    deadline, found = time.monotonic() + 60, set()
    while time.monotonic() < deadline:
        status, view = call("GET", f"/api/v1/workflow-executions/{execution_id}")
        if status == 200 and view:
            found = set(artifacts_of(view))
            if wanted <= found:
                return view
        time.sleep(0.5)
    raise SystemExit(
        f"the log fold never showed {sorted(wanted - found)} of execution {execution_id}"
    )


def artifacts_of(view: dict[str, Any]) -> dict[tuple[str, str], dict[str, Any]]:
    """Every artifact the fold holds, by (step, artifact name)."""
    found: dict[tuple[str, str], dict[str, Any]] = {}
    for node in view.get("nodes", []):
        for artifact in node.get("artifacts", []):
            found[(node["node_id"], artifact["name"])] = artifact
    return found


def rows_of(home: Path, artifact: dict[str, Any]) -> Any:
    """The stored bytes an artifact points at, read back out of the store.

    The gate is outside the attempt, so it reads the object rather than the
    attempt's own input route — this deployment's store is a directory under
    the data directory, and `object://<key>` is a key in it.
    """
    key = str(artifact["uri"]).removeprefix("object://")
    path = home / ".data" / "prompts" / key
    if not path.exists():
        raise SystemExit(f"{artifact['name']} points at {path}, which is not there")
    return json.loads(path.read_text())


# ── the phases ───────────────────────────────────────────────────────────────


def pod_phase(runtime: Runtime, cluster: Cluster, workflow: Workflow) -> tuple[str, Seen]:
    print("· four stages, four pods")
    handle = runtime.run(workflow)
    view, seen = watch(handle.execution_id, cluster, within=RUN_WITHIN)
    state = view["execution"]["state"]
    if state["state_type"] != "completed":
        raise SystemExit(f"the pod run ended {state}:\n{json.dumps(view, indent=2)[:4000]}")

    jobs = seen.steps(handle.execution_id)
    steps = sorted(jobs.values())
    if steps != sorted(pod_stages.STAGES):
        raise SystemExit(f"expected one Job per stage, saw {jobs}")
    if len(jobs) != len(pod_stages.STAGES):
        raise SystemExit(f"expected {len(pod_stages.STAGES)} Jobs, saw {jobs}")
    for name, step in jobs.items():
        if not name.startswith("aiwatcher-"):
            raise SystemExit(f"Job {name} for {step} is not named from its key")
    print(f"  ✓ {len(jobs)} Jobs, one per stage, each named from its attempt")
    return handle.execution_id, seen


def direct_phase(runtime: Runtime, cluster: Cluster, workflow: Workflow) -> str:
    print("· the same four stages, one long-lived worker")
    handle = runtime.run(workflow)
    view, seen = watch(handle.execution_id, cluster, within=RUN_WITHIN)
    state = view["execution"]["state"]
    if state["state_type"] != "completed":
        raise SystemExit(f"the direct run ended {state}:\n{json.dumps(view, indent=2)[:4000]}")
    if seen.steps(handle.execution_id):
        raise SystemExit("a step naming no template was given a pod")
    print("  ✓ completed, and no Job was created for it")
    return handle.execution_id


#: What each stage hands on, and beside it the artifact saying who ran it.
OUTPUTS = (
    ("acquire", "plan", "acquired_by"),
    ("normalize", "normalized", "normalized_by"),
    ("analyze", "analysis", "analyzed_by"),
    ("persist", "review", "persisted_by"),
)


def compare(home: Path, pods: str, direct: str) -> set[str]:
    print("· the review, byte for byte")
    wanted = {(step, name) for step, rows, by in OUTPUTS for name in (rows, by)}
    by_pods = artifacts_of(fold(pods, wanted))
    by_worker = artifacts_of(fold(direct, wanted))
    for step, name, _ in OUTPUTS:
        one, other = by_pods.get((step, name)), by_worker.get((step, name))
        if one is None or other is None:
            raise SystemExit(
                f"{step}/{name} is missing from one of the two runs\n"
                f"  pods:   {sorted(by_pods)}\n"
                f"  direct: {sorted(by_worker)}"
            )
        if one["digest"] != other["digest"]:
            raise SystemExit(
                f"{step}/{name} differs between the paths:\n"
                f"  pods  {one['digest']}\n  direct {other['digest']}\n"
                f"  pods  {json.dumps(rows_of(home, one))[:600]}\n"
                f"  direct {json.dumps(rows_of(home, other))[:600]}"
            )
    review = rows_of(home, by_pods[("persist", "review")])
    print(f"  ✓ every stage's output has one digest across both paths ({len(review)} rows)")

    # And who ran it differs, or the two runs were the same run twice.
    workers = {}
    for step, _, name in OUTPUTS:
        pod_rows = rows_of(home, by_pods[(step, name)])
        worker_rows = rows_of(home, by_worker[(step, name)])
        workers[step] = (pod_rows[0]["worker"], worker_rows[0]["worker"])
    pod_names = {pods for pods, _ in workers.values()}
    local_names = {local for _, local in workers.values()}
    if len(pod_names) != len(pod_stages.STAGES):
        raise SystemExit(f"the four stages did not run in four different pods: {workers}")
    if not all(name.startswith("aiwatcher-") for name in pod_names):
        raise SystemExit(f"a stage did not run in a launched pod: {pod_names}")
    if local_names != {"long-lived-worker"}:
        raise SystemExit(f"the direct path did not run on the local worker: {local_names}")
    print(f"  ✓ four distinct pods against one worker: {sorted(pod_names)}")
    return pod_names


def cancel_phase(runtime: Runtime, cluster: Cluster, workflow: Workflow) -> None:
    print("· a cancel reaches a running pod")
    handle = runtime.run(workflow)
    # Wait for `analyze`'s own pod, not just for any: the two stages before it
    # have to have run for the cancel to be about a pod holding work.
    seen, deadline = Seen(), time.monotonic() + RUN_WITHIN
    while time.monotonic() < deadline:
        seen.take(cluster)
        if "analyze" in seen.steps(handle.execution_id).values():
            break
        time.sleep(1)
    else:
        raise SystemExit("analyze never got a pod")
    held = {name for name, step in seen.steps(handle.execution_id).items() if step == "analyze"}
    started = time.monotonic()
    handle.cancel("a gate cancelling a running pod")
    view, after = watch(handle.execution_id, cluster, within=CANCEL_WITHIN)
    took = time.monotonic() - started
    state = view["execution"]["state"]
    if state["state_type"] != "cancelled":
        raise SystemExit(f"a cancelled run ended {state}")
    # Polled rather than read once: the attempt ends first and the Job goes
    # after its log is kept, so a run is `cancelled` a moment before the Job it
    # was holding is deleted.
    live = held
    while time.monotonic() < started + CANCEL_WITHIN and live:
        live = held & {job.get("metadata", {}).get("name") for job in cluster.jobs()}
        if live:
            time.sleep(1)
    if live:
        raise SystemExit(f"analyze's Job outlived the cancel: {live}")
    steps = {step["step_id"]: step["state"]["state_type"] for step in view["execution"]["steps"]}
    if steps.get("persist") in ("running", "completed"):
        raise SystemExit(f"a step after the cancelled one ran: {steps}")
    print(f"  ✓ cancelled in {took:.1f}s; analyze's Job is gone and persist never started")
    after.take(cluster)


def oom_phase(runtime: Runtime, cluster: Cluster, workflow: Workflow) -> None:
    """One stage over its limit fails that stage, and the budget decides next.

    The isolation this whole design is for. Nothing here stands in for
    anything: the kernel kills the pod, the Job says it ended, the pod says
    why, and the word in the attempt's error is the cluster's own.
    """
    print("· one stage over its memory limit")
    handle = runtime.run(workflow)
    view, seen = watch(handle.execution_id, cluster, within=RUN_WITHIN)
    state = view["execution"]["state"]
    if state["state_type"] != "failed":
        raise SystemExit(f"a run whose stage cannot fit ended {state}")
    steps = {step["step_id"]: step for step in view["execution"]["steps"]}
    attempts = steps["analyze"]["attempts"]
    if len(attempts) != 2:
        raise SystemExit(f"the budget gave analyze {len(attempts)} attempts, not two")
    for record in attempts:
        error = record.get("error") or {}
        if error.get("class") != "infrastructure":
            raise SystemExit(f"attempt {record['attempt']} failed as {error}")
        if "OOMKilled" not in str(error.get("message", "")):
            raise SystemExit(f"attempt {record['attempt']} does not say why: {error}")
    jobs = seen.steps(handle.execution_id)
    for_analyze = [name for name, step in jobs.items() if step == "analyze"]
    if len(for_analyze) != 2:
        raise SystemExit(f"expected a Job per attempt of analyze, saw {jobs}")
    standing = {name: steps[name]["state"]["state_type"] for name in ("acquire", "normalize")}
    if set(standing.values()) != {"completed"}:
        raise SystemExit(f"the stages before it did not stand: {standing}")
    if steps["persist"]["state"]["state_type"] in ("running", "completed"):
        raise SystemExit("a stage after the one that failed ran")
    print(
        f"  ✓ {attempts[0]['error']['message']} — twice, in two pods; "
        "acquire and normalize stand and persist never started"
    )


def logs_kept(home: Path, cluster: Cluster, pods: set[str]) -> None:
    """Every pod's own words, in the store, after the pod itself is gone.

    Two halves. No Job left behind is the behavioural one: the launcher keeps a
    log and *then* deletes the Job, so a Job that is gone is a log that was
    stored and recorded — on the pass after the attempt ended, which is why
    this waits rather than reading once. And the stored objects are read, each
    pod looked for by the name it printed, because a log is named by the hash
    of its own bytes: counting them counts *distinct* output, and seven pods
    that printed the same thing are one object.
    """
    print("· every pod's log")
    deadline = time.monotonic() + 120
    left: set[str | None] = set()
    while time.monotonic() < deadline:
        left = {job.get("metadata", {}).get("name") for job in cluster.jobs()}
        if not left:
            break
        time.sleep(2)
    if left:
        raise SystemExit(f"Jobs the launcher never finished with: {left} — its log is why")
    stored = sorted((home / ".data" / "prompts" / "artifacts" / "log").glob("*/*/data"))
    kept = [path.read_text(errors="replace") for path in stored]
    missing = {pod for pod in pods if not any(pod in text for text in kept)}
    if missing:
        raise SystemExit(f"nothing in the store holds what these pods printed: {sorted(missing)}")
    size = sum(path.stat().st_size for path in stored)
    print(
        f"  ✓ no Job left; {len(stored)} logs stored ({size} bytes), all {len(pods)} pods in them"
    )


# ── plumbing ─────────────────────────────────────────────────────────────────


def run(*args: str, check: bool = True, cwd: Path | None = None) -> str:
    outcome = subprocess.run(  # noqa: S603 — commands this script composes itself
        list(args), capture_output=True, text=True, check=False, cwd=cwd
    )
    if check and outcome.returncode != 0:
        raise SystemExit(f"{' '.join(args)} failed:\n{outcome.stdout}{outcome.stderr}")
    return outcome.stdout


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--namespace", default="aiwatcher-pod-e2e")
    parser.add_argument("--api-host", default=HOST_ALIAS, help="how a pod reaches this host")
    parser.add_argument(
        "--no-build", action="store_true", help="use the image and binary that are there"
    )
    parser.add_argument("--keep", action="store_true", help="leave the namespace and the data")
    arguments = parser.parse_args()

    context = current_context()
    if not local(context):
        raise SystemExit(
            f"refusing to run against {context!r}: it is not a known-local cluster. "
            "This kubeconfig has production clusters in it; switch with "
            "`kubectl config use-context orbstack`."
        )
    print(f"· cluster {context}")

    if not arguments.no_build:
        build_server()
        build_image()

    home = Path(tempfile.mkdtemp(prefix="aiwatcher-pods-"))
    server: subprocess.Popen[bytes] | None = None
    cluster: Cluster | None = None
    try:
        cluster = open_namespace(kubeconfig_for(context, home), arguments.namespace)
        print(f"· namespace {cluster.namespace}")
        grant(cluster)
        server, api_url = serve(home, cluster, templates_file(home), arguments.api_host)
        reachable(cluster, api_url)

        # 192Mi is the step's own ask, which is its request and its limit both,
        # so a `hog_mb` above it is a pod the kernel stops.
        pod = PodRequest(template=TEMPLATE, image=IMAGE, memory="192Mi")
        in_pods = pod_stages.build_workflow(pod)
        direct = pod_stages.build_workflow()
        holding = pod_stages.build_workflow(pod, hold_seconds=120)
        hogging = pod_stages.build_workflow(pod, hog_mb=512)
        flows = (in_pods, direct, holding, hogging)
        runtime = Runtime(
            name="pods-e2e",
            url=BASE,
            workflows=list(flows),
            pools=[ExecutionPool("local", QUEUE, concurrency=1)],
            placement={flow.ref: "local" for flow in flows},
            poll_interval=0.2,
        )
        with runtime:
            runtime.register()
            # Up for every phase, registered for every task, polling the queue
            # the pods' attempts are on. Nothing but the key-only claim rule
            # keeps it off them.
            runtime.start()
            in_pods_id, _ = pod_phase(runtime, cluster, in_pods)
            direct_id = direct_phase(runtime, cluster, direct)
            pods = compare(home, in_pods_id, direct_id)
            cancel_phase(runtime, cluster, holding)
            oom_phase(runtime, cluster, hogging)
        logs_kept(home, cluster, pods)
    finally:
        if server is not None and server.poll() is None:
            server.terminate()
            try:
                server.wait(timeout=15)
            except subprocess.TimeoutExpired:
                server.kill()
        if cluster is not None and not arguments.keep:
            cluster.teardown()
        if arguments.keep:
            print(f"· kept {home}")
    print(
        "\n✓ four stages in four pods, the same review, a cancel that arrived, "
        "a stage the kernel stopped, every log kept"
    )


if __name__ == "__main__":
    main()
