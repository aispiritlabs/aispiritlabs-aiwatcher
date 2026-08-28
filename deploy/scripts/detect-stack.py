#!/usr/bin/env python3
"""Report which parts of the aiwatcher stack a cluster already runs.

The chart can install VictoriaTraces, VictoriaMetrics and a Collector, and most
clusters that would install aiwatcher already have some of them. Installing a
second VictoriaMetrics beside an existing one is not a harmless duplicate: it
splits the metrics for the same workload across two stores, and the dashboards
that read one silently stop covering the other half.

So this looks first, and prints what it found as helmfile state values:

    detected:
      victoriametrics:
        found: true
        url: http://planner-victoria-metrics.planner.svc.cluster.local:8428
        podSelector: {...}

What counts as "found" is a *pod running the right image* with a *Service in
front of it on the right port* — not a name, which is a convention, and not a
Service alone, which may point at nothing. The Service's selector is reported
too, because it is exactly the label set a NetworkPolicy needs in order to grant
aiwatcher's Collector a path to pods this chart does not own.

Exit code is always 0. A cluster that cannot be reached is a result
(`reachable: false`), not a crash: helmfile calls this while rendering, and a
developer running `helmfile template` on a laptop with no cluster should get a
render, not a stack trace. The install script checks `reachable` itself before
it touches anything.

Overrides, because detection is a convenience and never the last word:

    AIWATCHER_DETECT=off                     skip it entirely; find nothing
    AIWATCHER_VICTORIAMETRICS_URL=http://…   force this, do not look
    AIWATCHER_VICTORIATRACES_URL=http://…
    AIWATCHER_COLLECTOR_URL=http://…
    AIWATCHER_GRAFANA_URL=http://…
    AIWATCHER_<NAME>_URL=none                force "not present" for one target
    AIWATCHER_DOMAIN=aiwatcher.example.com   publish on this host, do not derive one
    AIWATCHER_DOMAIN=none                    derive nothing

The last two are about the other thing this reads: the domain the cluster
already publishes under. `planner.example.com` and `grafana.example.com` say
that a sibling belongs at `aiwatcher.example.com`, and a host that has to be
typed once per install is a host that ends up not typed at all — which installs
a release with no ingress at all, reachable only by port-forward, and says
nothing about it.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from dataclasses import dataclass, field


@dataclass(frozen=True)
class Target:
    """One thing worth looking for."""

    key: str
    # Substrings matched against every container image. A registry mirror or a
    # digest pin changes the prefix and the suffix but not this middle part.
    images: tuple[str, ...]
    # The Service port that makes it usable. A Service in front of the right
    # pods but on some other port is a different thing (a metrics sidecar, an
    # admin UI) and is not what we would point the Collector at.
    port: int
    # Images that contain one of `images` as a substring but are something else.
    not_images: tuple[str, ...] = ()
    note: str = ""


TARGETS: tuple[Target, ...] = (
    Target(
        key="victoriametrics",
        images=("victoria-metrics", "vmsingle"),
        port=8428,
        # The operator manages VictoriaMetrics; it is not one. vminsert and
        # vmselect are the cluster version, whose write path is a different port
        # and a tenant-scoped URL — set AIWATCHER_VICTORIAMETRICS_URL by hand
        # for those.
        not_images=("victoria-metrics-operator", "vminsert", "vmselect", "vmstorage", "vmagent", "vmalert"),
        note="cluster-mode VictoriaMetrics is not detected; set AIWATCHER_VICTORIAMETRICS_URL",
    ),
    Target(key="victoriatraces", images=("victoria-traces", "vtsingle"), port=10428),
    Target(key="collector", images=("opentelemetry-collector",), port=4318),
    Target(key="grafana", images=("grafana/grafana",), port=3000),
    # Reported, never derived — see `as_helm_values`. An object store is found
    # by its image; the credentials that would make it usable are not
    # discoverable from here, and neither is whether aiwatcher may create a
    # bucket in it.
    Target(
        key="objectstore",
        images=("minio/minio", "rustfs/rustfs", "chrislusf/seaweedfs", "quay.io/minio/minio"),
        port=9000,
        # The MinIO operator manages MinIO; it is not one. `mc` is the client.
        not_images=("minio-operator", "operator:", "minio/mc", "minio/console"),
        note="an S3 endpoint is reported, never reused: its credentials are not discoverable",
    ),
)


@dataclass
class Finding:
    found: bool = False
    namespace: str = ""
    service: str = ""
    url: str = ""
    pod_selector: dict[str, str] = field(default_factory=dict)
    reason: str = ""
    source: str = "detected"
    # Labels of the pod that matched, and whether some NetworkPolicy already
    # restricts ingress to it. See `fenced_by_policy`.
    pod_labels: dict[str, str] = field(default_factory=dict)
    fenced: bool = False
    fenced_by: str = ""


# The label an aiwatcher release is published under, in front of whatever domain
# the cluster turns out to use.
SUBDOMAIN = "aiwatcher"


@dataclass
class Domain:
    """The host aiwatcher would be published on.

    Not a Finding: there is no image to match and no Service to point at. What
    is read here is a convention rather than a component — every other ingress
    in the namespace, agreeing on a domain.

    It carries the host and nothing else on purpose. Whether to publish at all
    is not discoverable: an ingress in front of aiwatcher is safe exactly where
    something authenticates it, and no ingress in the cluster says whether this
    one would be. That decision stays in the environment file, beside the
    middlewares that make it true. See ADR_0009.
    """

    host: str = ""
    base: str = ""
    source: str = "detected"
    reason: str = ""
    derived_from: tuple[str, ...] = ()


def kubectl(args: list[str], context: str | None) -> tuple[bool, str]:
    command = ["kubectl"]
    if context:
        command += ["--context", context]
    command += args
    try:
        result = subprocess.run(command, capture_output=True, text=True, timeout=30)
    except FileNotFoundError:
        return False, "kubectl is not installed"
    except subprocess.TimeoutExpired:
        return False, "kubectl timed out after 30s"
    if result.returncode != 0:
        return False, (result.stderr.strip().splitlines() or ["kubectl failed"])[-1]
    return True, result.stdout


def images_of(pod: dict) -> list[str]:
    spec = pod.get("spec", {})
    containers = list(spec.get("containers", [])) + list(spec.get("initContainers", []))
    return [c.get("image", "") for c in containers]


def matches(target: Target, image: str) -> bool:
    if any(bad in image for bad in target.not_images):
        return False
    return any(good in image for good in target.images)


def selects(selector: dict[str, str], labels: dict[str, str]) -> bool:
    """A Service selector picks a pod when every entry of it is on the pod."""
    return bool(selector) and all(labels.get(k) == v for k, v in selector.items())


def label_selector_matches(selector: dict, labels: dict[str, str]) -> bool:
    """Evaluate a Kubernetes LabelSelector — the NetworkPolicy kind, not a Service's.

    An empty or absent selector selects every pod in the namespace, which is
    what a namespace-wide default-deny policy uses.
    """
    if not selector:
        return True
    for key, value in (selector.get("matchLabels") or {}).items():
        if labels.get(key) != value:
            return False
    for expression in selector.get("matchExpressions") or []:
        key = expression.get("key", "")
        operator = expression.get("operator", "")
        values = expression.get("values", []) or []
        present = key in labels
        if operator == "In" and (not present or labels[key] not in values):
            return False
        if operator == "NotIn" and present and labels[key] in values:
            return False
        if operator == "Exists" and not present:
            return False
        if operator == "DoesNotExist" and present:
            return False
    return True


def fenced_by_policy(namespace: str, labels: dict[str, str], policies: list[dict]) -> str:
    """Name of a NetworkPolicy that already restricts ingress to these pods, if any.

    This decides whether aiwatcher may attach an ingress rule of its own to pods
    it does not own, and the asymmetry is the whole point. Pods that no policy
    selects accept everything; pods that any ingress policy selects accept only
    the union of what those policies allow. So attaching a rule to pods that are
    already fenced *widens* their access by exactly one path, while attaching
    one to unfenced pods *narrows* them from "everything" to "aiwatcher only" —
    which would cut off whoever was already talking to them.

    Only the first case is safe to do to someone else's release.
    """
    for policy in policies:
        meta = policy.get("metadata", {})
        if meta.get("namespace") != namespace:
            continue
        spec = policy.get("spec", {})
        types = spec.get("policyTypes") or ["Ingress"]
        if "Ingress" not in types:
            continue
        if label_selector_matches(spec.get("podSelector") or {}, labels):
            return str(meta.get("name", ""))
    return ""


def find(target: Target, pods: list[dict], services: list[dict], prefer_namespace: str) -> Finding:
    candidates = [p for p in pods if any(matches(target, i) for i in images_of(p))]
    if not candidates:
        return Finding(reason="no pod runs a matching image")

    # A pod in the namespace we are installing into wins: it is the one the
    # release will actually be able to reach without crossing a NetworkPolicy.
    def rank(pod: dict) -> tuple[int, str, str]:
        meta = pod.get("metadata", {})
        namespace = meta.get("namespace", "")
        return (0 if namespace == prefer_namespace else 1, namespace, meta.get("name", ""))

    for pod in sorted(candidates, key=rank):
        meta = pod.get("metadata", {})
        namespace = meta.get("namespace", "")
        labels = meta.get("labels", {}) or {}
        for service in sorted(services, key=lambda s: s["metadata"]["name"]):
            service_meta = service.get("metadata", {})
            if service_meta.get("namespace") != namespace:
                continue
            selector = service.get("spec", {}).get("selector") or {}
            if not selects(selector, labels):
                continue
            ports = service.get("spec", {}).get("ports", []) or []
            if not any(p.get("port") == target.port for p in ports):
                continue
            name = service_meta.get("name", "")
            return Finding(
                found=True,
                namespace=namespace,
                service=name,
                url=f"http://{name}.{namespace}.svc.cluster.local:{target.port}",
                pod_selector={str(k): str(v) for k, v in selector.items()},
                pod_labels={str(k): str(v) for k, v in labels.items()},
            )

    first = sorted(candidates, key=rank)[0]["metadata"]
    return Finding(
        reason=(
            f"{first.get('namespace')}/{first.get('name')} runs a matching image but no "
            f"Service in front of it exposes port {target.port}"
        )
    )


def base_of(host: str) -> str:
    """The domain a sibling of `host` would live under.

    `planner.example.com` → `example.com`. A two-label host is already the
    domain, so `example.com` → `example.com` and the sibling is
    `aiwatcher.example.com` — never `aiwatcher.com`, which is somebody else's.
    """
    labels = host.split(".")
    return ".".join(labels[1:]) if len(labels) > 2 else host


def derive_domain(ingresses: list[dict], namespace: str) -> Domain:
    """Where a sibling of the cluster's existing ingresses would go."""
    hosts: list[tuple[str, str]] = []
    for ingress in ingresses:
        meta = ingress.get("metadata", {})
        labels = meta.get("labels") or {}
        # Our own ingress from a previous install is not evidence: deriving from
        # it would make the answer depend on whether this ran before. Same
        # reason the pod list drops part-of: aiwatcher.
        if labels.get("app.kubernetes.io/part-of") == "aiwatcher":
            continue
        for rule in ingress.get("spec", {}).get("rules") or []:
            host = str(rule.get("host") or "")
            # A rule with no host matches every name and a wildcard names no
            # single one. Neither says where a sibling belongs.
            if not host or "*" in host:
                continue
            hosts.append((host, f"{meta.get('namespace')}/{meta.get('name')}"))

    if not hosts:
        return Domain(reason="no Ingress in the cluster names a host")

    # An ingress in the namespace being installed into is the strongest evidence
    # there is: it is the convention of the thing aiwatcher is going in beside.
    same = [entry for entry in hosts if entry[1].split("/", 1)[0] == namespace]
    considered = same or hosts

    tally: dict[str, list[str]] = {}
    for host, ref in considered:
        tally.setdefault(base_of(host), []).append(f"{host} ({ref})")

    ranked = sorted(tally.items(), key=lambda item: (-len(item[1]), item[0]))
    # A tie is not a majority, and picking one alphabetically would publish on
    # whichever domain sorts first — a coin toss nobody would read as one.
    if len(ranked) > 1 and len(ranked[0][1]) == len(ranked[1][1]):
        names = ", ".join(base for base, _ in ranked)
        return Domain(reason=f"more than one domain in use ({names}); set AIWATCHER_DOMAIN")

    base, evidence = ranked[0]
    where = "in this namespace" if same else "in the cluster"
    return Domain(
        host=f"{SUBDOMAIN}.{base}",
        base=base,
        derived_from=tuple(sorted(evidence)),
        reason=f"{len(evidence)} ingress host(s) {where} are under {base}",
    )


def domain_override() -> Domain | None:
    raw = os.environ.get("AIWATCHER_DOMAIN", "").strip()
    if not raw:
        return None
    if raw.lower() in {"none", "off", "false"}:
        return Domain(reason="forced off by AIWATCHER_DOMAIN", source="override")
    return Domain(host=raw, base=base_of(raw), source="override", reason="set by AIWATCHER_DOMAIN")


def override_for(target: Target) -> Finding | None:
    raw = os.environ.get(f"AIWATCHER_{target.key.upper()}_URL", "").strip()
    if not raw:
        return None
    if raw.lower() in {"none", "off", "false"}:
        return Finding(reason="forced off by AIWATCHER_%s_URL" % target.key.upper(), source="override")
    return Finding(found=True, url=raw.rstrip("/"), source="override",
                   reason="set by AIWATCHER_%s_URL" % target.key.upper())


def as_yaml(findings: dict[str, Finding], domain: Domain, reachable: bool, reason: str, context: str) -> str:
    def scalar(value: object) -> str:
        if isinstance(value, bool):
            return "true" if value else "false"
        return json.dumps(str(value))

    lines = [
        "# Generated by deploy/scripts/detect-stack.py — do not edit; re-run it.",
        "detected:",
        "  cluster:",
        f"    reachable: {scalar(reachable)}",
        f"    context: {scalar(context)}",
        f"    reason: {scalar(reason)}",
    ]
    for key, finding in findings.items():
        lines.append(f"  {key}:")
        lines.append(f"    found: {scalar(finding.found)}")
        lines.append(f"    url: {scalar(finding.url)}")
        lines.append(f"    namespace: {scalar(finding.namespace)}")
        lines.append(f"    service: {scalar(finding.service)}")
        lines.append(f"    source: {scalar(finding.source)}")
        lines.append(f"    reason: {scalar(finding.reason)}")
        lines.append(f"    fenced: {scalar(finding.fenced)}")
        lines.append(f"    fencedBy: {scalar(finding.fenced_by)}")
        lines.append("    podSelector:")
        for label, value in sorted(finding.pod_selector.items()):
            lines.append(f"      {scalar(label)}: {scalar(value)}")
        if not finding.pod_selector:
            lines[-1] = "    podSelector: {}"
    lines.append("  domain:")
    lines.append(f"    host: {scalar(domain.host)}")
    lines.append(f"    base: {scalar(domain.base)}")
    lines.append(f"    source: {scalar(domain.source)}")
    lines.append(f"    reason: {scalar(domain.reason)}")
    lines.append("    derivedFrom:")
    for evidence in domain.derived_from:
        lines.append(f"      - {scalar(evidence)}")
    if not domain.derived_from:
        lines[-1] = "    derivedFrom: []"
    return "\n".join(lines) + "\n"


def as_text(findings: dict[str, Finding], domain: Domain, reachable: bool, reason: str, context: str) -> str:
    lines = []
    if reason == "detection disabled":
        lines.append("cluster: not consulted (AIWATCHER_DETECT=off)")
    elif not reachable:
        lines.append(f"cluster: unreachable ({reason})")
    else:
        lines.append(f"cluster: {context}")
    width = max(len(k) for k in findings)
    for key, finding in findings.items():
        if finding.found:
            where = finding.url
            if finding.source == "override":
                where += "   (forced)"
            lines.append(f"  {key.ljust(width)}  present   {where}")
            if finding.fenced:
                lines.append(f"  {' ' * width}            fenced by NetworkPolicy {finding.fenced_by}")
        else:
            lines.append(f"  {key.ljust(width)}  absent    {finding.reason}")
    label = "domain".ljust(width)
    if domain.host:
        forced = "   (forced)" if domain.source == "override" else ""
        lines.append(f"  {label}  derived   {domain.host}{forced}")
        if domain.source != "override":
            lines.append(f"  {' ' * width}            {domain.reason}")
    else:
        lines.append(f"  {label}  unknown   {domain.reason}")
    return "\n".join(lines) + "\n"


def as_helm_values(findings: dict[str, Finding], domain: Domain, reachable: bool, namespace: str) -> str:
    """The findings, translated into chart values.

    This is the only place that decides "found means external". The helmfile
    feeds it straight to the release, and a plain
    `helm upgrade -f <(detect-stack.py --format helm-values)` makes exactly the
    same decision — there is no second copy of this mapping to drift.
    """
    lines = [
        "# Generated by deploy/scripts/detect-stack.py --format helm-values.",
        "# Regenerate rather than edit: it is a picture of the cluster, not a preference.",
    ]

    # An override is knowledge whether or not the cluster answered, so it still
    # produces values. Nothing known at all does not.
    forced = any(f.source == "override" for f in findings.values()) or domain.source == "override"
    if not reachable and not forced:
        # Every mode stays at its chart default, which installs the lot. Said
        # out loud rather than implied, because "the cluster was not read, so we
        # assumed it was empty" is exactly the sentence that produces a second
        # VictoriaMetrics.
        lines.append("# The cluster was not read — unreachable, or AIWATCHER_DETECT=off with no")
        lines.append("# AIWATCHER_*_URL set. Nothing is overridden here, so the chart defaults")
        lines.append("# apply and this release would install a full stack of its own.")
        return "\n".join(lines) + "\n{}\n"

    # The host, and only the host. Whether to publish is not a finding — see
    # the Domain docstring — so this never writes ingress.enabled. An
    # environment that asks for an ingress and gets no host here does not
    # quietly install without one: the chart fails on the empty host, which is
    # the loud version of the failure this derivation exists to prevent.
    if domain.host:
        lines.append("ingress:")
        lines.append(f"  host: {json.dumps(domain.host)}")
        lines.append(f"  # {domain.reason}")
    else:
        lines.append(f"# No ingress host: {domain.reason}.")
        lines.append("# An environment with ingress.enabled will fail to render rather than")
        lines.append("# publish on an empty host. Set AIWATCHER_DOMAIN to name one.")

    for key in ("victoriametrics", "victoriatraces"):
        finding = findings[key]
        lines.append(f"{key}:")
        if finding.found:
            lines.append("  mode: external")
            lines.append("  external:")
            lines.append(f"    url: {json.dumps(finding.url)}")
            lines.append(f"    namespace: {json.dumps(finding.namespace)}")
            if finding.pod_selector:
                lines.append("    podSelector:")
                for label, value in sorted(finding.pod_selector.items()):
                    lines.append(f"      {json.dumps(label)}: {json.dumps(value)}")
            else:
                lines.append("    podSelector: {}")
        else:
            lines.append("  mode: install")
            lines.append(f"  # nothing found: {finding.reason}")

    # See fenced_by_policy: attaching an ingress rule to pods that no policy
    # selects would narrow them, not widen them. And a NetworkPolicy only
    # applies inside its own namespace, so a component found somewhere else is
    # not ours to unfence however much the Collector needs it.
    lines.append("networkPolicy:")
    for key, flag in (
        ("victoriametrics", "allowEgressToExternalVictoriaMetrics"),
        ("victoriatraces", "allowEgressToExternalVictoriaTraces"),
    ):
        finding = findings[key]
        needs = bool(finding.found and finding.fenced and finding.pod_selector and finding.source != "override")
        same_namespace = finding.namespace == namespace
        attach = needs and same_namespace
        lines.append(f"  {flag}: {'true' if attach else 'false'}")
        if attach:
            lines.append(f"  # {key} is fenced by NetworkPolicy {finding.fenced_by}; this adds one ingress rule for aiwatcher's Collector.")
        elif needs:
            lines.append(f"  # {key} is fenced by NetworkPolicy {finding.fenced_by} in namespace {finding.namespace},")
            lines.append(f"  # and this release goes into {namespace}. A NetworkPolicy only applies in its own")
            lines.append(f"  # namespace, so aiwatcher cannot add that rule from here — whoever owns")
            lines.append(f"  # {finding.namespace} has to. Until they do, writes from the Collector are refused.")

    # Never derived. A foreign Collector has neither this one's VictoriaTraces
    # exporter nor its redaction processor, so reusing it is a decision with a
    # blast radius and stays a human one.
    if findings["collector"].found:
        lines.append(f"# A Collector is already running at {findings['collector'].url}, and this")
        lines.append("# release still installs its own. Set collector.mode=external by hand if that")
        lines.append("# one redacts gen_ai.* and exports to VictoriaTraces — see docs/INSTALL.md.")
    if findings["grafana"].found:
        lines.append(f"# Grafana found at {findings['grafana'].url}. The datasource ConfigMap is")
        lines.append("# emitted; wiring it in is a change on Grafana's side — see docs/INSTALL.md.")

    # The same rule as the Collector, for a different reason. There, reuse is
    # unsafe; here it is undecidable: nothing in the cluster says which
    # credentials aiwatcher may use, which bucket it may write, or whether it
    # may create one. So this release installs its own store and says what it
    # found.
    store = findings["objectstore"]
    if store.found:
        lines.append(f"# An S3-compatible store is running at {store.url}, and this release")
        lines.append("# still installs its own RustFS for the prompt registry. To use that one instead,")
        lines.append("# set promptStore.mode=external plus the endpoint and a credentials Secret —")
        lines.append("# neither is discoverable from here, which is why this is not derived:")
        lines.append("#")
        lines.append("#   promptStore:")
        lines.append("#     mode: external")
        lines.append("#     external:")
        lines.append(f"#       endpoint: {store.url}")
        if store.pod_selector:
            lines.append(f"#       namespace: {json.dumps(store.namespace)}")
            lines.append("#       podSelector:")
            for label, value in sorted(store.pod_selector.items()):
                lines.append(f"#         {json.dumps(label)}: {json.dumps(value)}")
        lines.append("#     credentialsSecret: { name: <secret>, accessKeyKey: ..., secretKeyKey: ... }")
        if store.fenced:
            lines.append(f"# It is fenced by NetworkPolicy {store.fenced_by}, so that also needs")
            lines.append("# networkPolicy.allowEgressToExternalPromptStore=true.")
        else:
            lines.append("# Nothing fences it, so leave networkPolicy.allowEgressToExternalPromptStore")
            lines.append("# off: a rule attached to unfenced pods would cut off its existing clients.")

    return "\n".join(lines) + "\n"


def render(fmt: str, findings: dict[str, Finding], domain: Domain, reachable: bool, reason: str,
           context: str, namespace: str) -> str:
    if fmt == "helm-values":
        return as_helm_values(findings, domain, reachable, namespace)
    if fmt == "text":
        return as_text(findings, domain, reachable, reason, context)
    return as_yaml(findings, domain, reachable, reason, context)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("-n", "--namespace", default=os.environ.get("AIWATCHER_NAMESPACE", "aiwatcher"),
                        help="the namespace aiwatcher is being installed into; a match there wins over one elsewhere")
    parser.add_argument("--context", default=os.environ.get("AIWATCHER_K8S_CONTEXT") or None)
    parser.add_argument("--format", choices=("yaml", "text", "helm-values"), default="yaml",
                        help="yaml: helmfile state values. helm-values: chart values, ready for helm -f. text: for humans.")
    args = parser.parse_args()

    context = args.context or ""
    findings: dict[str, Finding] = {}

    if os.environ.get("AIWATCHER_DETECT", "").lower() == "off":
        for target in TARGETS:
            findings[target.key] = override_for(target) or Finding(reason="detection disabled (AIWATCHER_DETECT=off)",
                                                                   source="override")
        off = domain_override() or Domain(reason="detection disabled (AIWATCHER_DETECT=off)", source="override")
        sys.stdout.write(render(args.format, findings, off, False, "detection disabled", context, args.namespace))
        return 0

    ok_pods, pods_raw = kubectl(["get", "pods", "--all-namespaces", "-o", "json"], args.context)
    ok_services, services_raw = kubectl(["get", "services", "--all-namespaces", "-o", "json"], args.context)
    reachable = ok_pods and ok_services
    reason = "" if reachable else (pods_raw if not ok_pods else services_raw)

    # Neither of these is required: a token that may not list NetworkPolicies
    # still gets a usable answer, it just never offers to attach an ingress
    # rule, and one that may not list Ingresses gets one without a host.
    ok_policies, policies_raw = kubectl(["get", "networkpolicies", "--all-namespaces", "-o", "json"], args.context)
    ok_ingresses, ingresses_raw = kubectl(["get", "ingresses", "--all-namespaces", "-o", "json"], args.context)

    pods: list[dict] = []
    services: list[dict] = []
    policies: list[dict] = []
    ingresses: list[dict] = []
    if reachable:
        try:
            pods = json.loads(pods_raw).get("items", [])
            services = json.loads(services_raw).get("items", [])
            policies = json.loads(policies_raw).get("items", []) if ok_policies else []
            ingresses = json.loads(ingresses_raw).get("items", []) if ok_ingresses else []
        except json.JSONDecodeError as error:
            reachable, reason = False, f"kubectl returned something that is not JSON: {error}"

    if reachable and not context:
        ok_context, current = kubectl(["config", "current-context"], None)
        context = current.strip() if ok_context else ""

    # aiwatcher's own pods carry app.kubernetes.io/part-of: aiwatcher. Without
    # this, a second `helmfile apply` would find the VictoriaTraces the first
    # one installed, call it external, and uninstall it — flipping the stack
    # back and forth on every run.
    pods = [p for p in pods if (p.get("metadata", {}).get("labels") or {}).get("app.kubernetes.io/part-of") != "aiwatcher"]

    for target in TARGETS:
        forced = override_for(target)
        if forced is not None:
            findings[target.key] = forced
        elif not reachable:
            findings[target.key] = Finding(reason="cluster not reached")
        else:
            finding = find(target, pods, services, args.namespace)
            if finding.found and finding.pod_labels:
                finding.fenced_by = fenced_by_policy(finding.namespace, finding.pod_labels, policies)
                finding.fenced = bool(finding.fenced_by)
            findings[target.key] = finding

    domain = domain_override() or (
        derive_domain(ingresses, args.namespace) if reachable
        else Domain(reason="cluster not reached")
    )

    sys.stdout.write(render(args.format, findings, domain, reachable, reason, context, args.namespace))
    return 0


if __name__ == "__main__":
    sys.exit(main())
