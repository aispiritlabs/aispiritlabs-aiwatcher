# Installing aiwatcher

Two ways in, depending on what the target already runs.

| | |
|---|---|
| An empty namespace on a cluster of your own | `deploy/scripts/install.sh` |
| Beside something that already has half the stack | `deploy/scripts/install.sh -e planner -n planner` |

Both go through `deploy/helmfile.yaml.gotmpl`, which looks at the cluster before it
decides what to install. The rest of this document is what that means and what
is left for you.

---

## What gets installed, and what does not

The chart can install six things. Four of them are things a cluster may already
run:

| Component | Default | Detected? |
|---|---|---|
| aiwatcher server (projector + API) | always | — |
| panel (nginx + the React build) | always | — |
| OpenTelemetry Collector | installed | detected, but **never** reused automatically |
| VictoriaTraces | installed | yes → `mode: external` |
| VictoriaMetrics | installed | yes → `mode: external` |
| RustFS (the prompt registry's store) | installed | detected, but **never** reused automatically |
| Grafana | never installed | yes → the datasource ConfigMap is emitted |

Detection is `deploy/scripts/detect-stack.py`. Run it on its own to see what a
cluster would give you, without installing anything:

```bash
deploy/scripts/detect-stack.py --namespace planner --format text
```

```
cluster: vps
  victoriametrics  present   http://planner-victoria-metrics.planner.svc.cluster.local:8428
                             fenced by NetworkPolicy planner-observability-ingress
  victoriatraces   absent    no pod runs a matching image
  collector        absent    no pod runs a matching image
  grafana          present   http://planner-grafana.planner.svc.cluster.local:3000
  objectstore      present   http://planner-rustfs-svc.planner.svc.cluster.local:9000
```

### Why this is not just a set of flags

A second VictoriaMetrics beside an existing one is not a harmless duplicate. The
same workload's metrics end up split across two stores, and every dashboard
reading either one silently stops covering half the data. The failure has no
error message; it looks like a gap in a graph. So the default is to look, and
the flags are the override rather than the interface.

### What "present" means

A pod running a matching image, with a Service in front of it on the expected
port. Not a name — names are a convention — and not a Service on its own, which
may point at nothing. What is matched:

| | image contains | port |
|---|---|---|
| VictoriaMetrics | `victoria-metrics`, `vmsingle` | 8428 |
| VictoriaTraces | `victoria-traces`, `vtsingle` | 10428 |
| Collector | `opentelemetry-collector` | 4318 |
| Grafana | `grafana/grafana` | 3000 |

Cluster-mode VictoriaMetrics (`vminsert`/`vmselect`/`vmstorage`) is deliberately
not matched: its write path is a different port and a tenant-scoped URL. Point
at it by hand with `AIWATCHER_VICTORIAMETRICS_URL`.

aiwatcher's own pods are excluded by the `app.kubernetes.io/part-of: aiwatcher`
label. Without that, the second `install.sh` would find the VictoriaTraces the
first one installed, decide it was external, and uninstall it.

### Overriding it

```bash
AIWATCHER_VICTORIAMETRICS_URL=http://vm.observability.svc.cluster.local:8428  # use this
AIWATCHER_VICTORIATRACES_URL=none                                            # there is none; install one
AIWATCHER_DETECT=off                                                         # do not look at all
```

An override is reported as `source: override` in the detection output, so what
you are looking at always says where it came from.

### The Collector is never reused automatically

It is detected and reported, and the install still creates its own. A Collector
that belongs to something else almost certainly has neither the VictoriaTraces
exporter nor the `attributes/redact` processor that drops `gen_ai.prompt` and
`gen_ai.completion` before anything is exported. Sending prompt text through a
pipeline that does not redact is not a thing to arrive at by inference.

To reuse one deliberately, having checked its config:

```yaml
collector:
  mode: external
  external:
    endpoint: http://otel-collector.observability.svc.cluster.local:4318
```

### The object store is never reused automatically either

For a different reason from the Collector's. There, reuse is unsafe; here it is
**undecidable**: nothing in the cluster says which credentials aiwatcher may
use, which bucket it may write, or whether it may create one. Detection reports
the store and prints the block you would need, with the pod selector already
filled in.

Reusing one deliberately:

```yaml
promptStore:
  mode: external
  bucket: aiwatcher-prompts
  external:
    endpoint: http://minio.storage.svc.cluster.local:9000
    createBucket: false   # true only if these credentials may create one
  credentialsSecret:
    name: minio-credentials
    accessKeyKey: MINIO_ROOT_USER
    secretKeyKey: MINIO_ROOT_PASSWORD
```

`mode: none` is also a real answer: every `/api/v1/prompts` route then answers
501 and the panel's Prompts tab says which variable is unset, rather than
showing an empty registry. What is *not* a good answer is leaving prompts on
the server's own volume in a cluster — that volume holds the write-ahead log,
which is a rolling window a retention policy may delete, and a prompt has to
outlive every run that used it.

**A NetworkPolicy for an external store is off by default and usually should
stay off.** Policies are additive, so an ingress rule attached to a store that
*no* policy currently selects narrows it from "accepts everything" to "accepts
aiwatcher only" — which would cut off whoever else was writing to that bucket.
`detect-stack.py` reports whether the store is fenced; only then does
`networkPolicy.allowEgressToExternalPromptStore: true` grant a path rather than
take three away.

---

## Installing beside planner

planner runs VictoriaMetrics, VictoriaLogs, Grafana and a RustFS on its k3s,
behind authentik, with a NetworkPolicy per component. aiwatcher goes into the
same namespace as a guest.

Two of those it borrows. VictoriaMetrics is derived by detection.
`planner-rustfs-svc` is not — `deploy/environments/planner.yaml` sets
`promptStore.mode: external` by hand, because the credentials Secret
(`planner-rustfs-secrets`, under planner's own `RUSTFS_ACCESS_KEY` /
`RUSTFS_SECRET_KEY` key names) is a thing a person knows and detection cannot.
`planner-web`, `planner-import-api` and `planner-mlflow` already write to that
store, which is also why no NetworkPolicy is attached to it: see above.

```bash
cd deploy
AIWATCHER_IMAGE=ghcr.io/you/aiwatcher \
AIWATCHER_PANEL_IMAGE=ghcr.io/you/aiwatcher-panel \
AIWATCHER_IMAGE_TAG=v0.1.0 \
  ./scripts/install.sh -e planner -n planner --plan     # look first
  ./scripts/install.sh -e planner -n planner            # then apply
```

What that install decides, on the cluster as it stands today:

* **VictoriaMetrics** — planner's, at `planner-victoria-metrics:8428`. Not
  installed again. OTLP metrics ingestion is on by default in vmsingle, so
  nothing changes on planner's side for the write path to work.
* **VictoriaTraces** — installed. planner has no trace store, and this is the
  one backend aiwatcher genuinely adds.
* **Grafana** — planner's. The datasource ConfigMap is emitted; wiring it in is
  a change on planner's side (below).
* **NetworkPolicy** — planner's `planner-observability-ingress` restricts port
  8428 to its own observability components, so aiwatcher's Collector would be
  refused. The install adds one ingress rule to those pods for the Collector.

### That NetworkPolicy, in detail

NetworkPolicies are additive: a pod selected by two policies accepts the union
of what they allow. So aiwatcher can grant its Collector a path to planner's
VictoriaMetrics without editing planner's chart, and deleting the aiwatcher
release restores planner's own rules exactly.

The asymmetry that makes this safe in one direction only: a pod that **no**
policy selects accepts everything. Attaching a policy to such a pod does not
widen it — it narrows it, from "everything" to "aiwatcher's Collector only",
cutting off whoever was already talking to it. So the rule is only attached when
the target pods are **already fenced** by some ingress policy, which is what the
`fenced by NetworkPolicy …` line in the detection output is reporting. If
planner ever set `networkPolicy.enabled: false`, detection would stop reporting
the fence and the install would stop attaching the rule — correctly, because it
would no longer be needed.

The one case this cannot handle is a VictoriaMetrics in a **different**
namespace: a NetworkPolicy only applies in its own, so that rule has to be added
by whoever owns that namespace. The chart fails with that message rather than
rendering a policy that would do nothing.

### Wiring the datasource into planner's Grafana

planner's Grafana provisions from a fixed ConfigMap mounted by `subPath`, not
from the discovery sidecar, so it does not pick up a new ConfigMap on its own.
One volume and one mount in `deploy/helm/planner/templates/observability.yaml`:

```yaml
            - name: aiwatcher-datasources
              mountPath: /etc/grafana/provisioning/datasources/aiwatcher.yaml
              subPath: aiwatcher-datasources.yaml
              readOnly: true
```

```yaml
        - name: aiwatcher-datasources
          configMap: { name: aiwatcher-grafana-datasources }
```

The datasource is a Jaeger one: VictoriaTraces answers the Jaeger query API, so
Grafana reads it with no plugin. Its trace-to-metrics link points at uid
`victoria-metrics`, which is what planner's own VictoriaMetrics datasource
already uses. The planner environment installer restarts `planner-grafana`
after the ConfigMap appears, so its init container can copy the newly available
datasource into Grafana's provisioning directory.

### Publishing the panel

Left off by default. aiwatcher has no authentication of its own, so an ingress
without planner's authentik middlewares would put it on the public internet.
`environments/planner.yaml` carries the same two middlewares every authenticated
planner route uses, plus the `/outpost.goauthentik.io/` route the forward-auth
callback needs — that path must skip the authentik middleware, because a route
that requires a session in order to establish one never completes.

```bash
AIWATCHER_DOMAIN=aiwatcher.example.com ./scripts/install.sh -e planner -n planner
```

Add a DNS record for the host first; cert-manager will not issue against a name
that does not resolve.

---

## Replacing MLflow

Install aiwatcher before deploying the Planner cutover. Planner no longer runs
both systems in parallel: once its workloads point at the server ingest URL,
the old tracking stack is removed in the same release. The measurements that
justified that decision remain in `docs/mlflow-comparison.md`.

What actually moves is in `planner-mlplatform`, which uses MLflow two ways:

1. `mlflow.pydantic_ai.autolog(log_traces=True)` in `app/observability.py`, for
   agent traces. This is the part aiwatcher replaces: the Python SDK
   (`sdk/python`) publishes to `POST /api/v1/events` on the server, which is on
   in both environments (`server.ingestEnabled`).

   ```
   AIWATCHER_URL=http://aiwatcher-server.planner.svc.cluster.local:8080
   ```

2. `start_run` / `log_params` / `log_metrics` / `log_dict` in
   `log_evaluation_run`, for evaluation reports. This is **not** a trace, and
   for a while it had nowhere to go. It does now:
   [ADR_0010](ADR/ADR_0010_EVALUATION_REPORTS.md) added evaluation reports as a
   first-class record that rides the same log and forms no span, and the panel's
   Evaluation area renders them.

   The swap in `app/observability.py` is the same four pieces:

   ```python
   def log_evaluation_run(*, run_name, parameters, metrics, report) -> str | None:
       client = aiwatcher_client()          # the one already built for tracing
       if client is None:
           return None
       try:
           return client.record_evaluation(
               suite=run_name,               # "dom-projekt-catalog-gate"
               dataset=f"catalog-cases@{len(CATALOG_CASES)}",
               params=parameters,
               metrics=metrics,
               report=report,
           )
       except Exception as error:           # a report must not fail the gate
           logger.warning("aiwatcher_evaluation_write_failed", error=str(error)[:500])
           return None
   ```

   Two things worth getting right rather than defaulting:

   * **`dataset` is what makes two reports comparable.** aiwatcher compares a
     report only against the previous one of the same suite *on the same
     dataset*. Passing nothing still works — an unversioned suite compares
     against itself — but the day `CATALOG_CASES` grows, a pass rate that
     "dropped" will just be a different set of cases, and nothing will say so.
   * **`report` is not redacted.** The Collector strips prompts and completions
     from spans; an evaluation forms no span, so whatever
     `house_catalog.py` puts in the report is stored verbatim. That is fine for
     scores and problem lists, and a decision worth making before it holds model
     output.

   Optionally, `client.evaluation(...)` publishes a case per catalogue project
   as it is scored, which is what fills the regression view — the cases that
   passed on the previous gate and fail on this one.

The release order is therefore: install aiwatcher, deploy the Planner cutover,
run one traced inference and one evaluation, then verify both in the panel.
Planner removes the complete old tracking surface: its Deployment and Service,
ingress, dependency, runtime configuration, authentication application, object
bucket and persistent-volume settings. No artifact store or registry from that
stack remains wired into Planner.

DeepEval stays because it is the evaluation harness, not a tracking backend.
aiwatcher records the *result* and has no opinion about how it was produced —
no scorers, no judges, no suite runner.

---

## The install script

```
./scripts/install.sh [-e ENV] [-n NAMESPACE] [--plan] [-y]
```

It refuses to go further than a preflight when:

* `kubectl`, `helm`, `helmfile` or `python3` is missing — with the install line
  for each;
* the cluster cannot be read well enough to detect what is on it. Installing
  anyway would risk exactly the duplicate this whole mechanism exists to avoid.

And it asks before applying to any context that is not a known-local one
(`orbstack`, `docker-desktop`, `minikube`, `colima`, `rancher-desktop`, `kind-*`,
`k3d-*`, `k3s-*`). This kubeconfig has production contexts in it and
`helmfile apply` is not a command that asks. `-y` skips the question; nothing
skips the preflight.

`--plan` runs `helmfile diff` and applies nothing.

## Prerequisites

```bash
mise use -g helmfile        # or: brew install helmfile
```

`kubectl`, `helm` and `python3` you already have. The detection script is
standard-library Python — no `jq`, no `pip install`.

The `helm-diff` plugin is optional. With it, `--plan` shows a diff against what
is running and the install goes through `helmfile apply`. Without it, `--plan`
prints the full manifest and the install goes through `helmfile sync` — the same
manifests, no pre-flight diff. It is not a hard requirement because its Helm 4
support is its own moving target, and that should not stand between you and an
install:

```bash
helm plugin install https://github.com/databus23/helm-diff
```

## Images

```bash
REGISTRY=ghcr.io/you TAG=v0.1.0 deploy/scripts/build-images.sh --push
```

Two images: `aiwatcher` (the Rust binary) and `aiwatcher-panel` (nginx plus the
built React app). Separate, so a panel change does not rebuild the Rust binary.
The panel's nginx config is not in its image — it comes from a ConfigMap in the
chart, because it has to name the server's Service, which is a deployment-time
fact.

## Uninstalling

```bash
deploy/scripts/uninstall.sh -e planner -n planner
```

Volumes are kept. `--delete-data` removes them, after asking. Nothing that was
borrowed is touched, and the one NetworkPolicy attached to someone else's pods
goes away with the release.

## Without helmfile

The chart is a normal Helm chart and the detection script emits chart values
directly, so this is the same decision by hand:

```bash
helm upgrade --install aiwatcher deploy/helm/aiwatcher \
  --namespace planner --create-namespace \
  --values deploy/environments/planner.yaml \
  --values <(deploy/scripts/detect-stack.py -n planner --format helm-values)
```

The mapping from "what is on the cluster" to "what to install" lives in
`--format helm-values`, in one place, so this and the helmfile cannot drift.
