# ADR_0009: Installation decides what to install by reading the cluster, not by flags

- **Status**: accepted
- **Date**: 2026-08-28

## Context

aiwatcher is five things in a trench coat: the projector/API, the panel, an
OpenTelemetry Collector, VictoriaTraces and VictoriaMetrics. `deploy/k8s` and
`deploy/docker-compose.yml` install all five, which is right for a test stack on
an empty namespace and wrong for every cluster that would actually run this.

The concrete case that forced it: planner's k3s. It already runs
VictoriaMetrics, VictoriaLogs and Grafana, behind authentik, with a
NetworkPolicy per component. It runs MLflow for the job aiwatcher is meant to
take over. What aiwatcher genuinely adds there is the projector, the panel and a
trace store — three of the five.

Getting that wrong is not a loud failure. A second VictoriaMetrics comes up
green beside the first and splits the same workload's metrics across two stores;
every dashboard reading either one keeps working and quietly covers half the
data. There is no error, only a gap in a graph that nobody attributes to the
install.

The usual answer is a values file per environment — `victoriametrics.enabled:
false` written by hand. That is a statement about the cluster kept somewhere the
cluster cannot correct, and it is wrong the first time someone installs
VictoriaMetrics and forgets to update it, or removes one and forgets the same.

## Decision

`deploy/helmfile.yaml.gotmpl` runs `deploy/scripts/detect-stack.py` while it renders,
and the findings decide the release's values.

Three parts to it:

**What "present" means is a pod, not a name.** A pod running a matching image,
with a Service in front of it exposing the expected port. Names are a
convention; a Service alone may point at nothing. The Service's selector is
reported too, because it is exactly the label set a NetworkPolicy needs.

**Every backend is a three-way mode, never a boolean.** `install | external |
none`. A boolean cannot tell "do not install it, it already exists" apart from
"do not install it, we do not want it", and those need different wiring: one
points the Collector somewhere, the other removes a pipeline.

**The mapping from findings to values lives in one place** —
`detect-stack.py --format helm-values`. The helmfile pipes that in and adds no
logic of its own, so `helm upgrade -f <(detect-stack.py --format helm-values)`
reaches the same result and there is no second copy to drift.

**The domain is read the same way.** A cluster already serving
`planner.example.com` and `grafana.example.com` is saying where a sibling
belongs, so detection derives `aiwatcher.example.com` and passes it in as
`ingress.host`. Two-label hosts stay whole (`example.com` yields
`aiwatcher.example.com`, never `aiwatcher.com`), wildcard and host-less rules
are ignored, aiwatcher's own ingress is skipped so a second run does not derive
from the first, and more than one domain in use is reported rather than settled
by sort order.

Three things are deliberately *not* derived:

* **The Collector is never reused automatically.** It is detected, reported, and
  the install still creates its own. A foreign Collector almost certainly has
  neither the VictoriaTraces exporter nor the `attributes/redact` processor that
  drops `gen_ai.prompt` and `gen_ai.completion`. Routing prompt text through an
  unknown pipeline is not a conclusion to arrive at by inference.
* **A NetworkPolicy is attached to someone else's pods only when they are
  already fenced.** Policies are additive, so adding an ingress rule to pods
  that a policy already restricts widens them by exactly one path — which is how
  aiwatcher's Collector reaches planner's VictoriaMetrics without editing
  planner's chart. But adding one to pods that *no* policy selects narrows them,
  from "accepts everything" to "accepts aiwatcher only", cutting off whoever was
  already talking to them. So the script reports whether a matching ingress
  policy exists, and only then is the rule attached.
* **Whether to publish is not derived, only where.** An ingress in front of
  aiwatcher is safe exactly where something authenticates it, and no ingress in
  a cluster says whether the route it describes is guarded. So the host is a
  finding and the switch is not: `environments/planner.yaml` turns
  `ingress.enabled` on in the same file that attaches planner's authentik
  middlewares, and the default environment leaves it off. An environment that
  asks for an ingress and gets no host fails to render — which is the loud form
  of the failure this pairing exists to prevent, a release that installs
  cleanly and answers 404 on the host the SSO app points at.

Detection always exits 0. An unreachable cluster is a result (`reachable:
false`), not a crash, so `helmfile template` works on a laptop with no cluster.
The install script checks that field itself and refuses to apply without it.

## Alternatives considered

**A values file per environment.** Simplest, and the normal thing. Rejected
because it stores a claim about the cluster outside the cluster: correct on the
day it is written and silently wrong afterwards. It remains available — every
detected value can be overridden — but as the exception, not the interface.

**Helm `lookup`.** Helm can query the cluster from inside a template. Rejected
because `lookup` returns empty during `helm template` and `--dry-run`, so the
rendered output would differ from what gets applied — which makes `--plan`
useless exactly when it matters. Detection outside the template renders the
same thing it applies.

**Sub-charts with conditions** (`victoria-metrics.enabled`). Rejected because
the upstream charts bring their own naming, their own values trees and their own
release-name coupling, and the three components here are a Deployment, a Service
and a PVC each. The chart is smaller than the wiring would be.

**Kustomize overlays**, which is what `deploy/k8s` already uses. Rejected for
this path because an overlay per target cluster is the values-file problem with
more files, and because there is nowhere in kustomize to put "ask the cluster".
`deploy/k8s` stays as the Tilt test stack (ADR_0006); this is the install path.

**An operator.** Correct answer at ten clusters. At two it is a controller to
run, upgrade and debug in order to avoid running a script.

## Consequences

Installing needs `helmfile`, the `helm-diff` plugin and read access to pods,
services and networkpolicies across namespaces. That is more than `helm install`
needs, and the preflight in `install.sh` names each missing piece rather than
failing from inside helmfile.

Rendering costs three `kubectl get --all-namespaces` calls, twice, because the
helmfile calls the script for both output formats. On a small cluster that is
about a second. On a large one it will be noticed, and the fix is caching the
report to a file rather than making the templates smarter.

`helmfile template` on a machine with no cluster renders the full stack, because
"unreachable" falls back to the chart defaults. That is the right render to show
and the wrong thing to apply, which is why the check lives in `install.sh`.

Detection can be fooled by an image name it does not know — a VictoriaMetrics
pulled through a registry whose path does not contain `victoria-metrics`, or the
cluster version, whose write path is a different port. Both produce a *visible*
wrong answer (`absent`, and then a duplicate install in the plan) rather than a
silent one, and `AIWATCHER_VICTORIAMETRICS_URL` is the override.

**What would make this wrong.** If the override variables turn out to be what
people actually use — if `AIWATCHER_*_URL` appears in most installs — then
detection is guessing badly and the honest thing is a values file per cluster
after all. The signal is in the `source: override` field, which every report
carries for exactly this reason.

Also wrong if a third target cluster needs a fourth environment file whose
contents are mostly ingress annotations. At that point what varies is not what
to install but how to publish it, and that belongs in a chart of its own rather
than in more environments here.
